// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Chat panel rendering: 对话主面板（随终端大小自适应）。

use ratatui::{
    layout::Rect,
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, MessageRole};
use crate::gccp::FlowPhase;
use crate::theme;

/// thinking... 动效帧（11 个字符逐一循环：t → th → … → thinking...，0.1s 一帧）。
const THINKING: [&str; 11] = [
    "t", "th", "thi", "thin", "think", "thinki", "thinkin", "thinking", "thinking.", "thinking..",
    "thinking...",
];

/// 渲染对话主面板。
///
/// 参考 Claude Code 的简洁：无边框、内容直接铺开（靠留白分层），
/// 行级滚动 + 右侧滚动条。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let viewport = area.height as usize;

    let mut lines: Vec<Line> = Vec::new();

    // 任务流阶段引导区（GCCP/GRAD/执行中展示进度与提示）
    render_flow_header(&mut lines, app, width);

    // 消息列表（全量拼行）
    if app.messages.is_empty() && app.flow_phase == FlowPhase::Chat {
        append_welcome(&mut lines, width, viewport);
    } else {
        for msg in app.messages.iter() {
            append_message(&mut lines, msg, width);
        }
    }

    // 流式输出：SSE 增量块已累计在 streaming_text，实时渲染为「Airymax」气泡
    // （Claude 风格逐字上屏；完成后由 apply_stream_result 落为正式消息）
    if app.loading && !app.streaming_text.is_empty() {
        let streaming_msg = crate::app::ChatMessage {
            role: crate::app::MessageRole::Agent,
            content: app.streaming_text.clone(),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        };
        append_message(&mut lines, &streaming_msg, width);
    }

    if app.loading {
        // 思考动效：thinking... 11 字符逐一循环（与 ui.rs 同一时钟，0.05s 一帧；
        // 苹果轻量字重风格：无粗体无斜体，极浅色，优雅低调）
        let frame = (app.session_start.elapsed().as_millis() / 50) as usize % THINKING.len();
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {}", THINKING[frame]),
                Style::default().fg(theme::faint()),
            ),
        ]));
    }

    // 行级滚动：scroll_offset 语义为「距底部（最新）向上滚的行数」。
    // Paragraph::scroll 的 offset 语义是「距顶部滚过的行数」，需反转：
    //   scroll_offset=0 → offset=max_offset → 视口落在底部（跟随最新消息）
    //   scroll_offset=max_offset → offset=0 → 视口落在顶部（最早消息）
    let total = lines.len();
    let max_offset = total.saturating_sub(viewport);
    let from_top = max_offset
        .saturating_sub((app.scroll_offset as usize).min(max_offset));

    // 视口裁剪后渲染（行已手动裁剪到宽度内，无需 wrap）
    let visible: Vec<Line> = lines.iter().skip(from_top).take(viewport).cloned().collect();
    f.render_widget(Paragraph::new(Text::from(visible)), area);

    // 滚动条：内容超出视口且有对话时显示；窄屏（<44 列）隐藏避免挤占
    if max_offset > 0 && !app.messages.is_empty() && area.width >= 44 {
        let sb_area = Rect {
            x: area.right().saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut state = ScrollbarState::new(total)
            .position(from_top)
            .viewport_content_length(viewport);
        let sb = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("█")
            .thumb_style(Style::default().fg(theme::PRIMARY))
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(theme::faint()));
        f.render_stateful_widget(sb, sb_area, &mut state);
    }
}

/// 任务流阶段引导：GCCP 五问进度 / GRAD 确认（含 DAG 依赖图）/ 执行中（含进度与中止提示）。
fn render_flow_header(lines: &mut Vec<Line>, app: &App, width: usize) {
    match app.flow_phase {
        FlowPhase::Chat => {}
        FlowPhase::GccpRound(_) => {
            let answered = app.gccp.answered();
            let dots: String = (1..=5)
                .map(|i| {
                    let done = match i {
                        1 => !app.gccp.a1.trim().is_empty(),
                        2 => !app.gccp.a2.trim().is_empty(),
                        3 => !app.gccp.a3.trim().is_empty(),
                        4 => !app.gccp.a4.trim().is_empty(),
                        _ => !app.gccp.a5.trim().is_empty(),
                    };
                    if done { "●" } else { "○" }
                })
                .collect();
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "任务事实确认",
                    // 苹果轻量字重风格：无粗体，晶蓝
                    Style::default().fg(theme::PRIMARY),
                ),
                Span::styled(" ", Style::default()),
                Span::styled(dots, Style::default().fg(theme::PRIMARY)),
                Span::styled(format!("  {answered}/5"), Style::default().fg(theme::faint())),
            ]));
            lines.push(Line::raw(""));
        }
        FlowPhase::GradConfirm => {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "任务流程图确认",
                    Style::default().fg(theme::MAGENTA).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  输入「确认」开始执行，或输入修改意见", Style::default().fg(theme::dim())),
            ]));
            lines.push(Line::raw(""));
            // 结构化 DAG 依赖图（LLM 生成 [DAG] 块 → 解析成功时渲染）
            if let Some(dag) = &app.gccp.dag {
                let dag_width = width.saturating_sub(6).max(40);
                for line in crate::gccp::render_dag_lines(dag, dag_width) {
                    lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(line, Style::default().fg(theme::ACCENT)),
                    ]));
                }
                lines.push(Line::raw(""));
            }
        }
        FlowPhase::Executing => {
            // 任务集执行中：阶段徽章 + DAG 节点数（若有）+ 人工控制状态提示
            let node_hint = app
                .gccp
                .dag
                .as_ref()
                .filter(|d| !d.is_empty())
                .map(|d| format!(" · {} 个步骤", d.node_count()))
                .unwrap_or_default();
            let control_hint = match app.task_control {
                crate::gccp::TaskControl::Paused => "  ⏸ 已暂停（Ctrl+Z 恢复）",
                crate::gccp::TaskControl::Aborted => "  ✕ 已中止",
                crate::gccp::TaskControl::Running => "  Ctrl+X 中止 · Ctrl+Z 暂停",
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("任务集执行中{node_hint}"),
                    // 苹果轻量字重风格：无粗体，暖黄
                    Style::default().fg(theme::WARNING),
                ),
                Span::styled(control_hint, Style::default().fg(theme::dim())),
            ]));

            // DAG 节点状态持续渲染（P2-C 过程可视化）：
            // 执行期间 ◐ 运行中，完成 ●，失败 ✕，未达 ○
            if let Some(dag) = &app.gccp.dag {
                if !dag.is_empty() {
                    lines.push(Line::raw(""));
                    for (i, node) in dag.nodes.iter().enumerate() {
                        let state = app
                            .gccp
                            .node_states
                            .get(i)
                            .copied()
                            .unwrap_or(crate::gccp::NodeState::Pending);
                        let (mark, color) = match state {
                            crate::gccp::NodeState::Pending => ("○", theme::faint()),
                            crate::gccp::NodeState::Running => ("◐", theme::WARNING),
                            crate::gccp::NodeState::Done => ("●", theme::SUCCESS),
                            crate::gccp::NodeState::Failed => ("✕", theme::DANGER),
                        };
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(mark, Style::default().fg(color)),
                            Span::styled(" ", Style::default()),
                            Span::styled(node.label.clone(), Style::default().fg(theme::text())),
                        ]));
                    }
                }
            }
            lines.push(Line::raw(""));
        }
    }
}

/// 追加一条消息：角色名 + 时间戳头部 + markdown 渲染内容（P2-A/P2-B 修复）。
///
/// 统一中文角色名（你/Airymax/系统/工具/结果），时间戳展示在头部行；
/// 内容固定缩进 4 列（解决此前随角色名长度参差的问题）；用户消息以
/// 气泡背景色块区分（Claude 式左右分层），其余角色左对齐流式。
fn append_message(lines: &mut Vec<Line>, msg: &crate::app::ChatMessage, width: usize) {
    let (name, color) = match msg.role {
        MessageRole::User => ("你", theme::SUCCESS),
        MessageRole::Agent => ("Airymax", theme::PRIMARY),
        MessageRole::System => ("系统", theme::WARNING),
        MessageRole::ToolCall => ("工具", theme::MAGENTA),
        MessageRole::ToolResult => ("结果", theme::ACCENT),
    };

    // 头部行：角色名（加粗语义色）+ 时间戳（极弱色）
    lines.push(Line::from(vec![
        Span::styled(
            name.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", msg.timestamp),
            Style::default().fg(theme::faint()),
        ),
    ]));

    // 内容统一固定缩进 4 列（Claude 式克制排版；此前随角色名长度参差）
    const INDENT: usize = 4;
    let content_width = width.saturating_sub(INDENT).max(8);

    // 工具调用/结果为次要信息：轻量 dim 色
    let base = match msg.role {
        MessageRole::ToolCall | MessageRole::ToolResult => {
            Style::default().fg(theme::dim())
        }
        _ => Style::default().fg(theme::text()),
    };

    if msg.content.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{:width$}（空）", "", width = INDENT),
            Style::default().fg(theme::faint()),
        )));
    } else {
        let mut rendered = crate::markdown::render(&msg.content, INDENT, width, base);
        // 用户消息：气泡背景色块（Claude 式左右分层）；Airymax 回复左对齐流式
        if msg.role == MessageRole::User {
            for line in rendered.iter_mut() {
                let bg = theme::surface_active();
                let spans: Vec<Span> = line
                    .spans
                    .iter()
                    .map(|s| s.clone().bg(bg))
                    .collect();
                *line = Line::from(spans);
            }
        }
        lines.extend(rendered);
    }

    // 消息间留白（替代粗分隔线，视觉更轻）
    lines.push(Line::raw(""));
}

/// 欢迎页（无消息时，垂直+水平居中，随终端大小自适应）。
fn append_welcome(lines: &mut Vec<Line>, width: usize, height: usize) {
    let w = [
        "◈  AirymaxRT",
        "Agent 运行时 · 对话即生产力",
        "输入消息，Enter 发送",
        "F1 查看帮助",
    ];

    // 垂直居中：前置空行
    let pad = (height as usize).saturating_sub(w.len() + 2) / 2;
    for _ in 0..pad {
        lines.push(Line::raw(""));
    }

    for (i, s) in w.iter().enumerate() {
        let styled = if i == 0 {
            Span::styled(
                s.to_string(),
                Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
            )
        } else if i == 1 {
            Span::styled(s.to_string(), Style::default().fg(theme::ACCENT))
        } else {
            Span::styled(s.to_string(), Style::default().fg(theme::dim()))
        };
        // 水平居中：按实际显示宽度计算左侧补白
        let disp = s.width();
        let lead = if disp >= width { 1 } else { (width - disp) / 2 };
        lines.push(Line::from(vec![
            Span::styled(" ".repeat(lead), Style::default()),
            styled,
        ]));
    }
}

#[cfg(test)]
mod tests {
    // wrap_line 实现已迁至 markdown 模块（P2-A 渲染器共用）
    use crate::markdown::wrap_line;

    #[test]
    fn wrap_line_short_text_single_line() {
        assert_eq!(wrap_line("你好", 10), vec!["你好"]);
        assert_eq!(wrap_line("abc", 10), vec!["abc"]);
    }

    #[test]
    fn wrap_line_splits_by_display_width() {
        // 中文按 2 列计：宽度 5 时 "你好世" = 6 列超宽 → 拆行
        assert_eq!(wrap_line("你好世界", 5), vec!["你好", "世界"]);
        // 半角按 1 列计
        assert_eq!(wrap_line("abcdef", 3), vec!["abc", "def"]);
    }

    #[test]
    fn wrap_line_empty_and_narrow() {
        assert!(wrap_line("", 10).is_empty());
        // 极窄宽度兜底：整行返回，不产生空片段
        assert_eq!(wrap_line("abc", 1), vec!["abc"]);
    }

    #[test]
    fn wrap_line_mixed_widths() {
        // "a你好b" = 1+2+2+1 = 6 列，宽度 4 → "a你"（3 列）+ "好b"（3 列）
        assert_eq!(wrap_line("a你好b", 4), vec!["a你", "好b"]);
    }
}
