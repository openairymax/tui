// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后端日志面板（0.1.20 批次五）：daemon 日志尾部只读视图。
//
// 数据源为 AIRY_LOG_DIR（或 $AIRY_HOME/data/agentrt/logs）下各 *.log 的
// 尾部快照；r122 批次四在 executor_pool 与 llm_d 落的等待事件（queued /
// waiting / picked up / CHAIN-SEAT / CHAIN-QUEUE）整行高亮——「很多功能
// 卡住」类反馈由此定位卡在排队、等座还是执行。诚实语义：目录缺失或暂无
// 日志文件时如实说明路径，不伪造内容。

use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::style;

/// 等待事件判定：r122 批次四等待弧线关键字（executor_pool 排队 / 心跳 /
/// 取件 + llm_d 等座）。命中行整行高亮，让「卡在哪」一眼可见。
pub fn is_wait_event(msg: &str) -> bool {
    msg.contains("executor_pool")
        || msg.contains("queued")
        || msg.contains("picked up")
        || msg.contains("waiting ")
        || msg.contains("CHAIN-SEAT")
        || msg.contains("CHAIN-QUEUE")
}

/// 单条日志行渲染：时间取日内时分秒段，级别 / 来源着色，等待事件整行
/// 用警告色强调；ERROR / WARN 分别加重与着色。
fn entry_line(e: &crate::app::LogEntry) -> Line<'static> {
    let wait = is_wait_event(&e.message);
    let base = if wait {
        ratatui::style::Style::default().fg(style::warning())
    } else {
        match e.level.as_str() {
            "ERROR" => ratatui::style::Style::default().fg(style::danger()),
            "WARN" | "WARNING" => ratatui::style::Style::default().fg(style::warning()),
            _ => ratatui::style::Style::default().fg(style::text()),
        }
    };
    let ts = if e.timestamp.len() >= 23 {
        e.timestamp[11..].to_string()
    } else {
        e.timestamp.clone()
    };
    let mut spans = vec![Span::styled(
        if ts.is_empty() {
            String::new()
        } else {
            format!("{ts} ")
        },
        style::faint(),
    )];
    if !e.level.is_empty() {
        spans.push(Span::styled(
            format!("{:<5} ", e.level),
            if e.level == "ERROR" {
                ratatui::style::Style::default().fg(style::danger())
            } else if e.level.starts_with("WARN") {
                ratatui::style::Style::default().fg(style::warning())
            } else {
                ratatui::style::Style::default().fg(style::dim())
            },
        ));
    }
    if let Some(d) = &e.daemon {
        if !d.is_empty() {
            spans.push(Span::styled(
                format!("{d} "),
                ratatui::style::Style::default().fg(style::primary()),
            ));
        }
    }
    spans.push(Span::styled(e.message.clone(), base));
    Line::from(spans)
}

/// 面板正文构建（渲染与测试共用）：空态如实给出目录路径；有数据时
/// 首行为总览（行数 / 等待事件计数 / 目录），其后为日志行。
fn body_lines(dir: &std::path::Path, entries: &[crate::app::LogEntry]) -> Vec<Line<'static>> {
    let mut body: Vec<Line<'static>> = Vec::new();
    if entries.is_empty() {
        body.push(Line::from(Span::styled(
            format!("  日志目录为空或暂无日志文件：{}", dir.display()),
            style::dim(),
        )));
        body.push(Line::from(Span::styled(
            "  daemon 启动并产生日志后本页自动刷新（Esc 返回对话）。",
            style::faint(),
        )));
        return body;
    }
    let waits = entries.iter().filter(|e| is_wait_event(&e.message)).count();
    body.push(Line::from(vec![
        Span::styled("● ", style::success()),
        Span::styled(
            format!(
                "{} 行 · 等待事件 {} 条 · 目录 {}",
                entries.len(),
                waits,
                dir.display()
            ),
            style::dim(),
        ),
    ]));
    body.extend(entries.iter().map(entry_line));
    body
}

/// /dlogs 后端日志面板渲染（ActivePanel::BackendLog）。
pub fn render(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let dir = crate::app::log_dir();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(style::border()))
        .title(Span::styled(
            " 后端日志 ",
            ratatui::style::Style::default().fg(style::primary()),
        ));
    let body = body_lines(&dir, &app.backend_logs);
    // 滚动：backend_logs_scroll 为正文首行下标，上界按可见行数收敛。
    let visible = area.height.saturating_sub(2) as usize;
    let total = body.len().saturating_sub(1);
    let max_scroll = total.saturating_sub(visible);
    let scroll = app.backend_logs_scroll.min(max_scroll);
    f.render_widget(
        Paragraph::new(Text::from(body))
            .block(block)
            .scroll((scroll as u16, 0)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 等待事件关键字全线命中：executor_pool 三态 + llm_d 等座两态。
    #[test]
    fn wait_event_keywords_match_batch4_arcs() {
        assert!(is_wait_event("'web_search' queued (queue=1 workers=2)"));
        assert!(is_wait_event(
            "'web_search' waiting 5000 / 90000 ms: running"
        ));
        assert!(is_wait_event(
            "'web_search' picked up after 5200 ms queue wait"
        ));
        assert!(is_wait_event("CHAIN-SEAT-WAIT chain seat busy, waiting"));
        assert!(is_wait_event("CHAIN-SEAT acquired in 120 ms"));
        assert!(is_wait_event("CHAIN-QUEUE timeout after 1500 ms"));
        assert!(!is_wait_event("spawn cupolas worker ok"));
        assert!(!is_wait_event("request completed in 32 ms"));
    }

    /// 行渲染字段齐备且等待行保留原文（解析失败不丢行）。
    #[test]
    fn entry_line_renders_fields() {
        let e = crate::app::LogEntry {
            timestamp: "2026-10-30T12:54:12.340".into(),
            level: "INFO".into(),
            message: "'t' queued (queue=1 workers=2)".into(),
            daemon: Some("tool_d".into()),
        };
        let l = entry_line(&e);
        let text = l
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(text.contains("12:54:12.340"));
        assert!(text.contains("INFO"));
        assert!(text.contains("tool_d"));
        assert!(text.contains("queued"));
    }

    /// 空态如实给出目录路径与语义说明，不伪造内容。
    #[test]
    fn body_lines_empty_state_mentions_real_dir() {
        let lines = body_lines(std::path::Path::new("/tmp/agentrt-logs"), &[]);
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
        assert!(text.contains("/tmp/agentrt-logs"));
        assert!(text.contains("日志目录为空"));
    }

    /// 总览行给出行数与等待事件计数，日志行原样保留消息。
    #[test]
    fn body_lines_overview_counts_wait_events() {
        let entries = vec![
            crate::app::LogEntry {
                timestamp: "2026-10-30T12:54:12.340".into(),
                level: "INFO".into(),
                message: "'t' queued (queue=1 workers=2)".into(),
                daemon: Some("tool_d".into()),
            },
            crate::app::LogEntry {
                timestamp: "2026-10-30T12:54:13.001".into(),
                level: "INFO".into(),
                message: "request completed".into(),
                daemon: Some("llm_d".into()),
            },
        ];
        let lines = body_lines(std::path::Path::new("/logs"), &entries);
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
        assert!(text.contains("2 行"));
        assert!(text.contains("等待事件 1 条"));
        assert!(text.contains("queued (queue=1 workers=2)"));
    }
}
