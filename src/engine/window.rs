// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 投影窗口（0.1.19 TUI 建设方案 §5.3 投影窗口域、§6.10 #2）：把整段历史
// 收窄为本帧允许物化的消息区间。
//
// 三界：
//   - MAX_INITIAL_TURNS：冷启动且视口贴底时只投影最近 30 条，首帧不等历史；
//   - MAX_PROJECTED_TURNS：单帧物化消息条数上限 60；
//   - MAX_PROJECTED_CELLS：单帧物化行数上限 1,000。
//
// 可视区间 [first, last] 永远优先保留；预算有余先向更早的消息扩张（补阅读
// 方向的上文），再向更新的消息扩张。超界时按阅读顺序先裁头部、再裁尾部。
// 行高记账仍按全量精确进行（缓存命中 O(1)），窗口只约束 cell 物化——总行数
// 与滚动位置不受窗口影响。
//
// 本模块是纯函数：无 IO、无时钟、无全局状态，边界行为可由虚拟输入确定性
// 重放。

use crate::budget::{
    MAX_CELLS_PER_TURN, MAX_INITIAL_TURNS, MAX_PROJECTED_CELLS, MAX_PROJECTED_TURNS,
};

/// 投影区间：`[start, end)` 为消息下标，闭区间语义由调用方换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    fn of(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// 计算本帧投影窗口。`heights` 为每条消息块行数（含回合分隔线）。
///
/// `initial` 只在行高缓存冷启动且视口贴底时为真；此时窗口起点抬到最近
/// `MAX_INITIAL_TURNS` 条，但不越过可视首条——视口本身就已覆盖更多消息时
/// 以可视区间为准，避免首帧缺行、次帧跳变。
pub(crate) fn project(
    total: usize,
    first: usize,
    last: usize,
    heights: &[u32],
    initial: bool,
) -> Span {
    if total == 0 || heights.len() < total {
        return Span::of(0, 0);
    }
    let first = first.min(total - 1);
    let last = last.max(first).min(total - 1);
    let mut start = first;
    let mut end = last + 1;

    if initial {
        let floor = total.saturating_sub(MAX_INITIAL_TURNS);
        if floor <= first {
            start = floor;
        }
    }

    let mut cells: usize = heights[start..end].iter().map(|&h| h as usize).sum();
    let mut rounds = end - start;

    // 头部只裁预算外扩张的部分，可视首条之后一律不动
    while (rounds > MAX_PROJECTED_TURNS || cells > MAX_PROJECTED_CELLS) && start < first {
        cells -= heights[start] as usize;
        start += 1;
        rounds -= 1;
    }
    // 可视区间自身超界时按阅读顺序裁尾，渲染树规模恒有界
    while (rounds > MAX_PROJECTED_TURNS || cells > MAX_PROJECTED_CELLS) && end > first + 1 {
        end -= 1;
        cells -= heights[end] as usize;
        rounds -= 1;
    }

    if initial {
        return Span::of(start, end);
    }

    while start > 0 && end - start < MAX_PROJECTED_TURNS {
        let h = heights[start - 1] as usize;
        if cells + h > MAX_PROJECTED_CELLS {
            break;
        }
        start -= 1;
        cells += h;
    }
    while end < total && end - start < MAX_PROJECTED_TURNS {
        let h = heights[end] as usize;
        if cells + h > MAX_PROJECTED_CELLS {
            break;
        }
        end += 1;
        cells += h;
    }
    Span::of(start, end)
}

/// 单轮 cell 钳位标记行文案（`clipped` 为被裁剪的原始行数）。
pub(crate) fn clip_marker(clipped: usize) -> String {
    format!("… 已裁剪 {clipped} 行 · 单轮上限 {MAX_CELLS_PER_TURN} 行 · Alt+O 展开")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_history_yields_empty_span() {
        assert_eq!(project(0, 0, 0, &[], false), Span::of(0, 0));
        assert_eq!(
            project(4, 0, 3, &[1, 1], false),
            Span::of(0, 0),
            "行高缺失不猜"
        );
    }

    /// 可视区间永远在窗口内，且预算内向前补上下文。
    #[test]
    fn visible_range_is_always_kept() {
        let heights = vec![1u32; 200];
        let span = project(200, 100, 110, &heights, false);
        assert!(
            span.start <= 100 && span.end >= 111,
            "可视区间被裁: {span:?}"
        );
        assert!(span.len() <= MAX_PROJECTED_TURNS);
        assert_eq!(span.end, 111, "不向未读方向扩张");
    }

    /// 冷启动贴底：只投影最近 30 条（起点抬到 total - 30）。
    #[test]
    fn initial_frame_caps_recent_turns() {
        let heights = vec![1u32; 100];
        let span = project(100, 95, 99, &heights, true);
        assert_eq!(
            span,
            Span::of(100 - MAX_INITIAL_TURNS, 100),
            "首帧应恰为最近 30 条"
        );
    }

    /// 冷启动但视口本身已覆盖超过 30 条：以可视区间为准，不得缺行。
    #[test]
    fn initial_frame_never_drops_visible() {
        let heights = vec![1u32; 100];
        let span = project(100, 10, 12, &heights, true);
        assert_eq!(span, Span::of(10, 13));
    }

    /// 回合数上限：可视 11 条时向前补到 60 条即停。
    #[test]
    fn turn_budget_caps_projection() {
        let heights = vec![1u32; 200];
        let span = project(200, 100, 110, &heights, false);
        assert_eq!(span, Span::of(111 - MAX_PROJECTED_TURNS, 111));
    }

    /// cell 上限：每条 100 行时最多投影 10 条。
    #[test]
    fn cell_budget_caps_projection() {
        let heights = vec![100u32; 100];
        let span = project(100, 50, 51, &heights, false);
        assert_eq!(span, Span::of(42, 52));
        assert_eq!(span.len() * 100, MAX_PROJECTED_CELLS);
    }

    /// 可视区间自身超界（超高视口）：按阅读顺序裁尾，双界同时满足。
    #[test]
    fn oversized_visible_range_is_clipped() {
        let heights = vec![100u32; 200];
        let span = project(200, 0, 199, &heights, false);
        assert!(span.len() <= MAX_PROJECTED_TURNS, "回合界: {span:?}");
        assert!(span.len() * 100 <= MAX_PROJECTED_CELLS, "cell 界: {span:?}");
        assert_eq!(span.start, 0, "裁尾不裁头");
    }
}
