// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Help panel rendering: 分类着色帮助面板。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::theme;

/// 渲染帮助面板。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 帮助 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let mut lines: Vec<Line> = Vec::new();
    for s in app.help_text.iter() {
        let t = s.trim_end();
        if t.is_empty() {
            lines.push(Line::raw(""));
            continue;
        }
        if t.ends_with(':') || t.ends_with('：') {
            // 分类标题
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    t.to_string(),
                    Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
                ),
            ]));
            continue;
        }
        if t.starts_with("  ") {
            // 条目：键位（首个 token）用主色，说明用正文
            let t = t.trim_start();
            if let Some((key, rest)) = t.split_once(char::is_whitespace) {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(
                        format!("{key}"),
                        Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(rest, Style::default().fg(theme::dim())),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(t.to_string(), Style::default().fg(theme::dim())),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(t.to_string(), Style::default().fg(theme::text())),
            ]));
        }
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}
