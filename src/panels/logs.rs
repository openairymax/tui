// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Logs panel rendering.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// Render the logs panel.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 运行时日志 ",
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ));

    if app.logs.is_empty() {
        let text = vec![
            Line::from(Span::styled(
                "  暂无日志",
                Style::default().fg(theme::dim()),
            )),
            Line::from(Span::styled(
                "  连接 gateway 后日志将在此显示。",
                Style::default().fg(theme::faint()),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for entry in app.logs.iter().rev().take(area.height as usize) {
        let level_style = match entry.level.as_str() {
            "ERROR" => Style::default().fg(theme::DANGER).add_modifier(Modifier::BOLD),
            "WARN" => Style::default().fg(theme::WARNING).add_modifier(Modifier::BOLD),
            "INFO" => Style::default().fg(theme::ACCENT),
            _ => Style::default().fg(theme::text()),
        };

        lines.push(Line::from(vec![
            Span::styled(&entry.timestamp, Style::default().fg(theme::faint())),
            Span::raw(" "),
            Span::styled(&entry.level, level_style),
            Span::raw(" "),
            Span::styled(&entry.message, Style::default().fg(theme::dim())),
        ]));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).block(block),
        area,
    );
}