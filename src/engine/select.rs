// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 拖选模型（0.1.21 X5-10a）：只做选区几何与文本切片，不碰终端。
//
// 坐标一律是「正文区内的相对行列」（行 = 视口首行为 0，列 = 该行首格
// 为 0），与渲染回写的可见行快照同源。文本按显示列切片：宽字符被部分
// 覆盖时整字保留，零宽组合符跟随基字符——选区边界落在汉字中间也不会
// 丢掉半个字形。

use super::grid;

/// 选区端点：行相对视口首行，列相对行首，单位是终端格。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub row: u16,
    pub col: u16,
}

impl Point {
    pub fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }

    /// 按显示顺序（先行后列）比较，反向拖选也能得到同一段文本。
    fn key(self) -> (u16, u16) {
        (self.row, self.col)
    }
}

/// 拖选状态机：按下记锚点，拖动只更新游标，抬起后由调用方取文本再清空。
#[derive(Clone, Copy, Debug, Default)]
pub struct Select {
    anchor: Option<Point>,
    cursor: Option<Point>,
    moved: bool,
}

impl Select {
    pub fn begin(&mut self, p: Point) {
        self.anchor = Some(p);
        self.cursor = Some(p);
        self.moved = false;
    }

    /// 更新游标；未开始拖选（无锚点）时返回 false，调用方据此忽略杂散事件。
    pub fn drag(&mut self, p: Point) -> bool {
        if self.anchor.is_none() {
            return false;
        }
        if self.cursor != Some(p) {
            self.moved = true;
        }
        self.cursor = Some(p);
        true
    }

    pub fn active(&self) -> bool {
        self.anchor.is_some()
    }

    /// 是否真的拖动过：单击（无位移）不应触发复制。
    pub fn moved(&self) -> bool {
        self.moved
    }

    pub fn points(&self) -> Option<(Point, Point)> {
        Some((self.anchor?, self.cursor?))
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// 从可见行快照切出选区文本（行间以 `\n` 连接）。
///
/// 反向拖选按显示顺序归一；越界的行直接截到快照末尾，不 panic。
pub fn text_between(lines: &[String], a: Point, b: Point) -> String {
    let (start, end) = if a.key() <= b.key() { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    for row in start.row..=end.row {
        let Some(line) = lines.get(row as usize) else {
            break;
        };
        let from = if row == start.row { start.col as usize } else { 0 };
        let to = if row == end.row {
            end.col as usize + 1
        } else {
            usize::MAX
        };
        out.push(slice_columns(line, from, to));
    }
    out.join("\n")
}

/// 按显示列切一行：`[from, to)` 与字符显示区间相交即保留该字符。
fn slice_columns(line: &str, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    for ch in line.chars() {
        let w = grid::char_w(ch);
        if w == 0 {
            // 组合符没有独立列：基字符被选中时随之保留。
            if col > from && col <= to {
                out.push(ch);
            }
            continue;
        }
        if col + w > from && col < to {
            out.push(ch);
        }
        col += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<String> {
        vec!["hello world".into(), "中文双宽行".into(), String::new()]
    }

    #[test]
    fn forward_selection_joins_rows() {
        let got = text_between(&lines(), Point::new(0, 6), Point::new(1, 3));
        // 终点列 3 落在「文」的第二格：该宽字整字计入，后两字不在选区内。
        assert_eq!(got, "world\n中文");
    }

    #[test]
    fn reverse_selection_normalizes() {
        let a = Point::new(1, 3);
        let b = Point::new(0, 6);
        assert_eq!(text_between(&lines(), a, b), "world\n中文");
    }

    #[test]
    fn wide_char_partial_overlap_keeps_whole_glyph() {
        // 「中」占第 0-1 列；起点落在它的第二格也要整字取出。
        let got = text_between(&lines(), Point::new(1, 1), Point::new(1, 1));
        assert_eq!(got, "中");
        // 结束列落在宽字符首格：该字符属于选区。
        let got = text_between(&lines(), Point::new(1, 0), Point::new(1, 2));
        assert_eq!(got, "中文");
    }

    #[test]
    fn empty_row_and_out_of_range_are_safe() {
        assert_eq!(text_between(&lines(), Point::new(2, 0), Point::new(2, 9)), "");
        assert_eq!(text_between(&lines(), Point::new(9, 0), Point::new(9, 9)), "");
    }

    #[test]
    fn single_cell_selection_includes_that_cell() {
        assert_eq!(text_between(&lines(), Point::new(0, 0), Point::new(0, 0)), "h");
    }

    #[test]
    fn drag_requires_anchor_and_tracks_movement() {
        let mut sel = Select::default();
        assert!(!sel.drag(Point::new(1, 1)), "杂散拖动不得建区");
        sel.begin(Point::new(0, 0));
        assert!(sel.active());
        sel.drag(Point::new(0, 1));
        assert!(sel.moved());
        assert_eq!(sel.points(), Some((Point::new(0, 0), Point::new(0, 1))));
        sel.clear();
        assert!(!sel.active() && !sel.moved());
    }
}
