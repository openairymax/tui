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
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        ));

    if app.logs.is_empty() {
        let home = std::env::var("AIRY_HOME").unwrap_or_else(|_| "~/.airymaxrt".to_string());
        let text = vec![
            Line::from(Span::styled(
                "  暂无日志",
                Style::default().fg(theme::dim()),
            )),
            Line::from(Span::styled(
                "  连接 gateway 后事件日志将在此显示。",
                Style::default().fg(theme::faint()),
            )),
            Line::from(Span::styled(
                format!("  完整运行时日志（各 daemon）：{home}/logs/*.log"),
                Style::default().fg(theme::faint()),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for entry in app.logs.iter().rev().take(area.height as usize) {
        let level_style = match entry.level.as_str() {
            "ERROR" => Style::default().fg(theme::danger()).add_modifier(Modifier::BOLD),
            "WARN" => Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD),
            "INFO" => Style::default().fg(theme::accent()),
            _ => Style::default().fg(theme::text()),
        };
        let daemon_span = entry
            .daemon
            .as_deref()
            .map(|d| {
                vec![
                    Span::styled(format!("[{d}] "), Style::default().fg(theme::faint())),
                ]
            })
            .unwrap_or_default();
        let mut spans = vec![
            Span::styled(&entry.timestamp, Style::default().fg(theme::faint())),
            Span::raw(" "),
            Span::styled(&entry.level, level_style),
            Span::raw(" "),
        ];
        spans.extend(daemon_span);
        spans.push(Span::styled(&entry.message, Style::default().fg(theme::dim())));
        lines.push(Line::from(spans));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).block(block),
        area,
    );
}