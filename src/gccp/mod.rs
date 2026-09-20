// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP（任务事实确认）与 GRAD（任务流程图确认）。
//
// 大任务集启动时进入 GCCP：任务事实确认共 5 个问题，**逐一询问**（5 轮，每轮 1 问），
// 每轮之间让 LLM 基于已答事实思考，使下一个问题更精准：
//   第 1 轮：提出第 1 问，用户作答 → LLM 思考
//   第 2 轮：提出第 2 问，用户作答 → LLM 思考
//   ……
//   第 5 轮：提出第 5 问，用户作答
// 五问齐备后进入 GRAD（任务流程图确认）：LLM 生成执行流程图，用户确认后开始执行。
//
// 0.1.18 按职责域分文件（文件行数纪律：单文件 ≤800 行），本模块仅作装配与再导出，
// 外部路径仍为 `crate::gccp::X`。分层如下：
//   - state：阶段状态机 / 执行控制态 / 五问五答状态容器 / DAG 节点状态
//   - dag：DAG 数据结构 + LLM 输出解析 + ASCII 依赖图渲染
//   - prompt：GCCP 逐轮提示词 / GRAD 提示词 / 执行上下文装配
//   - parse：LLM 问题解析 / 用户回答解析 / 确认与完成指令识别
//
// 宽度纪律（0.1.18 A 轨 W2，§3.3）：显示宽度测量与截断一律委托 L2 唯一裁决点
// `crate::engine::grid`，本模块不得自行测量后截断或填充。

mod dag;
mod parse;
mod prompt;
mod state;

#[cfg(test)]
mod tests;

pub use dag::{parse_dag, render_dag_lines};
pub use parse::{
    has_task_done_marker, is_confirm, is_task_done_input, parse_answers, parse_questions,
    strip_task_done,
};
pub use prompt::{build_execute_prompt, build_grad_prompt, build_qn_prompt};
pub use state::{FlowPhase, GccpState, NodeState, TaskControl};
