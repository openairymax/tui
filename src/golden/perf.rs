// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 性能与预算能力矩阵（TUI 建设方案 §7.1 / §7.2）：帧预算与节流常量、视口窗口
// 化的 fail-closed 语义、超限裁剪文案、长会话窗口化、渲染降级开关、文本预算
// 上限与同状态渲染幂等。断言锁定「宁可少渲染也不猜」与「同态同帧」两条纪律。

use super::*;
use crate::budget::{
    CAP_PROBE_TIMEOUT_MS, DEFAULT_LIVE_PUBLISH_INTERVAL_MS, FRAME_BUDGET_US, MAX_OUTPUT_CHARACTERS,
    MAX_PASTE_FOLD_BYTES, MAX_TOOL_TEXT_BYTES, NOTIFY_TEXT_MAX,
};
use crate::engine::window::{clip_marker, project};

/// §7.2 #1：帧预算、探测超时与实时节流三者的数值口径。
#[test]
fn frame_budget_constants() {
    assert_eq!(FRAME_BUDGET_US, 8_000);
    assert_eq!(CAP_PROBE_TIMEOUT_MS, 250);
    assert_eq!(DEFAULT_LIVE_PUBLISH_INTERVAL_MS, 250);
}

/// §7.2 #2：行高缺失时必须 fail-closed，不得猜测可见窗口。
#[test]
fn viewport_fails_closed_on_missing_heights() {
    let span = project(5, 0, 4, &[1, 2], false);
    assert_eq!((span.start, span.end), (0, 0), "行高不足时窗口必须收敛为空");
}

/// §7.2 #3：超限内容以固定文案标注裁剪量并给出展开路径。
#[test]
fn clip_marker_is_exact() {
    assert_eq!(
        clip_marker(3),
        "… 已裁剪 3 行 · 单轮上限 120 行 · Alt+O 展开"
    );
}

/// §7.2 #4：长会话只渲染尾部窗口，视口外旧消息不入屏。
#[test]
fn long_session_windows_to_tail() {
    let mut fx = Fixture::new("perf-window");
    for i in 0..400 {
        add(&mut fx.app, MessageRole::Agent, &format!("第{i}条"));
    }
    let s = fx.draw();
    assert_has(&s, "第399条");
    assert_lacks(&s, "第0条");
}

/// §7.2 #5：渲染降级开关切换状态条提示，默认不显示。
#[test]
fn render_degrade_toggle() {
    let mut fx = Fixture::new("perf-degrade");
    assert!(!fx.app.render_degraded);
    let s = fx.draw_at(140, H);
    assert_lacks(&s, "渲染降级");

    fx.app.render_degraded = true;
    let s = fx.draw_at(140, H);
    assert_has(&s, "⚠ 渲染降级 · 重连中");
}

/// §7.2 #6：粘贴折叠、输出与工具文本、通知摘要的字节/字符上限。
#[test]
fn text_budget_constants() {
    assert_eq!(MAX_PASTE_FOLD_BYTES, 16 * 1024);
    assert_eq!(MAX_OUTPUT_CHARACTERS, 64 * 1024);
    assert_eq!(MAX_TOOL_TEXT_BYTES, 64 * 1024);
    assert_eq!(NOTIFY_TEXT_MAX, 120);
}

/// §7.2 #7：同一状态连续两帧逐行一致（品牌头实时时钟除外）。
#[test]
fn render_is_idempotent_across_frames() {
    let mut fx = Fixture::new("perf-idem");
    add(&mut fx.app, MessageRole::User, "第一条");
    add(&mut fx.app, MessageRole::Agent, "回复内容");
    let a = fx.draw();
    let b = fx.draw();
    let body = |s: &Screen| -> Vec<String> {
        s.rows()
            .iter()
            .filter(|r| !r.contains("AirymaxRT"))
            .cloned()
            .collect()
    };
    assert_eq!(body(&a), body(&b), "同状态两帧应逐行一致");
}
