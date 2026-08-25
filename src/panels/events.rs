// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Event stream panel (hall.stream): global event replay ordered by
// (ts_utc, seq) — the cross-process stable causal order of the hall store.
// Also exposes event_line() reused by the /chain decision-chain command.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::client::HallEvent;
use crate::theme;

/// 七类事件的语义标签（与 C 版 airy_cli 标签映射对齐）
pub fn category_label(cat: &str) -> &'static str {
    match cat {
        "blueprint" => "蓝图",
        "command" => "命令",
        "progress" => "进度",
        "result" => "结果",
        "issue" => "问题",
        "verify" => "复核",
        "chain" => "决策",
        _ => "事件",
    }
}

fn category_color(cat: &str) -> Color {
    match cat {
        "blueprint" => theme::PRIMARY,
        "command" => theme::CYAN,
        "progress" => theme::WARNING,
        "result" => theme::SUCCESS,
        "issue" => theme::DANGER,
        "verify" => theme::MAGENTA,
        "chain" => theme::ACCENT,
        _ => theme::dim(),
    }
}

/// 从事件 content 提取可读摘要（各类别取关键字段，回退紧凑 JSON）。
fn content_summary(e: &HallEvent) -> String {
    let c = &e.content;
    let pick = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Some(v) = c.get(*k) {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                };
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    };
    let s = match e.category.as_str() {
        "command" => {
            let plan = pick(&["plan_id", "plan", "task_plan_id"]);
            match plan {
                Some(p) => {
                    let nodes = c.get("nodes").and_then(|n| n.as_u64());
                    match nodes {
                        Some(n) => format!("计划 {} · {} 节点", p, n),
                        None => format!("计划 {}", p),
                    }
                }
                None => pick(&["action", "event"]).unwrap_or_else(|| raw_compact(c)),
            }
        }
        "progress" => {
            let status = pick(&["status", "event"]).unwrap_or_default();
            let pct = c
                .get("progress")
                .and_then(|p| p.as_f64())
                .map(|p| format!(" {:>3}%", (p * 100.0).round() as u64))
                .unwrap_or_default();
            format!("{}{}", status, pct)
        }
        "issue" => pick(&["error_code", "failure_class", "event"]).unwrap_or_else(|| raw_compact(c)),
        "result" => {
            let ok = pick(&["status", "verdict"]).unwrap_or_default();
            if !ok.is_empty() {
                ok
            } else {
                truncate(raw_compact(c), 72)
            }
        }
        "verify" => pick(&["verdict", "status", "event"]).unwrap_or_else(|| raw_compact(c)),
        "chain" => pick(&["msg", "event", "kind"]).unwrap_or_else(|| raw_compact(c)),
        _ => raw_compact(c),
    };
    truncate(s, 96)
}

/// 紧凑 JSON（无空白）
fn raw_compact(c: &serde_json::Value) -> String {
    serde_json::to_string(c).unwrap_or_default()
}

fn truncate(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut)
}

/// 单条事件的渲染行：`[类别:gseq] 任务 摘要`（/chain 命令复用）。
pub fn event_line(e: &HallEvent, max: usize) -> String {
    let label = category_label(&e.category);
    let task: String = e.task_id.chars().take(20).collect();
    let summary = content_summary(e);
    let total = format!("[{}:{}] {} {}", label, e.gseq, task, summary);
    truncate(total, max)
}

fn event_row(e: &HallEvent, selected: bool) -> Line<'static> {
    let label = category_label(&e.category);
    let task: String = e.task_id.chars().take(20).collect();
    let base = if selected {
        Style::default().bg(theme::surface_active())
    } else {
        Style::default()
    };
    let marker = if selected { "▸" } else { " " };
    Line::from(vec![
        Span::styled(
            format!(" {} [{}:{}]", marker, label, e.gseq),
            base.fg(category_color(&e.category)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} ", task), base.fg(theme::ACCENT)),
        Span::styled(content_summary(e), base.fg(theme::text())),
    ])
}

/// Render the event stream panel (newest first).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 事件流 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let total = app.hall_events.len();
    // 过滤提示：0=全部 · 1-7=类别；当前过滤突出显示
    let filter_tip = if app.events_filter.is_empty() {
        "全部".to_string()
    } else {
        format!("{} ({} 条)", category_label(&app.events_filter), app.events_visible_count())
    };
    let mut lines: Vec<Line> = vec![Line::from(vec![
        Span::styled(
            format!("  全局 gseq 因果序 · 最新 {} 条  ", total),
            Style::default().fg(theme::dim()),
        ),
        Span::styled("过滤: ", Style::default().fg(theme::faint())),
        Span::styled(
            filter_tip,
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.connected { "  ● ONLINE" } else { "  ● OFFLINE" },
            Style::default()
                .fg(if app.connected { theme::SUCCESS } else { theme::DANGER })
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.push(Line::raw(""));

    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  暂无事件：任务执行产生的 progress/result/issue/verify 事件将在此回放。",
            Style::default().fg(theme::faint()),
        )));
    } else {
        // hall.stream 已返回最新 N 条（升序），此处倒序展示（最新在前）
        let events: Vec<&HallEvent> = if app.events_filter.is_empty() {
            app.hall_events.iter().rev().collect()
        } else {
            app.hall_events
                .iter()
                .rev()
                .filter(|e| e.category == app.events_filter)
                .collect()
        };
        let sel = app.events_cursor % events.len().max(1);
        for (i, e) in events.iter().take(256).enumerate() {
            lines.push(event_row(e, i == sel));
        }
    }

    // 操作提示条（简约：↑↓ 选择 · 数字过滤 · Enter 详情 · Esc 返回）
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  ↑↓ 选择", Style::default().fg(theme::faint())),
        Span::styled("  0-7 过滤", Style::default().fg(theme::faint())),
        Span::styled("  Enter 详情", Style::default().fg(theme::faint())),
        Span::styled("  Esc 返回", Style::default().fg(theme::faint())),
    ]));

    f.render_widget(Paragraph::new(lines).block(block), area);
}
