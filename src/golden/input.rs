// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 输入与编辑（TUI 建设方案 §6.4）。断言分两层：状态层契约（折叠台账、
// 历史、补全）与屏上可见结果（回显、徽标、leader 提示）。

use std::time::{Duration, Instant};

use super::*;
use crate::budget::{LEADER_HINT_DELAY_MS, MAX_CHAT_MESSAGES, MAX_PASTE_FOLD_BYTES};

/// §6.4 #1：字符插入当帧即可见，不经任何异步通道。
#[test]
fn echo_is_immediate() {
    let mut fx = Fixture::new("input-echo");
    fx.app.input_insert_text("hello");
    let s = fx.draw();
    assert_has(&s, "hello");
}

/// §6.4 #2：小规模多行粘贴原样进入输入缓冲，不丢行。
#[test]
fn multi_line_paste_kept_verbatim() {
    let mut fx = Fixture::new("input-multiline");
    fx.app.input_paste("第一行\n第二行");
    assert_eq!(fx.app.input, "第一行\n第二行");
    assert_eq!(fx.app.cursor, fx.app.input.len());
}

/// §6.4 #3：超过折叠阈值的大粘贴只留占位，屏上可读且带 KB/行数。
#[test]
fn large_paste_folds_into_slot() {
    let mut fx = Fixture::new("input-fold");
    let body = "x".repeat(MAX_PASTE_FOLD_BYTES + 1);
    fx.app.input_paste(&body);
    assert!(fx.app.input.starts_with("[已粘贴 "));
    assert!(fx.app.input.contains(" KB · "));
    assert!(fx.app.input.contains(" 行]"));
    let s = fx.draw();
    assert_has(&s, "[已粘贴 ");
}

/// §6.4 #3：提交路径单遍展开占位，原文完整回归。
#[test]
fn take_input_expands_fold() {
    let mut fx = Fixture::new("input-expand");
    let body = format!("{}\n尾部", "y".repeat(MAX_PASTE_FOLD_BYTES));
    fx.app.input_paste(&body);
    let out = fx.app.take_input();
    assert_eq!(out, body);
    assert!(fx.app.input.is_empty());
}

/// §6.4 #3：占位被删改后不再展开——输入框所见即所发。
#[test]
fn edited_slot_discards_fold() {
    let mut fx = Fixture::new("input-edit");
    fx.app.input_paste(&"z".repeat(MAX_PASTE_FOLD_BYTES + 1));
    fx.app.input.clear();
    assert_eq!(fx.app.take_input(), "");
}

/// §6.4 #4：历史为空时 Alt+↑ 不清空也不污染当前输入。
#[test]
fn history_empty_is_noop() {
    let mut fx = Fixture::new("input-hist");
    fx.app.input_insert_text("进行中的草稿");
    let before = fx.app.input.clone();
    fx.app.history_prev();
    assert_eq!(fx.app.input, before);
}

/// §6.4 #5：`/` 前缀补全链按候选顺序推进。
#[test]
fn slash_completion_cycles() {
    let mut fx = Fixture::new("input-slash");
    fx.app.input_insert_text("/");
    fx.app.tab_complete();
    assert_eq!(fx.app.input, "/model");
    // 第二候选是同时以 "/model" 为前缀的 "/models"（补全按前缀匹配取下一个）。
    fx.app.tab_complete();
    assert_eq!(fx.app.input, "/models");
    assert_eq!(fx.app.cursor, fx.app.input.len());
}

/// §6.4 #5：无匹配前缀时补全为 no-op，不吞掉用户输入。
#[test]
fn completion_without_match_is_noop() {
    let mut fx = Fixture::new("input-nomatch");
    fx.app.input_insert_text("普通文本");
    fx.app.tab_complete();
    assert_eq!(fx.app.input, "普通文本");
}

/// §6.4 #6：IME 未激活时 Enter 不吞键、不产生消息（不误发）。
#[test]
fn ime_inactive_enter_does_not_fire() {
    let mut fx = Fixture::new("input-ime");
    let before = fx.app.messages.len();
    assert!(!fx.app.ime_commit_enter());
    assert_eq!(fx.app.messages.len(), before);
    assert!(!fx.app.ime_visible());
}

/// §6.4 #7：前缀键静置超过 500ms 才浮出提示，之前保持安静。
#[test]
fn leader_hint_waits_for_delay() {
    let mut fx = Fixture::new("input-leader");
    fx.app.leader_press();
    let now = Instant::now();
    let left = fx.app.leader_wait(now).expect("前缀待续态应给出剩余等待");
    // 剩余等待只能在按压时刻起的延迟窗口内，据此反推精确到期点。
    assert!(left <= Duration::from_millis(LEADER_HINT_DELAY_MS));
    assert!(left > Duration::from_millis(LEADER_HINT_DELAY_MS - 100));
    let due = now + left;
    assert!(!fx.app.leader_hint_due(due - Duration::from_millis(1)));
    assert!(fx.app.leader_hint_due(due));
    let s = fx.draw();
    assert_has(&s, "Ctrl+G");
    assert_has(&s, "前缀");
}

/// §6.4 #7：Esc 取消后提示立即撤销。
#[test]
fn leader_cancel_clears_hint() {
    let mut fx = Fixture::new("input-leader-x");
    fx.app.leader_press();
    let now = Instant::now();
    let due = now + fx.app.leader_wait(now).expect("前缀待续态应给出剩余等待");
    assert!(fx.app.leader_hint_due(due));
    assert!(fx.app.leader_hint());
    fx.app.leader_cancel();
    let s = fx.draw();
    assert_lacks(&s, "Ctrl+G");
}

/// §6.4 #8：宽屏下输入区常显模式、上下文占用与 token 计数。
#[test]
fn input_status_shows_context_and_tokens() {
    let mut fx = Fixture::new("input-status");
    fx.app.tokens = 42;
    let s = fx.draw();
    assert_has(&s, &format!("上下文 0/{MAX_CHAT_MESSAGES}"));
    assert_has(&s, "42 tok");
}

/// §6.4 #8：任务集模式用 plan 常显，普通对话为 normal。
#[test]
fn input_status_reflects_mode() {
    let mut fx = Fixture::new("input-mode");
    fx.app.task_mode = true;
    let s = fx.draw();
    assert_has(&s, "plan");
    assert_lacks(&s, "normal");
}
