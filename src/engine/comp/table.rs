// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// table 组件（0.1.21 X3-07~X3-12）：列宽约束求解、单元格裁剪与对齐、稳定
// 排序、选中态与键盘/鼠标导航。
//
// 列宽求解四档（X3-07b）：
//   1. 基准：`Min` 取内容宽与下限的较大者；`Max` 取内容宽与上限的较小者；
//      `Flex` 取内容宽。
//   2. 空间不足：从各列逐列回收（高列先减），任何列不低于 1 列。
//   3. 空间有余：先按权重分给 `Flex`，再把余量补进尚未到上限的 `Max` 列。
//   4. 仍有余量则留白——不把非弹性列拉宽，避免宽屏下稀疏内容被强行撑开。
// 结果保证 Σ列宽 + 间隙 ≤ 可用宽度（marker 另占 2 列，已在可用宽度中扣除），
// 这是 IS-34「40 列不溢出」的充分条件。
//
// 裁剪复用 `engine::grid::clip`：省略号按显示宽度预留 1 列，双宽字符不会被
// 截成半个（X3-09a）。对齐（左/右/中）在裁剪后按显示宽度补空格，不使用
// 字符数，避免全角单元格错列。
//
// 排序使用 `sort_by`（稳定），等值键保持输入序——看板按状态分组时组内
// 「最新在前」依赖这一性质（X3-10b）。选中态与渲染分离：`sel` 是数据层
// 下标，渲染只负责高亮，导航在越界时夹取而不回绕（X3-12b）。

use crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
};

use super::{Act, Component, Input, Size, Width};
use crate::engine::grid;

/// 单元格渲染函数：返回（显示文本，基础样式）。
pub(crate) type CellFn<T> = fn(&T) -> (String, Style);
/// 排序键函数：按列键排序，键为可比较字符串（数值列需零填充）。
pub(crate) type KeyFn<T> = fn(&T) -> String;

/// 一列的声明：宽度约束、对齐、取值与排序键。
pub(crate) struct Column<T> {
    pub(crate) width: Width,
    pub(crate) align: Alignment,
    pub(crate) cell: CellFn<T>,
    pub(crate) key: KeyFn<T>,
}

impl<T> Column<T> {
    pub(crate) fn new(
        width: Width,
        align: Alignment,
        cell: CellFn<T>,
        key: KeyFn<T>,
    ) -> Self {
        Self {
            width,
            align,
            cell,
            key,
        }
    }
}

/// 单行表格组件。行数据由调用方持有并传入（快照语义），组件不反向读取 App。
pub(crate) struct Table<T> {
    columns: Vec<Column<T>>,
    rows: Vec<T>,
    area: Rect,
    sel: usize,
    marker: bool,
    sel_style: Style,
}

impl<T> Table<T> {
    pub(crate) fn new(columns: Vec<Column<T>>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            area: Rect::default(),
            sel: 0,
            marker: false,
            sel_style: Style::default(),
        }
    }

    /// 替换行集合并把选中下标收敛到新范围内（数据刷新后不越界）。
    pub(crate) fn set_rows(&mut self, rows: Vec<T>) {
        self.rows = rows;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    /// 是否在首列前绘制 2 列选中标记（`▸ ` / 空格）。
    pub(crate) fn set_marker(&mut self, on: bool) {
        self.marker = on;
    }

    /// 选中行底色样式（由调用方注入，组件不依赖主题模块）。
    pub(crate) fn set_selected_style(&mut self, style: Style) {
        self.sel_style = style;
    }

    /// 直接设置选中行（调用方已按同一顺序裁剪过行集合）。
    pub(crate) fn select(&mut self, i: usize) {
        self.sel = i.min(self.rows.len().saturating_sub(1));
    }

    /// 当前选中行下标（事件处理后由上层写回业务光标）。
    pub(crate) fn selected(&self) -> usize {
        self.sel
    }

    /// 按列键稳定排序；`asc` 为假时降序（等值键仍保持输入序）。
    pub(crate) fn sort_by(&mut self, col: usize, asc: bool) {
        let Some(column) = self.columns.get(col) else {
            return;
        };
        let key = column.key;
        self.rows.sort_by(|a, b| {
            if asc {
                key(a).cmp(&key(b))
            } else {
                key(b).cmp(&key(a))
            }
        });
    }

    /// 选中行在视口内的起始偏移（选中行越出下边界时滚动跟随）。
    fn offset(&self) -> usize {
        let h = usize::from(self.area.height);
        if h == 0 || self.sel < h {
            return 0;
        }
        self.sel + 1 - h
    }

    /// 列间与标记占用的固定宽度。
    fn gaps(&self) -> usize {
        let n = self.columns.len();
        let marker = if self.marker { 2 } else { 0 };
        n.saturating_sub(1) + marker
    }

    fn widths(&self, content_avail: usize) -> Vec<usize> {
        let natural: Vec<usize> = self
            .columns
            .iter()
            .map(|c| {
                self.rows
                    .iter()
                    .map(|r| grid::width(&(c.cell)(r).0))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let widths: Vec<Width> = self.columns.iter().map(|c| c.width).collect();
        solve(&widths, &natural, content_avail)
    }

    fn styled(&self, base: Style, sel: bool) -> Style {
        if sel {
            base.patch(self.sel_style)
        } else {
            base
        }
    }

    fn draw_row(&self, buf: &mut Buffer, row: &T, sel: bool, y: u16, widths: &[usize]) {
        let right = self.area.x.saturating_add(self.area.width);
        if sel && self.area.width > 0 {
            for x in self.area.x..right {
                buf[(x, y)].set_style(self.sel_style);
            }
        }
        let mut x = self.area.x;
        if self.marker {
            let m = if sel { "▸ " } else { "  " };
            buf.set_stringn(x, y, m, 2, self.styled(Style::default(), sel));
            x = x.saturating_add(2);
        }
        for (i, col) in self.columns.iter().enumerate() {
            if x >= right {
                break;
            }
            let (text, style) = (col.cell)(row);
            let s = fit(&text, widths[i], col.align);
            let room = usize::from(right - x);
            buf.set_stringn(x, y, &s, room, self.styled(style, sel));
            x = x.saturating_add((widths[i] as u16).saturating_add(1));
        }
    }

    fn key(&mut self, code: KeyCode) -> Act {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return Act::None;
        };
        match code {
            KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                Act::Up
            }
            KeyCode::Down => {
                self.sel = (self.sel + 1).min(last);
                Act::Down
            }
            KeyCode::Home => {
                self.sel = 0;
                Act::Top
            }
            KeyCode::End => {
                self.sel = last;
                Act::Bottom
            }
            KeyCode::Enter => Act::Select(self.sel),
            _ => Act::None,
        }
    }

    fn click(&mut self, event: MouseEvent, row: u16) -> Act {
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Act::None;
        }
        let bottom = self.area.y.saturating_add(self.area.height);
        if row < self.area.y || row >= bottom {
            return Act::None;
        }
        let Some(last) = self.rows.len().checked_sub(1) else {
            return Act::None;
        };
        let idx = (self.offset() + usize::from(row - self.area.y)).min(last);
        if idx == self.sel {
            return Act::None;
        }
        self.sel = idx;
        Act::Select(idx)
    }
}

impl<T> Component for Table<T> {
    fn measure(&self, avail: Size) -> Size {
        let content = usize::from(avail.w).saturating_sub(self.gaps());
        let used: usize = self.widths(content).iter().sum::<usize>() + self.gaps();
        Size {
            w: used.min(usize::from(avail.w)) as u16,
            h: self.rows.len().min(usize::from(avail.h)) as u16,
        }
    }

    fn layout(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, buf: &mut Buffer) {
        let content = usize::from(self.area.width).saturating_sub(self.gaps());
        let widths = self.widths(content);
        let bottom = self.area.y.saturating_add(self.area.height);
        let off = self.offset();
        let mut y = self.area.y;
        for (i, row) in self.rows.iter().enumerate().skip(off) {
            if y >= bottom {
                break;
            }
            self.draw_row(buf, row, i == self.sel, y, &widths);
            y += 1;
        }
    }

    fn event(&mut self, input: Input) -> Act {
        match input {
            Input::Key(k) => self.key(k.code),
            Input::Mouse { event, row } => self.click(event, row),
        }
    }
}

/// 基准列宽：约束与内容宽的第一次收敛。
fn base(w: Width, n: usize) -> usize {
    match w {
        Width::Min(m) => n.max(m).max(1),
        Width::Max(m) => n.min(m).max(1),
        Width::Flex(_) => n.max(1),
    }
}

/// 列宽求解入口：先取基准，再按空间余缺调整。
fn solve(widths: &[Width], natural: &[usize], avail: usize) -> Vec<usize> {
    if widths.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<usize> = widths
        .iter()
        .zip(natural)
        .map(|(w, &n)| base(*w, n))
        .collect();
    let total: usize = out.iter().sum();
    if total > avail {
        shrink(&mut out, total - avail);
    } else {
        grow(&mut out, widths, avail - total);
    }
    out
}

/// 空间不足时逐列回收，任何列不低于 1 列（仍不足时由渲染侧裁剪兜底）。
fn shrink(out: &mut [usize], mut excess: usize) {
    while excess > 0 {
        let mut moved = false;
        for w in out.iter_mut() {
            if excess == 0 {
                break;
            }
            if *w > 1 {
                *w -= 1;
                excess -= 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

/// 空间有余时先按权重喂 `Flex`，再补 `Max` 列至上限，最后留白。
fn grow(out: &mut [usize], widths: &[Width], mut spare: usize) {
    let weights: usize = widths
        .iter()
        .map(|w| match *w {
            Width::Flex(x) => usize::from(x),
            _ => 0,
        })
        .sum();
    if weights > 0 {
        for (i, w) in widths.iter().enumerate() {
            let Width::Flex(weight) = *w else {
                continue;
            };
            let add = spare * usize::from(weight) / weights;
            out[i] += add;
            spare -= add;
        }
    }
    let caps: Vec<usize> = widths
        .iter()
        .map(|w| match *w {
            Width::Max(m) => m,
            _ => usize::MAX,
        })
        .collect();
    while spare > 0 && out.iter().zip(&caps).any(|(o, c)| o < c) {
        for (i, cap) in caps.iter().enumerate() {
            if spare == 0 {
                break;
            }
            if out[i] < *cap {
                out[i] += 1;
                spare -= 1;
            }
        }
    }
}

/// 裁剪 + 对齐：结果显示宽度恰为 `cols`（除非 `cols` 为 0）。
fn fit(s: &str, cols: usize, align: Alignment) -> String {
    let clipped = grid::clip(s, cols);
    let pad = cols.saturating_sub(grid::width(&clipped));
    let left = match align {
        Alignment::Right => pad,
        Alignment::Center => pad / 2,
        _ => 0,
    };
    format!(
        "{}{}{}",
        " ".repeat(left),
        clipped,
        " ".repeat(pad - left)
    )
}

#[cfg(test)]
mod tests;
