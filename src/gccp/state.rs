// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP/GRAD 流程状态子域（0.1.18 拆分自 gccp.rs）。
//
// 职责边界：任务流阶段状态机（FlowPhase）、执行控制态（TaskControl）、
// 「五问五答 + 流程图」状态容器（GccpState）与 DAG 节点状态（NodeState）。
// 本文件只管状态与状态迁移，不生成提示词（prompt.rs）、不解析 LLM 输出（parse.rs）。

use super::dag::TaskDag;

/// 任务流阶段状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPhase {
    /// 普通对话（非任务集）
    Chat,
    /// 任务事实确认（GCCP）第 N 轮：等待用户回答第 N 问（N = 1..=5）
    GccpRound(u8),
    /// 服务端 GCCP 目标澄清（P-A 两段式交互）：think.process 返回
    /// gccp_need_interaction，等待用户回答问题集（见 GccpPending）
    GccpClarify,
    /// 任务流程图确认（GRAD）：等待用户确认流程图
    GradConfirm,
    /// 任务集执行中
    Executing,
}

/// 任务集执行控制状态（人工中止/暂停）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskControl {
    /// 正常运行
    Running,
    /// 用户暂停（Ctrl+Z）：轮询挂起，可恢复
    Paused,
    /// 用户中止（Ctrl+X）：后台请求已取消，状态徽章展示中止态；
    /// 下次发起新交互（start_pending 等）时自动复位为 Running
    Aborted,
}

impl TaskControl {
    pub fn label(self) -> &'static str {
        match self {
            TaskControl::Running => "运行中",
            TaskControl::Paused => "已暂停",
            TaskControl::Aborted => "已中止",
        }
    }
}

impl FlowPhase {
    /// 状态栏展示名（中文术语）
    #[allow(dead_code)] // 单测使用；UI 按阶段匹配颜色自行展示
    pub fn label(self) -> &'static str {
        match self {
            FlowPhase::Chat => "对话",
            FlowPhase::GccpRound(_) => "任务事实确认",
            FlowPhase::GccpClarify => "目标澄清",
            FlowPhase::GradConfirm => "任务流程图确认",
            FlowPhase::Executing => "任务集",
        }
    }

    /// 输入框提示（引导用户作答）
    pub fn input_hint(self) -> String {
        match self {
            FlowPhase::GccpRound(n) => format!("回答第 {} 问：", n),
            FlowPhase::GccpClarify => {
                "回答 GCCP 澄清问题（空行逐条，或「跳过」放弃本轮问答）".to_string()
            }
            FlowPhase::GradConfirm => "输入「确认」通过流程图，或输入修改意见：".to_string(),
            // 普通对话 / 执行中：无引导语，前缀 ❯ 已足够
            FlowPhase::Chat | FlowPhase::Executing => String::new(),
        }
    }
}

/// GCCP 五问五答 + GRAD 流程图状态
#[derive(Debug, Clone, Default)]
pub struct GccpState {
    /// 任务目标（LLM 判定为大任务集时的原始输入）
    pub goal: String,
    pub q1: String,
    pub a1: String,
    pub q2: String,
    pub a2: String,
    pub q3: String,
    pub a3: String,
    pub q4: String,
    pub a4: String,
    pub q5: String,
    pub a5: String,
    /// GRAD 任务流程图（用户确认后进入执行）
    pub grad_plan: String,
    /// GRAD 结构化 DAG（由 [DAG] 块解析，用于可视化；解析失败时为 None）
    pub dag: Option<TaskDag>,
    /// DAG 节点执行状态（顺序与 dag.nodes 一致；无 dag 时为空数组）
    pub node_states: Vec<NodeState>,
}

/// DAG 节点执行状态（P2-C 过程可视化：Executing 阶段持续渲染）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeState {
    /// 未开始
    #[default]
    Pending,
    /// 执行中
    Running,
    /// 已完成
    Done,
    /// 执行失败/跳过（预留：逐节点失败反馈接线后构造）
    #[allow(dead_code)]
    Failed,
}

impl GccpState {
    /// 清空状态
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 初始化 DAG 节点状态（解析出 dag 后调用）：全部 Pending
    pub fn init_node_states(&mut self) {
        if let Some(dag) = &self.dag {
            self.node_states = vec![NodeState::Pending; dag.nodes.len()];
        } else {
            self.node_states.clear();
        }
    }

    /// 任务开始执行：全部节点进入 Running（无逐节点反馈时的诚实中间态）
    pub fn mark_all_running(&mut self) {
        for s in self.node_states.iter_mut() {
            if *s == NodeState::Pending {
                *s = NodeState::Running;
            }
        }
    }

    /// 任务完成：全部节点标记 Done
    pub fn mark_all_done(&mut self) {
        for s in self.node_states.iter_mut() {
            *s = NodeState::Done;
        }
    }

    /// 汇总 5 项已确认事实（Q+A 交错，供 GRAD 与后续执行使用）
    pub fn facts(&self) -> String {
        let mut out = String::new();
        for (q, a) in [
            (&self.q1, &self.a1),
            (&self.q2, &self.a2),
            (&self.q3, &self.a3),
            (&self.q4, &self.a4),
            (&self.q5, &self.a5),
        ] {
            if q.trim().is_empty() {
                continue;
            }
            out.push_str(&format!("Q: {}\nA: {}\n", q.trim(), a.trim()));
        }
        out
    }

    /// 已作答的问题数（a1-a5 非空计数，用于状态栏进度展示）
    pub fn answered(&self) -> usize {
        [&self.a1, &self.a2, &self.a3, &self.a4, &self.a5]
            .iter()
            .filter(|a| !a.trim().is_empty())
            .count()
    }
}
