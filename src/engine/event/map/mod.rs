// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件映射（0.1.21 X1-06a，§23.1.5 事件入口的语义归域）。
//
// 信封只给出类型与原始载荷，归域与字段收窄在这里一次做完：下游拿到的是
// 定型载荷，不再各自按字符串猜语义。映射按域分文件，域内再按子类展开，
// 使「新增一种帧」只改一处、只测一处。
//
// 未识别的类型返回 None 而不是猜一个域：CLI 的类型是白名单（[a-z0-9_.]），
// 新类型在旧渲染层上必然没有对应呈现，宁可计数丢弃，也不要把配置帧塞进
// 正文。字段缺失同理——少了 request_id 的审批帧无法回应，收下只会让界面
// 卡在一个点不动的按钮上。

#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod ctl;

use serde_json::Value;

use super::envelope::Envelope;
use super::model::{Body, Event};

/// 类型 + 载荷 → 归域载荷；未识别返回 None。
pub(crate) fn body(kind: &str, payload: &Value) -> Option<Body> {
    if ctl::known(kind) {
        return ctl::read(kind, payload).map(Body::Control);
    }
    None
}

/// 信封 → 事件：归域失败即丢弃，序列与时间戳原样带出。
pub(crate) fn event(env: Envelope) -> Option<Event> {
    let body = body(&env.kind, &env.payload)?;
    Some(Event {
        seq: env.seq,
        ts: env.ts,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::{body, event};
    use crate::engine::event::envelope::Envelope;
    use crate::engine::event::model::{Body, Control, Domain};
    use serde_json::json;

    fn env(kind: &str, payload: serde_json::Value) -> Envelope {
        Envelope::read(json!({
            "schema": 1,
            "seq": 11,
            "ts": 1763000000456u64,
            "type": kind,
            "payload": payload
        }))
        .expect("合契约的帧")
    }

    /// 控制面类型都落到 control 域，且信封字段不丢。
    #[test]
    fn routes_control_kinds() {
        let cases = [
            (
                "control_request",
                json!({"request_id": "c1", "tool": "shell", "input": "ls"}),
            ),
            ("task_state", json!({"state": "executing"})),
            ("run_start", json!({"run": "r1"})),
            ("snapshot", json!({"state": "running", "last_seq": 9})),
        ];
        for (kind, payload) in cases {
            let ev = event(env(kind, payload)).expect("可归域");
            assert_eq!(ev.seq, 11);
            assert_eq!(ev.ts, 1_763_000_000_456);
            assert_eq!(ev.domain(), Domain::Control);
            assert!(matches!(ev.body, Body::Control(_)));
        }
    }

    /// 未识别的类型不猜域：原样丢弃，交给上层计数。
    #[test]
    fn ignores_unknown_kind() {
        assert_eq!(body("blueprint", &json!({"steps": []})), None);
        assert_eq!(body("", &json!({})), None);
        assert!(event(env("brand_new_type", json!({"a": 1}))).is_none());
    }

    /// 字段不全的控制帧同样丢弃，而不是收下半条。
    #[test]
    fn ignores_incomplete_control() {
        assert_eq!(body("control_request", &json!({"tool": "shell"})), None);
        assert_eq!(body("snapshot", &json!({"last_seq": 3})), None);
        let ask = body("control_request", &json!({"request_id": "c2"})).expect("最小审批帧");
        assert_eq!(
            ask,
            Body::Control(Control::Ask {
                id: "c2".into(),
                tool: String::new(),
                target: String::new(),
            })
        );
    }
}
