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

#[derive(Debug, Default, Deserialize)]
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
    /// run_end 终态（completed/paused/failed）：任务集状态迁移的唯一事实源
    /// （C-14-07）；旧引擎不带该键时按 completed 处理。
    #[serde(default)]
    pub status: Option<String>,
    /// 预算耗尽（AIRY_EBUDGET）时引擎下发的续跑令牌；携带它即可从断点
    /// 继续，无需重放整轮历史。
    #[serde(default)]
    pub resume_token: Option<String>,
    /// 已消耗工具轮次 / 协商上限（预算耗尽时用于界面说明）。
    #[serde(default)]
    pub rounds_used: Option<i64>,
    #[serde(default)]
    pub rounds_limit: Option<i64>,
    /// C-15：服务端任务身份（task_id 即本次 run_id，CLI/TUI/SDK 与重连
    /// 共享同一条时间线）。
    #[serde(default)]
    pub task_id: Option<String>,
    /// C-15：计划标识（plan 帧同源）。
    #[serde(default)]
    pub plan_id: Option<String>,
    /// C-15：服务端模式裁决（chat/task）。TUI 阶段切换的唯一事实源；
    /// 正文 [MODE:*] 标记降级为提示。
    #[serde(default)]
    pub mode: Option<String>,
    /// C-15：模式裁决依据（rule:engineering_verb / explicit:client_declared_task …）
    #[serde(default)]
    pub mode_reason: Option<String>,
    /// C-15：needs_intent 门禁对象（未确认目标时零引擎/零工具调用）。
    /// data 形如 {status,task_id,mode,mode_reason,gccp_missing[],message}
    #[serde(default)]
    pub needs_intent: Option<serde_json::Value>,
    /// C-15：计划不可用原因（think_unavailable / plan_missing）；携带时
    /// 表示本轮按 ReAct 工具循环直接执行，界面如实告知降级。
    #[serde(default)]
    pub plan_unavailable: Option<String>,
    /// D-11：run_end.usage 结构化 token 账本。引擎与 llm_d 同源计数，
    /// 界面直接显示 total，不再按字符数或 use_ticks 换算。
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// D-11：run_end.usage 的 wire 形态。TUI 只消费 total 与 source —— 界面
/// 不做拆分换算；prompt/completion 由需要的调用方（SDK/审计）自行读取。
/// source=estimated 表示引擎未能取得 provider 计数，界面以 `~` 标注。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub source: String,
}

impl Usage {
    /// 估算来源：total 不是 provider 原生计数，展示时需与精确值区分。
    pub fn estimated(&self) -> bool {
        self.source == agentrt_rs::run_stream::gen::AIRY_RS_USAGE_ESTIMATE
    }
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
    /// 工具调用参数（/trace 轨迹面板按需查看，0.1.20 批次五）
    pub arguments: String,
    pub result: String,
    pub ok: Option<i64>,
    /// 换路原因（C-14-04）：同一调用连续失败触发强制换策略时由引擎标注，
    /// 失败步骤视图据此说明「为什么必须换参数/换工具」。
    #[serde(default)]
    pub switch_reason: Option<String>,
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
///
/// 网关返回的是账本落盘事件文件信封
/// `{"file":{id,category,task_id,node_id,ts_utc,seq,gseq,...},"access":{...},"content":<事件内容>}`
/// （C-18：账本单点写盘格式），而面板与 /chain 消费的是扁平视图。故此处
/// 手工反序列化：优先读 "file" 信封并摊平，同时兼容旧扁平形态，避免 TUI
/// 因信封嵌套而静默丢字段（文件 id 在信封中名为 "id"）。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HallEvent {
    pub file_id: String,
    pub category: String,
    pub task_id: String,
    pub tenant_id: String,
    pub node_id: String,
    pub ts_utc: String,
    pub seq: u64,
    pub gseq: u64,
    pub content: serde_json::Value,
}

impl HallEvent {
    fn from_value(v: &serde_json::Value) -> Self {
        let src = v.get("file").unwrap_or(v);
        let field_str = |key: &str| -> String {
            src.get(key)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let field_u64 = |key: &str| -> u64 {
            src.get(key).and_then(|x| x.as_u64()).unwrap_or_default()
        };
        HallEvent {
            file_id: {
                let id = field_str("file_id");
                if id.is_empty() {
                    field_str("id")
                } else {
                    id
                }
            },
            category: field_str("category"),
            task_id: field_str("task_id"),
            tenant_id: field_str("tenant_id"),
            node_id: field_str("node_id"),
            ts_utc: field_str("ts_utc"),
            seq: field_u64("seq"),
            gseq: field_u64("gseq"),
            content: v
                .get("content")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        }
    }
}

impl<'de> Deserialize<'de> for HallEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        Ok(HallEvent::from_value(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::HallEvent;

    /// 网关 hall.replay/hall.stream 的真实信封形态（账本落盘文件原样）。
    #[test]
    fn hall_event_unwraps_file_envelope() {
        let raw = r#"{"file":{"id":"default.t1.progress.20261117T010203004.0001.json",
            "category":"progress","schema":"hall.event.v1","tenant_id":"default",
            "task_id":"t1","node_id":null,"ts_utc":"20261117T010203004","seq":1,
            "gseq":7,"prev_file":""},
            "access":{"owner_role":"executor","write_roles":["executor"],
            "read_roles":["cognition"]},
            "content":{"event":"task_state","state":"executing","progress":0.5}}"#;
        let e: HallEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(e.file_id, "default.t1.progress.20261117T010203004.0001.json");
        assert_eq!(e.category, "progress");
        assert_eq!(e.task_id, "t1");
        assert_eq!(e.tenant_id, "default");
        assert_eq!(e.ts_utc, "20261117T010203004");
        assert_eq!(e.seq, 1);
        assert_eq!(e.gseq, 7);
        assert_eq!(e.content["state"], "executing");
    }

    /// 旧扁平形态仍可解析（兼容已缓存/手工构造的事件）。
    #[test]
    fn hall_event_accepts_flat_shape() {
        let raw = r#"{"file_id":"f1","category":"result","task_id":"t2",
            "gseq":3,"content":{"state":"completed"}}"#;
        let e: HallEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(e.file_id, "f1");
        assert_eq!(e.category, "result");
        assert_eq!(e.gseq, 3);
    }

    /// D-11：run_end.usage 按 wire 形态解析，total 直接采用；estimated
    /// 来源必须可识别（界面加 `~`），旧 result 无 usage 时保持 None。
    #[test]
    fn usage_is_read_verbatim() {
        let raw = r#"{"prompt":120,"completion":30,"total":150,"source":"provider"}"#;
        let u: super::Usage = serde_json::from_str(raw).unwrap();
        assert_eq!(u.total, 150);
        assert!(!u.estimated());

        let est: super::Usage =
            serde_json::from_str(r#"{"prompt":0,"completion":0,"total":42,"source":"estimated"}"#)
                .unwrap();
        assert!(est.estimated());

        let legacy: super::RunResponse =
            serde_json::from_str(r#"{"session_id":"s","response":"ok"}"#).unwrap();
        assert!(legacy.usage.is_none());
    }
}
