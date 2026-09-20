// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 表格块：`| a | b |` 表头/分隔/数据行的列宽对齐渲染。
//
// 宽度纪律（0.1.18 A 轨 W2，§3.3）：列宽分配与单元格截断一律委托 L2 唯一
// 裁决点 `crate::engine::grid`，本模块不得自行测量后截断或填充。

use crate::engine::grid;
use crate::theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// 渲染表格块：表头 + 分隔行 + 数据行，按列宽对齐（中文全角按 2 列计）。
pub(super) fn render_table(
    rows: &[String],
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    // 解析单元格（去掉首尾 |，按 | 分割）
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let trimmed = r.trim();
            let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
            inner
                .split('|')
                .map(|c| c.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    let cols = parsed.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return out;
    }
    // 列宽 = 各列最大显示宽度（表头/数据取最大；分隔行不参与）
    let mut want = vec![0usize; cols];
    for (ri, row) in parsed.iter().enumerate() {
        if is_separator_row(rows.get(ri).map(|s| s.as_str()).unwrap_or("")) {
            continue;
        }
        for (ci, cell) in row.iter().enumerate() {
            want[ci] = want[ci].max(grid::width(cell));
        }
    }
    // 每列除内容外另占 3 列（"│ " + 内容 + " "），整行另加 1 列收尾 "│"。
    // 列宽分配交由 L2 唯一裁决点，保证 Σ(列宽+3)+1 ≤ 可用宽度——这是右边界
    // 零溢出的充分条件。此前此处按 Σ(列宽+2)+1 估算，少算「列数」列，窄屏
    // 收缩不足导致 CJK 表格右溢（R4）。
    let avail = width.saturating_sub(indent).max(8);
    let Some(col_w) = grid::alloc_cols(&want, 3, 1, avail) else {
        // 连"每列 1 列内容"的最小布局都放不下：降级为纯文本，不画半个表格
        for row in rows {
            for piece in grid::wrap(row, avail) {
                out.push(Line::from(vec![
                    Span::styled(" ".repeat(indent), Style::default()),
                    Span::styled(piece, base),
                ]));
            }
        }
        return out;
    };

    for (ri, row) in parsed.iter().enumerate() {
        let is_sep = is_separator_row(rows.get(ri).map(|s| s.as_str()).unwrap_or(""));
        // 分隔行 → 水平线
        if is_sep {
            let line: String = col_w
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("┼");
            out.push(Line::from(vec![
                Span::styled(" ".repeat(indent), Style::default()),
                Span::styled(format!("┌{line}┐"), base.fg(theme::dim())),
            ]));
            continue;
        }
        let mut spans = vec![Span::styled(" ".repeat(indent), Style::default())];
        let is_header = ri == 0 && !is_sep;
        for (ci, cw) in col_w.iter().enumerate() {
            // 单元格按分配列宽截断：分配宽度是行宽有界的唯一依据，若不截断，
            // 窄屏下收缩后的列宽会被超宽单元格重新撑破（旧实现即如此）。
            let cell = grid::clip(&row.get(ci).cloned().unwrap_or_default(), *cw);
            let pad = cw.saturating_sub(grid::width(&cell));
            // 表头加粗 + 主色；数据行常规
            let style = if is_header {
                base.fg(theme::text()).add_modifier(Modifier::BOLD)
            } else {
                base.fg(theme::text())
            };
            spans.push(Span::styled(format!("│ {cell}{} ", " ".repeat(pad)), style));
        }
        spans.push(Span::styled("│", base.fg(theme::dim())));
        out.push(Line::from(spans));
    }
    out
}

/// 判断表格分隔行（如 |---|---|）。
pub(super) fn is_separator_row(s: &str) -> bool {
    let trimmed = s.trim().trim_start_matches('|').trim_end_matches('|');
    if trimmed.is_empty() {
        return false;
    }
    trimmed.split('|').all(|c| {
        let t = c.trim();
        !t.is_empty()
            && t.chars()
                .all(|ch| ch == '-' || ch == ':' || ch == ' ' || ch == '=')
            && t.contains('-')
    })
}
