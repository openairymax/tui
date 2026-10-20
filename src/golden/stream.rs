// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 流式输出体验（TUI 建设方案 §6.2）。驱动路径全部是真实应用状态与真实
// `present::render`：增量文本、思考链折叠预览、工具事件行、中止呈递、
// 未闭合代码块。断言只看用户屏上可见文本，不引入测试专用旁路。

use super::*;
use crate::app::ActivePanel;
use crate::budget::{MIN_RENDER_INTERVAL_MS, THINKING_PREVIEW_MAX_CHARS};

/// §6.2 #1：增量文本整块上屏，观感是成段涌现而非逐字动画；帧界 16ms。
#[test]
fn delta_merges_into_single_block() {
    assert_eq!(MIN_RENDER_INTERVAL_MS, 16);
    let mut fx = Fixture::new("stream-delta");
    fx.app.loading = true;
    fx.app.streaming_text = "第一段\n第二段".to_string();
    let s = fx.draw();
    assert_has(&s, "第一段");
    assert_has(&s, "第二段");
    assert_has(&s, "▍");
}

/// §6.2 #2：渲染不被 reveal 节奏门控——收到首块前状态行已显示「已受理」，
/// 收到正文后同帧转为「生成中」，状态机不等待吐字完成。
#[test]
fn reveal_never_gates_state_line() {
    let mut fx = Fixture::new("stream-gate");
    fx.app.loading = true;
    let s = fx.draw();
    assert_has(&s, "已受理");
    assert_lacks(&s, "生成中");

    fx.app.streaming_text = "已到达的正文".to_string();
    let s = fx.draw();
    assert_has(&s, "生成中");
    assert_has(&s, "已到达的正文");
    assert_lacks(&s, "已受理");
}

/// §6.2 #3：思考链折叠预览显示前 2,000 字符 + 「思考中 · N s」计时，
/// 超出部分给出折叠字数与 Alt+E 入口。
#[test]
fn thinking_preview_is_folded() {
    let mut fx = Fixture::new("stream-think");
    fx.app.loading = true;
    fx.app.stream_reasoning = "推".repeat(THINKING_PREVIEW_MAX_CHARS + 7);
    fx.app.stream_reasoning_start = Some(std::time::Instant::now());
    let s = fx.draw();
    assert_has(&s, "╭ 思考中 · 0s");
    assert_has(&s, "已折叠 7 字");
    assert_has(&s, "Alt+E 查看全文");
    assert_has(&s, "[Dual Think]");
    assert_has(&s, &format!("{} 字", THINKING_PREVIEW_MAX_CHARS + 7));
}

/// §6.2 #3：回合落定后预览自动收起，原文转存内存副本，经思考链视图按需取用。
#[test]
fn settled_thinking_moves_to_side_view() {
    let mut fx = Fixture::new("stream-settle");
    fx.app.loading = false;
    fx.app.stream_reasoning.clear();
    fx.app.stream_reasoning_start = None;
    fx.app.last_reasoning = Some("本轮推理原文".to_string());

    let s = fx.draw();
    assert_lacks(&s, "╭ 思考中");
    assert_lacks(&s, "已折叠");

    fx.app.active_panel = ActivePanel::Think;
    let s = fx.draw();
    assert_has(&s, "● 最近一轮思考链");
    assert_has(&s, "本轮推理原文");
}

/// §6.2 #4：工具事件行在正文气泡之前独立呈现，不污染回答正文。
#[test]
fn tool_events_render_above_bubble() {
    let mut fx = Fixture::new("stream-tool");
    fx.app.loading = true;
    fx.app
        .stream_tool_events
        .push("[Sub web_search Agent] 检索中".to_string());
    fx.app.streaming_text = "回答正文".to_string();
    let s = fx.draw();
    assert_has(&s, "[Sub web_search Agent] 检索中");
    assert_has(&s, "回答正文");
}

/// §6.2 #6：中断即时呈递——中止当帧即出现「已中止」徽章与系统说明，
/// 不再渲染未完成的打字机气泡与「生成中」状态行；裁决与落账不在 TUI。
/// 输入行的呼吸光标 `▍` 在可输入态恒在，故以流式专属的「生成中」与半成品
/// 气泡的消失证明呈递即时，而非以光标符号判定。
#[test]
fn aborted_turn_is_immediate() {
    let mut fx = Fixture::new("stream-abort");
    fx.app.loading = true;
    fx.app.streaming_text = "半截输出".to_string();
    let s = fx.draw();
    assert_has(&s, "半截输出");
    assert_has(&s, "生成中");

    fx.app.abort_task();
    let s = fx.draw();
    assert_has(&s, "✕ 已中止");
    assert_has(&s, "已中止本次请求。");
    assert_lacks(&s, "生成中");
    assert_lacks(&s, "半截输出");
}

/// §6.2 #7：未闭合代码块按已收内容增量呈现，闭合后内容仍连续可见，
/// 围栏标记本身永不上屏。
#[test]
fn unclosed_code_block_renders_incrementally() {
    let mut fx = Fixture::new("stream-fence");
    fx.app.loading = true;
    fx.app.streaming_text = "```rust\nfn demo() {\n".to_string();
    let s = fx.draw();
    assert_has(&s, "fn demo() {");
    assert_lacks(&s, "```");

    fx.app.streaming_text.push_str("    let x = 1;\n}\n```\n完成。");
    let s = fx.draw();
    assert_has(&s, "let x = 1;");
    assert_has(&s, "完成。");
    assert_lacks(&s, "```");
}

/// §6.2 #5：流式期间上滚脱离钉底，浮标即刻给出未读行数；回底后浮标消失。
#[test]
fn detached_stream_shows_badge() {
    let mut fx = Fixture::new("stream-follow");
    fx.app.loading = true;
    for i in 0..40 {
        add(&mut fx.app, MessageRole::Agent, &format!("流式历史 {i}"));
    }
    fx.app.streaming_text = "正在生成".to_string();
    fx.app.scroll_offset = 5;
    fx.app.follow_observe(fx.app.messages.len(), W);
    let s = fx.draw();
    assert_has(&s, "↓ 回到底部");

    fx.app.scroll_offset = 0;
    fx.app.follow_observe(fx.app.messages.len(), W);
    let s = fx.draw();
    assert_lacks(&s, "↓ 回到底部");
    assert_has(&s, "正在生成");
}
