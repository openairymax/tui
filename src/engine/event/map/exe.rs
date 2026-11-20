// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 执行面映射（0.1.21 X1-07a，§23.1.5）：工具目录、调用开始、调用结果。
//
// 载荷取自 daemon 事件信封与 airy_run_stream.h 的 AIRY_RS_K_*：
//   catalog     data.available/tools            可用工具数与目录；
//   tool_start  data.tool/tool_id/args_preview  调用开始，参数据服务端原样；
//   tool_end    data.tool_id/status/result_hash 成败与结果摘录。
//
// tool_end 不带工具名：调用与结果靠 tool_id 对齐（旧渲染层的 tool_names 表
// 就是做这件事），不在两侧重复维护名字。args_preview 是服务端给的字符串，
// 原样交给投影呈现，不在这里二次解析——解析失败时界面反而拿不到原文。
// tool_delta 在 0.1.21 没有生产者（全仓无发帧），不建无源映射。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use super::super::model::Execution;
use super::{at, field};

/// 类型是否属于执行面。
pub(crate) fn known(kind: &str) -> bool {
    matches!(kind, "catalog" | "tool_start" | "tool_end")
}

/// 类型 + 载荷 → 执行面子类；catalog 缺字段时计数为 0 而不是丢帧。
pub(crate) fn read(kind: &str, p: &Value) -> Option<Execution> {
    match kind {
        "catalog" => Some(Execution::Tools { count: count(p) }),
        "tool_start" => call(p),
        "tool_end" => result(p),
        _ => None,
    }
}

/// 可用工具数：优先服务端给的 available，缺失时退回目录长度。
fn count(p: &Value) -> usize {
    at(p, "available")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .or_else(|| at(p, "tools").and_then(Value::as_array).map(Vec::len))
        .unwrap_or(0)
}

/// 调用开始：缺 tool_id 时仍收下供展示，代价是无法与结果对齐。
fn call(p: &Value) -> Option<Execution> {
    Some(Execution::Call {
        id: field(p, "tool_id").unwrap_or_default(),
        name: field(p, "tool").unwrap_or_default(),
        args: at(p, "args_preview").cloned().unwrap_or(Value::Null),
    })
}

/// 调用结果：status 只有 "ok" 算成功，缺失按未成功（与旧渲染层一致）。
fn result(p: &Value) -> Option<Execution> {
    let ok = field(p, "status").map(|s| s == "ok").unwrap_or(false);
    Some(Execution::Result {
        id: field(p, "tool_id").unwrap_or_default(),
        ok,
        text: field(p, "result_hash").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::{known, read};
    use crate::engine::event::model::Execution;
    use serde_json::json;

    /// 目录计数以服务端 available 为准，缺失时退回目录长度。
    #[test]
    fn catalog_counts_available() {
        assert!(known("catalog"));
        assert_eq!(
            read(
                "catalog",
                &json!({"data": {"available": 7, "tools": [{"name": "a"}, {"name": "b"}]}})
            ),
            Some(Execution::Tools { count: 7 })
        );
        assert_eq!(
            read("catalog", &json!({"data": {"tools": [{}, {}]}})),
            Some(Execution::Tools { count: 2 })
        );
        assert_eq!(
            read("catalog", &json!({"data": {}})),
            Some(Execution::Tools { count: 0 })
        );
    }

    /// 调用开始保留服务端给定的参数原文。
    #[test]
    fn call_keeps_raw_args() {
        assert_eq!(
            read(
                "tool_start",
                &json!({"data": {"tool": "shell", "tool_id": "t1", "args_preview": "{\"cmd\":\"ls\"}"}})
            ),
            Some(Execution::Call {
                id: "t1".into(),
                name: "shell".into(),
                args: json!("{\"cmd\":\"ls\"}"),
            })
        );
        assert_eq!(
            read("tool_start", &json!({"data": {"tool": "read"}})),
            Some(Execution::Call {
                id: String::new(),
                name: "read".into(),
                args: serde_json::Value::Null,
            })
        );
    }

    /// 结果靠 tool_id 与调用对齐；非 ok 与缺 status 都算未成功。
    #[test]
    fn result_correlates_by_id() {
        assert_eq!(
            read(
                "tool_end",
                &json!({"data": {"tool_id": "t1", "status": "ok", "result_hash": "ok: 3 files"}})
            ),
            Some(Execution::Result {
                id: "t1".into(),
                ok: true,
                text: "ok: 3 files".into(),
            })
        );
        assert_eq!(
            read(
                "tool_end",
                &json!({"data": {"tool_id": "t2", "status": "error"}})
            ),
            Some(Execution::Result {
                id: "t2".into(),
                ok: false,
                text: String::new(),
            })
        );
        assert_eq!(
            read("tool_end", &json!({"data": {"tool_id": "t3"}})),
            Some(Execution::Result {
                id: "t3".into(),
                ok: false,
                text: String::new(),
            })
        );
    }

    /// 其他域的类型不越界收下。
    #[test]
    fn rejects_other_domains() {
        assert!(!known("token_delta"));
        assert_eq!(read("token_delta", &json!({"data": {"delta": "x"}})), None);
    }
}
