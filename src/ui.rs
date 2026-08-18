// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 主渲染：统一布局与主题（极简自适应）。

// Layout structure（主内容区随终端大小弹性伸缩）:
// ┌─ Brand Bar ─────────────────────────────────┐
// │ ◈ AirymaxRT v0.1.2  ● ONLINE    技能 0  [对话]│
// ├─ Main Content（自适应填充）───────────────────┤
// ├─ Input Bar ────────────────────────────────┤
// │ ❯ 输入提示…                                 │
// └─ Shortcuts（居中）──────────────────────────┘
//  F1 帮助  F2 配置  F3 日志  F4 记忆  F5 插件  Ctrl+C 退出

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    Frame,
};
use crate::app::{ActivePanel, App};
use crate::gccp::FlowPhase;
use crate::panels;
use crate::theme;
use crate::wizard;

/// 呼吸灯光标字符（在输入末尾闪烁，提示此处可输入）
const BREATH_CURSOR: &str = "▍";
/// 呼吸灯周期（ms）：2 秒完成一次"亮→暗→亮"
const BREATH_PERIOD_MS: u128 = 2000;

/// 呼吸灯颜色：在品牌主色（亮）与极弱色（暗）之间插值。
/// phase 为 0..=1000 的亮度曲线值（0 = 暗，1000 = 亮）。
fn breathing_color(phase: u128) -> ratatui::style::Color {
    let t = phase.min(1000) as f32 / 1000.0;
    // 正弦缓入缓出：呼吸更自然（亮→暗对称）
    let ease = (t * std::f32::consts::PI).sin();
    fn lerp(a: u8, b: u8, t: f32) -> u8 {
        (a as f32 + (b as f32 - a as f32) * t).round() as u8
    }
    if let (
        ratatui::style::Color::Rgb(dr, dg, db),
        ratatui::style::Color::Rgb(br, bg, bb),
    ) = (theme::faint(), theme::PRIMARY)
    {
        ratatui::style::Color::Rgb(
            lerp(dr, br, ease),
            lerp(dg, bg, ease),
            lerp(db, bb, ease),
        )
    } else {
        theme::PRIMARY
    }
}

/// 主渲染入口，每帧调用一次。
pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // 首次启动向导（或 /hiairy 重开）全屏接管，不渲染聊天/输入/快捷键
    if app.wizard.active {
        wizard::render(f, area, &app.wizard);
        return;
    }

    let mut constraints = vec![
        Constraint::Length(1), // Brand bar
        Constraint::Min(3),    // Main content（自适应）
    ];
    // 有未决议的工具审批时，在输入栏上方插入审批提示条（Claude Code 风格）
    let has_approval = !app.approvals.is_empty();
    if has_approval {
        constraints.push(Constraint::Length(2));
    }
    constraints.push(Constraint::Length(2)); // Input bar（内容行 + 底部细分隔线）
    constraints.push(Constraint::Length(1)); // Shortcuts
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_brand_bar(f, main_layout[0], app);
    match app.active_panel {
        ActivePanel::Chat => panels::chat::render(f, main_layout[1], app),
        ActivePanel::Help => panels::help::render(f, main_layout[1], app),
        ActivePanel::Config => panels::config::render(f, main_layout[1], app),
        ActivePanel::Logs => panels::logs::render(f, main_layout[1], app),
        ActivePanel::Memory => panels::memory::render(f, main_layout[1], app),
        ActivePanel::Plugins => panels::plugins::render(f, main_layout[1], app),
        ActivePanel::Board => panels::board::render(f, main_layout[1], app),
        ActivePanel::Events => panels::events::render(f, main_layout[1], app),
    }
    let mut idx = 2;
    if has_approval {
        render_approval_banner(f, main_layout[idx], app);
        idx += 1;
    }
    render_input_bar(f, main_layout[idx], app);
    render_shortcuts(f, main_layout[idx + 1], app);
}

/// 工具审批提示条：pending 审批时在输入栏上方高亮显示（Claude Code 风格
/// permission prompt），给出工具、主体与 [a]/[A]/[n] 快捷决议提示。
fn render_approval_banner(f: &mut Frame, area: Rect, app: &App) {
    let Some(a) = app.approvals.first() else {
        return;
    };
    // 参数预览单行截断：避免长 JSON 撑满横幅
    let mut params_preview = a.params.clone();
    if params_preview.chars().count() > area.width.saturating_sub(6) as usize {
        params_preview = params_preview
            .chars()
            .take(area.width.saturating_sub(9) as usize)
            .collect::<String>()
            + "…";
    }
    let line1 = Line::from(vec![
        Span::styled(
            " ⚠ ",
            Style::default().fg(theme::WARNING).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "工具审批请求",
            Style::default().fg(theme::WARNING).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ·  主体: {}", if a.agent_id.is_empty() { "unknown" } else { &a.agent_id }),
            Style::default().fg(theme::dim()),
        ),
        Span::styled(
            format!("  ·  工具: {}", a.tool),
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
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
            Style::default().fg(theme::faint()),
        ),
    ]);
    let line2 = Line::from(vec![
        Span::raw(format!("   {params_preview}")),
        Span::raw("  "),
        Span::styled(
            "[a] 允许本次",
            Style::default().fg(theme::SUCCESS).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "[A] 始终允许",
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "[n] 拒绝",
            Style::default().fg(theme::DANGER).add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(Text::from(vec![line1, line2])).style(Style::default().bg(theme::surface())),
        area,
    );
}

/// 品牌栏（左）：品牌 + 连接状态灯 + 宿主机当前时间；右：模型 + 用量 + 阶段徽章。
fn render_brand_bar(f: &mut Frame, area: Rect, app: &App) {
    // 窄屏（<52 列）只保留左侧，右侧徽章整体收起，避免挤占
    let wide = area.width >= 52;

    // 左侧：品牌 + 状态灯 + 时间（宿主机时区，每帧刷新）
    let (light, label, color) = if app.connected {
        ("●", "ONLINE", theme::SUCCESS)
    } else if app.loading {
        ("◐", "WAITING", theme::WARNING)
    } else {
        ("●", "OFFLINE", theme::DANGER)
    };
    let now = chrono::Local::now().format("%H:%M:%S").to_string();

    let left = Line::from(vec![
        Span::styled(" ◈ ", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
        Span::styled(
            "AirymaxRT",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}", app.gateway_version.as_deref().unwrap_or("unknown")),
            Style::default().fg(theme::faint()),
        ),
        Span::styled("    ", Style::default()),
        Span::styled(light, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label}"), Style::default().fg(color)),
        // 宿主机本地时间（与 ONLINE 状态并列，随帧刷新）
        Span::styled(
            format!("  {}", now),
            Style::default().fg(theme::dim()),
        ),
    ]);

    if !wide {
        f.render_widget(
            Paragraph::new(left).style(Style::default().bg(theme::surface())),
            area,
        );
        return;
    }

    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    f.render_widget(
        Paragraph::new(left).style(Style::default().bg(theme::surface())),
        split[0],
    );

    // 右侧：模型 · Token · 成本 · 技能 · 阶段徽章（右对齐，轻量分隔）
    let (badge, badge_color) = phase_badge(app.flow_phase);
    let badge_text = match app.flow_phase {
        FlowPhase::GccpRound(_) => {
            format!(" 任务事实确认 {}/5 ", app.gccp.answered())
        }
        _ => badge,
    };
    let model_text = if app.model.is_empty() {
        "默认模型".to_string()
    } else {
        app.model.clone()
    };
    let sep = Span::styled("·", Style::default().fg(theme::faint()));
    // 任务集人工控制状态徽章（运行中/已暂停/已中止）
    let control_badge: Option<(String, ratatui::style::Color)> = match app.task_control {
        crate::gccp::TaskControl::Running => None,
        crate::gccp::TaskControl::Paused => Some((
            " ⏸ 已暂停 ".to_string(),
            theme::PRIMARY,
        )),
        crate::gccp::TaskControl::Aborted => Some((
            " ✕ 已中止 ".to_string(),
            theme::DANGER,
        )),
    };
    let mut right_spans = vec![
        Span::styled(
            format!("{}  ", model_text),
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled(
            format!(" {} ", app.tokens),
            Style::default().fg(theme::dim()),
        ),
        Span::styled("tok", Style::default().fg(theme::faint())),
        Span::styled("  ", Style::default()),
        sep.clone(),
        Span::styled(
            format!(" ${:.4}  ", app.cost),
            Style::default().fg(theme::dim()),
        ),
        Span::styled(
            format!("技能 {}", app.skills.len()),
            Style::default().fg(theme::faint()),
        ),
        Span::styled("  ", Style::default()),
    ];
    if let Some((label, color)) = control_badge {
        right_spans.push(Span::styled(
            label,
            Style::default().fg(theme::ON_COLOR).bg(color).add_modifier(Modifier::BOLD),
        ));
        right_spans.push(Span::raw("  "));
    }
    right_spans.push(Span::styled(
        badge_text,
        Style::default().fg(theme::ON_COLOR).bg(badge_color).add_modifier(Modifier::BOLD),
    ));
    right_spans.push(Span::raw("  "));
    let right = Line::from(right_spans);
    f.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::default().bg(theme::surface())),
        split[1],
    );
}

/// 阶段徽章配色（对话 / 任务事实确认 / 任务流程图确认 / 任务集）
fn phase_badge(phase: FlowPhase) -> (String, ratatui::style::Color) {
    match phase {
        FlowPhase::Chat => (" 对话 ".into(), theme::SUCCESS),
        FlowPhase::GccpRound(_) => (" 任务事实确认 ".into(), theme::PRIMARY),
        FlowPhase::GradConfirm => (" 任务流程图确认 ".into(), theme::MAGENTA),
        FlowPhase::Executing => (" 任务集 ".into(), theme::WARNING),
    }
}

/// 输入栏：`❯` 前缀 + 阶段引导/占位提示 + 用户输入 + 呼吸灯光标。
///
/// 交互语义（参考 Claude 对话设计的克制输入框）：
///   - 可输入时：输入末尾有呼吸灯光标（颜色随时间明暗呼吸），提示此处可输入；
///   - 等待回复（loading）时：输入框保持中性安静，不显示 thinking 动画——
///     思考动效仅出现在对话主区（chat.rs），避免输入框与对话区重复提醒。
fn render_input_bar(f: &mut Frame, area: Rect, app: &App) {
    // 等待回复：prefix 保持普通，无呼吸灯、无动画（思考动效在对话主区，输入框安静克制）；
    // 但按阶段给出明确的等待语义，让用户知道"正在做什么"。
    if app.loading {
        let wait_hint = match app.flow_phase {
            FlowPhase::Chat => "…".to_string(),
            FlowPhase::GccpRound(n) => format!("正在思考第 {} 问…", n),
            FlowPhase::GradConfirm => "正在生成任务流程图…".to_string(),
            FlowPhase::Executing => match app.task_control {
                crate::gccp::TaskControl::Paused => "已暂停，Ctrl+Z 恢复…".to_string(),
                crate::gccp::TaskControl::Aborted => "已中止…".to_string(),
                crate::gccp::TaskControl::Running => "正在执行任务集…".to_string(),
            },
        };
        let line = Line::from(vec![
            Span::styled(" ❯ ", Style::default().fg(theme::faint())),
            Span::styled(
                wait_hint,
                Style::default().fg(theme::faint()),
            ),
            Span::styled(
                app.input.clone(),
                Style::default().fg(theme::text()),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line)
                .style(Style::default().bg(theme::surface()))
                .block(
                    Block::default()
                        .style(Style::default().bg(theme::surface()))
                        .borders(ratatui::widgets::Borders::BOTTOM)
                        .border_style(Style::default().fg(theme::border())),
                ),
            area,
        );
        return;
    }

    // 可输入状态：空输入（普通对话）显示占位引导，输入后自动消失
    let (hint, hint_color) = if app.flow_phase == FlowPhase::Chat && app.input.is_empty() {
        ("发送消息，Enter 发送".to_string(), theme::faint())
    } else {
        (app.flow_phase.input_hint(), theme::dim())
    };

    // 呼吸灯光标仅在对话面板显示（其他面板输入栏保持安静克制）；
    // 光标渲染在输入文本的实际位置（readline 风格：←→ 移动后光标可见）
    let focused = app.active_panel == ActivePanel::Chat;
    let mut spans = vec![
        Span::styled(" ❯ ", Style::default().fg(theme::PRIMARY)),
        Span::styled(hint, Style::default().fg(hint_color)),
    ];
    if focused {
        // 呼吸灯光标：相位按会话时间推进（100ms 一帧 → 20 帧/周期），颜色呼吸明暗
        let phase = app.session_start.elapsed().as_millis() % BREATH_PERIOD_MS;
        let phase_val = if phase < BREATH_PERIOD_MS / 2 {
            phase
        } else {
            BREATH_PERIOD_MS - phase
        };
        let cursor_color = breathing_color(phase_val * 1000 / (BREATH_PERIOD_MS / 2));
        // 光标处断开文本：前半 + 呼吸灯光标 + 后半
        let cursor_byte = app.cursor.min(app.input.len());
        if app.input.is_char_boundary(cursor_byte) {
            let (before, after) = app.input.split_at(cursor_byte);
            spans.push(Span::styled(
                before.to_string(),
                Style::default().fg(theme::text()),
            ));
            // 呼吸灯光标：替代终端方块光标，提示输入焦点
            spans.push(Span::styled(
                BREATH_CURSOR,
                Style::default().fg(cursor_color),
            ));
            spans.push(Span::styled(
                after.to_string(),
                Style::default().fg(theme::text()),
            ));
        } else {
            spans.push(Span::styled(
                app.input.clone(),
                Style::default().fg(theme::text()),
            ));
            spans.push(Span::styled(
                BREATH_CURSOR,
                Style::default().fg(cursor_color),
            ));
        }
    } else {
        spans.push(Span::styled(
            app.input.clone(),
            Style::default().fg(theme::text()),
        ));
    }

    let line = Line::from(spans);

    // 输入栏：surface 背景 + 底部细分隔线（与主内容区分，视觉更清晰）
    f.render_widget(
        Paragraph::new(line)
            .style(Style::default().bg(theme::surface()))
            .block(
                Block::default()
                    .style(Style::default().bg(theme::surface()))
                    .borders(ratatui::widgets::Borders::BOTTOM)
                    .border_style(Style::default().fg(theme::border())),
            ),
        area,
    );
}

/// 快捷键（居中、紧凑；窄屏自动收起文字标签）。
fn render_shortcuts(f: &mut Frame, area: Rect, app: &App) {
    let items = [
        ("F1", "帮助", ActivePanel::Help),
        ("F2", "配置", ActivePanel::Config),
        ("F3", "日志", ActivePanel::Logs),
        ("F4", "记忆", ActivePanel::Memory),
        ("F5", "插件", ActivePanel::Plugins),
        ("F6", "看板", ActivePanel::Board),
        ("F7", "事件流", ActivePanel::Events),
        ("F8", "CLI", ActivePanel::Chat),
    ];

    // 窄屏只显示键位胶囊，隐藏文字标签，避免溢出
    let compact = area.width < 72;

    let mut spans: Vec<Span> = Vec::with_capacity(items.len() * 2 + 2);
    for (key, label, panel) in items {
        let active = app.active_panel == panel;
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(if active { theme::ON_COLOR } else { theme::PRIMARY })
                .bg(if active { theme::PRIMARY } else { theme::surface_active() })
                .add_modifier(Modifier::BOLD),
        ));
        if !compact {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(if active { theme::PRIMARY } else { theme::dim() }),
            ));
        }
    }
    if area.width >= 52 {
        spans.push(Span::styled(
            "   Ctrl+Z 暂停  Ctrl+X 中止  ",
            Style::default().fg(theme::faint()),
        ));
    }
    if area.width >= 40 {
        spans.push(Span::styled(
            "  Ctrl+C 退出 ",
            Style::default().fg(theme::faint()),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Center)
            .style(Style::default().bg(theme::bg())),
        area,
    );
}
