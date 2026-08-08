// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Config panel rendering.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// Render the config panel.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 配置 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let model_display = if app.model.is_empty() {
        "默认（由网关 / llm_d 自动回落）".to_string()
    } else {
        app.model.clone()
    };

    let lines: Vec<Line> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  当前模型  ", Style::default().fg(theme::faint())),
            Span::styled(model_display, Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  切换模型：/model <模型名>（持久化到 $AIRY_HOME/tui/config.toml）",
            Style::default().fg(theme::dim()),
        )),
        Line::from(Span::styled(
            "  查看当前：/model",
            Style::default().fg(theme::dim()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  模型默认配置（model.yaml 用户覆盖，可自由增删 provider 与模型）：",
            Style::default().fg(theme::faint()),
        )),
        Line::from(Span::styled(
            format!("    {}", app.config_file),
            Style::default().fg(theme::text()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  回落优先级：请求 model 参数 → env AIRY_AGENT_MODEL →",
            Style::default().fg(theme::faint()),
        )),
        Line::from(Span::styled(
            "  $AIRY_HOME/config/model.yaml global.default_model → 内置默认",
            Style::default().fg(theme::faint()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  API Key 配置：编辑 $AIRY_HOME/config/secrets.env（勿提交到仓库）",
            Style::default().fg(theme::dim()),
        )),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}
