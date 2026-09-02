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
//
// 0.1.9 W8 分组懒加载：条数即版本——记忆库未变化时整段复用已构建的
// 分组视图（MemoryView），每帧不再克隆记录窗口与重分组；PgUp/PgDn 移动
// 记录窗口（每页 80 条），仅翻页/新增时重建。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::memory::{ConversationMemory, MemoryRecord};
use crate::theme;

/// 记忆目录（与 src/memory.rs memory_dir 对齐；文案用，避免魔法路径）
const MEMORY_DIR_HINT: &str = "$AIRY_HOME/data/agentrt/tui/memory.jsonl";

/// 每页展示的记录数（窗口分页粒度）。
const PAGE_RECORDS: usize = 80;

/// 记忆面板分组视图缓存（0.1.9 W8）：条数即版本，未变即复用已构建行。
#[derive(Default)]
pub struct MemoryView {
    /// 已构建视图对应的记忆条数；usize::MAX 表示失效
    gen: usize,
    /// 窗口起点（0 = 最新页，向更早方向递增）
    skip: usize,
    /// 整段已渲染内容（含头部与页码提示）
    lines: Vec<Line<'static>>,
}

impl MemoryView {
    /// PgDn：向更早方向翻一页，越界时钳到最远整页。
    pub fn page_down(&mut self, total: usize) {
        self.move_to(self.skip.saturating_add(PAGE_RECORDS).min(last_page_start(total)));
    }

    /// PgUp：向更新方向翻回一页，最新页为界。
    pub fn page_up(&mut self) {
        self.move_to(self.skip.saturating_sub(PAGE_RECORDS));
    }

    /// 进入面板：回到最新页并强制重建。
    pub fn reset(&mut self) {
        self.skip = 0;
        self.invalidate();
    }

    /// 当前视图是否可按新的条数判定为陈旧。
    fn needs_rebuild(&self, total: usize) -> bool {
        self.gen != total || self.lines.is_empty()
    }

    fn move_to(&mut self, skip: usize) {
        if skip != self.skip {
            self.skip = skip;
            self.invalidate();
        }
    }

    fn invalidate(&mut self) {
        self.gen = usize::MAX;
    }
}

/// 最远一页的窗口起点（保证末页仍有记录可展示）。
fn last_page_start(total: usize) -> usize {
    if total <= PAGE_RECORDS {
        0
    } else {
        (total - 1) / PAGE_RECORDS * PAGE_RECORDS
    }
}

/// Render the memory statistics panel.
///
/// 实时渲染本地对话记忆库（$AIRY_HOME/data/agentrt/tui/memory.jsonl）：
/// 按来源（标签）分组展示记忆链，无需依赖网关 HTTP 端点。内容整段缓存，
/// 条数与页码未变时零重建。
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 记忆库 ",
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        ));

    let total = app.memory.len();
    if app.memory_view.needs_rebuild(total) {
        rebuild(&mut app.memory_view, app.memory.as_ref(), total);
    }
    let body = app.memory_view.lines.clone();
    f.render_widget(Paragraph::new(body).block(block), area);
}

/// 重建整段视图：头部统计 + 当前页分组链 + 页码提示。
fn rebuild(view: &mut MemoryView, mem: &dyn ConversationMemory, total: usize) {
    let backend = mem.backend_name();
    let want = (view.skip + PAGE_RECORDS).min(total);
    let recs = mem.recent(want);
    let window = &recs[view.skip.min(recs.len())..];

    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
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
        page_hint(total, window.len()),
    ])];

    // L1-L4 分层记忆（MemoryRovol 后端启用时提示分层语义）
    if backend == "MemoryRovol" {
        lines.push(Line::from(Span::styled(
            "  L1-L4 分层：L1 工作 · L2 情景 · L3 语义 · L4 程序（遗忘衰减 + 语义检索）",
            Style::default().fg(theme::dim()),
        )));
    }

    if total == 0 {
        render_empty(&mut lines);
    } else {
        lines.push(Line::raw(""));
        if view.skip > 0 {
            lines.push(hidden_hint(view.skip));
        }
        for (src, entries) in build_groups(window) {
            push_group(&mut lines, &src, &entries);
        }
        let earlier = total.saturating_sub(view.skip + window.len());
        if earlier > 0 {
            lines.push(Line::from(Span::styled(
                format!("  … 更早 {} 条未展示 · PgDn 下一页 / PgUp 上一页", earlier),
                Style::default().fg(theme::faint()),
            )));
        }
        lines.push(Line::from(Span::styled(
            "  /mem <关键词> 语义检索 · 任务经验自动沉淀为技能（F5 查看）",
            Style::default().fg(theme::faint()),
        )));
    }

    view.lines = lines;
    view.gen = total;
}

/// 头部页码段：总量超过一页时说明本页条数与翻页键。
fn page_hint(total: usize, shown: usize) -> Span<'static> {
    let text = if total <= PAGE_RECORDS {
        format!("  ·  共 {} 条", total)
    } else {
        format!("  ·  本页 {} 条 · PgDn/PgUp 翻页", shown)
    };
    Span::styled(text, Style::default().fg(theme::faint()))
}

/// 窗口非首页时，说明被折叠的更新记录数。
fn hidden_hint(skip: usize) -> Line<'static> {
    Line::from(Span::styled(
        format!("  ↑ 较新 {} 条已隐藏 · PgUp 返回", skip),
        Style::default().fg(theme::faint()),
    ))
}

/// 按来源（tags 首标签）分组：每组为一段记忆链，组内保持后端时序。
fn build_groups(recs: &[MemoryRecord]) -> Vec<(String, Vec<&MemoryRecord>)> {
    let mut groups: Vec<(String, Vec<&MemoryRecord>)> = Vec::new();
    for rec in recs {
        let src = source_of(rec);
        if let Some(g) = groups.iter_mut().find(|(k, _)| k == &src) {
            g.1.push(rec);
        } else {
            groups.push((src, vec![rec]));
        }
    }
    groups
}

/// 渲染一个来源分组（含组内记忆链与组后留白）。
fn push_group(lines: &mut Vec<Line<'static>>, src: &str, entries: &[&MemoryRecord]) {
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
        let (stem, conn) = if i + 1 == n { ("└─", "  ") } else { ("├─", "│ ") };
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

/// 无数据时的引导提示。
fn render_empty(lines: &mut Vec<Line<'static>>) {
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
            format!(
                "  提示：未设置 AIRY_HOME，记忆将存入 ~{}/data/agentrt/tui/。",
                crate::paths::DEFAULT_DIR_NAME
            ),
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

    #[test]
    fn build_groups_keeps_first_seen_order() {
        let a = rec("user", "a", "task", "2026-01-01T00:00:00");
        let b = rec("user", "b", "chat", "2026-01-01T00:00:01");
        let c = rec("user", "c", "task", "2026-01-01T00:00:02");
        let recs = vec![c, b, a];
        let groups = build_groups(&recs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "task");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "chat");
        // 组内保持后端传入的时序（最新优先）
        assert_eq!(groups[0].1[0].content, "c");
        assert_eq!(groups[0].1[1].content, "a");
    }

    #[test]
    fn last_page_start_clamps_to_nonempty_page() {
        assert_eq!(last_page_start(0), 0);
        assert_eq!(last_page_start(PAGE_RECORDS), 0);
        assert_eq!(last_page_start(PAGE_RECORDS + 1), PAGE_RECORDS);
        assert_eq!(last_page_start(200), 160);
        assert_eq!(last_page_start(160), 80);
    }

    #[test]
    fn page_down_clamps_at_last_page() {
        let total = PAGE_RECORDS * 2 + 5;
        let mut view = MemoryView::default();
        view.page_down(total);
        assert_eq!(view.skip, PAGE_RECORDS);
        view.page_down(total);
        assert_eq!(view.skip, PAGE_RECORDS * 2);
        view.page_down(total);
        assert_eq!(view.skip, PAGE_RECORDS * 2);
    }

    #[test]
    fn page_up_returns_towards_latest() {
        let mut view = MemoryView {
            skip: PAGE_RECORDS * 2,
            ..MemoryView::default()
        };
        view.page_up();
        assert_eq!(view.skip, PAGE_RECORDS);
        view.page_up();
        assert_eq!(view.skip, 0);
        view.page_up();
        assert_eq!(view.skip, 0);
    }

    #[test]
    fn paging_and_reset_invalidate_cache() {
        let total = PAGE_RECORDS * 3;
        let mut view = MemoryView {
            gen: total,
            lines: vec![Line::raw("cached")],
            ..MemoryView::default()
        };
        // 条数与页码未变：缓存复用，不重建
        assert!(!view.needs_rebuild(total));
        view.page_down(total);
        assert!(view.needs_rebuild(total));
        view.gen = total;
        assert!(!view.needs_rebuild(total));
        view.reset();
        assert!(view.needs_rebuild(total));
        assert_eq!(view.skip, 0);
    }

    #[test]
    fn cold_view_always_rebuilds() {
        let view = MemoryView::default();
        assert!(view.needs_rebuild(0));
    }
}
