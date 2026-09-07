// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Logs panel rendering.
//
// 视图语义：最新在顶（与聊天一致），↑/↓ 在"距最新的条目偏移"上滚动
// （offset 由 app.logs_scroll 维护；0 = 最新）。条目内嵌换行/回车被展平
// 为空格，消息按面板宽度截断——保证一条日志恒占一行、行数与滚动偏移
// 严格一一对应，长消息不再把后续日志挤出屏外。

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
        let home = std::env::var("AIRY_HOME")
            .unwrap_or_else(|_| format!("~/{}", crate::paths::DEFAULT_DIR_NAME));
        let text = vec![
            Line::from(Span::styled(
                "  暂无日志",
                Style::default().fg(theme::dim()),
            )),
            Line::from(Span::styled(
                "  本 TUI 进程的连接/权限/控制等运行时事件将在此显示。",
                Style::default().fg(theme::faint()),
            )),
            Line::from(Span::styled(
                format!("  完整 daemon 运行日志：{home}/logs/*.log"),
                Style::default().fg(theme::faint()),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    // 去 block 边框后的可视区
    let inner_w = area.width.saturating_sub(2) as usize;
    let view_rows = (area.height.saturating_sub(2) as usize).max(1);
    let total = app.logs.len();
    // 滚动偏移钳到可视范围（offset 行被顶出视口顶部 → 已向更早滚动）
    let offset = app.logs_scroll.min(total.saturating_sub(view_rows));

    let mut lines: Vec<Line> = Vec::with_capacity(view_rows);
    for entry in app.logs.iter().rev().skip(offset).take(view_rows) {
        let level_style = match entry.level.as_str() {
            "ERROR" => Style::default().fg(theme::danger()).add_modifier(Modifier::BOLD),
            "WARN" => Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD),
            "INFO" => Style::default().fg(theme::accent()),
            _ => Style::default().fg(theme::text()),
        };
        // 条目恒占一行：内嵌换行展平为空格（anyhow 链式错误常含多行）
        let flat: String = entry.message.replace(['\n', '\r'], " ");
        // 行截断：消息可写宽度 = 面板内宽 − 前缀（时间/级别/来源）
        let daemon_head: String = entry
            .daemon
            .as_deref()
            .map(|d| format!("[{d}] "))
            .unwrap_or_default();
        let head_chars = 8 /*HH:MM:SS*/ + 1 + entry.level.chars().count() + 1
            + daemon_head.chars().count();
        let room = inner_w.saturating_sub(head_chars);
        let msg: String = if flat.chars().count() > room {
            let cut: String = flat.chars().take(room).collect();
            format!("{cut}…")
        } else {
            flat
        };
        let mut spans = vec![
            Span::styled(&entry.timestamp, Style::default().fg(theme::faint())),
            Span::raw(" "),
            Span::styled(&entry.level, level_style),
            Span::raw(" "),
        ];
        if !daemon_head.is_empty() {
            spans.push(Span::styled(daemon_head, Style::default().fg(theme::faint())));
        }
        spans.push(Span::styled(msg, Style::default().fg(theme::dim())));
        lines.push(Line::from(spans));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).block(block),
        area,
    );
}
