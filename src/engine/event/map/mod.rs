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
// 未识别的类型不产出事件而不是猜一个域：CLI 的类型是白名单（[a-z0-9_.]），
// 新类型在旧渲染层上必然没有对应呈现，宁可计数丢弃，也不要把配置帧塞进
// 正文。字段缺失同理——少了 request_id 的审批帧无法回应，收下只会让界面
// 卡在一个点不动的按钮上。

#![cfg_attr(not(test), allow(dead_code))]

mod cog;
pub(crate) mod ctl;
mod exe;
mod out;

use serde_json::Value;

use super::envelope::Envelope;
use super::model::{Body, Event};

/// 类型 + 载荷 + 帧序号 → 归域载荷（一帧可派生多条）；未识别返回空。
pub(crate) fn bodies(kind: &str, payload: &Value, seq: u64) -> Vec<Body> {
    let mut acc = Vec::new();
    if ctl::known(kind) {
        acc.extend(ctl::read(kind, payload, seq).map(Body::Control));
    } else if cog::known(kind) {
        acc.extend(cog::read(kind, payload).into_iter().map(Body::Cognition));
    } else if exe::known(kind) {
        acc.extend(exe::read(kind, payload).map(Body::Execution));
    } else if out::known(kind) {
        acc.extend(out::read(kind, payload).map(Body::Outcome));
    }
    acc
}

/// 信封 → 事件序列：主载荷一条，run_start 另按配置键逐条展开。
///
/// 同一帧派生的多条事件共用帧序号：序号标识的是「CLI 写出的第几帧」，
/// 不是渲染层自己编的序号，派生事件不得另造序号。
pub(crate) fn events(env: Envelope) -> Vec<Event> {
    let seq = env.seq;
    let ts = env.ts;
    let mk = |body: Body| Event { seq, ts, body };
    let mut out: Vec<Event> = bodies(&env.kind, &env.payload, seq)
        .into_iter()
        .map(mk)
        .collect();
    if env.kind == "run_start" {
        out.extend(
            ctl::cfgs(&env.payload)
                .into_iter()
                .map(|c| mk(Body::Control(c))),
        );
    }
    out
}

/// 取字符串字段：先 data 段（服务端事件），再顶层（CLI 本地帧）。
///
/// 两种放法都读是链路事实而非容错：同一状态不许因链路不同落到不同分支。
pub(super) fn field(p: &Value, key: &str) -> Option<String> {
    at(p, key)?
        .as_str()
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// 取原始值，层级规则同 `field`：服务端事件的值落在 data 段。
pub(super) fn at<'a>(p: &'a Value, key: &str) -> Option<&'a Value> {
    let path = format!("/data/{key}");
    p.pointer(&path).or_else(|| p.get(key))
}

#[cfg(test)]
mod tests {
    use super::{bodies, events};
    use crate::engine::event::envelope::Envelope;
    use crate::engine::event::model::{Body, Cognition, Control, Domain, Execution};
    use serde_json::{json, Value};

    const SEQ: u64 = 11;

    fn env(kind: &str, payload: Value) -> Envelope {
        Envelope::read(json!({
            "schema": 1,
            "seq": SEQ,
            "ts": 1763000000456u64,
            "type": kind,
            "payload": payload
        }))
        .expect("合契约的帧")
    }

    /// 控制面六类帧都落到 control 域，且信封字段不丢。
    #[test]
    fn routes_control_kinds() {
        let cases: [(&str, Value); 6] = [
            (
                "control_request",
                json!({"request_id": "c1", "tool": "shell", "input": "ls"}),
            ),
            ("task_state", json!({"state": "executing"})),
            ("run_start", json!({"data": {"model": "m"}})),
            ("snapshot", json!({"state": "running", "last_seq": 9})),
            ("notice", json!({"message": "checking"})),
            ("heartbeat", json!({"state": "running"})),
        ];
        for (kind, payload) in cases {
            let evs = events(env(kind, payload));
            assert!(!evs.is_empty(), "{kind} 应可归域");
            for ev in &evs {
                assert_eq!(ev.seq, SEQ);
                assert_eq!(ev.ts, 1_763_000_000_456);
                assert_eq!(ev.domain(), Domain::Control);
                assert!(matches!(&ev.body, Body::Control(_)));
            }
        }
    }

    /// 认知与执行面按各自载荷归域；message 一帧派生思考链与定稿两条。
    #[test]
    fn routes_cog_and_exec() {
        let plan = events(env(
            "plan",
            json!({"data": {"plan": "先读依赖", "step_count": 3}}),
        ));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].domain(), Domain::Cognition);
        assert_eq!(
            plan[0].body,
            Body::Cognition(Cognition::Plan {
                text: "先读依赖".into()
            })
        );

        let msg = events(env(
            "message",
            json!({"data": {"reasoning": "想一想", "content": "答案"}}),
        ));
        assert_eq!(msg.len(), 2);
        assert!(msg
            .iter()
            .all(|e| e.domain() == Domain::Cognition && e.seq == SEQ));
        assert_eq!(
            msg[1].body,
            Body::Cognition(Cognition::Final {
                text: "答案".into()
            })
        );

        let call = events(env(
            "tool_start",
            json!({"data": {"tool": "shell", "tool_id": "t1", "args_preview": "ls"}}),
        ));
        assert_eq!(call.len(), 1);
        assert_eq!(call[0].domain(), Domain::Execution);
        assert_eq!(
            call[0].body,
            Body::Execution(Execution::Call {
                id: "t1".into(),
                name: "shell".into(),
                args: json!("ls"),
            })
        );
    }

    /// 三域全覆盖：认知、执行、收口各有真实发帧，逐类落到本域。
    #[test]
    fn covers_three_domains() {
        let failure = json!({"reason": "provider_unreachable", "next_action": "retry"});
        let cases: [(&str, Value, Domain); 8] = [
            (
                "token_delta",
                json!({"data": {"delta": "片"}}),
                Domain::Cognition,
            ),
            (
                "plan",
                json!({"data": {"plan": "先读依赖"}}),
                Domain::Cognition,
            ),
            (
                "message",
                json!({"data": {"content": "答案"}}),
                Domain::Cognition,
            ),
            (
                "catalog",
                json!({"data": {"available": 3}}),
                Domain::Execution,
            ),
            (
                "tool_start",
                json!({"data": {"tool": "shell", "tool_id": "t1"}}),
                Domain::Execution,
            ),
            (
                "tool_end",
                json!({"data": {"tool_id": "t1", "status": "ok"}}),
                Domain::Execution,
            ),
            (
                "run_end",
                json!({"data": {"status": "completed"}}),
                Domain::Outcome,
            ),
            (
                "error",
                json!({"data": {"message": "上游不可达", "failure": failure}}),
                Domain::Outcome,
            ),
        ];
        for (kind, payload, want) in cases {
            let evs = events(env(kind, payload));
            assert!(!evs.is_empty(), "{kind} 应可归域");
            assert!(
                evs.iter().all(|e| e.domain() == want),
                "{kind} 归错域：{evs:?}"
            );
        }
    }

    /// run_start 把服务端裁决配置逐键展开，与状态事件共用帧序号。
    #[test]
    fn run_start_expands_config() {
        let evs = events(env(
            "run_start",
            json!({"data": {
                "task_id": "t1",
                "model": "qwen3",
                "mode": "plan",
                "mode_reason": "needs review",
                "gccp_confirmed": true,
                "success_criteria": "tests green"
            }}),
        ));
        let cfgs: Vec<&Control> = evs
            .iter()
            .filter_map(|e| match &e.body {
                Body::Control(c @ Control::Cfg { .. }) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(cfgs.len(), 4, "只有配置键展开，任务身份与判据不混入");
        assert!(evs.iter().any(
            |e| matches!(&e.body, Body::Control(Control::State { name, .. }) if name == "running")
        ));
        assert!(cfgs.iter().any(|c| matches!(
            c,
            Control::Cfg { key, value } if key == "gccp_confirmed" && value.as_bool() == Some(true)
        )));
        assert!(evs.iter().all(|e| e.seq == SEQ));
    }

    /// 未识别的类型不猜域：原样丢弃，交给上层计数。
    #[test]
    fn ignores_unknown_kind() {
        assert!(bodies("blueprint", &json!({"steps": []}), SEQ).is_empty());
        assert!(bodies("", &json!({}), 0).is_empty());
        assert!(events(env("brand_new_type", json!({"a": 1}))).is_empty());
    }

    /// 字段不全的控制帧同样丢弃，而不是收下半条。
    #[test]
    fn ignores_incomplete_control() {
        assert!(bodies("control_request", &json!({"tool": "shell"}), SEQ).is_empty());
        assert!(bodies("snapshot", &json!({"last_seq": 3}), SEQ).is_empty());
        assert!(bodies("notice", &json!({}), SEQ).is_empty());
        let ask = bodies("control_request", &json!({"request_id": "c2"}), SEQ);
        assert_eq!(
            ask,
            vec![Body::Control(Control::Ask {
                id: "c2".into(),
                tool: String::new(),
                target: String::new(),
            })]
        );
    }

    /// 六个控制子类都有真实发帧作为生产者：表驱动的全覆盖。
    #[test]
    fn every_control_variant_has_producer() {
        let cases: [(&str, Value); 6] = [
            (
                "control_request",
                json!({"request_id": "c1", "tool": "shell", "input": "ls"}),
            ),
            (
                "task_state",
                json!({"data": {"state": "cancelled", "reason": "user"}}),
            ),
            ("snapshot", json!({"state": "running"})),
            ("notice", json!({"message": "checking"})),
            ("heartbeat", json!({"state": "running"})),
            (
                "run_start",
                json!({"data": {
                    "model": "m", "mode": "plan",
                    "mode_reason": "r", "gccp_confirmed": false
                }}),
            ),
        ];
        let mut seen = Vec::new();
        for (kind, payload) in cases {
            for ev in events(env(kind, payload)) {
                if let Body::Control(c) = ev.body {
                    seen.push(c);
                }
            }
        }
        let has = |f: &dyn Fn(&Control) -> bool| seen.iter().any(f);
        assert!(has(&|c| matches!(c, Control::Ask { .. })));
        assert!(has(&|c| matches!(c, Control::Cancel { .. })));
        assert!(has(&|c| matches!(c, Control::State { .. })));
        assert!(has(&|c| matches!(c, Control::Cfg { .. })));
        assert!(has(&|c| matches!(c, Control::Notice { .. })));
        assert!(has(&|c| matches!(c, Control::Beat { .. })));
    }
}
