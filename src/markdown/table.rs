// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 表格块（0.1.18 W6）：列宽自适应 + 左/中/右对齐 + 超宽截断。
//
// 分隔行（`|---|`）由解析器在解析期消解为 `Tag::Table(Vec<Alignment>)`，本层不再
// 自行识别——旧实现的 `is_separator_row` 是对解析器语义的重复实现，两者对
// `|:--|--:|` 与 `| === |` 的判定并不一致。
//
// 宽度纪律：列宽分配与单元格截断一律委托 L2 唯一裁决点 `crate::engine::grid`，
// 本模块不得自行测量后截断或填充。

use crate::engine::grid;
use crate::theme;
use pulldown_cmark::Alignment;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// 单元格：已定型的行内样式片段序列。
pub(super) type Cell = Vec<Span<'static>>;

/// 渲染表格：表头 + 分隔线 + 数据行，按列宽对齐。
pub(super) fn render_table(
    head: &[Cell],
    rows: &[Vec<Cell>],
    aligns: &[Alignment],
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let cols = rows
        .iter()
        .map(|r| r.len())
        .chain(std::iter::once(head.len()))
        .max()
        .unwrap_or(0);
    if cols == 0 {
        return Vec::new();
    }

    let mut want = vec![0usize; cols];
    for row in std::iter::once(head).chain(rows.iter().map(|r| r.as_slice())) {
        for (ci, cell) in row.iter().enumerate() {
            want[ci] = want[ci].max(grid::line_w(cell));
        }
    }

    let avail = width.saturating_sub(indent).max(8);
    // 每列除内容外另占 3 列（"│ " + 内容 + " "），整行另加 1 列收尾 "│"。
    // 列宽分配交由 L2 唯一裁决点，保证 Σ(列宽+3)+1 ≤ 可用宽度——这是右边界
    // 零溢出的充分条件。
    let Some(col_w) = grid::alloc_cols(&want, 3, 1, avail) else {
        // 连"每列 1 列内容"的最小布局都放不下：降级为纯文本，不画半个表格
        return degrade(head, rows, aligns, indent, avail, base);
    };

    let mut out: Vec<Line<'static>> = Vec::new();
    if !head.is_empty() {
        out.push(row_line(head, &col_w, aligns, indent, base, true));
        let sep: String = col_w
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┼");
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent), Style::default()),
            Span::styled(format!("├{sep}┤"), base.fg(theme::dim())),
        ]));
    }
    for row in rows {
        out.push(row_line(row, &col_w, aligns, indent, base, false));
    }
    out
}

fn row_line(
    cells: &[Cell],
    col_w: &[usize],
    aligns: &[Alignment],
    indent: usize,
    base: Style,
    header: bool,
) -> Line<'static> {
    let border = base.fg(theme::dim());
    let mut spans = vec![Span::styled(" ".repeat(indent), Style::default())];

    for (ci, cw) in col_w.iter().enumerate() {
        let empty: Cell = Vec::new();
        let raw = cells.get(ci).unwrap_or(&empty);
        let mut styled: Cell = raw
            .iter()
            .map(|s| Span::styled(s.content.to_string(), cell_style(s.style, base, header)))
            .collect();
        if grid::line_w(&styled) > *cw {
            styled = grid::clip_spans(&styled, *cw);
        }
        let used = grid::line_w(&styled);
        let gap = cw.saturating_sub(used);
        let (left, right) = match aligns.get(ci).copied().unwrap_or(Alignment::None) {
            Alignment::Right => (gap, 0),
            Alignment::Center => (gap / 2, gap - gap / 2),
            _ => (0, gap),
        };

        spans.push(Span::styled(format!("│ {}", " ".repeat(left)), border));
        spans.extend(styled);
        spans.push(Span::styled(format!("{} ", " ".repeat(right)), border));
    }
    spans.push(Span::styled("│", border));
    Line::from(spans)
}

/// 单元格样式：表头补主色与粗体，数据行未着色时补默认文本色。
fn cell_style(s: Style, base: Style, header: bool) -> Style {
    let mut st = s;
    if st.fg.is_none() {
        st = st.fg(if header {
            theme::text()
        } else {
            base.fg.unwrap_or_else(theme::text)
        });
    }
    if header {
        st = st.add_modifier(Modifier::BOLD);
    }
    st
}

/// 过窄降级：逐行拼接为纯文本并折行，保留全部语义。
fn degrade(
    head: &[Cell],
    rows: &[Vec<Cell>],
    aligns: &[Alignment],
    indent: usize,
    avail: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let _ = aligns;
    let mut out: Vec<Line<'static>> = Vec::new();
    for row in std::iter::once(head).chain(rows.iter().map(|r| r.as_slice())) {
        if row.is_empty() {
            continue;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (ci, cell) in row.iter().enumerate() {
            if ci > 0 {
                spans.push(Span::styled(" | ".to_string(), base.fg(theme::dim())));
            }
            spans.extend(cell.iter().cloned());
        }
        let mut full = vec![Span::styled(" ".repeat(indent), Style::default())];
        full.extend(spans);
        for piece in grid::wrap_spans(&full, avail) {
            out.push(Line::from(piece));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str) -> Cell {
        vec![Span::raw(text.to_string())]
    }

    fn sample() -> (Vec<Cell>, Vec<Vec<Cell>>) {
        let head = vec![cell("名称"), cell("数值"), cell("备注")];
        let rows = vec![
            vec![
                cell("上下文窗口（全角标点）"),
                cell("１２３"),
                cell("混合 mix 文字"),
            ],
            vec![cell("a你b好c"), cell("456"), cell("组合字符 e\u{0301} 结束")],
        ];
        (head, rows)
    }

    #[test]
    fn renders_header_separator_and_rows() {
        let (head, rows) = sample();
        let aligns = vec![Alignment::Left, Alignment::Right, Alignment::Center];
        let lines = render_table(&head, &rows, &aligns, 2, 80, Style::default());
        assert_eq!(lines.len(), 4, "表头 + 分隔线 + 2 数据行");
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("名称"));
        assert!(joined.contains("１２３"));
    }

    #[test]
    fn right_edge_never_overflows() {
        let (head, rows) = sample();
        let aligns = vec![Alignment::Left, Alignment::Right, Alignment::Center];
        for width in 8..=90usize {
            for indent in [0usize, 2, 6] {
                for line in render_table(&head, &rows, &aligns, indent, width, Style::default()) {
                    let w = grid::line_w(&line.spans);
                    assert!(
                        w <= width.max(indent + 8),
                        "表格右溢: width={width} indent={indent} line_w={w} line={line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn degrades_to_plain_text_when_too_narrow() {
        let head = vec![cell("a"), cell("b"), cell("c"), cell("d")];
        let aligns = vec![Alignment::None; 4];
        let lines = render_table(&head, &[], &aligns, 0, 6, Style::default());
        assert!(!lines.is_empty());
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(!joined.contains('│'), "过窄时应降级为纯文本: {joined:?}");
        assert!(joined.contains('a') && joined.contains('d'));
    }

    #[test]
    fn alignment_positions_content() {
        // 列宽由最宽单元格决定，故须有更宽的表头才能观察到对齐差异
        let head = vec![cell("longer")];
        let rows = vec![vec![cell("x")]];
        let right = render_table(&head, &rows, &[Alignment::Right], 0, 40, Style::default());
        let left = render_table(&head, &rows, &[Alignment::Left], 0, 40, Style::default());
        let r = right.last().map(|l| l.to_string()).unwrap_or_default();
        let l = left.last().map(|l| l.to_string()).unwrap_or_default();
        assert_ne!(r, l, "左/右对齐应产生不同留白");
        assert!(r.starts_with("│      x"), "右对齐: {r:?}");
        assert!(l.starts_with("│ x"), "左对齐: {l:?}");
    }

    #[test]
    fn header_cells_are_bold() {
        let (head, rows) = sample();
        let aligns = vec![Alignment::Left; 3];
        let lines = render_table(&head, &rows, &aligns, 0, 80, Style::default());
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content.contains("名称") && s.style.add_modifier.contains(Modifier::BOLD)));
    }
}
