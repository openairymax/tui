// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 时间线归并（0.1.21 X6-01a，§23.5.2 X6 时间线数据折叠）。
//
// 三张表各自只记自己的事实：任务表记每轮身份与当前态，步骤表记计划与进度，
// 工具表记调用与结果。时间线要回答的是「这一轮按依赖序发生了什么」，因此在
// 读侧一次归并，不另建第四份状态——否则每次事件都要维护两处一致。
//
// 排列规则（依赖序，不是到达序）：
//   1. 任务行是每一轮的头部，按首次出现顺序排在最前；
//   2. 步骤行按 (任务在任务表中的次序, 服务端步序) 排列——计划本身就是依赖
//      骨架，乱序到达不得把第 3 步排到第 1 步前面；
//   3. 工具行按到达序（seq）排列；同一次调用在工具表内已收尾，这里只取一行。
//
// 状态词表（X6-01b）：三张表各带自己的状态串，若各处自行配色，状态栏与
// 主区就会各说各话。归并在 `Mark::classify` 做一次，下游只认 D-13 八态；
// skipped 并入 cancelled（都是「无产出收口」），done/ok 并入 completed，
// running 并入 executing。未知态按 submitted 呈现——中性排队，不猜成败。
//
// 归并是纯的、可重复的：同样的三张表每次折出同一个序列，渲染层可按帧缓存。
// 时间线不猜事件语义，只排列三张表已有的行；语义在各自的表里已经定过。

#![cfg_attr(not(test), allow(dead_code))]

use ratatui::style::Color;

use super::steps::{self, Steps};
use super::tasks::Tasks;
use super::tools::{self, Status, Tools};
use crate::style;

/// 条目类别：任务头 / 计划步骤 / 工具调用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Task,
    Step,
    Tool,
}

/// 一条时间线条目：渲染层拿到的是定值快照，不再回表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) kind: Kind,
    pub(crate) seq: u64,
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) state: String,
}

impl Entry {
    /// 条目状态折叠为 D-13 八态：渲染层据此取色，不再自带词表。
    pub(crate) fn mark(&self) -> Mark {
        Mark::classify(&self.state)
    }
}

/// D-13 八态词表（agent_task_contract.h 的 task_state 枚举）。
///
/// 三张表的状态串在此折叠成一份：步骤 running→executing、done/ok→completed、
/// skipped→cancelled；任务态本就是八态原词。未知串按 submitted 呈现——中性
/// 排队，不猜成败，服务端新增态时界面不至于说谎。配色同表给出，八态在
/// 深浅两档主题下两两不同色；8 色档的降级辨识由 X7-03 另行裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Submitted,
    Planned,
    Executing,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Unverified,
}

impl Mark {
    /// 状态串 → 八态：折叠点唯一，语义与配色都从这里走。
    pub(crate) fn classify(state: &str) -> Self {
        match state {
            "planned" => Self::Planned,
            "executing" | "running" => Self::Executing,
            "paused" | "waiting" => Self::Paused,
            "completed" | "done" | "ok" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" | "canceled" | "skipped" => Self::Cancelled,
            "unverified" => Self::Unverified,
            _ => Self::Submitted,
        }
    }

    /// 八态 → 状态字形：形状本身即语义，8 色/单色档不依赖颜色也读得出
    /// （X7-03b 的前置约定）。
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Submitted => "○",
            Self::Planned => "◌",
            Self::Executing => "◐",
            Self::Paused => "◍",
            Self::Completed => "●",
            Self::Failed => "✕",
            Self::Cancelled => "—",
            Self::Unverified => "?",
        }
    }

    /// 八态 → 主题色（呈现策略）：X6-02a 时间线与 X7 色档共用此表，
    /// 深浅主题与色深映射由 style 层统一裁决。
    pub(crate) fn color(self) -> Color {
        match self {
            Self::Submitted => style::faint(),
            Self::Planned => style::dim(),
            Self::Executing => style::accent(),
            Self::Paused => style::warning(),
            Self::Completed => style::success(),
            Self::Failed => style::danger(),
            Self::Cancelled => style::magenta(),
            Self::Unverified => style::cyan(),
        }
    }
}

/// 三表 → 依赖序条目序列。
pub(crate) fn merge(tasks: &Tasks, steps: &Steps, tools: &Tools) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(tasks.len() + steps.len() + tools.len());

    for row in tasks.rows() {
        out.push(Entry {
            kind: Kind::Task,
            seq: row.seq,
            id: row.id.clone(),
            label: row.id.clone(),
            state: row.state.clone(),
        });
    }

    // 步骤先按任务在任务表中的次序分组，再按服务端步序排列；任务表里没有
    // 的任务（例如步骤先于状态帧到达）排在已知任务之后，保持到达序。
    let mut ordered: Vec<&steps::Row> = steps.rows().iter().collect();
    ordered.sort_by_key(|row| (rank(tasks, &row.task), row.index, row.seq));
    for row in ordered {
        out.push(Entry {
            kind: Kind::Step,
            seq: row.seq,
            id: format!("{}#{}", row.task, row.index),
            label: step_label(row),
            state: row.status.clone(),
        });
    }

    for row in tools.rows() {
        out.push(Entry {
            kind: Kind::Tool,
            seq: row.seq,
            id: row.id.clone(),
            label: tool_label(row),
            state: tool_state(row.status).to_owned(),
        });
    }

    out
}

/// 任务在任务表中的次序；不在表内排到已知任务之后。
fn rank(tasks: &Tasks, task: &str) -> usize {
    tasks
        .rows()
        .iter()
        .position(|row| row.id == task)
        .unwrap_or(usize::MAX)
}

/// 步骤标签：标题为空时退回「步骤 N」（步序 0 基，显示 1 基）。
fn step_label(row: &steps::Row) -> String {
    if row.title.is_empty() {
        format!("步骤 {}", row.index.saturating_add(1))
    } else {
        row.title.clone()
    }
}

/// 工具标签：缺名调用不能显示为空白，退回固定文案由兜底卡细化。
fn tool_label(row: &tools::Row) -> String {
    if row.name.is_empty() {
        "(未知工具)".to_string()
    } else {
        row.name.clone()
    }
}

/// 工具状态 → 稳定短名（渲染层据此选色，见 X6-01b）。
fn tool_state(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Ok => "ok",
        Status::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::{merge, Kind, Mark};
    use crate::engine::event::model::{Body, Control, Event, Execution};
    use crate::engine::project::steps::Steps;
    use crate::engine::project::tasks::Tasks;
    use crate::engine::project::tools::Tools;
    use crate::engine::project::Projection;
    use serde_json::json;

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

    fn step(seq: u64, task: &str, index: usize, total: usize, status: &str, title: &str) -> Event {
        ev(
            seq,
            Body::Execution(Execution::Step {
                task: task.into(),
                index,
                total,
                status: status.into(),
                title: title.into(),
            }),
        )
    }

    fn call(seq: u64, id: &str, name: &str) -> Event {
        ev(
            seq,
            Body::Execution(Execution::Call {
                id: id.into(),
                name: name.into(),
                args: json!({"path": "a"}),
            }),
        )
    }

    fn done(seq: u64, id: &str, ok: bool) -> Event {
        ev(
            seq,
            Body::Execution(Execution::Result {
                id: id.into(),
                ok,
                text: if ok { "ok" } else { "denied" }.into(),
            }),
        )
    }

    /// 空三表归并为空。
    #[test]
    fn empty_tables_merge_empty() {
        let got = merge(&Tasks::new(), &Steps::new(), &Tools::new());
        assert!(got.is_empty());
    }

    /// 任务头在前，步骤按步序，工具按到达序。
    #[test]
    fn orders_tasks_steps_tools() {
        let (mut tasks, mut steps, mut tools) = (Tasks::new(), Steps::new(), Tools::new());
        tasks.apply(&state(1, "t1", "running"));
        steps.apply(&step(2, "t1", 1, 2, "planned", "生成报告"));
        steps.apply(&step(3, "t1", 0, 2, "planned", "读取依赖"));
        tools.apply(&call(4, "c1", "read"));
        tools.apply(&done(5, "c1", true));
        tools.apply(&call(6, "c2", "shell"));

        let got = merge(&tasks, &steps, &tools);
        let kinds: Vec<Kind> = got.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [Kind::Task, Kind::Step, Kind::Step, Kind::Tool, Kind::Tool]
        );
        assert_eq!((got[0].id.as_str(), got[0].seq), ("t1", 1));
        assert_eq!(got[0].label, "t1");
        // 步序优先于到达序：先到的第 2 步排在后面。
        assert_eq!(got[1].label, "读取依赖");
        assert_eq!((got[1].id.as_str(), got[1].seq), ("t1#0", 3));
        assert_eq!(got[2].label, "生成报告");
        assert_eq!(
            (got[3].label.as_str(), got[3].state.as_str()),
            ("read", "ok")
        );
        assert_eq!(
            (got[4].label.as_str(), got[4].state.as_str()),
            ("shell", "running")
        );
    }

    /// 无任务号的步骤、缺名工具都有可读标签，不留空白行。
    #[test]
    fn missing_labels_fall_back() {
        let (tasks, mut steps, mut tools) = (Tasks::new(), Steps::new(), Tools::new());
        steps.apply(&step(1, "", 0, 1, "planned", ""));
        tools.apply(&call(2, "c1", ""));
        let got = merge(&tasks, &steps, &tools);
        assert_eq!(got[0].label, "步骤 1");
        assert_eq!(got[1].label, "(未知工具)");
    }

    /// 走真实投影：事件流折叠后即可算出时间线，步骤推进不产生第二行。
    #[test]
    fn projection_exposes_merged_timeline() {
        let mut p = Projection::new();
        p.apply(&state(1, "t1", "running"));
        p.apply(&step(2, "t1", 0, 1, "planned", "读取依赖"));
        p.apply(&call(3, "c1", "read"));
        p.apply(&done(4, "c1", true));
        p.apply(&step(5, "t1", 0, 1, "done", ""));

        let got = p.timeline();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].kind, Kind::Task);
        assert_eq!((got[1].kind, got[1].state.as_str()), (Kind::Step, "done"));
        assert_eq!((got[2].kind, got[2].state.as_str()), (Kind::Tool, "ok"));
        assert_eq!(p.steps().len(), 1, "步骤推进不得追加第二行");
    }

    /// X6-01b：三张表的同义词折进八态，未知串归 submitted，不猜成败。
    #[test]
    fn states_fold_into_eight_marks() {
        let cases = [
            ("submitted", Mark::Submitted),
            ("planned", Mark::Planned),
            ("executing", Mark::Executing),
            ("running", Mark::Executing),
            ("paused", Mark::Paused),
            ("waiting", Mark::Paused),
            ("completed", Mark::Completed),
            ("done", Mark::Completed),
            ("ok", Mark::Completed),
            ("failed", Mark::Failed),
            ("cancelled", Mark::Cancelled),
            ("canceled", Mark::Cancelled),
            ("skipped", Mark::Cancelled),
            ("unverified", Mark::Unverified),
            ("", Mark::Submitted),
            ("brand-new-state", Mark::Submitted),
        ];
        for (state, want) in cases {
            assert_eq!(Mark::classify(state), want, "折叠走样: {state:?}");
        }
    }

    /// X6-01b：折叠正确——真实事件流折出的时间线，八态逐条判对。
    #[test]
    fn folded_marks_are_correct() {
        let mut p = Projection::new();
        p.apply(&state(1, "t1", "executing"));
        p.apply(&step(2, "t1", 0, 1, "running", "读取依赖"));
        p.apply(&call(3, "c1", "read"));
        p.apply(&done(4, "c1", true));
        p.apply(&step(5, "t1", 0, 1, "done", ""));

        let got = p.timeline();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].mark(), Mark::Executing);
        assert_eq!(got[1].mark(), Mark::Completed, "步骤 done 折为 completed");
        assert_eq!(got[2].mark(), Mark::Completed, "工具 ok 折为 completed");
    }

    /// X6-01b：八态配色表全量可达。色间可辨与对比度不在此断言——CI 无
    /// TERM 时色深为 Basic，多态会坍缩到同一 ANSI 档；可辨性归 X7-02b
    /// （对比度）与 X7-03b（8 色 + 符号）按色深另行裁决。
    #[test]
    fn eight_marks_map_to_theme_colors() {
        for m in [
            Mark::Submitted,
            Mark::Planned,
            Mark::Executing,
            Mark::Paused,
            Mark::Completed,
            Mark::Failed,
            Mark::Cancelled,
            Mark::Unverified,
        ] {
            let _ = m.color();
        }
    }
}
