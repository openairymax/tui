// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 工具轨迹面板（0.1.20 批次五）：只读展示最近一轮工具调用的全量明细
// （动作名 / 成败 / 参数 / 结果首行）。对话区仍只展示动作短语，克制
// 展示策略不变；本面板是显式请求下的按需查看入口（/trace 或 F 键）。
//
// 诚实语义：本轮尚无工具调用时显示空态说明，不合成记录。

use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::budget::TRACE_CLIP_CHARS;
use crate::client::ToolTrace;
use crate::style;

/// 单值行截断：换行折叠为空格，超 TRACE_CLIP_CHARS 加省略号（UTF-8 安全）。
fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TRACE_CLIP_CHARS {
        return flat;
    }
    let head: String = flat.chars().take(TRACE_CLIP_CHARS).collect();
    format!("{head}…")
}

/// 成败判定：ok 为 Some(>0) 视为成功，其余（含缺失）按失败如实标注。
fn verdict(ok: Option<i64>) -> (&'static str, ratatui::style::Color) {
    match ok {
        Some(n) if n > 0 => ("成功", style::success()),
        _ => ("失败", style::warning()),
    }
}

/// 全量轨迹展开为正文行（对话区克制策略之外的面板全量视图）。
fn trace_lines(traces: &[ToolTrace]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, t) in traces.iter().enumerate() {
        let (tag, color) = verdict(t.ok);
        lines.push(Line::from(vec![
            Span::styled(format!("  #{} ", i + 1), style::faint()),
            Span::styled(t.tool.clone(), style::accent()),
            Span::raw(" · "),
            Span::styled(tag.to_string(), ratatui::style::Style::default().fg(color)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("     参数 ", style::faint()),
            Span::styled(one_line(&t.arguments), style::text()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("     结果 ", style::faint()),
            Span::styled(one_line(&t.result), style::dim()),
        ]));
    }
    lines
}

/// /trace 工具轨迹面板渲染（ActivePanel::Trace）。
pub fn render(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(style::border()))
        .title(Span::styled(
            " 工具轨迹 ",
            ratatui::style::Style::default().fg(style::primary()),
        ));
    let mut body: Vec<Line<'static>> = Vec::new();
    if app.last_tool_trace.is_empty() {
        body.push(Line::from(Span::styled(
            "  本轮会话尚无工具调用。",
            style::dim(),
        )));
        body.push(Line::from(Span::styled(
            "  完成一次会触发工具的对话后，再执行 /trace 查看明细。",
            style::faint(),
        )));
    } else {
        let n = app.last_tool_trace.len();
        body.push(Line::from(vec![
            Span::styled("● ", style::success()),
            Span::styled(
                format!("最近一轮 {n} 次调用（回合落定后刷新，Esc 返回对话）"),
                style::dim(),
            ),
        ]));
        body.extend(trace_lines(&app.last_tool_trace));
    }
    body.push(Line::from(vec![crate::panels::nav::observe_hint()]));
    // 滚动：trace_scroll 为正文首行下标，上界按可见行数收敛。
    let visible = area.height.saturating_sub(2) as usize;
    let total = body.len().saturating_sub(1);
    let max_scroll = total.saturating_sub(visible);
    let scroll = app.trace_scroll.min(max_scroll);
    let text = Text::from(body);
    f.render_widget(
        Paragraph::new(text).block(block).scroll((scroll as u16, 0)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(tool: &str, args: &str, result: &str, ok: Option<i64>) -> ToolTrace {
        ToolTrace {
            tool: tool.to_string(),
            arguments: args.to_string(),
            result: result.to_string(),
            ok,
        }
    }

    /// 成败映射：Some(>0) 成功，其余如实按失败标注。
    #[test]
    fn verdict_maps_ok_field() {
        assert_eq!(verdict(Some(1)).0, "成功");
        assert_eq!(verdict(Some(0)).0, "失败");
        assert_eq!(verdict(None).0, "失败");
    }

    /// 单值行折叠换行并按预算截断（UTF-8 安全，不出半字符）。
    #[test]
    fn one_line_folds_and_clips() {
        assert_eq!(one_line("a\nb\tc"), "a b c");
        let long = "字".repeat(TRACE_CLIP_CHARS + 10);
        let clipped = one_line(&long);
        assert!(clipped.chars().count() <= TRACE_CLIP_CHARS + 1);
        assert!(clipped.ends_with('…'));
    }

    /// 全量视图展开参数与结果（对话区克制策略之外，面板如实呈现）。
    #[test]
    fn trace_lines_show_arguments_and_result() {
        let lines = trace_lines(&[trace(
            "web_search",
            "{\"query\":\"上海天气\"}",
            "上海今日多云 26°C",
            Some(1),
        )]);
        let text = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("web_search"));
        assert!(text.contains("{\"query\":\"上海天气\"}"));
        assert!(text.contains("上海今日多云"));
    }
}
