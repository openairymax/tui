// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全量会话视图键位（X8-04a）：只读回放面，只认滚动、翻页、顶底与退出。
//
// 与对话输入面互斥：打开期间字符键一律不落到输入框，避免「看不见却
// 在编辑」。Ctrl+C 不在此处理，由 `handle` 放回空闲/忙碌分派面复用
// 双击退出守卫，退出语义全局唯一。

use crossterm::event::{self, KeyCode, KeyEvent};

use super::Flow;
use crate::app::App;

/// 全量会话视图按键：Esc 与 Ctrl+O 均关闭，滚动方向与对话区一致。
pub(super) fn transcript_key(app: &mut App, key: KeyEvent) -> Flow {
    let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('o') | KeyCode::Char('O') if ctrl => app.close_transcript(),
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
