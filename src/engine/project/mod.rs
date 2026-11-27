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

use std::collections::BTreeMap;

use serde_json::Value;

use super::event::model::{Body, Control, Domain, Event, Outcome};

pub(crate) mod approvals;
pub(crate) mod dirty;
pub(crate) mod msgs;
pub(crate) mod steps;
pub(crate) mod tasks;
pub(crate) mod timeline;
pub(crate) mod tools;

#[cfg(test)]
mod support;

use self::approvals::Approvals;
use self::dirty::{Dirty, Span};
use self::msgs::Msgs;
use self::steps::Steps;
use self::tasks::Tasks;
use self::timeline::Entry;
use self::tools::Tools;

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

/// 门禁：暂停原因 + 服务端原始对象（呈递层不再向网关追问）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gate {
    pub(crate) name: String,
    pub(crate) payload: Value,
}

/// 失败：机器原因、人话、下一步动作（D-10 原样承接）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fault {
    pub(crate) reason: String,
    pub(crate) text: String,
    pub(crate) next: String,
}

/// 折叠结果：水位 + 计数 + 相位 + 消息/任务/步骤/工具/审批表 + 待刷新行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Projection {
    applied: u64,
    seq: u64,
    phase: Phase,
    counts: [u64; 4],
    msgs: Msgs,
    tasks: Tasks,
    steps: Steps,
    tools: Tools,
    approvals: Approvals,
    dirty: Dirty,
    terminal: Option<String>,
    gate: Option<Gate>,
    fault: Option<Fault>,
    cfg: BTreeMap<String, Value>,
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
            tasks: Tasks::new(),
            steps: Steps::new(),
            tools: Tools::new(),
            approvals: Approvals::new(),
            dirty: Dirty::new(),
            terminal: None,
            gate: None,
            fault: None,
            cfg: BTreeMap::new(),
        }
    }

    /// 折叠一条事件：只改本结构，返回值留空以便后续卡接脏区。
    ///
    /// 增量语义：每条事件只落到该落的表，已入表的行不重算、不改写；
    /// 新入表的消息行同时记为待刷新，渲染器按区间取走。
    pub(crate) fn apply(&mut self, ev: &Event) {
        self.applied = self.applied.saturating_add(1);
        self.seq = self.seq.max(ev.seq);
        let slot = slot(ev.domain());
        self.counts[slot] = self.counts[slot].saturating_add(1);
        self.phase = next_phase(self.phase, &ev.body);
        match &ev.body {
            Body::Control(Control::State { name, .. }) if name == "running" => {
                self.terminal = None;
                self.gate = None;
                self.fault = None;
                self.cfg.clear();
            }
            Body::Control(Control::Gate { name, payload }) => {
                self.gate = Some(Gate {
                    name: name.clone(),
                    payload: payload.clone(),
                });
            }
            Body::Control(Control::Cfg { key, value }) => {
                self.cfg.insert(key.clone(), value.clone());
            }
            Body::Outcome(Outcome::End { data, .. }) => {
                self.terminal = if data.is_empty() {
                    None
                } else {
                    Some(data.clone())
                };
            }
            Body::Outcome(Outcome::Err { reason, text, next }) => {
                self.fault = Some(Fault {
                    reason: reason.clone(),
                    text: text.clone(),
                    next: next.clone(),
                });
            }
            _ => {}
        }
        self.tasks.apply(ev);
        self.steps.apply(ev);
        self.tools.apply(ev);
        self.approvals.apply(ev);
        if let Some(row) = self.msgs.push(ev) {
            self.dirty.mark(row);
        }
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

    /// 本轮终态载荷原文（服务端 run_end 的 data 段）；未收口或未带载荷为空。
    pub(crate) fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    /// 当前门禁：运行暂停并等人作答；新一轮开跑时清空。
    pub(crate) fn gate(&self) -> Option<&Gate> {
        self.gate.as_ref()
    }

    /// 摘取门禁：门禁只在本轮结算时消费一次，避免跨轮重复呈递。
    pub(crate) fn take_gate(&mut self) -> Option<Gate> {
        self.gate.take()
    }

    /// 本轮失败：尚未被下一轮开跑清除前有效。
    pub(crate) fn fault(&self) -> Option<&Fault> {
        self.fault.as_ref()
    }

    /// 开跑配置：服务端裁决下发的键值（model/mode/mode_reason/gccp_confirmed）。
    pub(crate) fn cfg(&self, key: &str) -> Option<&Value> {
        self.cfg.get(key)
    }

    /// 某域已折叠的事件数（状态栏与诊断直接读）。
    pub(crate) fn count(&self, d: Domain) -> u64 {
        self.counts[slot(d)]
    }

    /// 消息表只读视图。
    pub(crate) fn msgs(&self) -> &Msgs {
        &self.msgs
    }

    /// 任务表只读视图。
    pub(crate) fn tasks(&self) -> &Tasks {
        &self.tasks
    }

    /// 步骤表只读视图（X6-01a）。
    pub(crate) fn steps(&self) -> &Steps {
        &self.steps
    }

    /// 任务/步骤/工具三表按依赖序归并成时间线（X6-01a）。
    pub(crate) fn timeline(&self) -> Vec<Entry> {
        timeline::merge(&self.tasks, &self.steps, &self.tools)
    }

    /// 工具表只读视图。
    pub(crate) fn tools(&self) -> &Tools {
        &self.tools
    }

    /// 审批表只读视图。
    pub(crate) fn approvals(&self) -> &Approvals {
        &self.approvals
    }

    /// 待刷新行只读视图（自检与诊断用）。
    pub(crate) fn dirty(&self) -> &Dirty {
        &self.dirty
    }

    /// 取走待刷新行：渲染器每帧消费一次。
    pub(crate) fn drain_dirty(&mut self) -> Vec<Span> {
        self.dirty.take()
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
        Body::Control(Control::State { name, .. }) => match name.as_str() {
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
    use serde_json::json;

    fn ev(seq: u64, body: Body) -> Event {
        Event { seq, ts: 0, body }
    }

    fn start(seq: u64) -> Event {
        ev(
            seq,
            Body::Control(Control::State {
                id: "t1".into(),
                name: "running".into(),
            }),
        )
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

    fn end(seq: u64) -> Event {
        ev(
            seq,
            Body::Outcome(Outcome::End {
                status: "completed".into(),
                data: "{\"status\":\"completed\"}".into(),
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

    /// 终态载荷随收口落地，并在下一次开跑时清空（不跨轮串用）。
    #[test]
    fn terminal_payload_follows_run_boundary() {
        let mut p = Projection::new();
        assert_eq!(p.terminal(), None);
        p.apply(&start(1));
        p.apply(&end(2));
        assert_eq!(p.terminal(), Some("{\"status\":\"completed\"}"));
        p.apply(&start(3));
        assert_eq!(p.terminal(), None);
    }

    /// 门禁、失败与开跑配置各就各位，并在下一轮开跑时一起清空。
    #[test]
    fn gate_fault_cfg_follow_run_boundary() {
        let mut p = Projection::new();
        p.apply(&start(1));
        p.apply(&ev(
            2,
            Body::Control(Control::Cfg {
                key: "mode".into(),
                value: json!("task"),
            }),
        ));
        p.apply(&ev(
            3,
            Body::Control(Control::Gate {
                name: "needs_criteria".into(),
                payload: json!({"goal": "g"}),
            }),
        ));
        assert_eq!(p.cfg("mode").and_then(|v| v.as_str()), Some("task"));
        let gate = p.gate().expect("门禁");
        assert_eq!(
            (gate.name.as_str(), &gate.payload),
            ("needs_criteria", &json!({"goal": "g"}))
        );

        p.apply(&ev(
            4,
            Body::Outcome(Outcome::Err {
                reason: "upstream".into(),
                text: "上游不可达".into(),
                next: "retry".into(),
            }),
        ));
        let fault = p.fault().expect("失败");
        assert_eq!(
            (
                fault.reason.as_str(),
                fault.text.as_str(),
                fault.next.as_str()
            ),
            ("upstream", "上游不可达", "retry")
        );

        p.apply(&start(5));
        assert!(p.gate().is_none(), "新一轮不留旧门禁");
        assert!(p.fault().is_none(), "新一轮不留旧失败");
        assert!(p.cfg("mode").is_none(), "新一轮不留旧配置");
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
                id: "t1".into(),
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

    /// 脏区可枚举：只标记入表行，取走后清空，相邻区间合并为一段。
    #[test]
    fn dirty_ranges_follow_message_rows() {
        let mut p = Projection::new();
        p.apply(&delta(1, "一"));
        p.apply(&delta(2, "二"));
        p.apply(&start(3));
        assert_eq!(p.dirty().count(), 2);
        assert_eq!(p.dirty().segs(), 1);
        assert!(p.dirty().covers(1));
        assert!(!p.dirty().covers(2), "控制帧没有消息行");

        p.apply(&delta(4, "四"));
        let spans: Vec<(usize, usize)> = p.drain_dirty().iter().map(|s| (s.from, s.to)).collect();
        assert_eq!(spans, vec![(0, 3)], "控制帧不占行，三段自动并成一段");
        assert!(p.dirty().is_empty(), "取走后清空");
    }

    /// 任务表随事件流推进：同一身份只有一行，消息表不受影响。
    #[test]
    fn tasks_track_state_transitions() {
        let mut p = Projection::new();
        p.apply(&start(1));
        p.apply(&state(2, "t1", "executing"));
        p.apply(&delta(3, "答"));

        assert_eq!(p.tasks().len(), 1);
        let row = p.tasks().get("t1").expect("任务行");
        assert_eq!((row.state.as_str(), row.rev), ("executing", 2));
        assert_eq!(p.msgs().len(), 1, "状态帧不进消息表");

        p.apply(&state(4, "t2", "submitted"));
        assert_eq!(p.tasks().len(), 2);
    }

    /// 工具表随执行面事件推进：调用与结果合成一行，消息表不受影响。
    #[test]
    fn tools_track_calls() {
        use crate::engine::event::model::Execution;
        use crate::engine::project::tools::Status;

        let mut p = Projection::new();
        p.apply(&ev(1, Body::Execution(Execution::Tools { count: 4 })));
        p.apply(&ev(
            2,
            Body::Execution(Execution::Call {
                id: "c1".into(),
                name: "shell".into(),
                args: serde_json::json!("ls"),
            }),
        ));
        p.apply(&ev(
            3,
            Body::Execution(Execution::Result {
                id: "c1".into(),
                ok: true,
                text: "done".into(),
            }),
        ));

        assert_eq!(p.tools().catalog(), Some(4));
        assert_eq!(p.tools().len(), 1);
        assert_eq!(p.tools().rows()[0].status, Status::Ok);
        assert!(p.msgs().is_empty(), "执行帧不进消息表");
        assert_eq!(p.count(Domain::Execution), 3);
    }

    /// 审批表随控制面事件推进：请求与裁决合成一行，超时由后续帧收口。
    #[test]
    fn approvals_follow_control_frames() {
        use crate::engine::project::approvals::Verdict;

        let ask = |seq: u64, ts: u64, wait: u64| Event {
            seq,
            ts,
            body: Body::Control(Control::Ask {
                id: "a1".into(),
                tool: "shell".into(),
                target: "rm -rf build".into(),
                timeout_ms: wait,
            }),
        };

        let mut p = Projection::new();
        p.apply(&ask(1, 1_000, 200));
        assert_eq!(p.approvals().pending().count(), 1);
        assert!(p.msgs().is_empty());

        p.apply(&ev(
            2,
            Body::Control(Control::Answer {
                id: "a1".into(),
                ok: true,
            }),
        ));
        assert_eq!(
            p.approvals().get("a1").map(|r| r.verdict),
            Some(Verdict::Approved)
        );

        p.apply(&ask(3, 2_000, 50));
        assert_eq!(
            p.approvals().get("a1").map(|r| r.verdict),
            Some(Verdict::Approved)
        );
        assert_eq!(p.approvals().len(), 1, "同号请求不重复建行");
    }
}
