// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Task board panel (hall.board): work_hall persisted execution instances
// + live agent roster from agent_d, refreshed by app.poll_hall().

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::client::HallBoardEntry;
use crate::theme;

/// 状态图标（与 C 版 airy_cli cli_panel_state_icon 对齐）
fn state_icon(state: &str) -> &'static str {
    match state {
        "completed" => "✓",
        "running" | "pending" | "scheduled" => "◇",
        "failed" => "✗",
        "canceled" => "■",
        _ => "•",
    }
}

/// 状态语义色（与 C 版 cli_panel_state_color 对齐）
fn state_color(state: &str) -> Color {
    match state {
        "completed" => theme::SUCCESS,
        "failed" | "canceled" => theme::DANGER,
        "running" | "pending" | "scheduled" => theme::WARNING,
        _ => theme::dim(),
    }
}

/// 8 格迷你进度条（单行面板，保持紧凑）
fn mini_bar(prog: f64) -> String {
    let p = prog.clamp(0.0, 1.0);
    let filled = (p * 8.0).round() as usize;
    let mut s = String::from("[");
    for i in 0..8 {
        s.push(if i < filled { '#' } else { '-' });
    }
    s.push(']');
    s
}

fn entry_line(e: &HallBoardEntry) -> Line<'static> {
    let name: String = e.workflow_name.chars().take(24).collect();
    let state = if e.state.is_empty() { "unknown" } else { &e.state };
    Line::from(vec![
        Span::styled(
            format!(" {}  ", state_icon(state)),
            Style::default().fg(state_color(state)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<24}", name),
            Style::default().fg(theme::text()),
        ),
        Span::styled(
            format!("{:<12}", state),
            Style::default().fg(state_color(state)),
        ),
        Span::styled(
            format!(" {} ", mini_bar(e.progress)),
            Style::default().fg(theme::ACCENT),
        ),
        Span::styled(
            format!("{:>3}%", (e.progress * 100.0).round() as u64),
            Style::default().fg(theme::dim()),
        ),
        Span::styled(
            format!("  #{}", e.task_id),
            Style::default().fg(theme::faint()),
        ),
    ])
}

/// Render the task board panel.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 任务看板 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let mut lines: Vec<Line> = Vec::new();

    let (entry_count, agent_count) = match &app.hall_board {
        Some(b) => (b.entries.len(), b.agents.len()),
        None => (0, 0),
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  执行实例 {} · 在线 Agent {}", entry_count, agent_count),
            Style::default().fg(theme::dim()),
        ),
        Span::styled(
            if app.connected { "  ● ONLINE" } else { "  ● OFFLINE" },
            Style::default()
                .fg(if app.connected { theme::SUCCESS } else { theme::DANGER })
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));

    if let Some(board) = &app.hall_board {
        // 最新在前（work_hall board 按提交序排列，倒序展示）
        let mut entries = board.entries.clone();
        entries.reverse();
        if entries.is_empty() {
            lines.push(Line::from(Span::styled(
                "  暂无执行实例：运行任务后 work_hall 状态在此展示（$AIRY_HOME/state/work_hall_state.json）。",
                Style::default().fg(theme::faint()),
            )));
        } else {
            for e in entries.iter().take(64) {
                lines.push(entry_line(e));
            }
            if entries.len() > 64 {
                lines.push(Line::from(Span::styled(
                    format!("  … 共 {} 条", entries.len()),
                    Style::default().fg(theme::faint()),
                )));
            }
        }
        if !board.agents.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  在线 Agent：",
                Style::default().fg(theme::dim()),
            )));
            for a in board.agents.iter().take(8) {
                lines.push(Line::from(vec![
                    Span::styled("   ● ", Style::default().fg(theme::SUCCESS)),
                    Span::styled(a.clone(), Style::default().fg(theme::text())),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  正在加载看板（gateway hall.board）…",
            Style::default().fg(theme::faint()),
        )));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
