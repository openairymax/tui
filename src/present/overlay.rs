// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 浮层：依赖降级横幅、前缀键提示、瞬态提示、IME 候选区、工具审批条。

use crate::app::App;
use crate::engine::grid;
use crate::engine::health::dur_cn;
use crate::style;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

/// 依赖降级横幅（0.1.19 U12，TUI 方案 §6.12 #1/#6）：单行常显最重依赖，
/// 附真实存在的修复命令；恢复后由状态机撤销并进入冷却期，抖动计数进 /health。
pub(super) fn render_degraded(f: &mut Frame, area: Rect, app: &App) {
    // 持续时长按注入的单调时刻计算：刚降级时不加噪声，超过 10s 才显示。
    let age = app.clock_ms.saturating_sub(app.banner.since_ms());
    let since = if age >= 10_000 {
        format!(" · 已持续 {} ", dur_cn(age))
    } else {
        String::new()
    };
    let line = Line::from(vec![
        Span::styled(
            " ▲ ",
            Style::default()
                .fg(style::on_color())
                .bg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} {} ", app.banner.text(), since),
            Style::default()
                .fg(style::on_color())
                .bg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  修复：{} ", app.banner.remedy()),
            Style::default().fg(style::warning()),
        ),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(style::surface())),
        area,
    );
}

/// leader 提示浮层（0.1.19 L7 U8，TUI 方案 §6.4 #7）：单行说明可用续键，
/// 前缀序列的取消与超时由状态机负责，此处只读展示。
pub(super) fn render_leader_hint(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" Ctrl+G ", Style::default().fg(style::primary())),
        Span::styled(
            "前缀 · y 复制最近代码块 · Esc 取消",
            Style::default().fg(style::dim()),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// 瞬态提示条（0.1.19 L7 U7，TUI 方案 §6.3 #4）：输入栏上方逐行展示，
/// 先进先出、同屏不超过三条。文本按显示宽度裁剪，长提示不撑破版面。
pub(super) fn render_toasts(f: &mut Frame, area: Rect, app: &App) {
    let limit = area.width.saturating_sub(4) as usize;
    let lines: Vec<Line> = app
        .toasts()
        .active()
        .map(|text| {
            Line::from(vec![
                Span::styled(
                    " ✓ ",
                    Style::default()
                        .fg(style::success())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(grid::clip(text, limit), Style::default().fg(style::text())),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(style::surface_2())),
        area,
    );
}

/// IME 候选区（输入框第二行）：`[中] 拼音` + 当前页候选。
///
/// 2.2.3 重新设计（2026-08-23）：Ctrl+1 激活后输入框恒为两行（第一行
/// 输入 + 第二行候选区），本函数渲染第二行。拼音缓冲为空时显示
/// `[中] 拼音输入中…` 占位，让激活瞬间即有明确视觉反馈；缓冲非空
/// 时显示拼音高亮 + 微信式分页候选（当前页 9 个，页内高亮蓝底，
/// 多页时尾部页码指示 ‹1/2›；空格/Enter 上屏高亮候选，数字选字，
/// ,/. 或 PgUp/PgDn 翻页，←/→ 移动高亮）。与 C 侧 CLI 的
/// tui_ime_draw_cands 语义一致。
pub(super) fn render_ime_cands(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(
            "[中]",
            Style::default()
                .fg(style::on_color())
                .bg(style::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if app.ime_buf.is_empty() {
        spans.push(Span::styled(
            "拼音输入中…（a-z 拼音 · 1-9 选字 · 空格/Enter 上屏 · ,/. 翻页 · Esc 取消）",
            Style::default().fg(style::faint()),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {} ", app.ime_buf),
            Style::default()
                .fg(style::on_color())
                .bg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        // 当前页切片（微信式分页：每页 9 个，页内高亮）
        let start = app.ime_page * 9;
        let end = (start + 9).min(app.ime_cands.len());
        for (off, cand) in app.ime_cands[start..end].iter().enumerate() {
            let tag = format!("{}.{} ", off + 1, cand);
            if off == app.ime_sel {
                spans.push(Span::styled(
                    tag,
                    Style::default()
                        .fg(style::on_color())
                        .bg(style::primary())
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(tag, Style::default().fg(style::dim())));
            }
        }
        // 页码指示（多页时显示 ‹cur/total›）
        if app.ime_pages > 1 {
            spans.push(Span::styled(
                format!("‹{}/{}›", app.ime_page + 1, app.ime_pages),
                Style::default().fg(style::faint()),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(style::surface())),
        area,
    );
}

/// 工具审批提示条：pending 审批时在输入栏上方高亮显示（Claude Code 风格
/// permission prompt），给出工具、主体与 [a]/[A]/[n] 快捷决议提示。
pub(super) fn render_approval_banner(f: &mut Frame, area: Rect, app: &App) {
    let Some(a) = app.approvals.first() else {
        return;
    };
    // 参数预览单行截断：避免长 JSON 撑满横幅。预算为列，故按显示宽度裁决：
    // 此前按字符数兑现列预算，全角参数会把右侧 [a]/[A]/[n] 决议提示挤出横幅。
    let params_preview = grid::clip(&a.params, area.width.saturating_sub(8) as usize);
    let line1 = Line::from(vec![
        Span::styled(
            " ⚠ ",
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "工具审批请求",
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.approvals.len() > 1 {
                format!("  ·  1 / {}", app.approvals.len())
            } else {
                String::new()
            },
            Style::default().fg(style::faint()),
        ),
        Span::styled(
            format!(
                "  ·  主体: {}",
                if a.agent_id.is_empty() {
                    "unknown"
                } else {
                    &a.agent_id
                }
            ),
            Style::default().fg(style::dim()),
        ),
        Span::styled(
            format!("  ·  工具: {}", a.tool),
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  ·  时间: {}",
                a.created_at
                    .map(|ts| chrono::DateTime::from_timestamp(ts as i64, 0)
                        .map(|dt| dt.format("%H:%M:%S").to_string())
                        .unwrap_or_else(|| "—".to_string()))
                    .unwrap_or_else(|| "—".to_string())
            ),
            Style::default().fg(style::faint()),
        ),
    ]);
    let line2 = Line::from(vec![
        Span::raw(format!("   {params_preview}")),
        Span::raw("  "),
        Span::styled(
            "[y] 允许本次",
            Style::default()
                .fg(style::success())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "[a] 本会话总是",
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "[n] 拒绝（Esc）",
            Style::default()
                .fg(style::danger())
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(Text::from(vec![line1, line2])).style(Style::default().bg(style::surface())),
        area,
    );
}
