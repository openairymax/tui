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
// 归并是纯的、可重复的：同样的三张表每次折出同一个序列，渲染层可按帧缓存。
// 时间线不猜事件语义，只排列三张表已有的行；语义在各自的表里已经定过。

#![cfg_attr(not(test), allow(dead_code))]

use super::steps::{self, Steps};
use super::tasks::Tasks;
use super::tools::{self, Status, Tools};

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
    use super::{merge, Kind};
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
}
