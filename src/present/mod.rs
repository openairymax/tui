// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 主渲染：统一布局与主题（极简自适应）。
//
// 外壳分区由 engine::layout 裁决，本单元只负责把布局计划落到帧上，
// 各视觉区域按职责拆入子单元：状态条、输入栏、浮层、快捷键。

// +-- 主内容区随终端大小弹性伸缩 --+
// | 12,345 tok · $0.0080 · [对话]     ← 状态条（hero）
// | Main Content（自适应填充）
// | ❯ 输入提示…                        ← 输入栏（input）
// | F1 帮助 … Ctrl+C 退出              ← 快捷键（footer）

mod footer;
mod hero;
mod input;
mod overlay;

use footer::render_shortcuts;
use hero::{render_hero, render_tab_bar};
use input::render_input_bar;
use overlay::render_approval_banner;
use overlay::render_degraded;
use overlay::render_leader_hint;
use overlay::render_toasts;

use crate::app::{ActivePanel, App};
use crate::budget;
use crate::engine::layout;
use crate::panels;
use crate::style;
use crate::wizard;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // 0.1.8：整屏先铺主题背景。此前各面板仅给自身控件设 bg，chat 正文区
    // 等未显式着色的区域透出**终端默认背景**——终端配色与主题不符时
    // （浅色终端跑深色主题、或反之）出现社区反馈的"页面色彩反差"：
    // 同一屏幕一半主题色一半终端色。Block 的 style 覆盖区域内全部
    // 单元格，一次铺底后所有子控件按需在自身层级覆盖。
    f.render_widget(
        Block::default().style(Style::default().bg(style::bg())),
        area,
    );

    // 首次启动向导（或 /hiairy 重开）全屏接管，不渲染聊天/输入/快捷键
    if app.wizard.active {
        wizard::render(f, area, &app.wizard);
        return;
    }

    // 最小可用尺寸守卫（0.1.19 §6.9 #1）：窄于最小可用尺寸时整屏渲染友好
    // 提示，而非让分区在窄屏上崩坏；46 列同时是窄布局断点。
    if area.width < budget::MIN_TERM_COLS || area.height < budget::MIN_TERM_ROWS {
        render_too_small(f, area);
        return;
    }

    // 顶部：1 行系统状态条（实时状态，紧凑不占屏；品牌/能力墙在
    // chat 空态欢迎区，避免与系统头重叠——2026-08-23 重设计）
    //
    // 外壳分区由 engine::layout 裁决（0.1.19 U13）：此处只提供
    // 「哪些浮层在场」的事实，槽位下标由计划给出，不再手算 `main_at`。
    // 依赖降级横幅（0.1.19 U12，TUI 方案 §6.12 #1）：硬依赖缺位时全程常显，
    // 附一行可复制的修复命令；恢复后由状态机撤销并进入冷却期。
    let banner_on = app.banner.visible();
    // 有未决议的工具审批时，在输入栏上方插入审批提示条（Claude Code 风格）
    let has_approval = !app.approvals.is_empty();
    // 瞬态提示（0.1.19 L7 U7，TUI 方案 §6.3 #4）：每条占一行，队列上限
    // 三条；无提示时不占版面，输入栏位置与历史布局一致。
    let toast_n = app.toasts().len();
    // leader 提示浮层（0.1.19 L7 U8，TUI 方案 §6.4 #7）：前缀键静置 500ms
    // 后浮出，单行、不遮挡对话。
    let leader_hint = app.leader_hint();
    // IME 拼音态：输入框扩为两行（输入行 + 候选区）再叠底部细分隔线
    let plan = layout::plan(
        banner_on,
        has_approval,
        toast_n,
        leader_hint,
        layout::input_h(app.ime_visible()),
    );
    let constraints: Vec<Constraint> = plan
        .rows
        .iter()
        .map(|row| match row.h {
            layout::RowH::Fixed(n) => Constraint::Length(n),
            layout::RowH::Min(n) => Constraint::Min(n),
        })
        .collect();
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let hero_area = main_layout[plan.slots.status];
    let hero_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(hero_area);
    render_hero(f, hero_split[0], app);
    // 状态条下方主色细分隔线：1 行内容与主内容区视觉分层（Claude Code
    // 顶部细线风格）。bar 底色 + 主色 BOTTOM 边框即细线，不占内容。
    f.render_widget(
        Block::default()
            .style(Style::default().bg(style::bar()))
            .borders(ratatui::widgets::Borders::BOTTOM)
            .border_style(Style::default().fg(style::primary())),
        hero_split[1],
    );
    if let Some(at) = plan.slots.banner {
        render_degraded(f, main_layout[at], app);
    }
    let main_area = main_layout[plan.slots.main];
    // 多会话 tab 栏（Chat 面板顶部一行；仅存在多会话时渲染）
    if app.active_panel == ActivePanel::Chat && app.tab_count() > 1 {
        let tab_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(main_area);
        render_tab_bar(f, tab_layout[0], app);
        panels::chat::render(f, tab_layout[1], app);
    } else {
        match app.active_panel {
            ActivePanel::Chat => panels::chat::render(f, main_area, app),
            ActivePanel::Focus => panels::chat::render_focus(f, main_area, app),
            ActivePanel::Help => panels::help::render(f, main_area, app),
            ActivePanel::Config => panels::config::render(f, main_area, app),
            ActivePanel::Logs => panels::logs::render(f, main_area, app),
            ActivePanel::Memory => panels::memory::render(f, main_area, app),
            ActivePanel::Plugins => panels::plugins::render(f, main_area, app),
            ActivePanel::Board => panels::board::render(f, main_area, app),
            ActivePanel::Events => panels::events::render(f, main_area, app),
            ActivePanel::Think => panels::think::render(f, main_area, app),
            ActivePanel::Health => panels::health::render(f, main_area, app),
            ActivePanel::Evolution => panels::evolution::render(f, main_area, app),
        }
    }
    // 回放骨架行（0.1.19 §6.1 #2）：会话恢复期间主区不空白，右下角常显
    // 进度；计数由 100ms 回放泵节流推进。
    if app.replay_left() > 0 {
        render_replay(f, main_area, app);
    }
    if let Some(at) = plan.slots.approval {
        render_approval_banner(f, main_layout[at], app);
    }
    if let Some(at) = plan.slots.toasts {
        render_toasts(f, main_layout[at], app);
    }
    if let Some(at) = plan.slots.leader {
        render_leader_hint(f, main_layout[at]);
    }
    render_input_bar(f, main_layout[plan.slots.input], app);
    render_shortcuts(f, main_layout[plan.slots.shortcuts], app);
}

/// 终端过小提示（TUI 方案 §6.9 #1）：布局不崩坏，给出可执行的恢复路径。
/// 三行垂直居中：标题、当前尺寸与最小尺寸、恢复动作。
fn render_too_small(f: &mut Frame, area: Rect) {
    let rows = [
        Line::from(Span::styled(
            "终端尺寸不足",
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "当前 {} × {}，至少 {} 列 × {} 行",
                area.width,
                area.height,
                budget::MIN_TERM_COLS,
                budget::MIN_TERM_ROWS
            ),
            Style::default().fg(style::text()),
        )),
        Line::from(Span::styled(
            "请放大窗口后继续 · Ctrl+C 退出",
            Style::default().fg(style::faint()),
        )),
    ];
    let mid = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(Text::from(rows.to_vec()))
            .alignment(Alignment::Center)
            .style(Style::default().bg(style::bg())),
        mid[1],
    );
}

/// 回放进度行（TUI 方案 §6.1 #2）：主区右下角常显剩余条数。
fn render_replay(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let text = format!(" 回放中 · {} 条 ", app.replay_left());
    let w = (UnicodeWidthStr::width(text.as_str()) as u16).min(area.width);
    let row = Rect {
        x: area.right().saturating_sub(w),
        y: area.bottom().saturating_sub(1),
        width: w,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Right)
            .style(Style::default().fg(style::faint()).bg(style::surface())),
        row,
    );
}
