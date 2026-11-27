// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Task board panel (hall.board): work_hall persisted execution instances
// + live agent roster from agent_d, refreshed on the Beat::Hall beat by
// app.poll_hall().

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::client::{HallBoard, HallBoardEntry};
use crate::engine::comp::table::{Column, Table};
use crate::engine::comp::{Act, Component, Input, Size, Width};
use crate::engine::grid;
use crate::engine::hit::Area;
use crate::style;

/// 看板单屏物化行数上限：渲染只取前 MAX_BOARD_ROWS 条，选中行与 Enter
/// 详情也必须收敛到该窗口内，否则光标越界后高亮消失、Enter 打开屏外
/// 条目（与 app/panel.rs 光标移动共用同一上限）。
/// 0.1.19 L7：界值收敛到 budget.rs 单一来源，此处仅保留调用路径。
pub(crate) use crate::budget::MAX_BOARD_ROWS;

/// 状态图标（2.3.10 细化：执行/等待/调度/完成/失败/取消各自独立图示）
fn state_icon(state: &str) -> &'static str {
    match state {
        "completed" => "✓",
        "running" => "▶",
        "pending" => "○",
        "scheduled" => "◷",
        "failed" => "✗",
        "canceled" => "⊘",
        _ => "•",
    }
}

/// 状态语义色（与 C 版 cli_panel_state_color 对齐）
fn state_color(state: &str) -> Color {
    match state {
        "completed" => style::success(),
        "failed" | "canceled" => style::danger(),
        "running" | "pending" | "scheduled" => style::warning(),
        _ => style::dim(),
    }
}

/// 8 格块状进度条（▰ 已填充 / ▱ 未填充；单行面板保持紧凑，视觉更清晰）
fn mini_bar(prog: f64) -> String {
    let p = prog.clamp(0.0, 1.0);
    let filled = (p * 8.0).round() as usize;
    let mut s = String::new();
    for i in 0..8 {
        s.push(if i < filled { '▰' } else { '▱' });
    }
    s
}

/// 状态分组优先级（2.3.10 排序编排：执行中 → 等待/调度 → 完成 → 失败/取消，
/// 组内保持最新在前，视觉上"进行中的事最醒目"）。
/// pub(crate)：app.rs 的 board_selected_exec 必须与渲染用同一排序，
/// 否则高亮行与 Enter 详情错位（混合状态时渲染按 rank 分组、详情按
/// 时间倒序取索引 → 选中/回放的不是同一条任务）。
pub(crate) fn state_rank(state: &str) -> u8 {
    match state {
        "running" => 0,
        "pending" | "scheduled" => 1,
        "completed" => 2,
        "failed" | "canceled" => 3,
        _ => 4,
    }
}

fn state_of(e: &HallBoardEntry) -> &str {
    if e.state.is_empty() {
        "unknown"
    } else {
        &e.state
    }
}

fn pct(e: &HallBoardEntry) -> u64 {
    (e.progress * 100.0).round() as u64
}

/// 看板列声明（X3-07a）：原有 24/12 列两处写死槽宽由 table 的 Min/Max
/// 约束取代，宽度按可用空间求解，窄屏下不再右溢（IS-34）。
pub(crate) fn columns() -> Vec<Column<HallBoardEntry>> {
    vec![
        Column::new(
            Width::Min(12),
            Alignment::Left,
            |e| {
                (
                    format!("{} {}", state_icon(state_of(e)), e.workflow_name),
                    Style::default().fg(style::text()),
                )
            },
            |e| e.workflow_name.to_lowercase(),
        ),
        Column::new(
            Width::Max(12),
            Alignment::Left,
            |e| {
                (
                    state_of(e).to_string(),
                    Style::default().fg(state_color(state_of(e))),
                )
            },
            // 键只含分组 rank：等值键由稳定排序保持「组内最新在前」
            |e| format!("{:02}", state_rank(state_of(e))),
        ),
        Column::new(
            Width::Max(14),
            Alignment::Right,
            |e| {
                (
                    format!("{} {:>3}%", mini_bar(e.progress), pct(e)),
                    Style::default().fg(style::accent()),
                )
            },
            |e| format!("{:03}", pct(e)),
        ),
        Column::new(
            // 末列吸收余量（X3-07a）：右对齐，余量体现为左侧留白，
            // 与「列宽之和恰好铺满」的旧观感一致，但不再右溢。
            Width::Flex(1),
            Alignment::Right,
            |e| {
                (
                    format!("#{}", e.task_id),
                    Style::default().fg(style::faint()),
                )
            },
            |e| format!("{:020}", e.task_id),
        ),
    ]
}

/// 状态中文标签（头部过滤提示用）
fn state_cn(state: &str) -> &'static str {
    match state {
        "completed" => "完成",
        "running" => "执行中",
        "pending" => "待处理",
        "scheduled" => "已调度",
        "failed" => "失败",
        "canceled" => "已取消",
        _ => "未知",
    }
}

/// 头部摘要行：实例/Agent 计数、过滤状态与在线状态。
fn filter_line(app: &App, entry_count: usize, agent_count: usize) -> Line<'static> {
    let filter_tip = if app.board_filter.is_empty() {
        "全部".to_string()
    } else {
        format!(
            "{} ({} 条)",
            state_cn(&app.board_filter),
            app.board_visible_count()
        )
    };
    Line::from(vec![
        Span::styled(
            format!("  执行实例 {} · 在线 Agent {}  ", entry_count, agent_count),
            Style::default().fg(style::dim()),
        ),
        Span::styled("过滤: ", Style::default().fg(style::faint())),
        Span::styled(
            filter_tip,
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.connected {
                "  ● ONLINE"
            } else {
                "  ● OFFLINE"
            },
            Style::default()
                .fg(if app.connected {
                    style::success()
                } else {
                    style::danger()
                })
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// 2.3.12 状态概览：一眼看清项目运行状态（执行中/待处理/完成/失败计数）。
fn overview_line(board: &HallBoard) -> Line<'static> {
    let mut n_run = 0usize;
    let mut n_wait = 0usize;
    let mut n_done = 0usize;
    let mut n_bad = 0usize;
    for e in &board.entries {
        match e.state.as_str() {
            "running" => n_run += 1,
            "pending" | "scheduled" => n_wait += 1,
            "completed" => n_done += 1,
            "failed" | "canceled" => n_bad += 1,
            _ => {}
        }
    }
    Line::from(vec![
        Span::styled("  概览 ", Style::default().fg(style::faint())),
        Span::styled(
            format!("▶ 执行中 {}", n_run),
            Style::default().fg(style::warning()),
        ),
        Span::styled("   ", Style::default().fg(style::faint())),
        Span::styled(
            format!("○ 待处理 {}", n_wait),
            Style::default().fg(style::cyan()),
        ),
        Span::styled("   ", Style::default().fg(style::faint())),
        Span::styled(
            format!("✓ 完成 {}", n_done),
            Style::default().fg(style::success()),
        ),
        Span::styled("   ", Style::default().fg(style::faint())),
        Span::styled(
            format!("✗ 失败/取消 {}", n_bad),
            Style::default().fg(style::danger()),
        ),
    ])
}

/// 操作提示条（简约：↑↓ 选择 · 数字过滤 · Enter 查看 · Esc 返回）
fn hint_line() -> Line<'static> {
    Line::from(vec![
        Span::styled("  ↑↓ 选择", Style::default().fg(style::faint())),
        Span::styled("  0-6 过滤", Style::default().fg(style::faint())),
        Span::styled("  Enter 决策链", Style::default().fg(style::faint())),
        Span::styled("  Esc 返回", Style::default().fg(style::faint())),
    ])
}

/// Render the task board panel.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(style::border()))
        .title(Span::styled(
            " 任务看板 ",
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));
    let rect = if app.hall_board.is_some() {
        render_loaded(f, area, block, app)
    } else {
        render_placeholder(f, area, block, app);
        None
    };
    // 落格快照随帧更新：鼠标点击按上一帧表格几何折算行号（X3-04c）。
    app.board_rows = rect;
}

/// 可见行快照：先倒序（最新在前）→ 按状态过滤 → 截取单屏窗口。
/// 排序不在此时做，统一交给 table 的稳定排序（X3-10b），保证渲染、
/// 键盘导航与鼠标命中三处行序完全一致。
fn visible(app: &App, board: &HallBoard) -> Vec<HallBoardEntry> {
    let mut rows: Vec<&HallBoardEntry> = board.entries.iter().collect();
    rows.reverse();
    if !app.board_filter.is_empty() {
        rows.retain(|e| e.state == app.board_filter);
    }
    rows.into_iter()
        .take(MAX_BOARD_ROWS)
        .cloned()
        .collect()
}

/// 按组件事件折算光标（X3-04c/X3-12a）：键盘与鼠标走同一 `Table::event`，
/// 上层只负责把结果写回 `App::board_cursor`，不再各自实现越界规则。
fn apply(app: &mut App, table: &Table<HallBoardEntry>, act: Act) -> bool {
    match act {
        Act::Up | Act::Down | Act::Top | Act::Bottom | Act::Select(_) => {
            app.board_cursor = table.selected();
            true
        }
        Act::None => false,
    }
}

/// 键盘导航入口：↑↓/Home/End 由键位表转交，组件侧口径即渲染口径。
pub(crate) fn nav(app: &mut App, key: KeyEvent) -> bool {
    let Some(board) = app.hall_board.as_ref() else {
        return false;
    };
    let rows = visible(app, board);
    if rows.is_empty() {
        return false;
    }
    let n = rows.len();
    let mut table = Table::new(columns());
    table.set_rows(rows);
    table.sort_by(1, true);
    table.select(app.board_cursor % n);
    let act = table.event(Input::Key(key));
    apply(app, &table, act)
}

/// 鼠标点击入口：命中表格落格后按相对行选中；未命中/空表不改变光标。
pub(crate) fn click(app: &mut App, m: MouseEvent) -> bool {
    let Some(rect) = app.board_rows else {
        return false;
    };
    if !rect.contains(m.column, m.row) {
        return false;
    }
    let Some(board) = app.hall_board.as_ref() else {
        return false;
    };
    let rows = visible(app, board);
    if rows.is_empty() {
        return false;
    }
    let n = rows.len();
    let mut table = Table::new(columns());
    table.set_rows(rows);
    table.sort_by(1, true);
    table.select(app.board_cursor % n);
    table.layout(Rect::new(rect.x, rect.y, rect.w, rect.h));
    let act = table.event(Input::Mouse { event: m, row: m.row });
    apply(app, &table, act)
}

/// 无看板数据：区分「拉取失败」与「加载中」，避免无限加载态误导
/// （每 1s 自动重试）。
fn render_placeholder(f: &mut Frame, area: Rect, block: Block<'static>, app: &App) {
    let mut lines = vec![filter_line(app, 0, 0), Line::raw("")];
    if let Some(err) = &app.hall_error {
        // 失败原因一行摘要，边界按列裁决（全角错误文本按字符取必右溢）
        let msg = grid::clip(err, 72);
        lines.push(Line::from(vec![
            Span::styled(
                "  ✗ 看板拉取失败 ",
                Style::default()
                    .fg(style::danger())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg, Style::default().fg(style::text())),
        ]));
        lines.push(Line::from(Span::styled(
            "  gateway 离线或未启动？启动后自动重试 · Esc 返回对话",
            Style::default().fg(style::faint()),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  正在加载看板（gateway hall.board）…",
            Style::default().fg(style::faint()),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(hint_line());
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 有看板数据：摘要行 + table 组件 + agents/提示尾部分段渲染。
///
/// 分段而非整段 Paragraph：表格行必须由 table 组件按列宽求解后直接写入
/// Buffer，否则宽屏/窄屏两种口径会再次分叉（X3-02b、IS-34）。
fn render_loaded(
    f: &mut Frame,
    area: Rect,
    block: Block<'static>,
    app: &App,
) -> Option<Area> {
    let board = app.hall_board.as_ref()?;
    // Block 先落位，内容只写 inner：table 组件不承担边框职责。
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut prefix = vec![
        filter_line(app, board.entries.len(), board.agents.len()),
        overview_line(board),
        Line::raw(""),
    ];

    let total = app.board_visible_count();
    if total == 0 {
        prefix.push(Line::from(Span::styled(
            "  暂无执行实例：运行任务后 work_hall 状态在此展示（$AIRY_HOME/state/work_hall_state.json）。",
            Style::default().fg(style::faint()),
        )));
    }

    // 只物化前 MAX_BOARD_ROWS 条；排序/选中/裁剪全部由 table 承担，
    // sel 在该窗口内取模，任何时刻高亮与 Enter 详情同一条目。
    let shown = visible(app, board);
    let n_rows = shown.len();
    let sel = if n_rows == 0 {
        0
    } else {
        app.board_cursor % n_rows
    };

    let mut table = Table::new(columns());
    table.set_rows(shown);
    table.sort_by(1, true);
    if n_rows > 0 {
        table.select(sel);
    }
    table.set_marker(true);
    table.set_selected_style(Style::default().bg(style::surface_active()));

    let mut tail: Vec<Line> = Vec::new();
    if total > MAX_BOARD_ROWS {
        tail.push(Line::from(Span::styled(
            format!("  … 共 {} 条，仅展示前 {} 条", total, MAX_BOARD_ROWS),
            Style::default().fg(style::faint()),
        )));
    }
    if !board.agents.is_empty() {
        tail.push(Line::raw(""));
        tail.push(Line::from(Span::styled(
            "  在线 Agent：",
            Style::default().fg(style::dim()),
        )));
        for a in board.agents.iter().take(8) {
            tail.push(Line::from(vec![
                Span::styled("   ● ", Style::default().fg(style::success())),
                Span::styled(a.clone(), Style::default().fg(style::text())),
            ]));
        }
    }
    tail.push(Line::raw(""));
    tail.push(hint_line());

    let prefix_h = (prefix.len() as u16).min(inner.height);
    let avail_h = inner.height.saturating_sub(prefix_h);
    let tail_h = (tail.len() as u16).min(avail_h);
    let comp: &mut dyn Component = &mut table;
    let table_h = comp
        .measure(Size {
            w: inner.width,
            h: avail_h.saturating_sub(tail_h),
        })
        .h;

    if prefix_h > 0 {
        let rect = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: prefix_h,
        };
        f.render_widget(Paragraph::new(prefix), rect);
    }
    let landed = if table_h > 0 {
        let rect = Rect {
            x: inner.x,
            y: inner.y.saturating_add(prefix_h),
            width: inner.width,
            height: table_h,
        };
        comp.layout(rect);
        comp.render(f.buffer_mut());
        Some(Area::new(rect.x, rect.y, rect.width, rect.height))
    } else {
        None
    };
    if tail_h > 0 {
        let rect = Rect {
            x: inner.x,
            y: inner
                .y
                .saturating_add(prefix_h)
                .saturating_add(table_h),
            width: inner.width,
            height: tail_h,
        };
        f.render_widget(Paragraph::new(tail), rect);
    }
    landed
}
