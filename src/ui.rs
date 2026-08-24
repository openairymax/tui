// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 主渲染：统一布局与主题（极简自适应）。

// Layout structure（主内容区随终端大小弹性伸缩）:
// ┌─ ◈ AirymaxRT v0.1.2 ───────────── ● ONLINE  22:45:33 ─┐
// │ 对话 2 · 技能 3 · 记忆 128     模型 deepseek-v4-flash  │
// │ 12,345 tok · $0.0080 · [对话]                         │
// └──────────────────────────────────────────────────────┘  ← 英雄区
// ├─ Main Content（自适应填充）─────────────────────────────┤
// ├─ Input Bar ───────────────────────────────────────────┤
// │ ❯ 输入提示…                                            │
// └─ Shortcuts（居中）─────────────────────────────────────┘
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

/// 输入光标字符（黑白两色交替闪动，替代终端方块光标）
const BREATH_CURSOR: &str = "▍";

/// 2.2.1.4 光标颜色：黑白两色交替（Word 风格）。Word 默认光标闪烁
/// 周期 ≈530ms（Windows 控制台 530ms 全周期），半周期取 265ms。
/// 深色终端白色可见、浅色终端黑色可见——任一时刻都保证与背景高对比；
/// 对当前背景不可见的那一态即"灭"，形成经典块状闪烁，视觉引导科学。
const BLINK_PERIOD_MS: u128 = 530;

/// 2.2.1.4 光标颜色：黑白两色交替（Word 风格，半周期 ≈265ms）。
fn blink_cursor_color(elapsed_ms: u128) -> ratatui::style::Color {
    if (elapsed_ms % BLINK_PERIOD_MS) < BLINK_PERIOD_MS / 2 {
        ratatui::style::Color::White
    } else {
        ratatui::style::Color::Black
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

    // 顶部：1 行系统状态条（实时状态，紧凑不占屏；品牌/能力墙在
    // chat 空态欢迎区，避免与系统头重叠——2026-08-23 重设计）
    let mut constraints = vec![
        Constraint::Length(2), // System status bar + 主色细分隔线
        Constraint::Min(3),    // Main content（自适应）
    ];
    // 有未决议的工具审批时，在输入栏上方插入审批提示条（Claude Code 风格）
    let has_approval = !app.approvals.is_empty();
    if has_approval {
        constraints.push(Constraint::Length(2));
    }
    // IME 拼音态：输入框扩为两行（输入行 + 候选区）再叠底部细分隔线
    let ime_bar = app.ime_visible();
    constraints.push(Constraint::Length(if ime_bar { 3 } else { 2 })); // Input bar
    constraints.push(Constraint::Length(1)); // Shortcuts
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let hero_area = main_layout[0];
    let hero_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(hero_area);
    render_hero(f, hero_split[0], app);
    // 状态条下方主色细分隔线：1 行内容与主内容区视觉分层（Claude Code
    // 顶部细线风格）。surface 背景 + 主色 BOTTOM 边框即细线，不占内容。
    f.render_widget(
        Block::default()
            .style(Style::default().bg(theme::surface()))
            .borders(ratatui::widgets::Borders::BOTTOM)
            .border_style(Style::default().fg(theme::PRIMARY)),
        hero_split[1],
    );
    // 多会话 tab 栏（Chat 面板顶部一行；仅存在多会话时渲染）
    if app.active_panel == ActivePanel::Chat && app.tab_count() > 1 {
        let tab_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(main_layout[1]);
        render_tab_bar(f, tab_layout[0], app);
        panels::chat::render(f, tab_layout[1], app);
    } else {
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
    }
    let mut idx = 2;
    if has_approval {
        render_approval_banner(f, main_layout[idx], app);
        idx += 1;
    }
    render_input_bar(f, main_layout[idx], app);
    render_shortcuts(f, main_layout[idx + 1], app);
}

/// IME 候选区（输入框第二行）：`[中] 拼音` + 当前页候选。
///
/// 2.2.3 重新设计（2026-08-23）：F10 激活后输入框恒为两行（第一行
/// 输入 + 第二行候选区），本函数渲染第二行。拼音缓冲为空时显示
/// `[中] 拼音输入中…` 占位，让激活瞬间即有明确视觉反馈；缓冲非空
/// 时显示拼音高亮 + 微信式分页候选（当前页 9 个，页内高亮蓝底，
/// 多页时尾部页码指示 ‹1/2›；空格/Enter 上屏高亮候选，数字选字，
/// ,/. 或 PgUp/PgDn 翻页，←/→ 移动高亮）。与 C 侧 CLI 的
/// tui_ime_draw_cands 语义一致。
fn render_ime_cands(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(
            "[中]",
            Style::default()
                .fg(theme::ON_COLOR)
                .bg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if app.ime_buf.is_empty() {
        spans.push(Span::styled(
            "拼音输入中…（a-z 拼音 · 1-9 选字 · 空格/Enter 上屏 · ,/. 翻页 · Esc 取消）",
            Style::default().fg(theme::faint()),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {} ", app.ime_buf),
            Style::default()
                .fg(theme::ON_COLOR)
                .bg(theme::PRIMARY)
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
                        .fg(theme::ON_COLOR)
                        .bg(theme::PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(tag, Style::default().fg(theme::dim())));
            }
        }
        // 页码指示（多页时显示 ‹cur/total›）
        if app.ime_pages > 1 {
            spans.push(Span::styled(
                format!("‹{}/{}›", app.ime_page + 1, app.ime_pages),
                Style::default().fg(theme::faint()),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(theme::surface())),
        area,
    );
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

/// 会话 tab 栏（2026-08-21 多会话）：`1 标题 | 2 标题 …` 胶囊。
/// 当前 tab 高亮（主色底 + 反色字），其余弱化；Alt+1..9 切换 · Ctrl+T 新建。
fn render_tab_bar(f: &mut Frame, area: Rect, app: &App) {
    let n = app.tab_count();
    let current = app.current_tab_index_pub();
    let mut spans: Vec<Span> = vec![Span::styled(" ", Style::default())];
    for i in 0..n {
        let active = i == current;
        let mut title = app.tab_title(i);
        let cnt = title.chars().count();
        if cnt > 16 {
            title = title.chars().take(16).collect();
            title.push('…');
        }
        spans.push(Span::styled(
            format!(" {} {} ", i + 1, title),
            Style::default()
                .fg(if active { theme::ON_COLOR } else { theme::dim() })
                .bg(if active { theme::PRIMARY } else { theme::surface_active() })
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::surface())),
        area,
    );
}

/// 系统状态条（顶部 1 行，2026-08-23 重设计）。
///
/// 2.2.1.5.1 曾把英雄区做成 4 行晶蓝 box，与 chat 空态欢迎墙
/// （append_welcome 的 ╭─╮ 大框）内容重叠（品牌/状态/模型/记忆重复），
/// 视觉上"两个头叠罗汉"。全新设计：顶部收敛为**单行紧凑状态条**
/// （实时运行状态：连接灯 + 时间 + 模型 + token + 成本 +
/// 阶段徽章），品牌/能力/硬件等静态展示移交 chat 空态欢迎墙独占，
/// 两区域职责分离、无重叠。窄屏自动收起冗余段。
///
/// 0.1.3 Claude 风格降噪：去掉会话耗时段，状态条保持安静克制，
/// 信息密度让位于对话主体。
fn render_hero(f: &mut Frame, area: Rect, app: &App) {
    // 状态灯
    let (light, label, color) = if app.connected {
        ("●", "ONLINE", theme::SUCCESS)
    } else if app.loading {
        ("◐", "WAITING", theme::WARNING)
    } else {
        ("●", "OFFLINE", theme::DANGER)
    };
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    let ver = app
        .gateway_version
        .clone()
        .unwrap_or_else(|| env!("AIRY_RT_VERSION").to_string());

    let mut spans: Vec<Span> = vec![
        Span::styled(" ◈ ", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
        Span::styled("AirymaxRT", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" v{ver}"), Style::default().fg(theme::faint())),
        Span::styled("   ", Style::default()),
        Span::styled(light, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label}"), Style::default().fg(color)),
        Span::styled(format!("  {now}"), Style::default().fg(theme::dim())),
    ];
    // 窄屏（<72 列）只保留左段（品牌 + 连接 + 时间），右侧运行数据收起
    if area.width >= 72 {
        let model_text = if app.model.is_empty() {
            "默认模型".to_string()
        } else {
            app.model.clone()
        };
        spans.push(Span::styled("   ", Style::default()));
        spans.push(Span::styled(
            model_text,
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("  {} tok · ${:.4}", app.tokens, app.cost),
            Style::default().fg(theme::dim()),
        ));
        let (badge, badge_color) = phase_badge(app.flow_phase);
        let badge_text = match app.flow_phase {
            FlowPhase::GccpRound(_) => format!(" 任务事实确认 {}/5 ", app.gccp.answered()),
            _ => badge,
        };
        spans.push(Span::styled(
            "  ",
            Style::default(),
        ));
        spans.push(Span::styled(
            badge_text,
            Style::default()
                .fg(theme::ON_COLOR)
                .bg(badge_color)
                .add_modifier(Modifier::BOLD),
        ));
        if let crate::gccp::TaskControl::Paused = app.task_control {
            spans.push(Span::styled(
                "  ⏸ 已暂停 ",
                Style::default()
                    .fg(theme::ON_COLOR)
                    .bg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if let crate::gccp::TaskControl::Aborted = app.task_control {
            spans.push(Span::styled(
                "  ✕ 已中止 ",
                Style::default()
                    .fg(theme::ON_COLOR)
                    .bg(theme::DANGER)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::surface())),
        area,
    );
}

/// 阶段徽章配色（对话 / 任务事实确认 / 目标澄清 / 任务流程图确认 / 任务集）
fn phase_badge(phase: FlowPhase) -> (String, ratatui::style::Color) {
    match phase {
        FlowPhase::Chat => (" 对话 ".into(), theme::SUCCESS),
        FlowPhase::GccpRound(_) => (" 任务事实确认 ".into(), theme::PRIMARY),
        FlowPhase::GccpClarify => (" 目标澄清 ".into(), theme::PRIMARY),
        FlowPhase::GradConfirm => (" 任务流程图确认 ".into(), theme::MAGENTA),
        FlowPhase::Executing => (" 任务集 ".into(), theme::WARNING),
    }
}

/// 输入栏（2026-08-23 重设计）：IME 激活时输入框变两行——
/// 第一行 `❯` 前缀 + 输入文本（含 IME 模式指示），第二行候选区
/// （render_ime_cands），末行底部细分隔线；非激活时仅 输入行 + 隔线。
///
/// 交互语义（参考 Claude 对话设计的克制输入框）：
///   - 可输入时：输入末尾有呼吸灯光标（颜色随时间明暗呼吸），提示此处可输入；
///   - 等待回复（loading）时：输入框保持中性安静，不显示 thinking 动画——
///     思考动效仅出现在对话主区（chat.rs），避免输入框与对话区重复提醒。
fn render_input_bar(f: &mut Frame, area: Rect, app: &App) {
    let ime_bar = app.ime_visible();
    // 内容行 +（IME 候选区）+ 底部细分隔线
    let mut parts = vec![Constraint::Length(1)];
    if ime_bar {
        parts.push(Constraint::Length(1));
    }
    parts.push(Constraint::Length(1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(parts)
        .split(area);

    render_input_line(f, layout[0], app);
    let mut idx = 1;
    if ime_bar {
        render_ime_cands(f, layout[idx], app);
        idx += 1;
    }
    render_input_sep(f, layout[idx]);
}

/// 输入行（第一行）：`❯` 前缀 + 阶段引导/占位提示 + 输入文本 + 光标。
fn render_input_line(f: &mut Frame, area: Rect, app: &App) {
    // 等待回复：prefix 保持普通，无呼吸灯、无动画（思考动效在对话主区，输入框安静克制）；
    // 但按阶段给出明确的等待语义，让用户知道"正在做什么"。
    if app.loading {
        let wait_hint = match app.flow_phase {
            FlowPhase::Chat => "…".to_string(),
            FlowPhase::GccpRound(n) => format!("正在思考第 {} 问…", n),
            FlowPhase::GccpClarify => "正在汇总目标澄清答案…".to_string(),
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
            Paragraph::new(line).style(Style::default().bg(theme::surface())),
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

    // IME 模式指示：激活时 [中] 高亮（晶蓝底），未激活 [英] 灰显
    let mut spans = vec![Span::styled(" ❯ ", Style::default().fg(theme::PRIMARY))];
    if app.ime_engine.is_some() {
        if app.ime_active {
            spans.push(Span::styled(
                "[中] ",
                Style::default()
                    .fg(theme::ON_COLOR)
                    .bg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                "[英] ",
                Style::default().fg(theme::faint()),
            ));
        }
    }
    spans.push(Span::styled(hint, Style::default().fg(hint_color)));

    // 呼吸灯光标仅在对话面板显示（其他面板输入栏保持安静克制）；
    // 光标渲染在输入文本的实际位置（readline 风格：←→ 移动后光标可见）
    let focused = app.active_panel == ActivePanel::Chat;
    if focused {
        // 黑白交替光标：相位按会话时间推进（500ms 切换一次，≈ Word 频率）
        let cursor_color = blink_cursor_color(app.session_start.elapsed().as_millis());
        // 光标处断开文本：前半 + 黑白交替光标 + 后半
        let cursor_byte = app.cursor.min(app.input.len());
        if app.input.is_char_boundary(cursor_byte) {
            let (before, after) = app.input.split_at(cursor_byte);
            spans.push(Span::styled(
                before.to_string(),
                Style::default().fg(theme::text()),
            ));
            // 黑白交替光标：替代终端方块光标，提示输入焦点
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
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::surface())),
        area,
    );
}

/// 输入栏底部细分隔线（surface 背景 + BOTTOM 边框，与主内容区分）。
fn render_input_sep(f: &mut Frame, area: Rect) {
    f.render_widget(
        Block::default()
            .style(Style::default().bg(theme::surface()))
            .borders(ratatui::widgets::Borders::BOTTOM)
            .border_style(Style::default().fg(theme::border())),
        area,
    );
}

/// 快捷键（单行低对比，Claude Code 风格克制；0.1.3 去掉彩色胶囊背景，
/// 仅当前面板键以主色高亮，其余灰显；窄屏自动收起文字标签）。
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

    // 窄屏只显示键位，隐藏文字标签，避免溢出
    let compact = area.width < 72;

    let mut spans: Vec<Span> = Vec::with_capacity(items.len() * 2 + 2);
    for (key, label, panel) in items {
        let active = app.active_panel == panel;
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(if active { theme::PRIMARY } else { theme::dim() }),
        ));
        if !compact {
            spans.push(Span::styled(
                format!("{label} "),
                Style::default().fg(if active { theme::PRIMARY } else { theme::faint() }),
            ));
        }
    }
    /* 0.1.3 美化：快捷键分组——导航组（F1-F8）与任务控制/退出组之间
     * 用主色竖线分隔，避免"一排灰字"失去节奏；窄屏自动收起分组。 */
    if area.width >= 80 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            "┃",
            Style::default().fg(theme::PRIMARY),
        ));
        spans.push(Span::styled(
            "  输入  Enter 发送 · Alt+Enter 换行 · ↑/↓ 历史",
            Style::default().fg(theme::faint()),
        ));
    }
    if area.width >= 52 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            "┃",
            Style::default().fg(theme::PRIMARY),
        ));
        spans.push(Span::styled(
            "  控制  Ctrl+Z 暂停 · Ctrl+X 中止",
            Style::default().fg(theme::faint()),
        ));
    }
    if area.width >= 40 {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            "┃",
            Style::default().fg(theme::PRIMARY),
        ));
        spans.push(Span::styled(
            "  Ctrl+C 退出",
            Style::default().fg(theme::faint()),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Left)
            .style(Style::default().bg(theme::surface())),
        area,
    );
}
