// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 执行面映射（0.1.21 X1-07a，§23.1.5）：工具目录、调用开始、调用结果。
//
// 载荷取自 daemon 事件信封与 airy_run_stream.h 的 AIRY_RS_K_*：
//   catalog         data.available/tools            可用工具数与目录；
//   tool_start      data.tool/tool_id/args_preview  调用开始，参数据服务端原样；
//   tool_end        data.tool_id/status/result_hash 成败与结果摘录；
//   step            data.task_id/step_index/step_count/step_status/step
//                                                   计划步骤（时间线依赖骨架）；
//   subagent_start  data.sub_id/goal/depth/budget   子任务委派开始；
//   subagent_end    data.sub_id/status/tokens       子任务收尾。
//
// tool_end 不带工具名：调用与结果靠 tool_id 对齐（旧渲染层的 tool_names 表
// 就是做这件事），不在两侧重复维护名字。args_preview 是服务端给的字符串，
// 原样交给投影呈现，不在这里二次解析——解析失败时界面反而拿不到原文。
// tool_delta 在 0.1.21 没有生产者（全仓无发帧），不建无源映射。
//
// 子任务委派是一次派发动作，故与工具调用同域：开始即一次调用行（目标为参数、
// 名字取「subagent」），收尾按 sub_id 回到同一行。子代理自身的 tool_* 帧带着
// 同一通道进来，各自成行；子树的归组交给按 sub_id 分栏的呈现层，不在映射层
// 展开——映射层只做单帧定型，不持有跨帧状态。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use super::super::model::Execution;
use super::{at, field};

/// 类型是否属于执行面。
pub(crate) fn known(kind: &str) -> bool {
    matches!(
        kind,
        "catalog" | "tool_start" | "tool_end" | "step" | "subagent_start" | "subagent_end"
    )
}

/// 类型 + 载荷 → 执行面子类；catalog 缺字段时计数为 0 而不是丢帧。
pub(crate) fn read(kind: &str, p: &Value) -> Option<Execution> {
    match kind {
        "catalog" => Some(Execution::Tools { count: count(p) }),
        "tool_start" => call(p),
        "tool_end" => result(p),
        "step" => step(p),
        "subagent_start" => sub_start(p),
        "subagent_end" => sub_end(p),
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

/// 计划步骤：缺步序的帧无法定位与排序，丢帧；标题缺失只留空标题。
///
/// 步序/总数是服务端给的数值，越界或缺失总数按 0 收下——渲染层据此决定
/// 是否显示「N/M」，不在这里替服务端补数。
fn step(p: &Value) -> Option<Execution> {
    Some(Execution::Step {
        task: field(p, "task_id").unwrap_or_default(),
        index: at(p, "step_index").and_then(Value::as_u64)? as usize,
        total: at(p, "step_count").and_then(Value::as_u64).unwrap_or(0) as usize,
        status: field(p, "step_status").unwrap_or_default(),
        title: field(p, "step").unwrap_or_default(),
    })
}

/// 子任务委派开始：一次委派占一行，目标即参数（原样呈现，不截断改写）。
fn sub_start(p: &Value) -> Option<Execution> {
    Some(Execution::Call {
        id: field(p, "sub_id").unwrap_or_default(),
        name: "subagent".into(),
        args: at(p, "goal").cloned().unwrap_or(Value::Null),
    })
}

/// 子任务收尾：stop 词表里只有 "done" 算成功；其余（cancelled/agent_error/
/// budget_exhausted/rounds_limit）原样作收尾文本，界面据此区分终局。
fn sub_end(p: &Value) -> Option<Execution> {
    let status = field(p, "status").unwrap_or_default();
    Some(Execution::Result {
        ok: status == "done",
        id: field(p, "sub_id").unwrap_or_default(),
        text: status,
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

    /// 子任务委派：开始建调用行（目标为参数），收尾只有 done 算成功。
    #[test]
    fn subagent_maps_to_action() {
        assert!(known("subagent_start"));
        assert!(known("subagent_end"));
        assert_eq!(
            read(
                "subagent_start",
                &json!({"data": {"sub_id": "sub_1", "goal": "核对依赖", "depth": 1, "budget": 4096}})
            ),
            Some(Execution::Call {
                id: "sub_1".into(),
                name: "subagent".into(),
                args: json!("核对依赖"),
            })
        );
        assert_eq!(
            read(
                "subagent_end",
                &json!({"data": {"sub_id": "sub_1", "status": "done", "tokens": 128}})
            ),
            Some(Execution::Result {
                id: "sub_1".into(),
                ok: true,
                text: "done".into(),
            })
        );
        assert_eq!(
            read(
                "subagent_end",
                &json!({"data": {"sub_id": "sub_1", "status": "agent_error"}})
            ),
            Some(Execution::Result {
                id: "sub_1".into(),
                ok: false,
                text: "agent_error".into(),
            })
        );
    }

    /// 计划步骤：步序缺失即丢帧（无法定位），标题缺失留空；其余字段原样。
    #[test]
    fn step_needs_index() {
        assert!(known("step"));
        assert_eq!(
            read(
                "step",
                &json!({"data": {"task_id": "t1", "step_index": 1, "step_count": 3, "step_status": "planned", "step": "生成报告"}})
            ),
            Some(Execution::Step {
                task: "t1".into(),
                index: 1,
                total: 3,
                status: "planned".into(),
                title: "生成报告".into(),
            })
        );
        assert_eq!(
            read("step", &json!({"data": {"task_id": "t1", "step_index": 0}})),
            Some(Execution::Step {
                task: "t1".into(),
                index: 0,
                total: 0,
                status: String::new(),
                title: String::new(),
            })
        );
        assert_eq!(read("step", &json!({"data": {"task_id": "t1"}})), None);
    }

    /// 其他域的类型不越界收下。
    #[test]
    fn rejects_other_domains() {
        assert!(!known("token_delta"));
        assert_eq!(read("token_delta", &json!({"data": {"delta": "x"}})), None);
    }
}
