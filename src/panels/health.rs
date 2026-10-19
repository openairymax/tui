// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 健康面板（0.1.19 U12：TUI 建设方案 §6.12 #3、§6.13 #2/#3/#4）。
//
// 只读视图：逐依赖给出端到端可达性与最后成功时长（「正常 · 3m12s 前」），
// 下方是错误聚合（同一错误码 ×N）与时间线校准标注。展示层不读时钟，
// 所有时长按注入的单调时刻计算；不显示裸 errno、内部通道名或原始错误串。
//
// 只读视图：↑↓ 逐行滚动 · PgUp/PgDn 翻页 · Esc 返回对话。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{dep_cn, App};
use crate::engine::health::{dur_cn, Dep, ErrEntry};
use crate::style;

/// 状态行：依赖总数、降级数与累计错误次数（一眼判断系统是否健康）。
fn stat_line(deps: usize, down: usize, total: u32, degraded: bool) -> Line<'static> {
    let (mark, color, word) = if degraded {
        ("▲", style::warning(), "存在降级")
    } else {
        ("●", style::success(), "全部正常")
    };
    Line::from(vec![
        Span::styled(
            format!("  {mark} {word}  "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{deps} 项依赖 · {down} 项降级 · {total} 次错误"),
            Style::default().fg(style::faint()),
        ),
    ])
}

/// 单依赖行：可达性 + 最后成功/不可达时长 + 失败与抖动计数。
fn dep_line(dep: &Dep, now_ms: u64) -> Line<'static> {
    let label = dep_cn(&dep.name);
    let (mark, color, span) = if dep.up {
        let span = match dep.ok_age(now_ms) {
            Some(age) => format!("{} · {} 前", dep.state(), dur_cn(age)),
            None => dep.state().to_string(),
        };
        ("✓", style::success(), span)
    } else {
        let span = match dep.down_age(now_ms) {
            Some(age) => format!("{} · {} 前起", dep.state(), dur_cn(age)),
            None => dep.state().to_string(),
        };
        ("✗", style::danger(), span)
    };
    let mut spans = vec![
        Span::styled(
            format!("  {mark} {label}  "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(span, Style::default().fg(style::dim())),
    ];
    if dep.fails > 0 || dep.flaps > 0 {
        spans.push(Span::styled(
            format!(" · 失败 {} 次 / 抖动 {} 次", dep.fails, dep.flaps),
            Style::default().fg(style::faint()),
        ));
    }
    Line::from(spans)
}

/// 单类聚合错误行：×N 计数 + 人话成因 + 首次/最近时长 + 修复命令。
/// 不显示原始错误串；修复命令真实存在，可直接复制执行（§6.12 #4）。
fn err_line(e: &ErrEntry, now_ms: u64) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  ×{} ", e.count),
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(e.label.to_string(), Style::default().fg(style::dim())),
        Span::styled(
            format!(
                " · 首次 {} 前 · 最近 {} 前",
                dur_cn(now_ms.saturating_sub(e.first_ms)),
                dur_cn(now_ms.saturating_sub(e.last_ms))
            ),
            Style::default().fg(style::faint()),
        ),
        Span::styled(
            format!(" · 修复：{}", e.remedy),
            Style::default().fg(style::warning()),
        ),
    ])
}

/// 空态：尚未完成首轮探测时的明确说明（不伪装成「全部正常」）。
fn empty_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "  尚无依赖探测结果。",
            Style::default().fg(style::dim()),
        )),
        Line::from(Span::styled(
            "  健康探测每 5s 自动执行一次（网关 /health 与各服务 health_check）。",
            Style::default().fg(style::faint()),
        )),
        Line::from(Span::styled(
            "  探测结果到达后本页自动更新，按 Esc 返回对话。",
            Style::default().fg(style::faint()),
        )),
    ]
}

/// 正文行：依赖清单 → 错误聚合 → 时间线/抖动标注。
fn body_lines(app: &App) -> Vec<Line<'static>> {
    let deps = app.health.deps();
    if deps.is_empty() {
        return empty_lines();
    }
    let now = app.clock_ms;
    let mut out: Vec<Line<'static>> = Vec::with_capacity(deps.len() + app.err_agg.len() + 4);
    for dep in deps {
        out.push(dep_line(dep, now));
    }

    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(
        format!(
            "  错误聚合（{} 类，共 {} 次）",
            app.err_agg.len(),
            app.err_agg.total()
        ),
        Style::default()
            .fg(style::primary())
            .add_modifier(Modifier::BOLD),
    )));
    if app.err_agg.len() == 0 {
        out.push(Line::from(Span::styled(
            "  本轮未记录错误。",
            Style::default().fg(style::faint()),
        )));
    }
    for e in app.err_agg.entries() {
        out.push(err_line(e, now));
    }
    if app.err_agg.overflow() > 0 {
        out.push(Line::from(Span::styled(
            format!("  其他合并 {} 次", app.err_agg.overflow()),
            Style::default().fg(style::faint()),
        )));
    }

    if let Some(note) = app.timeline.note() {
        out.push(Line::raw(""));
        out.push(Line::from(Span::styled(
            format!("  {note}"),
            Style::default().fg(style::warning()),
        )));
    }
    if app.banner.flaps() > 0 {
        out.push(Line::from(Span::styled(
            format!("  降级横幅冷却期抑制重弹 {} 次", app.banner.flaps()),
            Style::default().fg(style::faint()),
        )));
    }
    out
}

/// Render the health panel (read-only; opened by /health).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(style::border()))
        .title(Span::styled(
            " 依赖健康 ",
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));

    let inner_h = (area.height.saturating_sub(2) as usize).max(1);
    let down = app.health.deps().iter().filter(|d| !d.up).count();
    let body = body_lines(app);

    // 顶部状态行与底部提示行固定，仅正文窗口滚动（offset = 正文首行下标）。
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    lines.push(stat_line(
        app.health.deps().len(),
        down,
        app.err_agg.total(),
        app.health.degraded(),
    ));
    if inner_h >= 3 {
        let body_rows = inner_h - 2;
        let offset = app.health_scroll.min(body.len().saturating_sub(body_rows));
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

    fn dep(name: &str, up: bool, fails: u32, flaps: u32) -> Dep {
        Dep {
            name: name.to_string(),
            up,
            last_ok_ms: up.then_some(0),
            last_fail_ms: (!up).then_some(0),
            fails,
            flaps,
        }
    }

    /// 可达依赖：显示「正常」与最后成功时长，用户读到的是人话而非 errno。
    #[test]
    fn up_dep_shows_ok_age() {
        let text = dep_line(&dep("llm", true, 0, 0), 192_000).to_string();
        assert!(text.contains("✓"), "缺少可达标记: {text}");
        assert!(text.contains("LLM 后端"), "应显示中文依赖名: {text}");
        assert!(text.contains("正常 · 3m12s 前"), "缺少最后成功时长: {text}");
        assert!(!text.contains("llm"), "不得暴露内部命名空间: {text}");
    }

    /// 不可达依赖：显示「不可达」与持续时长，并带失败/抖动计数。
    #[test]
    fn down_dep_shows_down_age() {
        let text = dep_line(&dep("gateway", false, 3, 1), 65_000).to_string();
        assert!(text.contains("✗"), "缺少不可达标记: {text}");
        assert!(text.contains("网关"), "应显示中文依赖名: {text}");
        assert!(text.contains("不可达 · 1m5s 前起"), "缺少持续时长: {text}");
        assert!(text.contains("失败 3 次 / 抖动 1 次"), "缺少计数: {text}");
    }

    /// 错误聚合行：×N 计数与最近发生时长，且不出现原始错误正文。
    #[test]
    fn err_line_shows_count_and_age() {
        let e = ErrEntry {
            code: "unreachable",
            label: "无法连接后端服务",
            remedy: "airymaxrt logs llm_d",
            count: 7,
            first_ms: 0,
            last_ms: 60_000,
        };
        let text = err_line(&e, 180_000).to_string();
        assert!(text.contains("×7"), "缺少聚合计数: {text}");
        assert!(text.contains("无法连接后端服务"), "缺少人话成因: {text}");
        assert!(text.contains("首次 3m0s 前"), "缺少首次发生时长: {text}");
        assert!(text.contains("最近 2m0s 前"), "缺少最近发生时长: {text}");
        assert!(
            text.contains("修复：airymaxrt logs llm_d"),
            "缺少可行动修复命令: {text}"
        );
        assert!(!text.contains("unreachable"), "不得显示内部错误码: {text}");
    }

    /// 空态不得伪装成健康：明确说明尚未探测。
    #[test]
    fn empty_state_is_explicit() {
        let text: String = empty_lines().iter().map(|l| l.to_string()).collect();
        assert!(text.contains("尚无依赖探测结果"), "空态说明缺失: {text}");
        assert!(!text.contains("全部正常"), "空态不得冒充健康: {text}");
    }

    /// 状态行：降级数非零时必须显式示警。
    #[test]
    fn stat_line_flags_degraded() {
        let text = stat_line(3, 1, 5, true).to_string();
        assert!(text.contains("存在降级"), "缺少降级示警: {text}");
        assert!(
            text.contains("3 项依赖 · 1 项降级 · 5 次错误"),
            "统计缺失: {text}"
        );
        assert!(stat_line(2, 0, 0, false).to_string().contains("全部正常"));
    }
}
