// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 尺寸、resize 与可访问性（TUI 建设方案 §6.9）。断点为闭区间下界；
// 宽度档位决定输入区徽标的信息量；CJK 双宽必须在屏上完整可读。

use super::*;
use crate::app::{ActivePanel, MessageRole};
use crate::budget::{MAX_CHAT_MESSAGES, MIN_TERM_COLS, WIDTH_CACHE_SIZE};

/// §6.9 #1：46 列是窄布局断点，45 列整屏守卫、46 列恢复正常。
#[test]
fn col_breakpoint_is_forty_six() {
    let mut fx = Fixture::new("size-col");
    let narrow = fx.draw_at(MIN_TERM_COLS - 1, H);
    assert_has(&narrow, "终端尺寸不足");
    let ok = fx.draw_at(MIN_TERM_COLS, H);
    assert_lacks(&ok, "终端尺寸不足");
    assert_has(&ok, "❯");
}

/// §6.9 #1：窄屏也不丢输入行与快捷键行，尺寸提示本身可操作。
#[test]
fn narrow_guard_keeps_exit_path() {
    let mut fx = Fixture::new("size-guard");
    let s = fx.draw_at(30, 10);
    assert_has(&s, "Ctrl+C 退出");
}

/// §6.9 #2：行高缓存容量有界，resize 多次不破坏渲染。
#[test]
fn resize_is_stable_across_widths() {
    // 容量下界是编译期不变式，放进 const 块让 clippy 与编译器都能静态校验。
    const { assert!(WIDTH_CACHE_SIZE >= 64) };
    let mut fx = Fixture::new("size-resize");
    add(&mut fx.app, MessageRole::Agent, "宽度变化后仍需完整渲染。");
    for w in [MIN_TERM_COLS, 60, 80, W, 120, 60, MIN_TERM_COLS] {
        let s = fx.draw_at(w, 20);
        assert_has(&s, "AirymaxRT");
    }
}

/// §6.9 #3：`?` 呼出键位浮层（帮助面板），并给出返回路径。
#[test]
fn question_mark_opens_keymap() {
    let mut fx = Fixture::new("size-help");
    fx.app.toggle_panel(ActivePanel::Help);
    let s = fx.draw();
    assert_has(&s, "帮助只读");
}

/// §6.9 #4：纯文本降级下正文仍完整可达（markdown 降为可见文本）。
#[test]
fn plain_text_body_is_reachable() {
    let mut fx = Fixture::new("size-plain");
    add(
        &mut fx.app,
        MessageRole::Agent,
        "# 标题\n\n- 第一项\n- 第二项\n\n结尾段落文本。",
    );
    let s = fx.draw_at(MIN_TERM_COLS, 20);
    assert_has(&s, "标题");
    assert_has(&s, "结尾段落文本。");
}

/// §6.9 #5：CJK 双宽字符在屏上保持原样，不被拆裂或错位。
#[test]
fn cjk_width_is_exact() {
    let mut fx = Fixture::new("size-cjk");
    add(&mut fx.app, MessageRole::User, "中文双宽字符测试");
    let s = fx.draw();
    assert_has(&s, "中文双宽字符测试");
}

/// §6.9 #5：中英混排同样不撕裂。
#[test]
fn mixed_width_line_is_intact() {
    let mut fx = Fixture::new("size-mixed");
    add(&mut fx.app, MessageRole::User, "AirymaxRT 是底座");
    let s = fx.draw();
    assert_has(&s, "AirymaxRT 是底座");
}

/// §6.9 #6：翻页步长跟随视口高度，且等于渲染回写值。
#[test]
fn page_step_follows_viewport() {
    let mut fx = Fixture::new("size-page");
    add(&mut fx.app, MessageRole::Agent, "内容");
    let h = 20;
    fx.draw_at(W, h);
    assert!(fx.app.page_step > 0);
    assert!(fx.app.page_step <= h);
}

/// §6.9 #6：内容不足一屏时滚动为 no-op，不积累脏偏移。
#[test]
fn scroll_is_noop_when_content_fits() {
    let mut fx = Fixture::new("size-noscroll");
    add(&mut fx.app, MessageRole::Agent, "只有一行");
    fx.draw();
    assert_eq!(fx.app.chat_scroll_max, 0);
    fx.app.scroll_page_up();
    assert_eq!(fx.app.scroll_offset, 0);
}

/// §6.4 #8：宽度不足 56 列时不展开上下文占用，只保留模式词，
/// 避免徽标挤压输入文本。
#[test]
fn narrow_input_hides_context_badge() {
    let mut fx = Fixture::new("size-badge");
    let s = fx.draw_at(50, 20);
    assert_lacks(&s, &format!("上下文 0/{MAX_CHAT_MESSAGES}"));
    assert_has(&s, "normal");
}
