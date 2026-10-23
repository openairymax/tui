// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 状态条与会话 tab 栏：品牌徽记、连接灯、运行数据段、阶段徽章。

use crate::app::{dep_cn, App};
use crate::engine::grid;
use crate::gccp::FlowPhase;
use crate::style;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// 会话 tab 栏（2026-08-21 多会话）：`1 标题 | 2 标题 …` 胶囊。
/// 当前 tab 高亮（主色底 + 反色字），其余弱化；Alt+1..9 切换 · Ctrl+T 新建。
pub(super) fn render_tab_bar(f: &mut Frame, area: Rect, app: &App) {
    let n = app.tab_count();
    let current = app.current_tab_index();
    let mut spans: Vec<Span> = vec![Span::styled(" ", Style::default())];
    for i in 0..n {
        let active = i == current;
        // 胶囊标题预算 16 列：按显示宽度裁决，否则全角标题会撑破 tab 栏
        // （一个汉字占 2 列，按字符取 16 实占 32 列，后续 tab 被挤出屏幕）。
        let title = grid::clip(&app.tab_title(i), 16);
        spans.push(Span::styled(
            format!(" {} {} ", i + 1, title),
            Style::default()
                .fg(if active {
                    style::on_color()
                } else {
                    style::dim()
                })
                .bg(if active {
                    style::primary()
                } else {
                    style::surface_3()
                })
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(style::surface_2())),
        area,
    );
}

/// 系统状态条（顶部 1 行，2.0 重设计）。
///
/// 新视觉：品牌徽记块（PRIMARY 反色底 " ◈ AirymaxRT " + 版本号）锚定最左，
/// 其后依次是 连接灯+时间 → 模型 → token/成本 → 阶段徽章 → 任务控制，
/// 各段用主色细分隔符 "│" 连接——状态条成为分层"品牌头"（底色 style::bar()），
/// 与内容区 surface 拉开纵深。窄屏（<72 列）自动收起右侧运行数据段，
/// 仅保留 品牌徽记 + 连接 + 时间，保证任何宽度下清晰不溢出。
///
/// 品牌/能力/硬件等静态展示移交 chat 空态欢迎墙独占，职责分离无重叠。
pub(super) fn render_hero(f: &mut Frame, area: Rect, app: &App) {
    let bar_bg = style::bar();
    // 状态灯（§6.6 #2 三态：在线 / 重连中·n / 已补账·k）
    let (light, label, warn) = app.link.label();
    let color = match (warn, app.link.is_online()) {
        (true, _) => style::warning(),
        (false, true) => style::success(),
        (false, false) => style::danger(),
    };
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    let ver = app
        .gateway_version
        .clone()
        .unwrap_or_else(|| env!("AIRY_RT_VERSION").to_string());

    // 左锚：品牌徽记块（主色底反白）+ 版本号（bar 底灰显）
    let mut spans: Vec<Span> = vec![
        Span::styled(
            " ◈ AirymaxRT ",
            Style::default()
                .fg(style::on_color())
                .bg(style::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" v{ver} "), Style::default().fg(style::faint())),
    ];
    // 连接灯 + 时间（窄屏也保留）
    spans.push(Span::styled(
        light,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!(" {label}"),
        Style::default().fg(color),
    ));
    spans.push(Span::styled(
        format!("  {now}"),
        Style::default().fg(style::dim()),
    ));

    // 宽屏（≥72 列）展开右侧运行数据段（分段分隔符连接）
    if area.width >= 72 {
        let model_text = if app.model.is_empty() {
            "默认模型".to_string()
        } else {
            app.model.clone()
        };
        spans.push(seg());
        spans.push(Span::styled(
            model_text,
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("  {} tok · ${:.4}", app.tokens, app.cost),
            Style::default().fg(style::dim()),
        ));
        let (badge, badge_color) = phase_badge(app.flow_phase);
        let badge_text = match app.flow_phase {
            FlowPhase::GccpRound(_) => format!(" 任务事实确认 {}/5 ", app.gccp.answered()),
            _ => badge,
        };
        spans.push(seg());
        spans.push(Span::styled(
            badge_text,
            Style::default()
                .fg(style::on_color())
                .bg(badge_color)
                .add_modifier(Modifier::BOLD),
        ));
        if let crate::gccp::TaskControl::Paused = app.task_control {
            spans.push(Span::styled(
                "  ⏸ 已暂停 ",
                Style::default()
                    .fg(style::on_color())
                    .bg(style::primary())
                    .add_modifier(Modifier::BOLD),
            ));
        } else if let crate::gccp::TaskControl::Aborted = app.task_control {
            spans.push(Span::styled(
                "  ✕ 已中止 ",
                Style::default()
                    .fg(style::on_color())
                    .bg(style::danger())
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    // 依赖降级徽章（0.1.19 U12，TUI 方案 §6.12 #2）：符号 + 文字双编码，
    // 只取最重依赖（持续不可达最长）；多依赖全量在 /health 面板。
    if let Some(dep) = app.health.worst() {
        spans.push(seg());
        spans.push(Span::styled(
            format!(" ▲ degraded: {} ", dep_cn(dep)),
            Style::default()
                .fg(style::on_color())
                .bg(style::warning())
                .add_modifier(Modifier::BOLD),
        ));
    }
    // 渲染降级横幅（0.1.18 §5A.3 W10）：合成层故障（渲染回调 panic / 后端
    // 写失败）时置位。窄屏也保留——这是告警而非数据段；画面本身停在上一帧，
    // 横幅提示用户「会话仍在推进，画面正在重连」，自愈后由主循环回写消隐。
    if app.render_degraded {
        spans.push(seg());
        spans.push(Span::styled(
            " ⚠ 渲染降级 · 重连中 ",
            Style::default()
                .fg(style::on_color())
                .bg(style::danger())
                .add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bar_bg)),
        area,
    );
}

/// 状态条分段分隔符（" │ "，主色弱化细竖线）
fn seg() -> Span<'static> {
    Span::styled("  │  ", Style::default().fg(style::separator()))
}

/// 阶段徽章配色（对话 / 任务事实确认 / 目标澄清 / 任务流程图确认 / 任务集）
fn phase_badge(phase: FlowPhase) -> (String, ratatui::style::Color) {
    match phase {
        FlowPhase::Chat => (" 对话 ".into(), style::success()),
        FlowPhase::GccpRound(_) => (" 任务事实确认 ".into(), style::primary()),
        FlowPhase::GccpClarify => (" 目标澄清 ".into(), style::primary()),
        FlowPhase::GradConfirm => (" 任务流程图确认 ".into(), style::magenta()),
        FlowPhase::Executing => (" 任务集 ".into(), style::warning()),
    }
}
