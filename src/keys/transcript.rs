// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全量会话视图键位（X8-04a/X8-04b）：只读回放面，滚动、翻页、顶底、退出，
// 以及 `/` 打开的增量检索。
//
// 与对话输入面互斥：打开期间字符键一律不落到输入框，避免「看不见却
// 在编辑」。检索打开后按键面再收窄一层：字符只进检索串，Esc 先关检索、
// 再按才退视图。Ctrl+C 不在此处理，由 `handle` 放回空闲/忙碌分派面复用
// 双击退出守卫，退出语义全局唯一。

use crossterm::event::{self, KeyCode, KeyEvent};

use super::Flow;
use crate::app::App;

/// 全量会话视图按键：Esc 与 Ctrl+O 均关闭，滚动方向与对话区一致。
pub(super) fn transcript_key(app: &mut App, key: KeyEvent) -> Flow {
    if app.search.open {
        return search_key(app, key);
    }
    let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('o') | KeyCode::Char('O') if ctrl => app.close_transcript(),
        KeyCode::Char('/') if !ctrl => app.open_search(),
        KeyCode::Esc => app.close_transcript(),
        KeyCode::Up => app.transcript_scroll_up(),
        KeyCode::Down => app.transcript_scroll_down(),
        KeyCode::PageUp => app.transcript_page_up(),
        KeyCode::PageDown => app.transcript_page_down(),
        KeyCode::Home => app.transcript_top(),
        KeyCode::End => app.transcript_bottom(),
        _ => {}
    }
    Flow::Continue
}

/// 检索态按键：字符进查询串，Enter 下一条，↑/↓ 双向跳转，Esc 收检索。
///
/// 检索态不再响应视图滚动快捷键：方向键先服务命中跳转，避免「翻页翻出
/// 命中」与「跳转」两套语义在同一键上打架。
fn search_key(app: &mut App, key: KeyEvent) -> Flow {
    let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('o') | KeyCode::Char('O') if ctrl => app.close_transcript(),
        KeyCode::Esc => app.close_search(),
        KeyCode::Enter | KeyCode::Down => app.search_next(),
        KeyCode::Up => app.search_prev(),
        KeyCode::Backspace => app.search_pop(),
        KeyCode::Char(c) if !ctrl => app.search_push(c),
        _ => {}
    }
    Flow::Continue
}
