// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全量会话视图（X8-04a）：Ctrl+O 打开的全屏只读回放，断言用户真正看到
// 的整屏文本；滚动、键位与退出都走产品公开入口，不复制内部状态。

use super::*;
use crate::keys::{self, Flow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

/// X8-04a #1：Ctrl+O 后整屏回放会话，输入面让行（不再出现提示符）。
#[test]
fn transcript_replaces_chat_and_hides_input() {
    let mut fx = Fixture::new("transcript-open");
    add(&mut fx.app, MessageRole::User, "回看这条用户消息");
    add(&mut fx.app, MessageRole::Agent, "回看这条助手回复");
    fx.app.toggle_transcript();
    let s = fx.draw();
    assert_has(&s, "全文会话");
    assert_has(&s, "回看这条用户消息");
    assert_has(&s, "回看这条助手回复");
    assert_lacks(&s, "❯");
}

/// X8-04a #2：默认停在最新；上滚到顶后旧消息可见、新消息退出视口。
#[test]
fn transcript_scrolls_back_to_oldest() {
    let mut fx = Fixture::new("transcript-scroll");
    for i in 0..40 {
        add(&mut fx.app, MessageRole::Agent, &format!("第 {i} 条会话内容"));
    }
    fx.app.toggle_transcript();
    let s = fx.draw();
    assert_has(&s, "第 39 条会话内容");
    assert_lacks(&s, "第 0 条会话内容");

    fx.app.transcript_top();
    let s = fx.draw();
    assert_has(&s, "第 0 条会话内容");
    assert_lacks(&s, "第 39 条会话内容");
}

/// X8-04a #3：Ctrl+O 为开关键，Esc 亦可退出；退出后回到对话区。
#[tokio::test]
async fn ctrl_o_and_esc_toggle_back() {
    let mut fx = Fixture::new("transcript-keys");
    add(&mut fx.app, MessageRole::Agent, "键位往返");

    let flow = keys::handle(
        &mut fx.app,
        key(KeyCode::Char('o'), KeyModifiers::CONTROL),
    )
    .await
    .expect("打开");
    assert!(matches!(flow, Flow::Continue));
    assert!(fx.app.transcript_open);

    let s = fx.draw();
    assert_has(&s, "全文会话");

    keys::handle(&mut fx.app, key(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .expect("关闭");
    assert!(!fx.app.transcript_open);
    let s = fx.draw();
    assert_has(&s, "❯");
    assert_lacks(&s, "全文会话");
}

/// X8-04a #4：只读面吞掉字符键，不会在看不见的输入框里累积文本。
#[tokio::test]
async fn transcript_swallows_typing() {
    let mut fx = Fixture::new("transcript-readonly");
    fx.app.toggle_transcript();
    let before = fx.app.input.clone();
    keys::handle(&mut fx.app, key(KeyCode::Char('x'), KeyModifiers::NONE))
        .await
        .expect("吞键");
    assert_eq!(fx.app.input, before);
}

/// X8-04a #5：窄屏下提示行不越界，正文按宽度折行后仍可滚动。
#[test]
fn transcript_survives_narrow_viewport() {
    let mut fx = Fixture::new("transcript-narrow");
    add(
        &mut fx.app,
        MessageRole::Agent,
        "窄屏回放内容需要按显示宽度折行后仍完整呈现",
    );
    fx.app.toggle_transcript();
    let s = fx.draw_at(46, 12);
    assert_has(&s, "全文会话");
    assert!(s.rows().iter().all(|r| r.chars().count() <= 46));
}
