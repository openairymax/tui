// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 输入栏：输入行、模式徽标、只读面板提示、IME 候选行、底部隔线。

use super::overlay::render_ime_cands;

use crate::app::{ActivePanel, App};
use crate::budget;
use crate::gccp::FlowPhase;
use crate::style;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// 输入光标字符（黑白两色交替闪动，替代终端方块光标）
const BREATH_CURSOR: &str = "\u{258d}";

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

/// 输入栏（2026-08-23 重设计）：IME 激活时输入框变两行——
/// 第一行 `❯` 前缀 + 输入文本（含 IME 模式指示），第二行候选区
/// （render_ime_cands），末行底部细分隔线；非激活时仅 输入行 + 隔线。
///
/// 交互语义（参考 Claude 对话设计的克制输入框）：
///   - 可输入时：输入末尾有呼吸灯光标（颜色随时间明暗呼吸），提示此处可输入；
///   - 等待回复（loading）时：输入框保持中性安静，不显示 thinking 动画——
///     思考动效仅出现在对话主区（chat.rs），避免输入框与对话区重复提醒。
pub(super) fn render_input_bar(f: &mut Frame, area: Rect, app: &App) {
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

    // 输入区状态常显（§6.4 #8）：右栏固定宽徽标，左栏保留输入语义；
    // 宽度不足时徽标整体让位，绝不让输入可用宽度低于 20 列。
    let mut line_area = layout[0];
    let mut badge_at = None;
    if let Some(text) = input_status(app, area.width) {
        let w = UnicodeWidthStr::width(text.as_str()) as u16;
        if area.width > w + 20 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0), Constraint::Length(w)])
                .split(layout[0]);
            line_area = cols[0];
            badge_at = Some((cols[1], text));
        }
    }
    render_input_line(f, line_area, app);
    if let Some((at, text)) = badge_at.as_ref() {
        render_input_status(f, *at, app, text);
    }
    let mut idx = 1;
    if ime_bar {
        render_ime_cands(f, layout[idx], app);
        idx += 1;
    }
    render_input_sep(f, layout[idx]);
}

/// 输入区常显状态文本（TUI 方案 §6.4 #8）：模式徽标 + 上下文占用 +
/// token 计数，按宽度分档降级——最窄只留模式，过窄整体让位给输入。
fn input_status(app: &App, width: u16) -> Option<String> {
    let mode = if app.task_mode { "plan" } else { "normal" };
    let used = app.messages.len();
    let text = if width >= 84 {
        format!(
            "{} · 上下文 {}/{} · {} tok ",
            mode,
            used,
            budget::MAX_CHAT_MESSAGES,
            app.tokens
        )
    } else if width >= 56 {
        format!("{} · 上下文 {}/{} ", mode, used, budget::MAX_CHAT_MESSAGES)
    } else if width >= 40 {
        format!("{} ", mode)
    } else {
        return None;
    };
    Some(text)
}

/// 输入区状态徽标：plan 态用警示色常显，normal 态灰显克制。
fn render_input_status(f: &mut Frame, area: Rect, app: &App, text: &str) {
    let fg = if app.task_mode {
        style::warning()
    } else {
        style::faint()
    };
    f.render_widget(
        Paragraph::new(text.to_string())
            .alignment(Alignment::Right)
            .style(Style::default().fg(fg).bg(style::surface())),
        area,
    );
}

/// 只读展示面板（无输入语义）在输入栏占位处给出的状态提示：输入栏仍显示，
/// 但 Enter/普通字符不会汇入或发出对话共享输入（按键守卫在 main.rs）。
fn panel_read_hint(p: ActivePanel) -> Option<&'static str> {
    match p {
        ActivePanel::Help => Some("帮助只读 · Esc 返回对话"),
        ActivePanel::Config => Some("配置只读 · Esc 返回对话"),
        ActivePanel::Logs => Some("日志只读 · ↑↓ 滚动 · Esc 返回对话"),
        ActivePanel::Memory => Some("记忆只读 · PgUp/PgDn 翻页 · Esc 返回对话"),
        ActivePanel::Plugins => Some("插件只读 · Esc 返回对话"),
        ActivePanel::Think => Some("思考链只读 · ↑↓ 滚动 · PgUp/PgDn 翻页 · Esc 返回对话"),
        ActivePanel::Health => Some("健康只读 · ↑↓ 滚动 · PgUp/PgDn 翻页 · Esc 返回对话"),
        ActivePanel::Focus => Some("焦点视图只读 · ↑↓ 滚动 · PgUp/PgDn 翻页 · Esc 返回对话"),
        _ => None,
    }
}

/// 输入行（第一行）：`❯` 前缀 + 阶段引导/占位提示 + 输入文本 + 光标。
fn render_input_line(f: &mut Frame, area: Rect, app: &App) {
    // 等待回复（loading）：输入框保持中性安静（无呼吸灯、无等待文案）——
    // 忙碌中由另外两层各司其职：顶部 hero 阶段徽章（含 Paused/Aborted）
    // 标明"正在做什么"，对话主区 spinner 表达进行中动效；输入行不再
    // 重复 wait_hint 文案（2026-09 状态栏三层去重）。
    if app.loading {
        let line = Line::from(vec![
            Span::styled(" ❯ ", Style::default().fg(style::faint())),
            Span::styled(app.input.clone(), Style::default().fg(style::text())),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(style::surface())),
            area,
        );
        return;
    }

    // 可输入状态：空输入（普通对话）显示占位引导，输入后自动消失。
    // 只读面板（F1-F5 帮助/配置/日志/记忆/插件）在空输入时给出面板态提示：
    // 输入栏仍可见但与对话解耦——Enter/字符不会汇入或发出共享输入
    let (hint, hint_color) = if app.flow_phase == FlowPhase::Chat && app.input.is_empty() {
        match panel_read_hint(app.active_panel) {
            Some(h) => (h.to_string(), style::dim()),
            None => ("发送消息，Enter 发送".to_string(), style::faint()),
        }
    } else {
        (app.flow_phase.input_hint(), style::dim())
    };

    // IME 模式指示（§A9「输入区常驻 IME 态」）：引擎可用即常驻三态——
    //   [英]        未激活（灰显，尾随 Ctrl+1 唤起提示）
    //   [中]        激活·缓冲空
    //   [中 拼音]   激活·组合中（候选态，拼音原文可见）
    let mut spans = vec![Span::styled(" ❯ ", Style::default().fg(style::primary()))];
    if app.ime_engine.is_some() {
        if app.ime_active {
            let tag = if app.ime_buf.is_empty() {
                "[中] ".to_string()
            } else {
                format!("[中 {}] ", &app.ime_buf[..app.ime_buf.len().min(12)])
            };
            spans.push(Span::styled(
                tag,
                Style::default()
                    .fg(style::on_color())
                    .bg(style::primary())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled("[英] ", Style::default().fg(style::faint())));
        }
    }
    spans.push(Span::styled(hint, Style::default().fg(hint_color)));
    if app.ime_engine.is_some() && !app.ime_active {
        spans.push(Span::styled(
            " · Ctrl+1 输入法",
            Style::default().fg(style::faint()),
        ));
    }

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
                Style::default().fg(style::text()),
            ));
            // 黑白交替光标：替代终端方块光标，提示输入焦点
            spans.push(Span::styled(
                BREATH_CURSOR,
                Style::default().fg(cursor_color),
            ));
            spans.push(Span::styled(
                after.to_string(),
                Style::default().fg(style::text()),
            ));
        } else {
            spans.push(Span::styled(
                app.input.clone(),
                Style::default().fg(style::text()),
            ));
            spans.push(Span::styled(
                BREATH_CURSOR,
                Style::default().fg(cursor_color),
            ));
        }
    } else {
        spans.push(Span::styled(
            app.input.clone(),
            Style::default().fg(style::text()),
        ));
    }

    let line = Line::from(spans);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(style::surface())),
        area,
    );
}

/// 输入栏底部细分隔线（surface 背景 + BOTTOM 边框，与主内容区分）。
fn render_input_sep(f: &mut Frame, area: Rect) {
    f.render_widget(
        Block::default()
            .style(Style::default().bg(style::surface()))
            .borders(ratatui::widgets::Borders::BOTTOM)
            .border_style(Style::default().fg(style::border())),
        area,
    );
}
