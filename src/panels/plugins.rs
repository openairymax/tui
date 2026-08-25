// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Plugins panel rendering.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// Render the plugins panel.
///
/// 实时渲染本地技能库（$AIRY_HOME/tui/skills.jsonl）：Agent 在任务中
/// 自我沉淀的可复用技能，无需依赖网关 HTTP 端点。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 插件 / 技能库 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let skills = app.skills.list();
    if skills.is_empty() {
        let text = vec![
            Line::from(Span::styled("  技能库为空", Style::default().fg(theme::dim()))),
            Line::from(Span::styled(
                "  任务成功后经验会自动沉淀为可复用技能（$AIRY_HOME/tui/skills.jsonl）。",
                Style::default().fg(theme::faint()),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  技能条数  ", Style::default().fg(theme::faint())),
            Span::styled(format!("{}", skills.len()), Style::default().fg(theme::MAGENTA).add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
    ];

    for skill in skills.iter().take(10) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", skill.name), Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
            Span::styled(format!("[{}] ", skill.category), Style::default().fg(theme::faint())),
            Span::styled(format!("复用 {} 次", skill.success_count), Style::default().fg(theme::dim())),
        ]));
        let summary: String = skill.summary.chars().take(70).collect();
        lines.push(Line::from(Span::styled(
            format!("    {}", summary),
            Style::default().fg(theme::text()),
        )));
    }
    if skills.len() > 10 {
        lines.push(Line::from(Span::styled(
            format!("  … 共 {} 条", skills.len()),
            Style::default().fg(theme::faint()),
        )));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
