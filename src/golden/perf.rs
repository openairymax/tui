// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 性能与预算能力矩阵（TUI 建设方案 §6.10 / §7.2）：帧预算与节流常量、视口窗口
// 化的 fail-closed 语义、超限裁剪文案、长会话窗口化、卡顿降帧提示、文本预算
// 上限与同态同帧幂等。断言锁定「宁可少渲染也不猜」与「同态同帧」两条纪律。

use super::*;
use crate::budget::{
    CAP_PROBE_TIMEOUT_MS, DEFAULT_LIVE_PUBLISH_INTERVAL_MS, FRAME_BUDGET_US, MAX_OUTPUT_CHARACTERS,
    MAX_PASTE_FOLD_BYTES, MAX_TOOL_TEXT_BYTES, NOTIFY_TEXT_MAX,
};
use crate::engine::window::{clip_marker, project};

/// §6.7 #5：实时统计 250ms 节流口径；探测超时同口径（§6.8 #3），
/// 帧预算为渲染拉模型闸门（§6.10 #1）。
#[test]
fn frame_budget_constants() {
    assert_eq!(FRAME_BUDGET_US, 8_000);
    assert_eq!(CAP_PROBE_TIMEOUT_MS, 250);
    assert_eq!(DEFAULT_LIVE_PUBLISH_INTERVAL_MS, 250);
}

/// §6.10 #2：视口投影行高缺失时必须 fail-closed，不得猜测可见窗口。
#[test]
fn viewport_fails_closed_on_missing_heights() {
    let span = project(5, 0, 4, &[1, 2], false);
    assert_eq!((span.start, span.end), (0, 0), "行高不足时窗口必须收敛为空");
}

/// §6.10 #2：单轮超限以固定文案标注裁剪量并给出展开路径。
#[test]
fn clip_marker_is_exact() {
    assert_eq!(
        clip_marker(3),
        "… 已裁剪 3 行 · 单轮上限 120 行 · Alt+O 展开"
    );
}

/// §6.10 #2：长会话只投影尾部窗口，视口外旧回合不入屏。
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

/// §6.10 #4：卡顿降帧先提示用户（状态条呈递），不静默劣化，默认不显示。
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

/// §6.4 #3：大粘贴折叠 16KiB；工具文本与输出 64KiB（§6.10 #6）；通知摘要 120 字。
#[test]
fn text_budget_constants() {
    assert_eq!(MAX_PASTE_FOLD_BYTES, 16 * 1024);
    assert_eq!(MAX_OUTPUT_CHARACTERS, 64 * 1024);
    assert_eq!(MAX_TOOL_TEXT_BYTES, 64 * 1024);
    assert_eq!(NOTIFY_TEXT_MAX, 120);
}

/// §7.2 #2：引擎化 + 增量重绘纪律：同一状态连续两帧逐行一致（品牌头实时时钟除外）。
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
