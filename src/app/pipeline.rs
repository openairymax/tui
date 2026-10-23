// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后台请求流水线类型：在途请求种类、句柄、结果载荷，以及看板/事件流
// 轮询与运维命令的结果包。请求在 tokio 任务中执行，结果经 oneshot 回传。

use anyhow::Result;

use crate::client::{GccpQuestion, HallBoard, HallEvent, HallTask, RunResponse};


/// hall 面板轮询结果（看板/事件流二选一）。
pub(super) enum HallPollOutcome {
    Board(Result<HallBoard>),
    Events(Result<Vec<HallEvent>>),
}

/// /chain 决策链查询结果（任务列表 / 单任务事件链）。
pub(super) enum ChainOutcome {
    Tasks(Result<Vec<HallTask>>),
    Events(Result<Vec<HallEvent>>),
}

/// 运维命令结果（/daemons 聚合 / 通用方法调用）。
pub(super) enum OpsOutcome {
    /// 16 个 daemon 的 health_check 结果（ns, 结果）
    Daemons(Vec<(String, Result<serde_json::Value>)>),
    /// 单个 gateway 方法调用结果
    Call(Result<serde_json::Value>),
}

/// /daemons 探测清单（0.1.9 M4 整编后口径）：14 个可经 gateway FWD 探测
/// 的业务 daemon 命名空间。plugin/info/observe 为整编兼容别名（→tool/
/// monit），列入即同一 daemon 重复计数；gateway_d 自身以顶部连接状态呈现
/// （连接断开时 /daemons 本就不可达）；maths_d 随 0.1.9 M4 补入。
pub(super) const OPS_DAEMON_NS: [&str; 14] = [
    "agent", "tool", "think", "monit", "sched", "channel", "market", "llm", "cupolas", "mem",
    "notify", "hook", "a2a", "maths",
];

/// 后台 LLM 请求的类型（决定结果如何应用）。
#[derive(Debug)]
pub(super) enum PendingKind {
    /// 普通对话 / 任务执行轮（原 chat_round）
    ChatRound { input: String },
    /// 流式对话轮（SSE 增量渲染，普通对话走此路径）
    StreamRound { input: String },
    /// GCCP 提问轮（原 ask_gccp_round）
    AskGccp { round: u8 },
    /// 五问齐备 → 生成 GRAD 任务流程图（原 gccp_round3 网络部分）
    GradPlan,
    /// GRAD 确认（confirmed=true，开始执行）或修订（confirmed=false）
    GradConfirm { confirmed: bool },
    /// 任务完成经验蒸馏（原 complete_task 网络部分）
    Distill,
    /// 未连接时的连接检查：通过后继续执行 kind/prompt 的真实请求
    CheckConnect {
        kind: Box<PendingKind>,
        prompt: String,
        /// 待继续请求携带的 agent 编排 spec（连接通过后透传）
        agent: Option<serde_json::Value>,
        /// 待继续请求携带的完整对话历史（连接通过后透传）
        history: Option<serde_json::Value>,
        /// 待继续请求携带的 GCCP 交互答案 JSON（连接通过后透传）
        gccp_answers: Option<String>,
    },
}

/// 后台 LLM 请求句柄：网关调用在 tokio 任务中执行，结果经 oneshot 回传。
/// `task` 持有 JoinHandle，供人工中止（Ctrl+X）时取消后台请求。
pub(super) struct PendingTurn {
    pub(super) rx: tokio::sync::oneshot::Receiver<PendingOutcome>,
    pub(super) kind: PendingKind,
    pub(super) task: Option<tokio::task::JoinHandle<()>>,
    /// 客户端预分配会话 ID（Ctrl+X 时调用 gateway agent.cancel 中止运行中请求）
    pub(super) session_id: String,
    /// 流式输出接收端（StreamRound）：SSE 增量块（option：非流式请求为 None）
    pub(super) stream_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// 流式工具事件接收端（tool_call/tool_result JSON，option：非流式请求为 None）
    pub(super) tool_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
}

/// 后台请求的结果载荷（LLM 调用结果 / 连接检查结果）。
pub(super) enum PendingOutcome {
    /// LLM 调用结果
    Run(Result<RunResponse>),
    /// 连接检查结果（成功与否）
    Connect(bool),
}

/// GCCP 两段式交互第一段挂起状态（P-A）：think.process 返回
/// gccp_need_interaction 后暂存问题集与原始请求，待用户作答后以
/// gccp_answers 重发同一 prompt 完成澄清闭环（见 think_service.h）。
pub struct GccpPending {
    /// 原始用户输入（重发时作为 ChatRound.input，保持记忆/模式判定语义）
    pub(super) raw_input: String,
    /// 原始 prompt（与第一段发送的一致，重发时保持同一任务上下文）
    pub(super) prompt: String,
    /// 原始完整对话历史（重发时透传 gateway）
    pub(super) history: Option<serde_json::Value>,
    /// 服务端回传的问题集（id/question/hint/required；panels/chat.rs 渲染用）
    pub questions: Vec<GccpQuestion>,
    /// 已收集答案（key = 问题 id；panels/chat.rs 渲染进度用）
    pub answers: std::collections::BTreeMap<String, serde_json::Value>,
}
