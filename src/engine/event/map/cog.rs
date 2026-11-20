// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 认知面映射（0.1.21 X1-07a，§23.1.5）：思维链、计划、正文增量与定稿。
//
// 载荷形状取自 daemon 事件信封与 airy_run_stream.h 的 AIRY_RS_K_*：
//   plan         data.plan                 计划的自然语言全文；
//   token_delta  data.delta                流式正文增量；
//   message      data.reasoning/content    收尾帧：思考链 + 权威全文。
//
// message 的 content 是全文而不是增量：归 Final 让投影以定稿替换流式占位，
// 若并入 Delta，每次运行末尾都会把正文重复追加一遍。reasoning 是可展开的
// 思考链，不混进正文，归 Think。
//
// step 与 plan_unavailable 暂不在这里：前者是任务视图的步骤进度（X1-13a
// 承接），后者的降级呈递随后续渲染卡落地；四域形状里当前没有它们的落点，
// 与其猜一个子类，不如让它们继续走「未识别即计数」的既有路径。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use super::super::model::Cognition;
use super::field;

/// 类型是否属于认知面。
pub(crate) fn known(kind: &str) -> bool {
    matches!(kind, "plan" | "token_delta" | "message")
}

/// 类型 + 载荷 → 认知面子类；message 可派生两条（思考链与定稿）。
pub(crate) fn read(kind: &str, p: &Value) -> Vec<Cognition> {
    match kind {
        "plan" => field(p, "plan")
            .map(|text| Cognition::Plan { text })
            .into_iter()
            .collect(),
        "token_delta" => field(p, "delta")
            .map(|text| Cognition::Delta { text })
            .into_iter()
            .collect(),
        "message" => msg(p),
        _ => Vec::new(),
    }
}

/// 收尾帧：思考链先于定稿，保持服务端给出的先后。
fn msg(p: &Value) -> Vec<Cognition> {
    let mut out = Vec::new();
    if let Some(text) = field(p, "reasoning") {
        out.push(Cognition::Think { text });
    }
    if let Some(text) = field(p, "content") {
        out.push(Cognition::Final { text });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{known, read};
    use crate::engine::event::model::Cognition;
    use serde_json::json;

    /// 计划取自然语言全文，缺字段或空串都不产出事件。
    #[test]
    fn plan_maps_text() {
        assert!(known("plan"));
        assert_eq!(
            read(
                "plan",
                &json!({"data": {"plan": "先读依赖", "step_count": 3}})
            ),
            vec![Cognition::Plan {
                text: "先读依赖".into()
            }]
        );
        assert!(read("plan", &json!({"data": {"step_count": 3}})).is_empty());
        assert!(read("plan", &json!({"data": {"plan": ""}})).is_empty());
    }

    /// 正文增量只收非空片段，空增量不占一条事件。
    #[test]
    fn delta_skips_empty() {
        assert_eq!(
            read("token_delta", &json!({"data": {"delta": "ab"}})),
            vec![Cognition::Delta { text: "ab".into() }]
        );
        assert!(read("token_delta", &json!({"data": {"delta": ""}})).is_empty());
        assert!(read("token_delta", &json!({"data": {}})).is_empty());
    }

    /// 收尾帧拆成思考链与定稿两条，思考链在前。
    #[test]
    fn message_splits_think_and_final() {
        assert_eq!(
            read(
                "message",
                &json!({"data": {
                    "role": "assistant",
                    "reasoning": "想一想",
                    "content": "答案"
                }})
            ),
            vec![
                Cognition::Think {
                    text: "想一想".into()
                },
                Cognition::Final {
                    text: "答案".into()
                },
            ]
        );
        assert_eq!(
            read("message", &json!({"data": {"content": "只有答案"}})),
            vec![Cognition::Final {
                text: "只有答案".into()
            }]
        );
        assert!(read("message", &json!({"data": {"role": "assistant"}})).is_empty());
    }

    /// 其他域的类型不越界收下。
    #[test]
    fn rejects_other_domains() {
        assert!(!known("tool_start"));
        assert!(read("tool_start", &json!({"data": {"tool": "shell"}})).is_empty());
    }
}
