// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 块级层（0.1.18 W6）：pulldown-cmark 事件流 → ratatui 行序列。
//
// 解析与渲染分离：块级结构（段落/标题/列表/引用/代码/表格/脚注/分隔线）由
// CommonMark 解析器裁定，本层只做事件流到行的编排。行内样式归 inline 层，
// 代码高亮归 code 层，表格布局归 table 层，数学降级归 math 层。
//
// 宽度纪律：所有折行经 L2 唯一裁决点 `crate::engine::grid::wrap_spans`，本模块
// 不自行测量后截断或填充；续行按前缀实测宽度补空格，保证悬挂缩进对齐。

use crate::engine::grid;
use crate::markdown::code::render_code;
use crate::markdown::inline::Inline;
use crate::markdown::plot::render_plot;
use crate::markdown::table::{render_table, Cell};
use crate::theme;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// 列表栈帧。
struct Frame {
    ordered: bool,
    next: u64,
}

/// 当前列表项标记。
#[derive(Clone, Copy)]
enum Mark {
    None,
    Bullet,
    Ordered(u64),
    Task(bool),
}

/// 表格缓冲：表头、数据行与列对齐在 `Tag::Table` 区间内累积，闭合时统一布局。
struct TableBuf {
    aligns: Vec<Alignment>,
    head: Vec<Cell>,
    rows: Vec<Vec<Cell>>,
}

struct State {
    out: Vec<Line<'static>>,
    inline: Inline,
    base: Style,
    indent: usize,
    width: usize,
    quote: usize,
    lists: Vec<Frame>,
    mark: Mark,
    prefix: Vec<Span<'static>>,
    code: Option<String>,
    body: String,
    in_code: bool,
    table: Option<TableBuf>,
    row: Vec<Cell>,
}

/// 渲染入口：消费事件流，产出终端行序列。
pub(super) fn render<'a, I>(events: I, indent: usize, width: usize, base: Style) -> Vec<Line<'static>>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut st = State {
        out: Vec::new(),
        inline: Inline::new(base),
        base,
        indent,
        width,
        quote: 0,
        lists: Vec::new(),
        mark: Mark::None,
        prefix: Vec::new(),
        code: None,
        body: String::new(),
        in_code: false,
        table: None,
        row: Vec::new(),
    };
    for ev in events {
        st.event(ev);
    }
    st.finish()
}

impl State {
    fn event(&mut self, ev: Event<'_>) {
        if self.in_code {
            match ev {
                Event::Text(t) => self.body.push_str(&t),
                Event::End(TagEnd::CodeBlock) => self.close_code(),
                _ => {}
            }
            return;
        }
        if self.table.is_some() && self.table_event(&ev) {
            return;
        }
        if self.inline.on(&ev) {
            return;
        }
        match ev {
            Event::Start(tag) => self.open(tag),
            Event::End(tag) => self.close(tag),
            Event::HardBreak => self.flush(),
            Event::Rule => self.rule(),
            Event::TaskListMarker(done) => self.task_mark(done),
            _ => {}
        }
    }

    fn open(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                self.inline
                    .set_base(self.base.fg(head_color(level)).add_modifier(Modifier::BOLD));
                self.prefix = self.quote_prefix();
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote += 1;
                self.rebuild_prefix();
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.in_code = true;
                self.body.clear();
                self.code = match kind {
                    CodeBlockKind::Fenced(info) => Some(info.trim().to_string()),
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(Frame {
                    ordered: start.is_some(),
                    next: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.flush();
                self.mark = match self.lists.last_mut() {
                    Some(f) if f.ordered => {
                        let n = f.next;
                        f.next += 1;
                        Mark::Ordered(n)
                    }
                    _ => Mark::Bullet,
                };
                self.inline.set_base(self.base);
                self.rebuild_prefix();
            }
            Tag::Table(aligns) => {
                self.flush();
                self.table = Some(TableBuf {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                });
            }
            Tag::FootnoteDefinition(name) => {
                self.flush();
                self.prefix = vec![Span::styled(
                    format!("[{name}] "),
                    self.base.fg(theme::accent()),
                )];
            }
            _ => {}
        }
    }

    fn close(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.block_break(),
            TagEnd::Heading(_) => {
                self.block_break();
                self.inline.set_base(self.base);
                self.rebuild_prefix();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                self.rebuild_prefix();
            }
            TagEnd::Item => {
                self.flush();
                self.mark = Mark::None;
                self.inline.set_base(self.base);
                self.rebuild_prefix();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                self.rebuild_prefix();
                if self.lists.is_empty() {
                    self.out.push(Line::raw(""));
                }
            }
            TagEnd::Table => self.close_table(),
            TagEnd::FootnoteDefinition => {
                self.block_break();
                self.rebuild_prefix();
            }
            _ => {}
        }
    }

    /// 表格区间内的事件：单元格内容仍走行内层，闭合时统一交给表格层布局。
    fn table_event(&mut self, ev: &Event<'_>) -> bool {
        match ev {
            // 表头不包 TableRow：解析器直接以 TableHead → TableCell… → TableHead
            // 收束，数据行才走 TableRow。两者各自落定，不共用行状态。
            Event::Start(Tag::TableHead) => {
                self.inline.take();
                self.row.clear();
                true
            }
            Event::End(TagEnd::TableHead) => {
                let row = std::mem::take(&mut self.row);
                if let Some(t) = self.table.as_mut() {
                    t.head = row;
                }
                true
            }
            Event::Start(Tag::TableRow) => {
                self.row.clear();
                true
            }
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut self.row);
                if let Some(t) = self.table.as_mut() {
                    t.rows.push(row);
                }
                true
            }
            Event::Start(Tag::TableCell) => {
                self.inline.take();
                true
            }
            Event::End(TagEnd::TableCell) => {
                let cell = self.inline.take();
                self.row.push(cell);
                true
            }
            Event::End(TagEnd::Table) => {
                self.close_table();
                true
            }
            _ => self.inline.on(ev),
        }
    }

    fn close_code(&mut self) {
        self.in_code = false;
        let lang = self.code.take().unwrap_or_default();
        let body = std::mem::take(&mut self.body);
        let body = body.strip_suffix('\n').unwrap_or(&body);

        if lang == "plot" {
            let rows: Vec<String> = body.lines().map(|l| l.trim().to_string()).collect();
            self.out
                .extend(render_plot(&rows, self.indent, self.width, self.base));
        } else {
            self.out
                .extend(render_code(&lang, body, self.indent, self.width, self.base));
        }
        self.out.push(Line::raw(""));
    }

    fn close_table(&mut self) {
        let Some(t) = self.table.take() else {
            return;
        };
        self.out.extend(render_table(
            &t.head,
            &t.rows,
            &t.aligns,
            self.indent,
            self.width,
            self.base,
        ));
        self.out.push(Line::raw(""));
    }

    fn rule(&mut self) {
        self.flush();
        let n = self.width.saturating_sub(self.indent).clamp(6, 40);
        self.out.push(Line::from(vec![
            Span::styled(" ".repeat(self.indent), Style::default()),
            Span::styled("─".repeat(n), self.base.fg(theme::separator())),
        ]));
    }

    fn task_mark(&mut self, done: bool) {
        self.mark = Mark::Task(done);
        self.inline.set_base(if done {
            self.base.fg(theme::faint()).add_modifier(Modifier::CROSSED_OUT)
        } else {
            self.base
        });
        self.rebuild_prefix();
    }

    /// 段落收束：落定当前行并留出段间空白。
    fn block_break(&mut self) {
        self.flush();
        self.out.push(Line::raw(""));
    }

    fn flush(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let content = self.inline.take();
        let lead_w = grid::line_w(&self.prefix);

        // 悬挂缩进：续行以等宽空格替代前缀，故正文预算须先扣除前缀宽度。
        // 若把前缀并入待折行内容再给续行补空格，续行会额外多出前缀宽度而右溢。
        let budget = self.width.saturating_sub(self.indent).max(8);
        let text_w = budget.saturating_sub(lead_w).max(1);

        for (i, piece) in grid::wrap_spans(&content, text_w).into_iter().enumerate() {
            let mut spans = vec![Span::styled(" ".repeat(self.indent), Style::default())];
            if i == 0 {
                spans.extend(self.prefix.iter().cloned());
            } else if lead_w > 0 {
                spans.push(Span::raw(" ".repeat(lead_w)));
            }
            spans.extend(piece);
            self.out.push(Line::from(spans));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        if self.in_code {
            self.close_code();
        }
        if self.table.is_some() {
            self.close_table();
        }
        while self
            .out
            .last()
            .is_some_and(|l| grid::line_w(&l.spans) == 0)
        {
            self.out.pop();
        }
        self.out
    }

    fn quote_prefix(&self) -> Vec<Span<'static>> {
        let mut v: Vec<Span<'static>> = Vec::new();
        for _ in 0..self.quote {
            v.push(Span::styled("▏ ".to_string(), self.base.fg(theme::dim())));
        }
        v
    }

    fn rebuild_prefix(&mut self) {
        let mut v = self.quote_prefix();
        let depth = self.lists.len();
        if depth > 1 {
            v.push(Span::raw("  ".repeat(depth - 1)));
        }
        let dim = self.base.fg(theme::dim());
        match self.mark {
            Mark::None => {}
            Mark::Bullet => v.push(Span::styled("• ".to_string(), dim)),
            Mark::Ordered(n) => v.push(Span::styled(format!("{n}. "), dim)),
            Mark::Task(true) => v.push(Span::styled("● ".to_string(), self.base.fg(theme::accent()))),
            Mark::Task(false) => v.push(Span::styled("○ ".to_string(), dim)),
        }
        self.prefix = v;
    }
}

fn head_color(level: HeadingLevel) -> Color {
    match level {
        HeadingLevel::H1 => theme::primary(),
        HeadingLevel::H2 => theme::accent(),
        _ => theme::text(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldown_cmark::{Options, Parser};

    fn render_md(md: &str, width: usize) -> Vec<Line<'static>> {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TASKLISTS);
        opts.insert(Options::ENABLE_MATH);
        opts.insert(Options::ENABLE_FOOTNOTES);
        render(Parser::new_ext(md, opts), 0, width, Style::default())
    }

    fn joined(md: &str, width: usize) -> String {
        render_md(md, width)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn paragraph_is_rendered() {
        let out = joined("你好世界", 40);
        assert!(out.contains("你好世界"));
    }

    #[test]
    fn nested_quote_uses_two_bars() {
        let out = joined("> 一级\n>\n> > 二级", 60);
        assert!(out.contains("▏ 一级"));
        assert!(out.contains("▏ ▏ 二级"), "{out}");
    }

    #[test]
    fn ordered_and_bullet_lists() {
        let out = joined("1. 第一\n2. 第二\n\n- 甲\n- 乙", 60);
        assert!(out.contains("1. 第一"));
        assert!(out.contains("2. 第二"));
        assert!(out.contains("• 甲"));
    }

    #[test]
    fn nested_list_is_indented() {
        let out = joined("- 外层\n  - 内层", 60);
        assert!(out.contains("• 外层"));
        assert!(out.contains("  • 内层"), "{out}");
    }

    #[test]
    fn task_list_uses_state_symbols() {
        let out = joined("- [ ] 待办\n- [x] 完成", 60);
        assert!(out.contains("○ 待办"));
        assert!(out.contains("● 完成"));
    }

    #[test]
    fn heading_is_bold_and_colored() {
        let lines = render_md("# 大标题", 40);
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("大标题"));
        assert!(span.is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(span.is_some_and(|s| s.style.fg == Some(theme::primary())));
    }

    #[test]
    fn rule_draws_separator() {
        let out = joined("前\n\n---\n\n后", 40);
        assert!(out.contains('─'));
    }

    #[test]
    fn long_paragraph_wraps_within_width() {
        let md = "上下文窗口 hello 世界，全角标点：（测试）与组合字符 e\u{0301} 混排";
        for width in 12..=60usize {
            for line in render_md(md, width) {
                assert!(
                    grid::line_w(&line.spans) <= width,
                    "右溢: width={width} line={line:?}"
                );
            }
        }
    }

    #[test]
    fn hanging_indent_aligns_continuation() {
        let md = "- 一个足够长的列表项内容，需要在多个物理行上折行显示";
        let lines = render_md(md, 24);
        let lead = grid::line_w(&lines[0].spans);
        assert!(lines.len() > 1, "应折行: {lines:?}");
        for line in &lines[1..] {
            let w = grid::line_w(&line.spans);
            assert!(w <= 24, "续行右溢: {line:?}");
        }
        assert!(lead > 0);
    }

    #[test]
    fn table_is_laid_out() {
        let md = "| 名称 | 数值 |\n|:---|---:|\n| 苹果 | 12 |";
        let out = joined(md, 60);
        assert!(out.contains("名称"), "OUT=[{out}]");
        assert!(out.contains("苹果"), "OUT=[{out}]");
        assert!(out.contains('│'), "OUT=[{out}]");
    }

    #[test]
    fn code_fence_renders_body() {
        let out = joined("```rust\nfn main() {}\n```", 60);
        assert!(out.contains("fn main() {}"));
    }

    #[test]
    fn footnote_reference_and_definition() {
        let out = joined("正文[^1]\n\n[^1]: 脚注内容", 60);
        assert!(out.contains("[1]"));
        assert!(out.contains("脚注内容"));
    }

    #[test]
    fn unclosed_fence_keeps_content() {
        let out = joined("```\n未闭合代码", 60);
        assert!(out.contains("未闭合代码"));
    }

    #[test]
    fn indent_is_applied_to_every_line() {
        let lines = render(Parser::new("你好"), 4, 40, Style::default());
        assert!(lines[0].to_string().starts_with("    你好"));
    }
}
