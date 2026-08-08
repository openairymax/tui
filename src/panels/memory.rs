// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Memory panel rendering.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// Render the memory statistics panel.
///
/// 实时渲染本地对话记忆库（$AIRY_HOME/tui/memory.jsonl）：
/// 条数 + 最近 N 条摘要，无需依赖网关 HTTP 端点。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 记忆库 ",
            Style::default().fg(theme::SUCCESS).add_modifier(Modifier::BOLD),
        ));

    let total = app.memory.len();
    if total == 0 {
        let text = vec![
            Line::from(Span::styled("  暂无记忆记录", Style::default().fg(theme::dim()))),
            Line::from(Span::styled(
                "  对话内容将自动持久化到 $AIRY_HOME/tui/memory.jsonl。",
                Style::default().fg(theme::faint()),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  记忆条数  ", Style::default().fg(theme::faint())),
            Span::styled(format!("{}", total), Style::default().fg(theme::SUCCESS).add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
        Line::from(Span::styled("  最近记录：", Style::default().fg(theme::dim()))),
    ];

    for rec in app.memory.recent(8) {
        let speaker = match rec.role.as_str() {
            "user" => "用户",
            "assistant" => "助手",
            _ => &rec.role,
        };
        let content: String = rec.content.chars().take(80).collect();
        lines.push(Line::from(vec![
            Span::styled(format!("  [{}] ", speaker), Style::default().fg(theme::ACCENT)),
            Span::styled(content, Style::default().fg(theme::text())),
        ]));
    }
    if total > 8 {
        lines.push(Line::from(Span::styled(
            format!("  … 共 {} 条", total),
            Style::default().fg(theme::faint()),
        )));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
