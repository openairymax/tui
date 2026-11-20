// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 收口面映射（0.1.21 X1-07b，§23.1.5）：终态与失败三件套。
//
// 两张帧互为收口的两面（agent_run_exec.c 的 run_tail 固定顺序
// message → run_end → error）：
//   run_end  data.status                终态锚点（completed/paused/
//                                       unverified/failed/cancelled）；
//   error    data.message               一句人话；
//            data.failure.reason        机器可读原因（D-10 稳定枚举）；
//            data.failure.next_action   下一步动作（retry/resume/inspect/adjust）；
//            data.error_code/data.code  CLI 本地 error 帧没有 failure 对象，
//                                       只能拿数字码当机器键。
//
// 终态只认 status：缺了就无法判定收口，宁可不收（与未识别类型同策略）。
// 失败三件套按 D-10 原样取用，不在渲染层二次归一化（§23.8 #3）；本地帧
// 没有 failure 对象时 reason 回退为数字码、next 留空——不替用户猜下一步。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use super::super::model::Outcome;
use super::{at, field};

/// 类型是否属于收口面。
pub(crate) fn known(kind: &str) -> bool {
    matches!(kind, "run_end" | "error")
}

/// 类型 + 载荷 → 收口子类；缺终态或说明的帧不产出。
pub(crate) fn read(kind: &str, p: &Value) -> Option<Outcome> {
    match kind {
        "run_end" => end(p),
        "error" => err(p),
        _ => None,
    }
}

/// 终态：status 是唯一事实源；data 段整体随行，供语义层取会话/用量/判据。
fn end(p: &Value) -> Option<Outcome> {
    let data = p
        .pointer("/data")
        .map(|d| d.to_string())
        .unwrap_or_default();
    field(p, "status").map(|status| Outcome::End { status, data })
}

/// 失败三件套：说明是收下的前提（reason/next 缺失另有回退）。
fn err(p: &Value) -> Option<Outcome> {
    let text = field(p, "message")?;
    let fail = at(p, "failure");
    Some(Outcome::Err {
        reason: reason(p, fail).unwrap_or_default(),
        text,
        next: sub(fail, "next_action").unwrap_or_default(),
    })
}

/// 机器可读原因：D-10 的 reason 优先，本地帧回退到数字码。
fn reason(p: &Value, fail: Option<&Value>) -> Option<String> {
    sub(fail, "reason").or_else(|| code(p))
}

/// 错误码标量化为字符串：数字与字符串两种写法都收。
fn code(p: &Value) -> Option<String> {
    let v = at(p, "error_code").or_else(|| at(p, "code"))?;
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 失败对象内的字符串字段（对象不存在时 None）。
fn sub(fail: Option<&Value>, key: &str) -> Option<String> {
    field(fail?, key)
}

#[cfg(test)]
mod tests {
    use super::{known, read};
    use crate::engine::event::model::Outcome;
    use serde_json::json;

    fn failure() -> serde_json::Value {
        json!({
            "origin": "provider",
            "code": 5,
            "reason": "provider_unreachable",
            "retryable": true,
            "budget_used": 1,
            "budget_total": 8,
            "affected_node": "llm_d",
            "next_action": "retry"
        })
    }

    /// 终态以 status 为唯一事实源，缺失或为空都不收。
    #[test]
    fn end_reads_status() {
        assert!(known("run_end"));
        assert_eq!(
            read(
                "run_end",
                &json!({"data": {"status": "paused", "rounds_used": 3}})
            ),
            Some(Outcome::End {
                status: "paused".into(),
                data: json!({"status": "paused", "rounds_used": 3}).to_string(),
            })
        );
        assert_eq!(read("run_end", &json!({"data": {}})), None);
        assert_eq!(read("run_end", &json!({"data": {"status": ""}})), None);
    }

    /// 失败三件套按 D-10 原样取用：原因、人话、动作各就各位。
    #[test]
    fn err_carries_triad() {
        assert!(known("error"));
        let p = json!({"data": {
            "code": -32603,
            "error_code": 5,
            "message": "上游不可达",
            "failure": failure()
        }});
        assert_eq!(
            read("error", &p),
            Some(Outcome::Err {
                reason: "provider_unreachable".into(),
                text: "上游不可达".into(),
                next: "retry".into(),
            })
        );
    }

    /// 本地 error 帧无 failure 对象：reason 回退数字码，next 不猜。
    #[test]
    fn local_error_falls_back_to_code() {
        let p = json!({"code": -32603, "message": "事件流已关闭"});
        assert_eq!(
            read("error", &p),
            Some(Outcome::Err {
                reason: "-32603".into(),
                text: "事件流已关闭".into(),
                next: String::new(),
            })
        );
        assert_eq!(read("error", &json!({"code": -1})), None);
        assert_eq!(read("error", &json!({"message": ""})), None);
    }

    /// 其他域的类型不越界收下。
    #[test]
    fn rejects_other_domains() {
        assert!(!known("run_start"));
        assert_eq!(read("run_start", &json!({"data": {"model": "m"}})), None);
    }
}
