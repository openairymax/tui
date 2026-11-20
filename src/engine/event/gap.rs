// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 断点台账（0.1.21 X1-09a，§23.1.5 / §19.1 D-15 判据②）。
//
// `seq::Watch` 只回答「这一帧是不是断点」，不留明细；明细在这里：每一段
// 缺失的帧号区间连同发现时刻记一条，供事后定位"哪几帧没到、什么时候断的"。
// 队列有界（`MAX_GAPS`），满后丢最旧并计数——断点台账是现场取证，不是
// 长期遥测，不能因长时间断网把内存拖垮。
//
// 时刻由调用方给出（毫秒），本模块不读时钟：判定与记录保持纯函数，
// 测试可用固定时刻复现同一段区间。

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;

use super::seq::Verdict;

/// 台账容量：足以覆盖一次长时间断网，又不会随会话时长增长。
pub(crate) const MAX_GAPS: usize = 256;

/// 一段缺失的帧号区间：from..=to 没有到达，发现于 at 毫秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) from: u64,
    pub(crate) to: u64,
    pub(crate) at: u64,
}

impl Gap {
    /// 缺失帧数（真空区间不出台账，故至少 1）。
    pub(crate) fn missed(&self) -> u64 {
        self.to.saturating_sub(self.from).saturating_add(1)
    }
}

/// 有界断点队列：顺序入、顺序出，满则丢最旧。
#[derive(Debug)]
pub(crate) struct Gaps {
    slots: VecDeque<Gap>,
    lost: u64,
}

impl Gaps {
    /// 空台账。
    pub(crate) fn new() -> Self {
        Self {
            slots: VecDeque::with_capacity(MAX_GAPS),
            lost: 0,
        }
    }

    /// 记一条断点事实：只收 gap，顺序帧与重复帧不进台账。
    ///
    /// 重复帧单独计数在 `Watch` 里；台账记的是"区间缺失"，两类事实口径不同，
    /// 混在一起会让 dropped 的统计失去意义。
    pub(crate) fn note(&mut self, verdict: Verdict, at: u64) -> bool {
        let (from, to) = match verdict {
            Verdict::Gap { from, to } => (from, to),
            _ => return false,
        };
        if self.slots.len() >= MAX_GAPS {
            self.slots.pop_front();
            self.lost = self.lost.saturating_add(1);
        }
        self.slots.push_back(Gap { from, to, at });
        true
    }

    /// 最早一段（定位断点起点用）。
    pub(crate) fn oldest(&self) -> Option<&Gap> {
        self.slots.front()
    }

    /// 最近一段（状态栏与告警用）。
    pub(crate) fn latest(&self) -> Option<&Gap> {
        self.slots.back()
    }

    /// 现存条数。
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否无断点。
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 因容量丢掉的条数：非零说明台账已被截断，定位须查审计落盘。
    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }

    /// 缺失帧总数：现存区间之和，不含已丢弃部分。
    pub(crate) fn missed(&self) -> u64 {
        self.slots.iter().map(Gap::missed).sum()
    }
}

impl Default for Gaps {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Gaps, MAX_GAPS};
    use crate::engine::event::seq::{Tracker, Verdict};

    fn gap(from: u64, to: u64) -> Verdict {
        Verdict::Gap { from, to }
    }

    /// 只收区间缺失；顺序帧与重复帧不占台账。
    #[test]
    fn notes_gaps_only() {
        let mut g = Gaps::new();
        assert!(!g.note(Verdict::Ok, 1));
        assert!(!g.note(Verdict::Repeat { seq: 4 }, 2));
        assert!(g.note(gap(2, 3), 10));
        assert_eq!(g.len(), 1);
        assert_eq!(g.latest().map(|x| (x.from, x.to, x.at)), Some((2, 3, 10)));
        assert_eq!(g.missed(), 2);
    }

    /// 与断言器直连：整条流的断点按发生顺序落账。
    #[test]
    fn records_in_stream_order() {
        let mut t = Tracker::new();
        let mut g = Gaps::new();
        for (seq, at) in [(1u64, 100u64), (4, 130), (5, 140), (9, 170)] {
            g.note(t.accept(seq), at);
        }
        assert_eq!(g.len(), 2);
        assert_eq!(g.oldest().map(|x| (x.from, x.at)), Some((2, 130)));
        assert_eq!(g.latest().map(|x| (x.from, x.to)), Some((6, 8)));
        assert_eq!(g.missed(), 5);
    }

    /// 满后丢最旧并计数：内存不随断网时长增长。
    #[test]
    fn bounded_queue_drops_oldest() {
        let mut g = Gaps::new();
        for i in 0..MAX_GAPS as u64 + 3 {
            g.note(gap(i, i), i);
        }
        assert_eq!(g.len(), MAX_GAPS);
        assert_eq!(g.lost(), 3);
        assert_eq!(g.oldest().map(|x| x.from), Some(3));
        assert_eq!(g.latest().map(|x| x.from), Some(MAX_GAPS as u64 + 2));
    }

    /// 真空区间不计入缺失：from > to 只会出现在构造错误，饱和为 1 而不下溢。
    #[test]
    fn missed_never_underflows() {
        let mut g = Gaps::new();
        g.note(gap(7, 3), 1);
        assert_eq!(g.missed(), 1);
        assert!(Gaps::default().is_empty());
    }
}
