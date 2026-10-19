// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L1 块索引（0.1.19 TUI 建设方案 §4.2 引擎化）：把「每个回合块在整屏中的
// 起始行与高度」从对话面板里抽出为纯数据索引，视口裁剪只依赖此索引。
//
// 此前 view.rs 在 layout 内手工维护 offsets/spans 两个平行向量，并在收尾
// 处散落「与窗口相交的消息区间」与「片段按窗口截取」两段内联循环。两段
// 逻辑都只吃行高、不吃内容，却与消息物化耦合在同一函数里，宽度/折叠态
// 失效路径难以独立验证。本模块把这两段收为 `Index` 与 `slice_out`：
// 引擎裁决「该显示哪几行」，业务只负责把相交块物化。
//
// 纯函数：无 IO、无时钟、无全局状态，也不引用 ratatui——可直接单测。

/// 块索引：各块起始行 + 行高，前缀和一次建好。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Index {
    starts: Vec<usize>,
    spans: Vec<u32>,
    end: usize,
}

impl Index {
    /// 以 `head`（头部已物化行数）为起点，按块行高序列建索引。
    pub(crate) fn build(heights: impl IntoIterator<Item = u32>, head: usize) -> Self {
        let spans: Vec<u32> = heights.into_iter().collect();
        let mut starts = Vec::with_capacity(spans.len());
        let mut cursor = head;
        for span in &spans {
            starts.push(cursor);
            cursor += *span as usize;
        }
        Self {
            starts,
            spans,
            end: cursor,
        }
    }

    /// 块数。
    pub(crate) fn len(&self) -> usize {
        self.starts.len()
    }

    /// 第 `idx` 块的起始行。
    pub(crate) fn start(&self, idx: usize) -> usize {
        self.starts[idx]
    }

    /// 全部块结束后的首行（正文终点）。
    pub(crate) fn end(&self) -> usize {
        self.end
    }

    /// 与行窗口 `[w0, w1)` 相交的块下标区间 `[first, end)`；无相交时返回空区间。
    pub(crate) fn visible(&self, w0: usize, w1: usize) -> (usize, usize) {
        let mut first = self.len();
        let mut end = self.len();
        for idx in 0..self.len() {
            let start = self.starts[idx];
            if start >= w1 {
                break;
            }
            if start + self.spans[idx] as usize <= w0 {
                continue;
            }
            if first == self.len() {
                first = idx;
            }
            end = idx + 1;
        }
        (first, end)
    }

    /// 块行高序列（供投影预算按块裁量）。
    pub(crate) fn spans(&self) -> &[u32] {
        &self.spans
    }
}

/// 截取片段 `seg`（全局起始行 = `seg_start`）与窗口 `[w0, w1)` 相交部分追加。
pub(crate) fn slice_out<T: Clone>(
    out: &mut Vec<T>,
    seg: &[T],
    seg_start: usize,
    w0: usize,
    w1: usize,
) {
    let s = w0.saturating_sub(seg_start);
    let e = w1.saturating_sub(seg_start).min(seg.len());
    if s < e {
        out.extend_from_slice(&seg[s..e]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 索引按行高前缀和累加，起点自头部行数起算。
    #[test]
    fn build_accumulates_from_head() {
        let idx = Index::build([2, 3, 1], 4);
        assert_eq!(idx.len(), 3);
        assert_eq!(idx.start(0), 4);
        assert_eq!(idx.start(1), 6);
        assert_eq!(idx.start(2), 9);
        assert_eq!(idx.end(), 10);
        assert_eq!(idx.spans(), &[2, 3, 1]);
    }

    /// 窗口落在块中部：首尾块都算相交。
    #[test]
    fn visible_includes_partial_edges() {
        let idx = Index::build([2, 3, 1], 0);
        assert_eq!(idx.visible(1, 4), (0, 2));
        assert_eq!(idx.visible(2, 6), (1, 3));
    }

    /// 窗口完全在正文之外（上方或下方）时返回空区间。
    #[test]
    fn visible_outside_is_empty() {
        let idx = Index::build([2, 3], 5);
        assert_eq!(idx.visible(0, 5), (2, 2));
        assert_eq!(idx.visible(10, 20), (2, 2));
    }

    /// 零高块不阻断窗口：可见性由行区间判定，与块数无关。
    #[test]
    fn zero_height_blocks_do_not_hide() {
        let idx = Index::build([0, 0, 1], 0);
        assert_eq!(idx.visible(0, 1), (2, 3));
    }

    /// 片段截取：两端按窗口裁剪，完全在窗口外时不追加。
    #[test]
    fn slice_out_clips_both_edges() {
        let seg = [10, 11, 12, 13];
        let mut out = Vec::new();
        slice_out(&mut out, &seg, 5, 6, 8);
        assert_eq!(out, vec![11, 12]);
        out.clear();
        slice_out(&mut out, &seg, 5, 0, 5);
        assert!(out.is_empty());
        slice_out(&mut out, &seg, 5, 9, 20);
        assert!(out.is_empty());
    }
}
