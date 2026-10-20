// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 滚动、选择与复制（TUI 建设方案 §6.3）。滚动控制面消费渲染每帧回写的
// `page_step` / `chat_scroll_max`；复制走两段式 leader 前缀，不占用
// 终端原生选择（默认不捕获鼠标）。

use std::time::{Duration, Instant};

use super::*;
use crate::app::MessageRole;
use crate::budget::{MAX_ACTIVE_TOASTS, MAX_OSC52_ENCODED_LENGTH, MAX_TOOL_TEXT_BYTES, TOAST_TTL_MS};

/// 灌满一屏以上的对话内容，使滚动量可测。
fn fill(fx: &mut Fixture) {
    for i in 0..40 {
        add(
            &mut fx.app,
            MessageRole::Agent,
            &format!("第 {i} 段内容，用于制造可滚动区间。"),
        );
    }
}

/// §6.3 #1：修饰键改变滚轮粒度——Ctrl 细粒度、Shift 整页。
#[test]
fn wheel_modifiers_change_granularity() {
    let mut fx = Fixture::new("scroll-wheel");
    fill(&mut fx);
    fx.draw();
    let page = fx.app.page_step;
    assert!(page > 3, "视口高度应大于默认滚轮步长");
    assert_eq!(fx.app.wheel_lines(false, true), 1);
    assert_eq!(fx.app.wheel_lines(true, false), page);
    assert_eq!(fx.app.wheel_lines(false, false), 3);
}

/// §6.3 #2：默认不捕获鼠标，保留终端原生文本选择能力。
#[test]
fn mouse_capture_defaults_off() {
    let mut fx = Fixture::new("scroll-mouse");
    assert!(!fx.app.mouse_capture);
    assert!(fx.app.toggle_mouse_capture());
    assert!(!fx.app.toggle_mouse_capture());
}

/// §6.3 #2：滚轮滚动与翻页键结果一致（同一控制面）。
#[test]
fn wheel_and_page_share_control_plane() {
    let mut fx = Fixture::new("scroll-share");
    fill(&mut fx);
    fx.draw();
    let max = fx.app.chat_scroll_max;
    assert!(max > 0);
    fx.app.wheel_up(3);
    assert_eq!(fx.app.scroll_offset, 3);
    fx.app.scroll_page_up();
    assert_eq!(fx.app.scroll_offset, 3 + fx.app.page_step);
    fx.app.scroll_top();
    assert_eq!(fx.app.scroll_offset, max);
}

/// §6.3 #5：脱离底部后浮标常显，回到未脱离态即消失。
#[test]
fn follow_badge_appears_when_detached() {
    let mut fx = Fixture::new("scroll-badge");
    fill(&mut fx);
    fx.draw();
    fx.app.scroll_top();
    let s = fx.draw();
    assert_has(&s, "↓ 回到底部");
    fx.app.scroll_bottom();
    let s = fx.draw();
    assert_lacks(&s, "↓ 回到底部");
}

/// §6.3 #3：OSC52 载荷上限为 100000 字节，超出即走平台命令链。
#[test]
fn osc52_payload_is_bounded() {
    assert_eq!(MAX_OSC52_ENCODED_LENGTH, 100_000);
}

/// §6.3 #4：复制成功给出 1.5s 瞬态提示。
#[test]
fn copy_toast_is_transient() {
    let mut fx = Fixture::new("scroll-toast");
    let t0 = Instant::now();
    fx.app.toast("已复制代码块");
    let s = fx.draw();
    assert_has(&s, "✓ 已复制代码块");
    assert!(!fx.app.toast_expire(t0 + Duration::from_millis(TOAST_TTL_MS)));
    assert!(fx.app.toast_expire(t0 + Duration::from_millis(TOAST_TTL_MS + 1)));
    let s = fx.draw();
    assert_lacks(&s, "已复制代码块");
}

/// §6.3 #4：同屏最多三条，溢出淘汰最旧。
#[test]
fn toast_queue_is_bounded() {
    let mut fx = Fixture::new("scroll-toast3");
    for i in 0..(MAX_ACTIVE_TOASTS + 2) {
        fx.app.toast(format!("提示 {i}"));
    }
    assert_eq!(fx.app.toasts().len(), MAX_ACTIVE_TOASTS);
    let s = fx.draw();
    assert_lacks(&s, "提示 0");
    assert_has(&s, &format!("提示 {}", MAX_ACTIVE_TOASTS + 1));
}

/// §6.3 #5：leader + y 复制的是最近一个完整代码块原文（不含围栏）。
#[test]
fn leader_yields_last_code_block() {
    let mut fx = Fixture::new("scroll-code");
    add(
        &mut fx.app,
        MessageRole::Agent,
        "示例：\n```rust\nfn demo() {}\n```\n结束。",
    );
    assert_eq!(fx.app.last_code().as_deref(), Some("fn demo() {}"));
}

/// §6.3 #5：未闭合代码块不参与复制，避免复制半截内容。
#[test]
fn unclosed_block_is_not_copied() {
    let mut fx = Fixture::new("scroll-unclosed");
    add(&mut fx.app, MessageRole::Agent, "```rust\nfn half() {");
    assert!(fx.app.last_code().is_none());
}

/// §6.3 #6：超长输出头尾各半保留，并标注原始字节数。
#[test]
fn long_output_is_truncated_with_marker() {
    let mut fx = Fixture::new("scroll-trunc");
    let body = format!("HEAD{}TAIL", "m".repeat(MAX_TOOL_TEXT_BYTES));
    add(&mut fx.app, MessageRole::ToolResult, &body);
    let msg = fx.app.messages.back().expect("消息已入队");
    assert!(msg.content.contains("[truncated: "));
    assert!(msg.content.contains("bytes total]"));
    assert!(msg.content.starts_with("HEAD"));
    assert!(msg.content.ends_with("TAIL"));
    assert_eq!(msg.original_bytes, body.len());
}
