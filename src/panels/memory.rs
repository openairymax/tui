// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Memory panel rendering.
//
// 2.2.1.5 任务 4 强化（2026-08-23）：清晰展示记忆与记忆链——
//   · 头部：记忆条数 + 后端名（MemoryRovol 时提示 L1-L4 分层语义）；
//   · 按来源（记忆标签 tags）分组，组内按时间序连接成记忆链；
//   · 每条目：内容摘要 + 时间 + 来源 + 关联链（├/└ + ↳ 承接）+ 思考链标记；
//   · 无数据时给出引导提示（存储路径 + /mem 语义检索）。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::memory::MemoryRecord;
use crate::theme;

/// 记忆目录（与 src/memory.rs memory_dir 对齐；文案用，避免魔法路径）
const MEMORY_DIR_HINT: &str = "$AIRY_HOME/data/agentrt/tui/memory.jsonl";

/// Render the memory statistics panel.
///
/// 实时渲染本地对话记忆库（$AIRY_HOME/data/agentrt/tui/memory.jsonl）：
/// 按来源（标签）分组展示记忆链，无需依赖网关 HTTP 端点。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 记忆库 ",
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        ));

    let total = app.memory.len();
    let backend = app.memory.backend_name();

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  记忆条数  ", Style::default().fg(theme::faint())),
            Span::styled(
                format!("{}", total),
                Style::default().fg(theme::success()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  后端  ", Style::default().fg(theme::faint())),
            Span::styled(
                backend,
                Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    // L1-L4 分层记忆（MemoryRovol 后端启用时提示分层语义）
    if backend == "MemoryRovol" {
        lines.push(Line::from(Span::styled(
            "  L1-L4 分层：L1 工作 · L2 情景 · L3 语义 · L4 程序（遗忘衰减 + 语义检索）",
            Style::default().fg(theme::dim()),
        )));
    }

    if total == 0 {
        render_empty(&mut lines);
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    lines.push(Line::raw(""));

    // 按来源（tags 首标签）分组：每组为一段记忆链，组内按时间序承接
    let recs = app.memory.recent(total.min(80));
    let mut groups: Vec<(String, Vec<&MemoryRecord>)> = Vec::new();
    for rec in recs.iter() {
        let src = source_of(rec);
        if let Some(g) = groups.iter_mut().find(|(k, _)| k == &src) {
            g.1.push(rec);
        } else {
            groups.push((src, vec![rec]));
        }
    }

    for (src, entries) in groups.iter() {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("── 来源：{}（{} 条） ──", src, entries.len()),
                Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
            ),
        ]));
        let n = entries.len();
        for (i, rec) in entries.iter().enumerate() {
            let speaker = role_cn(&rec.role);
            let content: String = rec.content.chars().take(60).collect();
            let (stem, conn) = if i + 1 == n {
                ("└─", "  ")
            } else {
                ("├─", "│ ")
            };
            let hhmm = rec.timestamp.chars().take(16).collect::<String>();
            // 关联链标记：含思考链（reasoning）的记忆条目弱化提示
            let has_reasoning = rec
                .reasoning
                .as_deref()
                .map(|r| !r.trim().is_empty())
                .unwrap_or(false);
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", stem), Style::default().fg(theme::border())),
                Span::styled(format!("[{}]", speaker), Style::default().fg(theme::accent())),
                Span::styled(format!(" {} ", hhmm), Style::default().fg(theme::faint())),
                Span::styled(content, Style::default().fg(theme::text())),
                Span::styled(
                    if has_reasoning { " · 含思考链" } else { "" },
                    Style::default().fg(theme::faint()),
                ),
            ]));
            // 下一条目在前一节点下方缩进对齐，形成纵向记忆链
            if i + 1 < n {
                lines.push(Line::from(Span::styled(
                    format!("  {}  ↳", conn),
                    Style::default().fg(theme::border()),
                )));
            }
        }
        lines.push(Line::raw(""));
    }

    if total > recs.len() {
        lines.push(Line::from(Span::styled(
            format!("  … 共 {} 条，仅展示最近 {} 条（滚动查看更多）", total, recs.len()),
            Style::default().fg(theme::faint()),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  /mem <关键词> 语义检索 · 任务经验自动沉淀为技能（F5 查看）",
        Style::default().fg(theme::faint()),
    )));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 无数据时的引导提示。
fn render_empty(lines: &mut Vec<Line>) {
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  暂无记忆记录",
        Style::default().fg(theme::dim()),
    )));
    lines.push(Line::from(Span::styled(
        format!("  对话内容将自动持久化到 {}。", MEMORY_DIR_HINT),
        Style::default().fg(theme::faint()),
    )));
    lines.push(Line::from(Span::styled(
        "  开始对话后，这里会按来源（标签）展示记忆链；/mem <关键词> 可语义检索。",
        Style::default().fg(theme::faint()),
    )));
    if std::env::var("AIRY_HOME").is_err() {
        lines.push(Line::from(Span::styled(
            "  提示：未设置 AIRY_HOME，记忆将存入 ~/.airymaxrt/data/agentrt/tui/。",
            Style::default().fg(theme::warning()),
        )));
    }
}

/// 记忆来源：tags 首个非空标签；无标签时回退角色。
fn source_of(rec: &MemoryRecord) -> String {
    rec.tags
        .split(',')
        .map(|t| t.trim())
        .find(|t| !t.is_empty())
        .map(|t| t.to_string())
        .unwrap_or_else(|| role_cn(&rec.role).to_string())
}

/// 角色中文名。
fn role_cn(role: &str) -> &str {
    match role {
        "user" => "用户",
        "assistant" => "助手",
        "system" => "系统",
        _ => role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(role: &str, content: &str, tags: &str, ts: &str) -> MemoryRecord {
        MemoryRecord {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: ts.to_string(),
            tags: tags.to_string(),
            reasoning: None,
        }
    }

    #[test]
    fn source_of_uses_first_tag() {
        let r = rec("user", "内容", "task,chat", "2026-01-01T00:00:00");
        assert_eq!(source_of(&r), "task");
    }

    #[test]
    fn source_of_falls_back_to_role() {
        let r = rec("assistant", "内容", " ", "2026-01-01T00:00:00");
        assert_eq!(source_of(&r), "助手");
    }

    #[test]
    fn role_cn_maps_known_roles() {
        assert_eq!(role_cn("user"), "用户");
        assert_eq!(role_cn("assistant"), "助手");
        assert_eq!(role_cn("system"), "系统");
        assert_eq!(role_cn("memory"), "memory");
    }
}
