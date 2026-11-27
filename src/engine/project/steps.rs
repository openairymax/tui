// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 计划步骤表（0.1.21 X6-01a，§23.5.2 时间线折叠的数据源）。
//
// 计划确认后 daemon 逐条下发 step 帧（当前生产者只发 step_status=planned），
// 先于任何工具调用。本表按 (任务号, 步序) 归并：同一歩的后续状态帧原地推进，
// 不追加第二行——否则时间线会在「计划」与「执行」之间出现两条同名步骤。
//
// 行序是到达序，但时间线要的是依赖序：步序由服务端给出（0 基），呈现层按
// 步序排序，乱序到达不会把第 3 步排到第 1 步前面。缺步序的帧在映射层已被
// 丢弃，这里不再兜底编序号——本地编出来的步序与服务端进度对不上。
//
// 与任务表同规：客户端不校验状态机，只记录最后一条到达的状态；终态之后不再
// 改写（重传或迟到的中间态会把已收口的步骤拉回运行中）。状态帧可能只带
// step_status 而不带标题，空标题不覆盖已记住的标题。
//
// `epoch` 是渲染层的变更计数：步骤表没有行号脏区，面板比较前后计数即可。

#![cfg_attr(not(test), allow(dead_code))]

use crate::engine::event::model::{Body, Event, Execution};

/// 步骤终态：到达后该步不再变化。
const CLOSED: [&str; 5] = ["done", "failed", "skipped", "completed", "cancelled"];

/// 一行步骤：任务号 + 步序 + 总数 + 状态 + 标题 + 帧锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) task: String,
    pub(crate) index: usize,
    pub(crate) total: usize,
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) seq: u64,
    pub(crate) ts: u64,
}

impl Row {
    /// 是否已收口。
    pub(crate) fn done(&self) -> bool {
        CLOSED.contains(&self.status.as_str())
    }
}

/// 步骤表：按 (任务号, 步序) 寻址的当前态索引。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Steps {
    rows: Vec<Row>,
    epoch: u64,
}

impl Steps {
    /// 空表。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 折叠一条事件：只认执行面的步骤帧，其余原样放过。
    pub(crate) fn apply(&mut self, ev: &Event) -> bool {
        let Body::Execution(Execution::Step {
            task,
            index,
            total,
            status,
            title,
        }) = &ev.body
        else {
            return false;
        };
        self.upsert(ev, task, *index, *total, status, title)
    }

    /// 新建或推进一步；重复帧与终态之后的推进都不改动。
    fn upsert(
        &mut self,
        ev: &Event,
        task: &str,
        index: usize,
        total: usize,
        status: &str,
        title: &str,
    ) -> bool {
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.task == task && r.index == index)
        else {
            self.rows.push(Row {
                task: task.to_owned(),
                index,
                total,
                status: status.to_owned(),
                title: title.to_owned(),
                seq: ev.seq,
                ts: ev.ts,
            });
            self.bump();
            return true;
        };
        if row.done() || stale(ev.seq, row.seq) {
            return false;
        }
        row.status = status.to_owned();
        row.total = total;
        // 状态帧可以省略标题：空值表示"没说"，不是"标题没了"。
        if !title.is_empty() {
            row.title = title.to_owned();
        }
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

    /// 按 (任务号, 步序) 取行。
    pub(crate) fn get(&self, task: &str, index: usize) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.task == task && r.index == index)
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
    use super::Steps;
    use crate::engine::event::model::{Body, Cognition, Event, Execution};

    fn ev(seq: u64, exe: Execution) -> Event {
        Event {
            seq,
            ts: 1_000 + seq,
            body: Body::Execution(exe),
        }
    }

    fn step(seq: u64, index: usize, total: usize, status: &str, title: &str) -> Event {
        ev(
            seq,
            Execution::Step {
                task: "t1".into(),
                index,
                total,
                status: status.into(),
                title: title.into(),
            },
        )
    }

    /// 计划帧建行：任务号、步序、总数、状态与标题如实入表。
    #[test]
    fn creates_row_from_plan_frame() {
        let mut s = Steps::new();
        assert!(s.is_empty());
        assert!(s.apply(&step(1, 0, 3, "planned", "读取依赖")));
        let row = s.get("t1", 0).expect("已建行");
        assert_eq!((row.index, row.total), (0, 3));
        assert_eq!(
            (row.status.as_str(), row.title.as_str()),
            ("planned", "读取依赖")
        );
        // 帧锚点随行保存：渲染层据此判断「这一步是不是本轮新到的」。
        assert_eq!((row.seq, row.ts), (1, 1_001));
        assert_eq!(s.len(), 1);
        assert_eq!(s.epoch(), 1);
    }

    /// 乱序到达按到达序入表，但每步各占一行（排序交给时间线）。
    #[test]
    fn out_of_order_steps_are_distinct() {
        let mut s = Steps::new();
        s.apply(&step(1, 2, 3, "planned", "收尾"));
        s.apply(&step(2, 0, 3, "planned", "读取依赖"));
        assert_eq!(s.len(), 2);
        assert_eq!(s.rows()[0].index, 2);
        assert_eq!(s.rows()[1].index, 0);
    }

    /// 同一步的状态推进原地更新，不追加第二行；省略标题不覆盖。
    #[test]
    fn updates_in_place_without_losing_title() {
        let mut s = Steps::new();
        s.apply(&step(1, 1, 3, "planned", "生成报告"));
        assert!(s.apply(&step(2, 1, 3, "running", "")));
        assert_eq!(s.len(), 1);
        let row = s.get("t1", 1).expect("仍是同一行");
        assert_eq!(
            (row.status.as_str(), row.title.as_str()),
            ("running", "生成报告")
        );
        assert_eq!(row.seq, 2);
        assert_eq!(s.epoch(), 2);
    }

    /// 终态之后不接受推进；重传帧不产生第二次变更。
    #[test]
    fn closed_and_replayed_frames_are_ignored() {
        let mut s = Steps::new();
        s.apply(&step(1, 0, 1, "running", "读取依赖"));
        assert!(s.apply(&step(2, 0, 1, "done", "")));
        assert!(s.get("t1", 0).expect("已建行").done());
        assert!(!s.apply(&step(3, 0, 1, "running", "")));
        assert_eq!(s.epoch(), 2);
    }

    /// 非步骤帧原样放过；无任务号的帧也能入表（单轮内的步序仍可定位）。
    #[test]
    fn ignores_other_frames_and_accepts_empty_task() {
        let mut s = Steps::new();
        let other = Event {
            seq: 1,
            ts: 1,
            body: Body::Cognition(Cognition::Plan {
                text: "先读依赖".into(),
            }),
        };
        assert!(!s.apply(&other));
        assert!(s.apply(&ev(
            2,
            Execution::Step {
                task: String::new(),
                index: 0,
                total: 1,
                status: "planned".into(),
                title: "步骤".into(),
            }
        )));
        assert!(s.get("", 0).is_some());
    }
}
