// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// gateway 响应类型定义（0.1.18 拆分自 client.rs）。
//
// 本模块只承载 wire → Rust 的反序列化契约：字段名与 serde 默认值即
// gateway JSON-RPC result 节点的真实形态，不含任何请求构造与网络逻辑。
// 未被 UI 消费但协议存在的字段一律 `#[allow(dead_code)]` 保留，保证
// 反序列化数据完整性（缺字段会污染整条响应链）。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct HealthResponse {
    pub status: String,
    pub version: Option<String>,
    pub uptime_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RunResponse {
    /// 网关侧会话 ID（客户端预分配的 sid_for_task 已用于 Ctrl+X 取消，
    /// 此处保留字段保证反序列化数据完整性，UI 刻意不展示）
    #[allow(dead_code)]
    pub session_id: String,
    pub response: String,
    pub tokens_used: Option<u64>,
    pub cost_usd: Option<f64>,
    /// 双思考轨迹：{plan: DAG 计划, feedback: GRAD 反馈, stats}
    pub thinking: Option<serde_json::Map<String, serde_json::Value>>,
    /// Agent 工具调用轨迹（LLM→工具→结果），供对话面板展示
    pub tool_trace: Option<Vec<ToolTrace>>,
    /// GCCP 两段式交互第一段：think.process 挂起，需要用户澄清
    /// （gccp_need_interaction=true 时客户端进入问答轮）
    #[serde(default)]
    pub gccp_need_interaction: bool,
    /// GCCP 交互问题集（id/question/hint/required），展示给用户收集答案
    #[serde(default)]
    pub gccp_questions: Vec<GccpQuestion>,
}

/// GCCP 目标澄清问题（think.process 第一段回传，见 gccp.h airy_gccp_question_t）
#[derive(Debug, Clone, Deserialize)]
pub struct GccpQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub hint: String,
    /// 是否必答（GCCP 协议字段；UI 当前不区分展示，保留保证反序列化完整）
    #[serde(default)]
    #[allow(dead_code)]
    pub required: bool,
}

/// 单次工具调用记录
#[derive(Debug, Clone, Deserialize)]
pub struct ToolTrace {
    pub tool: String,
    /// 工具调用参数（UI 克制设计不展示；保留字段保证反序列化数据完整性）
    #[allow(dead_code)]
    pub arguments: String,
    pub result: String,
    pub ok: Option<i64>,
}

/// 待人工决议的工具审批请求（tool.pending / tool.approve）。
#[derive(Debug, Clone, Deserialize)]
pub struct PendingApproval {
    pub request_id: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub params: String,
    #[serde(default)]
    pub created_at: Option<u64>,
}

/// 任务看板条目（work_hall 持久化执行实例快照）。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallBoardEntry {
    pub execution_id: String,
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub workflow_name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub task_id: u64,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub completed_at: u64,
}

/// hall.board 聚合结果：执行实例 + 在线 agent。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallBoard {
    pub entries: Vec<HallBoardEntry>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub agent_total: Option<u64>,
    #[serde(default)]
    pub source: String,
}

/// hall.tasks 条目：hall_store 磁盘任务枚举。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallTask {
    pub tenant_id: String,
    pub task_id: String,
    #[serde(default)]
    pub latest_ts: String,
    #[serde(default)]
    pub event_count: u64,
}

/// hall.replay / hall.stream 单条事件。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallEvent {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub ts_utc: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub gseq: u64,
    #[serde(default)]
    pub content: serde_json::Value,
}
