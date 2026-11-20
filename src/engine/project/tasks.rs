// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 任务视图表（0.1.21 X1-13a，§23.1.5 / N3）。
//
// 与消息表相反，这张表只保留**当前态**：任务面板要看的是「这条任务现在到
// 哪一步了」，而不是它走过的每一步。历史在事件流与账本里，投影不复制第二
// 份；因此这里允许原地更新，行序按身份首次出现固定，更新不会让任务跳行。
//
// 身份是唯一的键，来自帧载荷的 `task_id`（run_start 与 task_state 都带）。
// 没有身份的迁移帧不进表：快照描述的是「这次运行」而不是「某条任务」，若
// 给它编一个空身份的行，任务面板就会凭空多出一条没有名字的条目。
//
// 客户端不做状态机校验：状态抵达顺序即事实，daemon 才是推进的权威。表只
// 记录最后一条到达的迁移与它的原因，不推断「下一步应该是什么」——一旦客户
// 端自持一套状态机，任何服务端新增的中间态都会被本地判成非法而吞掉。
//
// 终态（completed/failed/cancelled/unverified）到达后该身份不再变化：任务号
// 一次一用，新提交会带新号；若旧号又收到迁移，那是链路重传或上游 bug，改
// 写已收口的行只会让面板在两个终态之间来回闪。
//
// 重传按帧序号去重：同号帧是同一帧的重放，重放不得产生第二次迁移计数。
// `epoch` 是给渲染层的变更计数——任务表没有行号脏区，面板比较前后计数即可
// 知道要不要重排，不必逐行比对。

#![cfg_attr(not(test), allow(dead_code))]

use crate::engine::event::model::{Body, Control, Event};

/// 终态取值（agent_task_contract.h 的终态集合）。
const CLOSED: [&str; 4] = ["completed", "failed", "cancelled", "unverified"];

/// 取消态名：取消是终态的一种，归表时与普通迁移同形。
const CANCEL: &str = "cancelled";

/// 一行任务：身份 + 当前态 + 迁移次数 + 最后一条迁移的锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) state: String,
    pub(crate) reason: String,
    pub(crate) rev: u32,
    pub(crate) seq: u64,
    pub(crate) ts: u64,
}

impl Row {
    /// 是否已收口。
    pub(crate) fn done(&self) -> bool {
        CLOSED.contains(&self.state.as_str())
    }
}

/// 任务表：按身份寻址的当前态索引，行序为首次出现顺序。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tasks {
    rows: Vec<Row>,
    epoch: u64,
}

impl Tasks {
    /// 空表。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 折叠一条事件：只认带身份的迁移帧与取消帧，其余原样放过。
    ///
    /// 返回是否真的改动了表；渲染层据此决定要不要重排面板。
    pub(crate) fn apply(&mut self, ev: &Event) -> bool {
        let (id, state, reason) = match &ev.body {
            Body::Control(Control::State { id, name }) => (id, name.as_str(), ""),
            Body::Control(Control::Cancel { id, reason }) => (id, CANCEL, reason.as_str()),
            _ => return false,
        };
        if id.is_empty() {
            return false;
        }
        self.upsert(id, state, reason, ev)
    }

    /// 新建或推进一行；重复帧与终态之后的迁移都不改动。
    fn upsert(&mut self, id: &str, state: &str, reason: &str, ev: &Event) -> bool {
        let Some(idx) = self.rows.iter().position(|r| r.id == id) else {
            self.rows.push(Row {
                id: id.to_owned(),
                state: state.to_owned(),
                reason: reason.to_owned(),
                rev: 1,
                seq: ev.seq,
                ts: ev.ts,
            });
            self.bump();
            return true;
        };
        let row = &mut self.rows[idx];
        if row.done() || stale(ev.seq, row.seq) {
            return false;
        }
        row.state = state.to_owned();
        row.reason = reason.to_owned();
        row.rev = row.rev.saturating_add(1);
        row.seq = ev.seq;
        row.ts = ev.ts;
        self.bump();
        true
    }

    /// 变更计数：每次真实改动加一，渲染层比较前后值。
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    fn bump(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    /// 行数。
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// 是否空表。
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 按身份取行（不在表内为 None）。
    pub(crate) fn get(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// 全表只读视图（行序即首次出现顺序）。
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// 帧序号是否落在已折叠水位之前；序号 0 表示未编号，不参与去重。
fn stale(seq: u64, mark: u64) -> bool {
    seq != 0 && seq <= mark
}

#[cfg(test)]
mod tests {
    use super::{stale, Tasks};
    use crate::engine::event::model::{Body, Cognition, Control, Event};

    fn ev(seq: u64, body: Body) -> Event {
        Event {
            seq,
            ts: 1_000 + seq,
            body,
        }
    }

    fn state(seq: u64, id: &str, name: &str) -> Event {
        ev(
            seq,
            Body::Control(Control::State {
                id: id.into(),
                name: name.into(),
            }),
        )
    }

    fn cancel(seq: u64, id: &str, reason: &str) -> Event {
        ev(
            seq,
            Body::Control(Control::Cancel {
                id: id.into(),
                reason: reason.into(),
            }),
        )
    }

    /// 开跑帧建行：身份、状态与锚点如实入表。
    #[test]
    fn creates_row_at_start() {
        let mut t = Tasks::new();
        assert!(t.is_empty());
        assert!(t.apply(&state(1, "t1", "running")));
        let row = t.get("t1").expect("已建行");
        assert_eq!((row.state.as_str(), row.rev, row.seq), ("running", 1, 1));
        assert_eq!(t.len(), 1);
        assert_eq!(t.epoch(), 1);
    }

    /// 同一身份逐帧推进：只有一行，迁移计数递增，行序不动。
    #[test]
    fn advances_row_in_place() {
        let mut t = Tasks::new();
        t.apply(&state(1, "t1", "submitted"));
        t.apply(&state(2, "t1", "planned"));
        assert!(t.apply(&state(3, "t1", "executing")));
        assert_eq!(t.len(), 1);
        let row = t.get("t1").expect("行仍在");
        assert_eq!((row.state.as_str(), row.rev, row.seq), ("executing", 3, 3));
        assert_eq!(t.epoch(), 3);
    }

    /// 重传同号帧不产生第二次迁移；原因随最后一条迁移，不做历史拼接。
    #[test]
    fn retransmit_is_idempotent() {
        let mut t = Tasks::new();
        t.apply(&state(5, "t1", "executing"));
        assert!(!t.apply(&state(5, "t1", "executing")), "同号帧是重放");
        assert!(!t.apply(&state(4, "t1", "planned")), "旧号帧不得回退");
        assert_eq!(t.get("t1").map(|r| r.rev), Some(1));
        assert_eq!(t.epoch(), 1);

        t.apply(&state(6, "t1", "paused"));
        assert_eq!(t.get("t1").map(|r| r.reason.as_str()), Some(""));
    }

    /// 取消落成终态，原因留作追溯；终态之后不再改写。
    #[test]
    fn cancel_is_terminal() {
        let mut t = Tasks::new();
        t.apply(&state(1, "t1", "executing"));
        assert!(t.apply(&cancel(2, "t1", "user")));
        let row = t.get("t1").expect("行仍在");
        assert_eq!(
            (row.state.as_str(), row.reason.as_str()),
            ("cancelled", "user")
        );
        assert!(row.done());
        assert!(!t.apply(&state(3, "t1", "executing")), "终态不复用身份");
        assert_eq!(t.get("t1").map(|r| r.state.as_str()), Some("cancelled"));
    }

    /// 正常收口同样是终态：完成之后的重传或推进都不改行。
    #[test]
    fn done_is_sticky() {
        let mut t = Tasks::new();
        t.apply(&state(1, "t1", "executing"));
        t.apply(&state(2, "t1", "completed"));
        assert!(t.get("t1").is_some_and(|r| r.done()));
        assert!(!t.apply(&state(3, "t1", "executing")));
        assert!(!t.apply(&cancel(4, "t1", "user")));
        assert_eq!(t.get("t1").map(|r| r.state.as_str()), Some("completed"));
    }

    /// 无身份的迁移帧不进表：快照描述运行，不描述任务。
    #[test]
    fn ignores_unidentified_states() {
        let mut t = Tasks::new();
        assert!(!t.apply(&state(1, "", "running")));
        assert!(!t.apply(&state(2, "", "done")));
        assert!(t.is_empty());
        assert_eq!(t.epoch(), 0);
    }

    /// 非控制事件不触碰任务表。
    #[test]
    fn ignores_other_domains() {
        let mut t = Tasks::new();
        let cog = ev(1, Body::Cognition(Cognition::Delta { text: "片".into() }));
        assert!(!t.apply(&cog));
        assert!(t.is_empty());
    }

    /// 多任务行序按首次出现固定，后续更新不重排。
    #[test]
    fn keeps_first_seen_order() {
        let mut t = Tasks::new();
        t.apply(&state(1, "t2", "submitted"));
        t.apply(&state(2, "t1", "submitted"));
        t.apply(&state(3, "t2", "executing"));
        let order: Vec<&str> = t.rows().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, vec!["t2", "t1"]);
        assert_eq!(t.get("t2").map(|r| r.rev), Some(2));
    }

    /// 序号 0 视为未编号：不去重，便于本地造帧按序折叠。
    #[test]
    fn zero_seq_skips_watermark() {
        assert!(!stale(0, 0));
        assert!(!stale(0, 9));
        assert!(stale(9, 9));
        let mut t = Tasks::new();
        t.apply(&state(0, "t1", "submitted"));
        assert!(t.apply(&state(0, "t1", "executing")));
        assert_eq!(t.get("t1").map(|r| r.rev), Some(2));
    }
}
