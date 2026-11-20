// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 脏区间（0.1.21 X1-12a，§23.1.5 增量渲染的写入口）。
//
// 消息表只增不改，所以"哪里变了"永远是一串行号区间，而不是逐行布尔。
// 相邻或重叠的标记合并成一段：流式增量每帧追加一行，若不合并，一次回答
// 就会留下上千个单行区间，渲染器逐段遍历的开销反而超过整屏重绘。
//
// 区间语义是半开 [from, to)：行号从 0 起，空区间不入账。合并只发生在尾部
// ——append-only 表的行号只增不减，旧区间之后的标记不可能穿回前面，因此
// 不需要有序插入或重排。

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;
use std::ops::Range;

/// 半开行号区间 [from, to)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) from: usize,
    pub(crate) to: usize,
}

impl Span {
    /// 区间行数（空区间为 0）。
    pub(crate) fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }
}

/// 脏区间队列：尾部合并，按产生顺序消费。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Dirty {
    spans: VecDeque<Span>,
}

impl Dirty {
    /// 空队列。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 标记一行脏。
    pub(crate) fn mark(&mut self, row: usize) {
        self.mark_span(row..row.saturating_add(1));
    }

    /// 标记一段脏；空段与逆序段直接忽略。
    pub(crate) fn mark_span(&mut self, rows: Range<usize>) {
        if rows.start >= rows.end {
            return;
        }
        if let Some(last) = self.spans.back_mut() {
            if rows.start <= last.to {
                last.to = last.to.max(rows.end);
                return;
            }
        }
        self.spans.push_back(Span {
            from: rows.start,
            to: rows.end,
        });
    }

    /// 是否没有脏区。
    pub(crate) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// 区间段数（诊断用；不是行数）。
    pub(crate) fn segs(&self) -> usize {
        self.spans.len()
    }

    /// 脏行总数。
    pub(crate) fn count(&self) -> usize {
        self.spans.iter().map(Span::len).sum()
    }

    /// 只读枚举：按段遍历当前脏区。
    pub(crate) fn spans(&self) -> impl Iterator<Item = Span> + '_ {
        self.spans.iter().copied()
    }

    /// 查询某行是否脏（自检与用例定位用，渲染走 `take`）。
    pub(crate) fn covers(&self, row: usize) -> bool {
        self.spans.iter().any(|s| s.from <= row && row < s.to)
    }

    /// 取走全部脏区，队列清空（渲染器消费一次）。
    pub(crate) fn take(&mut self) -> Vec<Span> {
        self.spans.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Dirty;

    /// 单行标记按尾部合并：连续追加只留一段。
    #[test]
    fn merges_adjacent_marks() {
        let mut d = Dirty::new();
        for row in 0..5 {
            d.mark(row);
        }
        assert_eq!(d.segs(), 1);
        assert_eq!(d.count(), 5);
        let spans = d.take();
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].from, spans[0].to), (0, 5));
        assert!(d.is_empty());
    }

    /// 不相邻的标记各自成段，顺序保持产生顺序。
    #[test]
    fn keeps_disjoint_spans() {
        let mut d = Dirty::new();
        d.mark(2);
        d.mark(7);
        d.mark_span(10..13);
        assert_eq!(d.segs(), 3);
        assert_eq!(d.count(), 5);
        let got: Vec<(usize, usize)> = d.take().iter().map(|s| (s.from, s.to)).collect();
        assert_eq!(got, vec![(2, 3), (7, 8), (10, 13)]);
    }

    /// 尾部重叠合并取并集；空段与逆序段不入账。
    #[test]
    fn merges_overlap_only() {
        let mut d = Dirty::new();
        d.mark_span(1..4);
        d.mark_span(3..6);
        d.mark_span(6..6);
        d.mark_span(9..8);
        assert_eq!(d.segs(), 1);
        assert_eq!(d.count(), 5);
        assert_eq!(d.take()[0].to, 6);
    }

    /// 枚举接口按段给出全部脏区，且不消费队列。
    #[test]
    fn enumerates_segments() {
        let mut d = Dirty::new();
        d.mark_span(0..2);
        d.mark(5);
        d.mark_span(8..9);
        let got: Vec<(usize, usize)> = d.spans().map(|s| (s.from, s.to)).collect();
        assert_eq!(got, vec![(0, 2), (5, 6), (8, 9)]);
        assert_eq!(d.segs(), 3, "枚举不得消费");
    }

    /// 行命中查询：区间内为真，边界半开，区间外为假。
    #[test]
    fn covers_rows() {
        let mut d = Dirty::new();
        d.mark_span(2..4);
        assert!(!d.covers(1));
        assert!(d.covers(2));
        assert!(d.covers(3));
        assert!(!d.covers(4), "半开区间不含右端");
        assert!(!d.covers(99));
    }
}
