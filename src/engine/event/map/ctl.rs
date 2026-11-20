// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 控制面映射（0.1.21 X1-06a/X1-06b，§23.1.5）：审批、取消、状态、配置、
// 通知、心跳。
//
// 载荷形状以 CLI 与 daemon 的实际发帧为准：
//   control_request  CLI 本地造帧，载荷平铺 request_id/tool/input/timeout_ms，
//                    input 是待执行命令的转义字符串，不是对象；
//   snapshot         CLI 续传造帧，state 在顶层；
//   run_start        服务端事件，类型本身即「已进入运行」，并随帧下发服务端
//                    裁决的配置（model/mode/mode_reason/gccp_confirmed），
//                    daemon 明确要求客户端据此渲染任务头、不得自行推断；
//   task_state       服务端事件，state 在 data 段（daemon 事件信封固定形状），
//                    cancelled 时另带 reason/cancelled_by/cancelled_at；
//   notice/heartbeat CLI 本地造帧，载荷分别只有 message 与 state。
// 两种状态放法都读，先 data 后顶层：同一状态不许因链路不同落到不同分支。
//
// 任务身份（task_id）随状态与取消一起带出：任务表按身份寻址，身份只认帧
// 载荷，客户端不拿会话号或运行号去冒充。快照帧通常没有身份——它描述的是
// 「这次运行」，身份缺席即留空，由投影决定用不用。
//
// 取消单列而不并进普通状态：cancelled 携带「谁、为什么」的追溯，界面要能
// 说清中断来源，与 submitted→executing 这类推进不是同一类呈现。
// 通知无级别字段，一律 Info：错误另有 error 帧，按文本猜级别只会误导。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use super::super::model::{Control, Level};
use super::{at, field};

/// task_state 的取消取值（agent_task_contract.h）。
const CANCEL: &str = "cancelled";

/// run_start 随帧下发的服务端裁决配置键（agent_run_exec.c 的 run_start 段）。
const CFG_KEYS: [&str; 4] = ["model", "mode", "mode_reason", "gccp_confirmed"];

/// 类型是否属于控制面。
pub(crate) fn known(kind: &str) -> bool {
    matches!(
        kind,
        "control_request" | "task_state" | "run_start" | "snapshot" | "notice" | "heartbeat"
    )
}

/// 类型 + 载荷 + 帧序号 → 控制面子类；字段不合契约返回 None。
///
/// 帧序号只被心跳使用：心跳没有自己的序号字段，帧头的 seq 就是 CLI 到此
/// 已写出的进度锚，二者本是一件事。
pub(crate) fn read(kind: &str, p: &Value, seq: u64) -> Option<Control> {
    match kind {
        "control_request" => ask(p),
        "task_state" => state(p),
        "run_start" => start(p),
        "snapshot" => snap(p),
        "notice" => note(p),
        "heartbeat" => Some(Control::Beat { seq }),
        _ => None,
    }
}

/// run_start 下发的配置：逐键成事件，界面按 key 取用，不合并成一坨。
pub(crate) fn cfgs(p: &Value) -> Vec<Control> {
    CFG_KEYS.iter().filter_map(|k| cfg(p, k)).collect()
}

/// 单个配置键；值原样保留（字符串、布尔都合法）。
fn cfg(p: &Value, key: &str) -> Option<Control> {
    let v = at(p, key)?;
    Some(Control::Cfg {
        key: key.to_owned(),
        value: v.clone(),
    })
}

/// 审批请求：request_id 是与 control_response 对上的唯一凭据，缺了就不能收。
fn ask(p: &Value) -> Option<Control> {
    let id = field(p, "request_id")?;
    if id.is_empty() {
        return None;
    }
    Some(Control::Ask {
        id,
        tool: field(p, "tool").unwrap_or_default(),
        target: field(p, "input").unwrap_or_default(),
    })
}

/// 任务状态迁移；cancelled 归取消，其余归状态。两者都带任务身份。
fn state(p: &Value) -> Option<Control> {
    let name = field(p, "state")?;
    let id = field(p, "task_id").unwrap_or_default();
    if name == CANCEL {
        return Some(Control::Cancel {
            id,
            reason: field(p, "reason").unwrap_or_default(),
        });
    }
    Some(Control::State { id, name })
}

/// 开跑帧：类型本身即「已进入运行」，任务身份随帧下发（可缺席）。
fn start(p: &Value) -> Option<Control> {
    Some(Control::State {
        id: field(p, "task_id").unwrap_or_default(),
        name: "running".into(),
    })
}

/// 续传快照携带的当前状态；快照描述运行而非任务，通常没有任务身份。
fn snap(p: &Value) -> Option<Control> {
    field(p, "state").map(|name| Control::State {
        id: field(p, "task_id").unwrap_or_default(),
        name,
    })
}

/// CLI 本地提示：契约只有 message，没有级别。
fn note(p: &Value) -> Option<Control> {
    Some(Control::Notice {
        level: Level::Info,
        text: field(p, "message")?,
    })
}

#[cfg(test)]
mod tests {
    use super::{known, read};
    use crate::engine::event::model::{Control, Level};
    use serde_json::json;

    const SEQ: u64 = 11;

    /// 审批帧按 CLI 实际载荷收下，input 即待执行内容。
    #[test]
    fn asks_with_contract_fields() {
        let p = json!({
            "type": "control_request",
            "request_id": "c7",
            "subtype": "can_use_tool",
            "tool": "shell",
            "input": "rm -rf build",
            "timeout_ms": 30000
        });
        assert!(known("control_request"));
        assert_eq!(
            read("control_request", &p, SEQ),
            Some(Control::Ask {
                id: "c7".into(),
                tool: "shell".into(),
                target: "rm -rf build".into(),
            })
        );
    }

    /// 无 request_id 的审批帧不可回应，宁可不收。
    #[test]
    fn ask_needs_request_id() {
        assert_eq!(
            read("control_request", &json!({"tool": "shell"}), SEQ),
            None
        );
        assert_eq!(
            read(
                "control_request",
                &json!({"request_id": "", "tool": "shell"}),
                SEQ
            ),
            None
        );
    }

    /// 状态迁移两种载荷放法都能读；任务身份随帧带出，缺席时为空。
    #[test]
    fn state_reads_both_shapes() {
        let nested = json!({
            "type": "task_state",
            "data": {"task_id": "t1", "state": "executing"}
        });
        let flat = json!({"type": "task_state", "state": "planned"});
        assert_eq!(
            read("task_state", &nested, SEQ),
            Some(Control::State {
                id: "t1".into(),
                name: "executing".into()
            })
        );
        assert_eq!(
            read("task_state", &flat, SEQ),
            Some(Control::State {
                id: String::new(),
                name: "planned".into()
            })
        );
        assert_eq!(
            read("task_state", &json!({"data": {"state": "submitted"}}), SEQ),
            Some(Control::State {
                id: String::new(),
                name: "submitted".into()
            })
        );
    }

    /// 取消带追溯：reason 在 data 段，缺失时为空而非编造。
    #[test]
    fn cancel_carries_reason() {
        let p = json!({
            "type": "task_state",
            "data": {
                "task_id": "t9",
                "state": "cancelled",
                "reason": "user",
                "cancelled_by": "tui",
                "cancelled_at": 1763000000000u64
            }
        });
        assert_eq!(
            read("task_state", &p, SEQ),
            Some(Control::Cancel {
                id: "t9".into(),
                reason: "user".into()
            })
        );
        assert_eq!(
            read("task_state", &json!({"state": "cancelled"}), SEQ),
            Some(Control::Cancel {
                id: String::new(),
                reason: String::new()
            })
        );
    }

    /// 运行开始即 running，任务身份取 run_start 的 task_id；快照取其 state。
    #[test]
    fn start_and_snapshot() {
        let start = json!({"data": {"task_id": "t3", "model": "m"}});
        assert_eq!(
            read("run_start", &start, SEQ),
            Some(Control::State {
                id: "t3".into(),
                name: "running".into()
            })
        );
        assert_eq!(
            read("run_start", &json!({"run": "r1"}), SEQ),
            Some(Control::State {
                id: String::new(),
                name: "running".into()
            })
        );
        let snap = json!({"state": "running", "last_seq": 12, "recent": []});
        assert_eq!(
            read("snapshot", &snap, SEQ),
            Some(Control::State {
                id: String::new(),
                name: "running".into()
            })
        );
        assert_eq!(read("snapshot", &json!({"last_seq": 12}), SEQ), None);
    }

    /// 非控制面类型与空状态都不收。
    #[test]
    fn rejects_out_of_scope() {
        assert!(!known("plan"));
        assert_eq!(read("plan", &json!({"text": "x"}), SEQ), None);
        assert_eq!(read("task_state", &json!({"state": ""}), SEQ), None);
        assert_eq!(read("task_state", &json!({"state": 3}), SEQ), None);
    }

    /// 通知无级别字段：一律 Info，文本取 message。
    #[test]
    fn notice_is_info() {
        assert_eq!(
            read("notice", &json!({"message": "checking"}), SEQ),
            Some(Control::Notice {
                level: Level::Info,
                text: "checking".into(),
            })
        );
        assert_eq!(read("notice", &json!({}), SEQ), None);
    }

    /// 心跳以帧序号为进度锚：没有自有 seq 字段也能对上帧。
    #[test]
    fn heartbeat_anchors_frame_seq() {
        assert_eq!(
            read("heartbeat", &json!({"state": "running"}), 42),
            Some(Control::Beat { seq: 42 })
        );
    }

    /// 配置逐键展开：只收四个裁决键，缺键省略，布尔值原样保留。
    #[test]
    fn config_expands_known_keys() {
        let p = json!({"data": {
            "task_id": "t1",
            "model": "qwen3",
            "mode": "plan",
            "mode_reason": "needs review",
            "gccp_confirmed": true
        }});
        let cfgs = super::cfgs(&p);
        assert_eq!(cfgs.len(), 4, "任务身份不混入配置");
        assert!(cfgs.iter().any(|c| matches!(
            c,
            Control::Cfg { key, value } if key == "gccp_confirmed" && value.as_bool() == Some(true)
        )));
        assert!(super::cfgs(&json!({"data": {"model": "m"}})).len() == 1);
        assert!(super::cfgs(&json!({"data": {"task_id": "t1"}})).is_empty());
    }
}
