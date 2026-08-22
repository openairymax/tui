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

    let recents = app.memory.recent(8);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  记忆条数  ", Style::default().fg(theme::faint())),
            Span::styled(format!("{}", total), Style::default().fg(theme::SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("  ·  记忆链  ", Style::default().fg(theme::faint())),
            Span::styled(
                format!("最近 {} 条", recents.len()),
                Style::default().fg(theme::ACCENT),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled("  记忆链（按时间序）：", Style::default().fg(theme::dim()))),
    ];

    // 记忆链渲染：每条记录为链上一个节点，时间戳 + 角色 + 内容；末条用
    // 收尾符，中间的用连接符，直观呈现记忆的前后承接关系（2.2.1.8）。
    let n = recents.len();
    for (i, rec) in recents.iter().enumerate() {
        let speaker = match rec.role.as_str() {
            "user" => "用户",
            "assistant" => "助手",
            "system" => "系统",
            _ => &rec.role,
        };
        let content: String = rec.content.chars().take(72).collect();
        let (stem, conn) = if i + 1 == n {
            ("└─", "  ")
        } else {
            ("├─", "│ ")
        };
        let hhmm = rec.timestamp.chars().take(16).collect::<String>();
        lines.push(Line::from(vec![
            Span::styled(format!("  {}", stem), Style::default().fg(theme::border())),
            Span::styled(format!("[{}] ", speaker), Style::default().fg(theme::ACCENT)),
            Span::styled(format!("{} ", hhmm), Style::default().fg(theme::faint())),
            Span::styled(content, Style::default().fg(theme::text())),
        ]));
        // 下一条内容在前一节点下方缩进对齐，形成纵向记忆链
        if i + 1 < n {
            lines.push(Line::from(Span::styled(
                format!("  {}  ↳", conn),
                Style::default().fg(theme::border()),
            )));
        }
    }
    if total > 8 {
        lines.push(Line::from(Span::styled(
            format!("  … 共 {} 条", total),
            Style::default().fg(theme::faint()),
        )));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
