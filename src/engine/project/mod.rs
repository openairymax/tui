// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 投影器（0.1.21 X1-10a，§23.1.5 事件入口 → 视图模型的唯一折叠点）。
//
// TUI 不再向 gateway 问状态，只把事件流折成视图模型：`apply` 吃一条已归域
// 的事件，只改自己的字段。折叠是纯的——不读时钟、不碰文件、不取锁，时刻
// 由事件自带的 `ts` 决定。这条纪律让同一段事件流在任何机器上折出同一份
// 快照，也是 X1-32 回放基线能成立的前提。
//
// 本卡只冻结签名与最小状态：帧号水位、逐域计数、是否在跑。消息表、任务表、
// 工具表、审批表分别由 X1-11/X1-13/X1-14 接入同一 `apply`，接入时只加字段
// 与分支，不改变调用方看到的签名。

#![cfg_attr(not(test), allow(dead_code))]

use super::event::model::{Body, Control, Domain, Event, Outcome};

pub(crate) mod msgs;

#[cfg(test)]
mod support;

use self::msgs::Msgs;

/// 运行态：由事件驱动，不由渲染层自己判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// 尚未收到任何开跑信号。
    Idle,
    /// 已开跑且未收口。
    Running,
    /// 已收口（正常或失败）。
    Closed,
}

/// 折叠结果：帧号水位 + 逐域计数 + 运行相位 + 消息表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Projection {
    applied: u64,
    seq: u64,
    phase: Phase,
    counts: [u64; 4],
    msgs: Msgs,
}

impl Projection {
    /// 空投影：未收帧、未开跑。
    pub(crate) fn new() -> Self {
        Self {
            applied: 0,
            seq: 0,
            phase: Phase::Idle,
            counts: [0; 4],
            msgs: Msgs::new(),
        }
    }

    /// 折叠一条事件：只改本结构，返回值留空以便后续卡接脏区。
    ///
    /// 增量语义：每条事件只落到该落的表，已入表的行不重算、不改写。
    pub(crate) fn apply(&mut self, ev: &Event) {
        self.applied = self.applied.saturating_add(1);
        self.seq = self.seq.max(ev.seq);
        let slot = slot(ev.domain());
        self.counts[slot] = self.counts[slot].saturating_add(1);
        self.phase = next_phase(self.phase, &ev.body);
        self.msgs.push(ev);
    }

    /// 已折叠事件数。
    pub(crate) fn applied(&self) -> u64 {
        self.applied
    }

    /// 帧号水位：重连握手与水合分页的起点。
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// 当前运行相位。
    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    /// 某域已折叠的事件数（状态栏与诊断直接读）。
    pub(crate) fn count(&self, d: Domain) -> u64 {
        self.counts[slot(d)]
    }

    /// 消息表只读视图。
    pub(crate) fn msgs(&self) -> &Msgs {
        &self.msgs
    }
}

impl Default for Projection {
    fn default() -> Self {
        Self::new()
    }
}

/// 域 → 计数槽位：四域各一，顺序固定。
fn slot(d: Domain) -> usize {
    match d {
        Domain::Control => 0,
        Domain::Cognition => 1,
        Domain::Execution => 2,
        Domain::Outcome => 3,
    }
}

/// 相位迁移：开跑与收口是仅有的两个边沿，其余事件不改变相位。
///
/// 收口后不回 Idle：下一次开跑由新的 `run_start` 拉起，旧终态保留到那时，
/// 否则状态栏会在两轮之间闪回未知态。
fn next_phase(cur: Phase, body: &Body) -> Phase {
    match body {
        Body::Control(Control::State { name }) => match name.as_str() {
            "running" | "executing" => Phase::Running,
            "done" | "completed" | "failed" | "cancelled" => Phase::Closed,
            _ => cur,
        },
        Body::Outcome(Outcome::End { .. } | Outcome::Result { .. } | Outcome::Err { .. }) => {
            Phase::Closed
        }
        _ => cur,
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, Projection};
    use crate::engine::event::model::{Body, Cognition, Control, Domain, Event, Outcome};

    fn ev(seq: u64, body: Body) -> Event {
        Event { seq, ts: 0, body }
    }

    fn start(seq: u64) -> Event {
        ev(
            seq,
            Body::Control(Control::State {
                name: "running".into(),
            }),
        )
    }

    fn end(seq: u64) -> Event {
        ev(
            seq,
            Body::Outcome(Outcome::End {
                status: "completed".into(),
            }),
        )
    }

    /// 骨架可调用：折叠后水位、计数与相位如实反映事件流。
    #[test]
    fn folds_stream_into_state() {
        let mut p = Projection::new();
        assert_eq!((p.applied(), p.seq(), p.phase()), (0, 0, Phase::Idle));

        p.apply(&start(1));
        p.apply(&ev(
            2,
            Body::Cognition(Cognition::Delta { text: "答".into() }),
        ));
        assert_eq!(p.phase(), Phase::Running);
        assert_eq!(p.count(Domain::Control), 1);
        assert_eq!(p.count(Domain::Cognition), 1);

        p.apply(&end(3));
        assert_eq!(p.phase(), Phase::Closed);
        assert_eq!(p.seq(), 3);
        assert_eq!(p.applied(), 3);
    }

    /// 收口是稳定终态：收口后的杂项事件不把它改回运行中。
    #[test]
    fn close_is_sticky_until_next_run() {
        let mut p = Projection::default();
        p.apply(&start(1));
        p.apply(&end(2));
        p.apply(&ev(
            3,
            Body::Control(Control::State {
                name: "unknown".into(),
            }),
        ));
        assert_eq!(p.phase(), Phase::Closed);
    }

    /// 帧号水位只增不减：乱序到达的旧帧不把水位拉回去。
    #[test]
    fn watermark_is_monotonic() {
        let mut p = Projection::new();
        p.apply(&start(7));
        p.apply(&start(3));
        assert_eq!(p.seq(), 7);
    }

    fn delta(seq: u64, text: &str) -> Event {
        ev(seq, Body::Cognition(Cognition::Delta { text: text.into() }))
    }

    /// 增量路径：新事件只追加自己的行，旧行逐字段不变（旧帧不重算）。
    #[test]
    fn appends_without_rewriting_history() {
        let mut p = Projection::new();
        for i in 1..=3u64 {
            p.apply(&delta(i, "片段"));
        }
        let frozen = p.msgs().rows().to_vec();
        assert_eq!(frozen.len(), 3);

        p.apply(&delta(4, "新片"));
        assert_eq!(&p.msgs().rows()[..3], frozen.as_slice());
        assert_eq!(p.msgs().len(), 4);
        assert_eq!(p.msgs().last().map(|m| m.seq), Some(4));
    }

    /// 非认知事件不占消息表：计数增加而表长度不变。
    #[test]
    fn control_events_leave_table_alone() {
        let mut p = Projection::new();
        p.apply(&start(1));
        p.apply(&end(2));
        assert!(p.msgs().is_empty());
        assert_eq!(p.count(Domain::Control), 1);
        assert_eq!(p.count(Domain::Outcome), 1);
    }
}
