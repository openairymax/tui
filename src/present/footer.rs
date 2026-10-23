// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 快捷键行：单行低对比，当前面板键高亮，窄屏自动收起文字标签。

use crate::app::{ActivePanel, App};
use crate::style;
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// 快捷键（单行低对比，Claude Code 风格克制；0.1.3 去掉彩色胶囊背景，
/// 仅当前面板键以主色高亮，其余灰显；窄屏自动收起文字标签）。
pub(super) fn render_shortcuts(f: &mut Frame, area: Rect, app: &App) {
    // F8 是"切到 CLI"动作键而非面板：active 恒为 None → 普通文字不高亮，
    // 修复此前与 ActivePanel::Chat 绑定导致的"伪激活"恒亮（当前在对话时
    // F8 也被误高亮成已激活面板）
    let items: [(&str, &str, Option<ActivePanel>); 8] = [
        ("F1", "帮助", Some(ActivePanel::Help)),
        ("F2", "配置", Some(ActivePanel::Config)),
        ("F3", "日志", Some(ActivePanel::Logs)),
        ("F4", "记忆", Some(ActivePanel::Memory)),
        ("F5", "插件", Some(ActivePanel::Plugins)),
        ("F6", "看板", Some(ActivePanel::Board)),
        ("F7", "事件流", Some(ActivePanel::Events)),
        ("F8", "CLI", None),
    ];

    // 窄屏只显示键位，隐藏文字标签，避免溢出
    let compact = area.width < 72;

    let mut spans: Vec<Span> = Vec::with_capacity(items.len() * 2 + 2);
    for (key, label, panel) in items {
        let active = panel == Some(app.active_panel);
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(if active {
                style::primary()
            } else {
                style::dim()
            }),
        ));
        if !compact {
            spans.push(Span::styled(
                format!("{label} "),
                Style::default().fg(if active {
                    style::primary()
                } else {
                    style::faint()
                }),
            ));
        }
    }
    /* 0.1.3 美化：快捷键分组——导航组（F1-F8）与任务控制/退出组之间
     * 用主色竖线分隔，避免"一排灰字"失去节奏；窄屏自动收起分组。 */
    if area.width >= 80 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled("┃", Style::default().fg(style::primary())));
        spans.push(Span::styled(
            "  输入  Enter 发送 · Alt+Enter 换行 · Alt+↑/↓ 历史",
            Style::default().fg(style::faint()),
        ));
    }
    if area.width >= 52 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled("┃", Style::default().fg(style::primary())));
        spans.push(Span::styled(
            "  控制  Ctrl+Z 暂停 · Ctrl+X 中止",
            Style::default().fg(style::faint()),
        ));
    }
    if area.width >= 40 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled("┃", Style::default().fg(style::primary())));
        spans.push(Span::styled(
            "  Ctrl+C 退出",
            Style::default().fg(style::faint()),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Left)
            .style(Style::default().bg(style::surface())),
        area,
    );
}
