// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 自托管专页 golden（D-1）：固定 `llm.get_stats` 夹具走真实解析与真实渲染，
// 断言模型/后端/显存/并发/队列/失败率/最近错误逐项上屏，以及两种空态
// （无端点、后端不可达）的诚实措辞——降级时不得残留上一轮端点数值。

use super::*;
use crate::app::ActivePanel;
use crate::engine::selfhost::{snapshot, unavailable};
use serde_json::json;

fn live_fixture() -> serde_json::Value {
    json!({
        "requests": {"total": 40, "ok": 35, "fail": 3, "timeout": 2},
        "endpoints": [
            {
                "provider": "gateapi",
                "models": ["gate-model", "gate-fast"],
                "capacity": 8,
                "api_base": "http://127.0.0.1:21490/v1",
                "props_state": 1,
                "total_slots": 4,
                "n_ctx_slot": 4096,
                "inflight": 2,
                "waiting": 1,
                "backend_ready": true,
                "backend_reason": ""
            },
            {
                "provider": "localqwen",
                "models": ["Qwen2.5-7B-Instruct"],
                "capacity": 0,
                "api_base": "http://127.0.0.1:18080/v1",
                "props_state": 0,
                "total_slots": 1,
                "n_ctx_slot": 8192,
                "inflight": 0,
                "waiting": 0,
                "backend_ready": false,
                "backend_reason": "api_key_missing"
            }
        ],
        "last_error": {"model": "gate-model", "rc": -12, "reason": "upstream_error", "age_ms": 42000}
    })
}

/// 在线态：七个字段（模型/后端/显存/并发/队列/失败率/最近错误）全部可读，
/// 有闸与未设闸两种容量语义分别呈现。
#[test]
fn selfhost_panel_renders_live_endpoints() {
    let mut fx = Fixture::new("sh-live");
    fx.app.active_panel = ActivePanel::SelfHost;
    let mut snap = snapshot(&live_fixture());
    snap.vram = Some((2048, 24576));
    fx.app.selfhost = Some(snap);
    fx.app.selfhost_at_ms = 0;
    fx.app.clock_ms = 2_000;
    let s = fx.draw();
    assert_has(&s, "自托管在线");
    assert_has(&s, "2 个端点");
    assert_has(&s, "2s 前更新");
    // 模型与后端
    assert_has(&s, "gateapi");
    assert_has(&s, "gate-model, gate-fast");
    assert_has(&s, "http://127.0.0.1:21490/v1");
    assert_has(&s, "就绪");
    assert_has(&s, "未就绪：api_key_missing");
    // 显存 / 并发 / 队列 / 失败率
    assert_has(&s, "2.0 GiB / 24.0 GiB");
    assert_has(&s, "并发 2/8");
    assert_has(&s, "并发 0（未设闸）");
    assert_has(&s, "队列 1");
    assert_has(&s, "槽位 4 × 4096 tok");
    assert_has(&s, "失败率 12.5%");
    // 最近错误
    assert_has(&s, "最近错误");
    assert_has(&s, "upstream_error");
    assert_has(&s, "rc=-12");
    assert_has(&s, "42s 前");
}

/// 已连接但无端点：如实说明，不伪造任何端点行。
#[test]
fn selfhost_panel_empty_endpoints_is_honest() {
    let mut fx = Fixture::new("sh-empty");
    fx.app.active_panel = ActivePanel::SelfHost;
    fx.app.selfhost = Some(snapshot(&json!({"endpoints": [], "requests": {"total": 0}})));
    let s = fx.draw();
    assert_has(&s, "未注册任何自托管端点");
    assert_has(&s, "失败率 —（无请求）");
    assert_lacks(&s, "▸");
}

/// llm_d 不可达：状态行降级 + 原因原样展示，且不残留上一轮端点与显存数值。
#[test]
fn selfhost_panel_unavailable_shows_degraded() {
    let mut fx = Fixture::new("sh-down");
    fx.app.active_panel = ActivePanel::SelfHost;
    fx.app.selfhost = Some(unavailable("llm.get_stats 失败：connection refused"));
    fx.app.selfhost_at_ms = 0;
    fx.app.clock_ms = 1_500;
    let s = fx.draw();
    assert_has(&s, "自托管后端不可用");
    assert_has(&s, "connection refused");
    assert_has(&s, "不显示任何缓存值");
    assert_lacks(&s, "gateapi");
    assert_lacks(&s, "2.0 GiB");
    assert_lacks(&s, "失败率");
}
