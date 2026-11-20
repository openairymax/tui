// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 审批视图表（0.1.21 X1-14a，§23.1.5）。
//
// 一条审批只有三个结局：还没批、批了、拒了。表按 request_id 寻址，允许或
// 拒绝都落在同一行上，不追加第二行——审批面板该显示的是这件事的结论，而
// 不是一段对话记录。
//
// 超时归拒绝，且判定只依赖事件自带的时刻：截止点 = 请求帧的 ts + 帧里给的
// timeout_ms，之后任何一条事件的 ts 越过截止点即收口为拒绝。投影不读本地
// 时钟，同一段事件流在任何机器上折出同一结论；心跳本身就是节拍，链路活着
// 时超时一定会被某条帧看见，没别的定时器要养。
//
// 超时之后到达的「允许」不改判决：服务端早已按超时关掉这次调用，本地再改
// 成允许只会让界面与事实相反。同理，第二次裁决一律忽略——一次审批只有一个
// 结论，后来的按键是重复输入或界面没来得及收起。
//
// timeout_ms 为 0 表示服务端没有设上限，表也不替它编一个：猜测时限会让本
// 该继续等待的审批被提前拒掉。

#![cfg_attr(not(test), allow(dead_code))]

use crate::engine::event::model::{Body, Control, Event};

/// 审批结论：未批 / 允许 / 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pending,
    Approved,
    Denied,
}

/// 一行审批：凭据 + 待批内容 + 结论 + 时刻锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) target: String,
    pub(crate) verdict: Verdict,
    pub(crate) asked: u64,
    pub(crate) deadline: u64,
    pub(crate) at: u64,
    pub(crate) seq: u64,
}

impl Row {
    /// 是否已有结论。
    pub(crate) fn done(&self) -> bool {
        self.verdict != Verdict::Pending
    }
}

/// 审批表：按请求凭据寻址，行序为请求到达顺序。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Approvals {
    rows: Vec<Row>,
    epoch: u64,
}

impl Approvals {
    /// 空表。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 折叠一条事件：先按本条事件的时刻收口超时，再处理请求与裁决。
    pub(crate) fn apply(&mut self, ev: &Event) -> bool {
        let mut changed = self.expire(ev.ts);
        changed |= match &ev.body {
            Body::Control(Control::Ask {
                id,
                tool,
                target,
                timeout_ms,
            }) => self.ask(ev, id, tool, target, *timeout_ms),
            Body::Control(Control::Answer { id, ok }) => self.answer(ev, id, *ok),
            _ => false,
        };
        changed
    }

    /// 超时收口：越过截止点的未批行一律归拒绝。
    fn expire(&mut self, now: u64) -> bool {
        let mut changed = false;
        for row in self.rows.iter_mut().filter(|r| !r.done()) {
            if row.deadline != 0 && now >= row.deadline {
                row.verdict = Verdict::Denied;
                row.at = now;
                changed = true;
            }
        }
        if changed {
            self.bump();
        }
        changed
    }

    /// 新请求建行；同号请求是重传，不再建第二行。
    fn ask(&mut self, ev: &Event, id: &str, tool: &str, target: &str, wait: u64) -> bool {
        if id.is_empty() || self.rows.iter().any(|r| r.id == id) {
            return false;
        }
        self.rows.push(Row {
            id: id.to_owned(),
            tool: tool.to_owned(),
            target: target.to_owned(),
            verdict: Verdict::Pending,
            asked: ev.ts,
            deadline: wait_until(ev.ts, wait),
            at: ev.ts,
            seq: ev.seq,
        });
        self.bump();
        true
    }

    /// 裁决落行：只认未批的行，重复裁决与迟到裁决都不改动。
    fn answer(&mut self, ev: &Event, id: &str, ok: bool) -> bool {
        let Some(row) = self.rows.iter_mut().find(|r| r.id == id && !r.done()) else {
            return false;
        };
        row.verdict = if ok {
            Verdict::Approved
        } else {
            Verdict::Denied
        };
        row.at = ev.ts;
        row.seq = ev.seq;
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

    /// 按凭据取行（不在表内为 None）。
    pub(crate) fn get(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// 未批行：界面按到达顺序取第一条来提示。
    pub(crate) fn pending(&self) -> impl Iterator<Item = &Row> + '_ {
        self.rows.iter().filter(|r| !r.done())
    }

    /// 全表只读视图（行序即请求顺序）。
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// 截止点：未设上限时为 0（永不超时）。
fn wait_until(asked: u64, wait: u64) -> u64 {
    if wait == 0 {
        0
    } else {
        asked.saturating_add(wait)
    }
}

#[cfg(test)]
mod tests {
    use super::{wait_until, Approvals, Verdict};
    use crate::engine::event::model::{Body, Cognition, Control, Event};

    fn ev(ts: u64, body: Body) -> Event {
        Event { seq: 1, ts, body }
    }

    fn ask(ts: u64, id: &str, wait: u64) -> Event {
        ev(
            ts,
            Body::Control(Control::Ask {
                id: id.into(),
                tool: "shell".into(),
                target: "rm -rf build".into(),
                timeout_ms: wait,
            }),
        )
    }

    fn answer(ts: u64, id: &str, ok: bool) -> Event {
        ev(ts, Body::Control(Control::Answer { id: id.into(), ok }))
    }

    fn beat(ts: u64) -> Event {
        ev(ts, Body::Control(Control::Beat { seq: ts }))
    }

    /// 请求入表即未批，截止点按给定上限推算。
    #[test]
    fn ask_opens_pending_row() {
        let mut t = Approvals::new();
        assert!(t.is_empty());
        assert!(t.apply(&ask(1_000, "a1", 500)));
        let row = t.get("a1").expect("已建行");
        assert_eq!(row.verdict, Verdict::Pending);
        assert_eq!((row.asked, row.deadline), (1_000, 1_500));
        assert_eq!(row.tool, "shell");
        assert!(!row.done());
        assert_eq!(t.pending().count(), 1);
        assert_eq!(t.epoch(), 1);
    }

    /// 允许与拒绝各落一行，结论不再变化。
    #[test]
    fn answer_decides_once() {
        let mut t = Approvals::new();
        t.apply(&ask(1_000, "a1", 0));
        t.apply(&ask(1_010, "a2", 0));
        assert!(t.apply(&answer(1_100, "a1", true)));
        assert!(t.apply(&answer(1_110, "a2", false)));
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Approved));
        assert_eq!(t.get("a2").map(|r| r.verdict), Some(Verdict::Denied));
        assert_eq!(t.len(), 2, "裁决不追加行");
        assert_eq!(t.pending().count(), 0);

        assert!(!t.apply(&answer(1_200, "a1", false)), "第二次裁决无效");
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Approved));
    }

    /// 超时由后续事件的时刻触发，归拒绝；正在等待时长的边界不算超时。
    #[test]
    fn timeout_denies_on_later_tick() {
        let mut t = Approvals::new();
        t.apply(&ask(1_000, "a1", 500));
        assert!(!t.apply(&beat(1_499)), "未到截止点不收口");
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Pending));

        assert!(t.apply(&beat(1_500)), "到点即拒");
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Denied));
        assert!(t.get("a1").is_some_and(|r| r.done()));
    }

    /// 超时之后到达的允许不改判决：迟到裁决不能把事实翻成允许。
    #[test]
    fn late_answer_is_ignored() {
        let mut t = Approvals::new();
        t.apply(&ask(1_000, "a1", 100));
        t.apply(&beat(1_100));
        assert!(!t.apply(&answer(1_150, "a1", true)));
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Denied));
    }

    /// 未设上限的审批不因时间流逝被拒。
    #[test]
    fn unbounded_waits_forever() {
        let mut t = Approvals::new();
        t.apply(&ask(1_000, "a1", 0));
        assert!(!t.apply(&beat(u64::MAX / 2)));
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Pending));
        assert_eq!(wait_until(1_000, 0), 0);
    }

    /// 同号请求是重传；无号请求不可回应，都不入表。
    #[test]
    fn duplicates_and_unnamed_are_dropped() {
        let mut t = Approvals::new();
        assert!(t.apply(&ask(1_000, "a1", 0)));
        assert!(!t.apply(&ask(1_001, "a1", 0)), "同号重传");
        assert!(!t.apply(&ask(1_002, "", 0)), "无凭据无法回应");
        assert_eq!(t.len(), 1);
    }

    /// 对未知请求的裁决无效：界面按键早于请求到达时不留下幽灵结论。
    #[test]
    fn answer_without_ask_is_dropped() {
        let mut t = Approvals::new();
        assert!(!t.apply(&answer(1_000, "a9", true)));
        assert!(t.is_empty());
        assert_eq!(t.epoch(), 0);
    }

    /// 非控制事件只作节拍，不产生审批行。
    #[test]
    fn other_domains_only_advance_time() {
        let mut t = Approvals::new();
        t.apply(&ask(1_000, "a1", 100));
        let delta = ev(
            1_200,
            Body::Cognition(Cognition::Delta { text: "x".into() }),
        );
        assert!(t.apply(&delta), "越点收口");
        assert_eq!(t.len(), 1, "认知事件不建审批行");
        assert_eq!(t.get("a1").map(|r| r.verdict), Some(Verdict::Denied));
    }
}
