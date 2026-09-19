// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 思考链独立视图（0.1.18 B4）。
//
// 模型内部推理原文默认不上屏、不落长期记忆（对话区零渲染），仅在用户显式
// 请求（Alt+E）时于本视图按需取用——隐私收口的同时不丢「可查看」能力。
// 数据源有二：流式中的实时链路（stream_reasoning）与本轮落定后的内存副本
// （last_reasoning）；前者是思考链的唯一查看入口（对话区只留一行字数状态）。
//
// 只读视图：↑↓ 逐行滚动 · PgUp/PgDn 翻页 · Esc 返回对话。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// 思考链数据源：流式中的实时链路优先（本视图是思考链的唯一查看入口），
/// 流式落定后回落到本轮的内存副本。返回 (原文, 是否实时)。
fn pick<'a>(streaming: &'a str, settled: Option<&'a str>) -> (&'a str, bool) {
    if !streaming.is_empty() {
        (streaming, true)
    } else {
        (settled.unwrap_or(""), false)
    }
}

/// 正文行：原文为空时给出隐私口径说明（而非空白屏）；否则 markdown 渲染
/// （弱化色——思考链是次要信息，对话正文才是产物）。
fn body_lines(content: &str, width: usize) -> Vec<Line<'static>> {
    if content.trim().is_empty() {
        return vec![
            Line::from(Span::styled(
                "  本轮暂无思考链。",
                Style::default().fg(theme::dim()),
            )),
            Line::from(Span::styled(
                "  思考链是模型的内部推理原文：默认不上屏、不写长期记忆，",
                Style::default().fg(theme::faint()),
            )),
            Line::from(Span::styled(
                "  仅在用户显式请求（Alt+E）时于本视图按需展示。",
                Style::default().fg(theme::faint()),
            )),
        ];
    }
    crate::markdown::render(content, 2, width, Style::default().fg(theme::dim()))
}

/// 状态行：区分「思考中（实时）」与「最近一轮（落定副本）」。
fn status_line(content: &str, live: bool, secs: f64) -> Line<'static> {
    let chars = content.chars().count();
    if live {
        Line::from(vec![
            Span::styled(
                "  ◐ 思考中… ",
                Style::default()
                    .fg(theme::warning())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{chars} 字 · {secs:.1}s（实时，仍在生成）"),
                Style::default().fg(theme::faint()),
            ),
        ])
    } else if chars > 0 {
        Line::from(vec![
            Span::styled(
                "  ● 最近一轮思考链  ",
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{chars} 字 · 仅存内存（未上屏 · 未落长期记忆）"),
                Style::default().fg(theme::faint()),
            ),
        ])
    } else {
        Line::from(Span::styled(
            "  ○ 无思考链",
            Style::default().fg(theme::dim()),
        ))
    }
}

/// Render the reasoning-chain panel (read-only; opened by Alt+E).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 思考链 ",
            Style::default()
                .fg(theme::primary())
                .add_modifier(Modifier::BOLD),
        ));

    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = (area.height.saturating_sub(2) as usize).max(1);

    let (content, live) = pick(&app.stream_reasoning, app.last_reasoning.as_deref());
    let secs = app
        .stream_reasoning_start
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let body = body_lines(content, inner_w);

    // 顶部状态行与底部提示行固定，仅正文窗口滚动（offset = 正文首行下标）
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    lines.push(status_line(content, live, secs));
    if inner_h >= 3 {
        let body_rows = inner_h - 2;
        let offset = app.think_scroll.min(body.len().saturating_sub(body_rows));
        lines.extend(body.into_iter().skip(offset).take(body_rows));
    }
    while lines.len() < inner_h.saturating_sub(1) {
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled("  ↑↓ 滚动", Style::default().fg(theme::faint())),
        Span::styled("  PgUp/PgDn 翻页", Style::default().fg(theme::faint())),
        Span::styled("  Esc 返回对话", Style::default().fg(theme::faint())),
    ]));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'static>]) -> String {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// 空态（无实时链路、无落定副本）也必须给出明确说明，且不渲染任何原文。
    #[test]
    fn empty_state_explains_without_content() {
        let (content, live) = pick("", None);
        assert!(content.is_empty() && !live);
        let text = text_of(&body_lines(content, 80));
        assert!(text.contains("本轮暂无思考链"), "空态说明缺失: {text}");
        assert!(text.contains("默认不上屏"), "应说明隐私口径: {text}");
    }

    /// 落定副本存在时正文取 last_reasoning（Alt+E 的按需取用能力不丢失）。
    #[test]
    fn settled_copy_is_the_source() {
        let (content, live) = pick("", Some("推理原文"));
        assert_eq!(content, "推理原文");
        assert!(!live, "落定副本不是实时态");
        assert!(text_of(&body_lines(content, 80)).contains("推理原文"));
    }

    /// 流式中：实时链路优先于落定副本（不混显上一轮）。
    #[test]
    fn live_stream_takes_precedence() {
        let (content, live) = pick("本轮实时", Some("上一轮"));
        assert_eq!(content, "本轮实时");
        assert!(live, "流式中应标记实时态");
    }

    /// 空原文的实时态不误报「思考中」状态行之外的原文。
    #[test]
    fn status_line_distinguishes_live_and_settled() {
        assert!(status_line("abc", true, 1.0).to_string().contains("思考中"));
        assert!(status_line("abc", false, 0.0)
            .to_string()
            .contains("最近一轮思考链"));
        assert!(status_line("", false, 0.0).to_string().contains("无思考链"));
    }
}
