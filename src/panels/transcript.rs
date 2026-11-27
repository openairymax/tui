// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全量会话视图（X8-04a）：Ctrl+O 打开的全屏只读回放。
//
// 与对话区共用同一虚拟视图与行高缓存，只有「展开态固定为全展开」与
// 「独立距底偏移」两处不同——回看长会话不另起一套测量，也不改写对话
// 区视口；偏移为 0 时持续跟随最新，流式输出到结束都能原地阅读。

use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::style;

use super::chat::focus_hint;

/// 顶部键位提示行高度。
const HINT_ROWS: u16 = 1;

/// 键位提示文案：与焦点视图同一「单行常显」体例，Esc 为唯一退出。
const HINT: &str = " 全文会话 · ↑↓ 滚动 · PgUp/PgDn 翻页 · Home/End 顶底 · Esc 返回";

/// 渲染全量会话视图：顶部提示 + 正文（虚拟视口）+ 右下滚动位置。
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height <= HINT_ROWS {
        return;
    }
    let head = Rect {
        height: HINT_ROWS,
        ..area
    };
    let body = Rect {
        y: area.y + HINT_ROWS,
        height: area.height - HINT_ROWS,
        ..area
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            HINT,
            Style::default().fg(style::faint()).bg(style::bar()),
        ))),
        head,
    );

    let viewport = body.height as usize;
    // 缓存视图从 App 取出，避免 layout 的 &App 与 &mut chat_view 借用冲突
    let mut view = std::mem::take(&mut app.chat_view);
    let frame = view.layout_from(
        app,
        body.width as usize,
        viewport,
        false,
        app.transcript_scroll,
        true,
    );
    app.chat_view = view;
    // 可滚总量每帧回写（与对话区同一单向契约），偏移越界即钳位。
    app.transcript_scroll_max = frame.total.saturating_sub(viewport) as u16;
    app.transcript_scroll = app.transcript_scroll.min(app.transcript_scroll_max);
    app.page_step = viewport as u16;

    f.render_widget(
        Paragraph::new(Text::from(frame.lines)).style(Style::default().bg(style::bg())),
        body,
    );

    if let Some(text) = focus_hint(frame.from_top, app.transcript_scroll_max as usize, viewport) {
        let w = (UnicodeWidthStr::width(text.as_str()) as u16 + 2).min(body.width);
        let hint = Rect {
            x: body.right().saturating_sub(w),
            y: body.bottom().saturating_sub(1),
            width: w,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Right)
                .style(Style::default().fg(style::faint()).bg(style::surface())),
            hint,
        );
    }
}
