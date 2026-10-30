// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 演化指标面板（0.1.20 W8）：只读展示 think_d 演化回路经 monit observe 表
// 上报的六项指标（名称 / 数值 / 量纲 / 类型 / 原始键），并标注数据出处。
//
// 诚实语义：monit 不可达时显示原始原因、指标为空时显示「尚未上报」，
// 两种空态都不合成 0 值，也不把未知键藏起来。展示层不读时钟。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::engine::evolution::{kind, Metric, Snapshot};
use crate::engine::health::dur_cn;
use crate::style;

/// 数值格式化：整数不带小数，比例保留两位；NaN 表示该指标值缺失。
fn fmt_val(v: f64) -> String {
    if !v.is_finite() {
        return "未上报".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// 单指标行：名称 + 数值 + 量纲 + 类型 + 原始键（键名便于对照 monit 数据）。
fn metric_line(m: &Metric) -> Line<'static> {
    let unit = if m.unit.is_empty() { "—" } else { &m.unit };
    let ty = kind(&m.key);
    let ty = if ty.is_empty() { "—" } else { ty };
    Line::from(vec![
        Span::styled(
            format!("  {}  ", m.label),
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>10}", fmt_val(m.value)),
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {unit}"), Style::default().fg(style::dim())),
        Span::styled(format!(" · {ty}"), Style::default().fg(style::faint())),
        Span::styled(format!(" · {}", m.key), Style::default().fg(style::faint())),
    ])
}

/// 状态行：在途 / 不可用 / 在线三态，在线时给出条目数与最近更新时长。
fn stat_line(evo: Option<&Snapshot>, now_ms: u64, at_ms: u64) -> Line<'static> {
    let age = dur_cn(now_ms.saturating_sub(at_ms));
    match evo {
        None => Line::from(Span::styled(
            "  ○ 正在拉取演化指标…",
            Style::default().fg(style::dim()),
        )),
        Some(s) if !s.available => Line::from(vec![
            Span::styled(
                "  ▲ 演化指标不可用  ",
                Style::default()
                    .fg(style::warning())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{age} 前查询"), Style::default().fg(style::faint())),
        ]),
        Some(s) => Line::from(vec![
            Span::styled(
                "  ● 演化回路在线  ",
                Style::default()
                    .fg(style::success())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} 项指标 · {age} 前更新", s.items.len()),
                Style::default().fg(style::faint()),
            ),
        ]),
    }
}

/// 在途空态：首轮结果未到，不显示占位数值。
fn pending_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "  正在经 gateway 拉取演化指标。",
            Style::default().fg(style::dim()),
        )),
        Line::from(Span::styled(
            "  结果到达后本页自动更新，按 Esc 返回对话。",
            Style::default().fg(style::faint()),
        )),
    ]
}

/// 不可用空态：原样给出 gateway 返回的原因（不臆造成因）。
fn down_lines(reason: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("  原因：{reason}"),
            Style::default().fg(style::warning()),
        )),
        Line::from(Span::styled(
            "  演化指标由 think_d 上报至 monit observe 表，经 gateway 只读转发。",
            Style::default().fg(style::faint()),
        )),
        Line::from(Span::styled(
            "  本页不显示任何替代数值；monit 恢复后自动刷新。",
            Style::default().fg(style::faint()),
        )),
    ]
}

/// 已连接但无演化指标：如实说明尚未上报，不补 0。
fn empty_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "  monit 已连接，但演化回路尚未上报指标。",
            Style::default().fg(style::dim()),
        )),
        Line::from(Span::styled(
            "  可能原因：think_d 未运行，或尚未触发演化节拍。",
            Style::default().fg(style::faint()),
        )),
        Line::from(Span::styled(
            "  指标出现后本页自动更新，按 Esc 返回对话。",
            Style::default().fg(style::faint()),
        )),
    ]
}

/// 正文行：指标清单 + 出处标注；空态按可用性分三种。
fn body_lines(app: &App) -> Vec<Line<'static>> {
    let Some(s) = app.evo.as_ref() else {
        return pending_lines();
    };
    if !s.available {
        return down_lines(&s.reason);
    }
    if s.items.is_empty() {
        return empty_lines();
    }
    let mut out: Vec<Line<'static>> = Vec::with_capacity(s.items.len() + 5);
    for m in &s.items {
        out.push(metric_line(m));
    }
    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(
        "  数据源：monit · 经 gateway 只读转发",
        Style::default().fg(style::faint()),
    )));
    out.push(Line::from(Span::styled(
        "  上报者：think_d 演化回路",
        Style::default().fg(style::faint()),
    )));
    out
}

/// Render the evolution-metrics panel (read-only; opened by /evolution).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(style::border()))
        .title(Span::styled(
            " 演化指标 ",
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));

    let inner_h = (area.height.saturating_sub(2) as usize).max(1);
    let body = body_lines(app);

    // 顶部状态行与底部提示行固定，仅正文窗口滚动（offset = 正文首行下标）。
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    lines.push(stat_line(app.evo.as_ref(), app.clock_ms, app.evo_at_ms));
    if inner_h >= 3 {
        let body_rows = inner_h - 2;
        let offset = app.evo_scroll.min(body.len().saturating_sub(body_rows));
        lines.extend(body.into_iter().skip(offset).take(body_rows));
    }
    while lines.len() < inner_h.saturating_sub(1) {
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled("  ↑↓ 滚动", Style::default().fg(style::faint())),
        Span::styled("  PgUp/PgDn 翻页", Style::default().fg(style::faint())),
        Span::styled("  Esc 返回对话", Style::default().fg(style::faint())),
    ]));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(key: &str, label: &str, value: f64, unit: &str) -> Metric {
        Metric {
            key: key.to_string(),
            label: label.to_string(),
            value,
            unit: unit.to_string(),
        }
    }

    /// 整数指标不带小数，比例保留两位，缺值显式标注「未上报」（不是 0）。
    #[test]
    fn fmt_val_keeps_integers_and_marks_nan() {
        assert_eq!(fmt_val(12.0), "12");
        assert_eq!(fmt_val(0.0), "0");
        assert_eq!(fmt_val(0.856), "0.86");
        assert_eq!(fmt_val(f64::NAN), "未上报");
    }

    /// 指标行四要素齐备：名称、数值、量纲、类型，且保留原始键便于对照。
    #[test]
    fn metric_line_shows_name_value_unit_type() {
        let text = metric_line(&metric("detections_total", "检出次数", 7.0, "次")).to_string();
        assert!(text.contains("检出次数"), "缺少名称: {text}");
        assert!(text.contains('7'), "缺少数值: {text}");
        assert!(text.contains("次"), "缺少量纲: {text}");
        assert!(text.contains("计数器"), "缺少类型: {text}");
        assert!(text.contains("detections_total"), "缺少原始键: {text}");
    }

    /// 指标为空时必须说明「尚未上报」，且不得出现任何指标行或 0 值。
    #[test]
    fn empty_state_is_honest() {
        let text: String = empty_lines().iter().map(|l| l.to_string()).collect();
        assert!(text.contains("尚未上报"), "空态说明缺失: {text}");
        assert!(text.contains("think_d"), "缺少原因说明: {text}");
    }

    /// monit 不可达时空态原样展示原因，不显示指标。
    #[test]
    fn down_state_shows_reason() {
        let text: String = down_lines("monit unreachable")
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert!(text.contains("monit unreachable"), "缺少原始原因: {text}");
        assert!(text.contains("不显示任何替代数值"), "缺少诚实声明: {text}");
    }

    /// 状态行三态区分：在途 / 不可用 / 在线（在线给条目数与更新时长）。
    #[test]
    fn stat_line_distinguishes_states() {
        assert!(stat_line(None, 0, 0).to_string().contains("正在拉取"));
        let down = Snapshot {
            available: false,
            reason: "monit unreachable".to_string(),
            items: Vec::new(),
        };
        assert!(stat_line(Some(&down), 0, 0).to_string().contains("不可用"));
        let live = Snapshot {
            available: true,
            reason: String::new(),
            items: vec![metric("patterns", "演化模式", 3.0, "个")],
        };
        let text = stat_line(Some(&live), 65_000, 0).to_string();
        assert!(text.contains("演化回路在线"), "在线态缺失: {text}");
        assert!(text.contains("1 项指标"), "缺少条目数: {text}");
        assert!(text.contains("1m5s 前更新"), "缺少更新时长: {text}");
    }
}
