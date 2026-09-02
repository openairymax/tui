// SPDX-FileCopyrightText: 2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! @file run_stream.rs
//! @brief agent.run_stream v1 事件帧解码（M5 W1，方案 §2.4）。
//!
//! SSoT：信封字段键 / 事件类型常量由 build.rs 从 C 侧唯一权威
//! airy_run_stream.h 生成（run_stream_gen 模块，与 sdk-rust 同机制），
//! 本模块只做结构化解析，禁止手写 wire 字符串字面量。
//!
//! 宽容读取（方案 §2.4.4）：未知事件类型归类 Unknown 不崩溃；未知字段
//! 透传；v/type 缺失视为无效帧返回 None。

/// 生成常量收纳：build.rs 生成全集（含 TUI 暂未消费的预留键）——
/// SSoT 要求与 C 头全集一致，未用常量放行不告警。
#[allow(dead_code)]
mod wire {
    include!(concat!(env!("OUT_DIR"), "/run_stream_gen.rs"));
}

pub use wire::gen;

use serde_json::Value;

/// 事件信封（wire JSON 外层，见方案 §2.4.2）。
#[derive(Debug, Clone, PartialEq)]
pub struct RunStreamEnvelope {
    /// 协议版本（v）
    pub protocol_v: i64,
    /// 事件类型（type，方案 §2.4.3 分层枚举）
    pub event_type: String,
    /// 帧序列号：同一 run 内单调递增
    pub id: i64,
    /// 本次运行唯一标识（run_ 前缀）
    pub run_id: Option<String>,
    /// 会话标识（sess_ 前缀，与 agent.cancel 对齐）
    pub session_id: Option<String>,
    /// 类型化负载（data，键名见 C 头各事件类型段）
    pub data: Value,
}

/// 事件分层（方案 §2.4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsKind {
    Control,
    Cognition,
    Execution,
    Outcome,
    Unknown,
}

impl RunStreamEnvelope {
    /// 从已解析 JSON 构造信封；v/type 缺失视为无效帧。
    pub fn from_value(v: &Value) -> Option<Self> {
        let protocol_v = v.get(gen::AIRY_RS_K_V)?.as_i64()?;
        let event_type = v.get(gen::AIRY_RS_K_TYPE)?.as_str()?.to_string();
        Some(Self {
            protocol_v,
            event_type,
            id: v.get(gen::AIRY_RS_K_ID).and_then(Value::as_i64).unwrap_or(0),
            run_id: v
                .get(gen::AIRY_RS_K_RUN_ID)
                .and_then(Value::as_str)
                .map(String::from),
            session_id: v
                .get(gen::AIRY_RS_K_SESSION)
                .and_then(Value::as_str)
                .map(String::from),
            data: v.get(gen::AIRY_RS_K_DATA).cloned().unwrap_or(Value::Null),
        })
    }

    /// 事件分层分类；未知类型返回 Unknown（宽容读取）。仅测试用。
    #[cfg(test)]
    pub fn kind(&self) -> RsKind {
        match self.event_type.as_str() {
            gen::AIRY_RS_TYPE_RUN_START
            | gen::AIRY_RS_TYPE_RUN_END
            | gen::AIRY_RS_TYPE_ERROR => RsKind::Control,
            gen::AIRY_RS_TYPE_PLAN => RsKind::Cognition,
            gen::AIRY_RS_TYPE_TOOL_START
            | gen::AIRY_RS_TYPE_TOOL_END
            | gen::AIRY_RS_TYPE_TOOL_DELTA => RsKind::Execution,
            gen::AIRY_RS_TYPE_TOKEN_DELTA | gen::AIRY_RS_TYPE_MESSAGE => RsKind::Outcome,
            _ => RsKind::Unknown,
        }
    }

    /// 取 data 内字符串字段（宽容缺失）。
    pub fn data_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }

    /// 取 data 内 i64 字段（宽容缺失）。
    pub fn data_i64(&self, key: &str) -> Option<i64> {
        self.data.get(key).and_then(Value::as_i64)
    }
}

/// 解析单条 SSE 行（"data: <json>"），返回事件信封；非 data 行 /
/// 无效帧返回 None（SSE 注释帧、空行、其他事件名均忽略）。
pub fn decode_sse_line(line: &str) -> Option<RunStreamEnvelope> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let rest = line.strip_prefix("data:")?.trim();
    let v: Value = serde_json::from_str(rest).ok()?;
    RunStreamEnvelope::from_value(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ---- SSoT 门禁：生成常量必须与 C 头语义一致 ---- */

    #[test]
    fn gen_constants_align_with_c_schema() {
        assert_eq!(gen::AIRY_RS_VERSION, 1);
        assert_eq!(gen::AIRY_RS_K_V, "v");
        assert_eq!(gen::AIRY_RS_K_TYPE, "type");
        assert_eq!(gen::AIRY_RS_K_RUN_ID, "run_id");
        assert_eq!(gen::AIRY_RS_K_SESSION, "session_id");
        assert_eq!(gen::AIRY_RS_K_DATA, "data");
        assert_eq!(gen::AIRY_RS_TYPE_RUN_START, "run_start");
        assert_eq!(gen::AIRY_RS_TYPE_RUN_END, "run_end");
        assert_eq!(gen::AIRY_RS_TYPE_ERROR, "error");
        assert_eq!(gen::AIRY_RS_TYPE_PLAN, "plan");
        assert_eq!(gen::AIRY_RS_TYPE_TOOL_START, "tool_start");
        assert_eq!(gen::AIRY_RS_TYPE_TOOL_END, "tool_end");
        assert_eq!(gen::AIRY_RS_TYPE_TOOL_DELTA, "tool_delta");
        assert_eq!(gen::AIRY_RS_TYPE_TOKEN_DELTA, "token_delta");
        assert_eq!(gen::AIRY_RS_TYPE_MESSAGE, "message");
        assert_eq!(gen::AIRY_RS_K_DELTA, "delta");
        assert_eq!(gen::AIRY_RS_K_CONTENT, "content");
        assert_eq!(gen::AIRY_RS_K_TOOL, "tool");
        assert_eq!(gen::AIRY_RS_K_TOOL_ID, "tool_id");
        assert_eq!(gen::AIRY_RS_K_STATUS, "status");
    }

    /* ---- 解码路径 ---- */

    #[test]
    fn decode_run_start_frame() {
        let line = r#"data: {"v":1,"type":"run_start","id":0,"session_id":"sess_x","ts":1,"epoch":0,"data":{"prompt":"hi"}}"#;
        let ev = decode_sse_line(line).expect("decode run_start");
        assert_eq!(ev.event_type, gen::AIRY_RS_TYPE_RUN_START);
        assert_eq!(ev.session_id.as_deref(), Some("sess_x"));
        assert_eq!(ev.kind(), RsKind::Control);
    }

    #[test]
    fn decode_token_delta_frame() {
        let line = r#"data: {"v":1,"type":"token_delta","id":3,"ts":4,"epoch":0,"data":{"delta":"你好"}}"#;
        let ev = decode_sse_line(line).expect("decode token_delta");
        assert_eq!(ev.kind(), RsKind::Outcome);
        assert_eq!(ev.data_str(gen::AIRY_RS_K_DELTA), Some("你好"));
    }

    #[test]
    fn decode_message_frame() {
        let line = r#"data: {"v":1,"type":"message","id":5,"ts":6,"epoch":0,"data":{"role":"assistant","content":"ok","reasoning":"思考"}}"#;
        let ev = decode_sse_line(line).expect("decode message");
        assert_eq!(ev.kind(), RsKind::Outcome);
        assert_eq!(ev.data_str(gen::AIRY_RS_K_CONTENT), Some("ok"));
        assert_eq!(ev.data_str(gen::AIRY_RS_K_REASONING), Some("思考"));
    }

    #[test]
    fn decode_tool_start_and_end() {
        let line = r#"data: {"v":1,"type":"tool_start","id":4,"ts":5,"epoch":0,"data":{"tool":"web_search","tool_id":"t1"}}"#;
        let ev = decode_sse_line(line).expect("decode tool_start");
        assert_eq!(ev.kind(), RsKind::Execution);
        assert_eq!(ev.data_str(gen::AIRY_RS_K_TOOL), Some("web_search"));

        let line = r#"data: {"v":1,"type":"tool_end","id":6,"ts":7,"epoch":0,"data":{"tool_id":"t1","status":"ok"}}"#;
        let ev = decode_sse_line(line).expect("decode tool_end");
        assert_eq!(ev.kind(), RsKind::Execution);
        assert_eq!(ev.data_str(gen::AIRY_RS_K_STATUS), Some("ok"));
    }

    #[test]
    fn decode_error_frame() {
        let line = r#"data: {"v":1,"type":"error","id":9,"ts":8,"epoch":0,"data":{"code":-32603,"message":"failed","recoverable":true}}"#;
        let ev = decode_sse_line(line).expect("decode error");
        assert_eq!(ev.kind(), RsKind::Control);
        assert_eq!(ev.data_i64(gen::AIRY_RS_K_CODE), Some(-32603));
        assert_eq!(ev.data_str(gen::AIRY_RS_K_MSG), Some("failed"));
    }

    #[test]
    fn decode_run_end_with_run_id() {
        let line = r#"data: {"v":1,"type":"run_end","id":10,"run_id":"run_abc_0001","ts":9,"epoch":0,"data":{"status":"completed","duration_ms":100,"use_ticks":42}}"#;
        let ev = decode_sse_line(line).expect("decode run_end");
        assert_eq!(ev.kind(), RsKind::Control);
        assert_eq!(ev.run_id.as_deref(), Some("run_abc_0001"));
        assert_eq!(ev.data_str(gen::AIRY_RS_K_STATUS), Some("completed"));
    }

    #[test]
    fn unknown_type_tolerated() {
        /* §2.4.4：未来新增类型，旧客户端必须忽略不崩 */
        let line = r#"data: {"v":1,"type":"stream_event","id":9,"ts":7,"epoch":0,"data":{"mime":"audio/wav"}}"#;
        let ev = decode_sse_line(line).expect("unknown type tolerated");
        assert_eq!(ev.kind(), RsKind::Unknown);
    }

    #[test]
    fn non_data_lines_ignored() {
        assert!(decode_sse_line("").is_none());
        assert!(decode_sse_line(": keep-alive").is_none());
        assert!(decode_sse_line("event: foo").is_none());
        assert!(decode_sse_line("data: not-json").is_none());
    }

    #[test]
    fn missing_core_fields_invalid() {
        /* v/type 缺失视为无效帧 */
        assert!(decode_sse_line(r#"data: {"id":1}"#).is_none());
        assert!(decode_sse_line(r#"data: {"v":1}"#).is_none());
    }

    #[test]
    fn optional_fields_defaulted() {
        let line = r#"data: {"v":1,"type":"run_start","data":{}}"#;
        let ev = decode_sse_line(line).expect("optional fields defaulted");
        assert_eq!(ev.id, 0);
        assert_eq!(ev.run_id, None);
    }
}
