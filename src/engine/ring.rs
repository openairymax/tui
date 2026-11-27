// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧指标环形缓冲写侧（0.1.21 X8-05a，§23.9 #10 帧预算口径）。
//
// 逐帧留存最近 1,024 帧的六段耗时，槽位定长、写满即覆盖最旧一帧：容量恒
// 定，采样开销与运行时长无关，长期驻留不会把指标本身变成内存泄漏。容量取
// 2 的幂，下标由写入计数按位与导出，写入路径无分支、无分配。
//
// 读侧（X8-05b）以快照迭代器暴露最近窗口，从最旧到最新逐帧交出 `Costs`：
// 迭代器借用环本身、按下标推进，不复制缓冲、不分配内存，调用频率与帧率无关
// 地保持零增长。槽内序号是「这一格当前装的是第几帧」，供读侧区分已写槽与
// 空槽并验证窗口连续。
//
// 序号与下标不随覆盖回退：计数只增（到 u64 上界后饱和，60fps 下约 97 亿年，
// 不作为可运行边界），覆盖只改槽内容，不改槽数量——这正是「不增长内存」
// 的结构前提。
//
// 分位聚合归 X8-06a，落盘归 X8-06b；在聚合器接线前，长度查询与写入计数在
// 非测试构建下暂无消费者，按 exit.rs 与 frame.rs 的同例放宽 dead_code。

#![cfg_attr(not(test), allow(dead_code))]

use super::frame::Costs;

/// 环容量：2 的幂，掩码即取模；4KB 页对齐后约 56KiB 常驻。
pub(crate) const CAP: usize = 1_024;

/// 单槽：写入序号（从 1 起，0 表示从未写入）与该帧耗时。
#[derive(Debug, Clone, Copy)]
struct Slot {
    seq: u64,
    costs: Costs,
}

const EMPTY: Slot = Slot {
    seq: 0,
    costs: Costs::new(),
};

/// 帧指标环形缓冲（写侧）。
#[derive(Debug)]
pub(crate) struct Ring {
    slots: Box<[Slot; CAP]>,
    /// 累计写入帧数，同时是下一槽的下标来源。
    written: u64,
}

impl Ring {
    pub(crate) fn new() -> Self {
        Self {
            slots: Box::new([EMPTY; CAP]),
            written: 0,
        }
    }

    /// 留存一帧：写入下一槽，写满后覆盖最旧一帧。
    pub(crate) fn push(&mut self, costs: Costs) {
        let idx = (self.written as usize) & (CAP - 1);
        self.slots[idx] = Slot {
            seq: self.written.saturating_add(1),
            costs,
        };
        self.written = self.written.saturating_add(1);
    }

    /// 当前可读帧数：写满前等于已写帧数，之后恒为容量。
    pub(crate) fn len(&self) -> usize {
        self.written.min(CAP as u64) as usize
    }

    /// 容量恒定，不随写入增长。
    pub(crate) fn capacity(&self) -> usize {
        CAP
    }

    /// 累计写入帧数（覆盖不回退）。
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// 最近窗口快照：最旧在前、最新在后，至多 [`CAP`] 帧。
    ///
    /// 迭代器只借用槽位、按下标推进，不复制缓冲也不分配；重复取快照不会
    /// 改变环内容，随时可与 `len()` 对照（两者恒一致）。
    pub(crate) fn snapshot(&self) -> impl Iterator<Item = Costs> + '_ {
        let count = self.len();
        let first = self.written.saturating_sub(count as u64);
        (0..count).map(move |i| {
            let idx = ((first + i as u64) as usize) & (CAP - 1);
            self.slots[idx].costs
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Ring, CAP};
    use crate::engine::frame::{Costs, Stage};

    /// 快照按帧号取出 compose 段，便于断言窗口内容。
    fn marks(ring: &Ring) -> Vec<u64> {
        ring.snapshot()
            .map(|costs| costs.get(Stage::Compose).unwrap_or(0))
            .collect()
    }

    /// 构造可区分的单帧耗时：把帧号编进 compose 段。
    fn cost(mark: u64) -> Costs {
        let mut costs = Costs::new();
        costs.set(Stage::Compose, mark);
        costs
    }

    /// 容量是 2 的幂且槽位数固定：掩码推导下标的前提。
    #[test]
    fn capacity_is_power_of_two() {
        assert!(CAP.is_power_of_two());
        assert_eq!(Ring::new().capacity(), CAP);
    }

    /// 空环没有可读帧，任何一槽都未被写过。
    #[test]
    fn new_ring_is_empty() {
        let ring = Ring::new();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.written(), 0);
        assert!(ring.slots.iter().all(|slot| slot.seq == 0));
    }

    /// 未写满时逐帧累积，序号从 1 起连续且与槽位一一对应。
    #[test]
    fn write_side_fills_slots_in_order() {
        let mut ring = Ring::new();
        for i in 1..=CAP as u64 {
            ring.push(cost(i));
            assert_eq!(ring.len(), i as usize);
        }
        assert_eq!(ring.written(), CAP as u64);
        for (i, slot) in ring.slots.iter().enumerate() {
            assert_eq!(slot.seq, i as u64 + 1, "槽 {i} 序号须连续");
            assert_eq!(slot.costs.get(Stage::Compose), Some(i as u64 + 1));
        }
    }

    /// 覆盖写：第 CAP+1 帧落在最旧槽，旧内容被整帧替换。
    #[test]
    fn overflow_overwrites_oldest_slot() {
        let mut ring = Ring::new();
        for i in 1..=CAP as u64 + 1 {
            ring.push(cost(i));
        }
        assert_eq!(ring.len(), CAP, "写满后长度恒为容量");
        assert_eq!(ring.slots[0].seq, CAP as u64 + 1, "第 0 槽被最新帧覆盖");
        assert_eq!(
            ring.slots[0].costs.get(Stage::Compose),
            Some(CAP as u64 + 1)
        );
        assert_eq!(ring.slots[1].seq, 2, "其余槽位不被牵连");
    }

    /// 连续覆盖多轮后，槽内序号仍是最近 CAP 帧的连续区间（无空洞、无乱序）。
    #[test]
    fn repeated_overwrites_keep_window_contiguous() {
        let mut ring = Ring::new();
        let rounds = 3;
        let total = CAP as u64 * rounds;
        for i in 1..=total {
            ring.push(cost(i));
        }
        assert_eq!(ring.len(), CAP);
        assert_eq!(ring.written(), total, "写入计数不随覆盖回退");
        let first = total - CAP as u64 + 1;
        for n in first..=total {
            let i = (n - 1) as usize % CAP;
            assert_eq!(ring.slots[i].seq, n, "槽 {i} 须装第 {n} 帧");
        }
        let mut seqs: Vec<u64> = ring.slots.iter().map(|slot| slot.seq).collect();
        seqs.sort_unstable();
        let expect: Vec<u64> = (first..=total).collect();
        assert_eq!(seqs, expect, "最近 CAP 帧必须完整保留");
    }

    /// 容量与内存占用不随写入轮数变化：写 10 轮后仍是 1,024 槽。
    #[test]
    fn capacity_stays_constant_after_many_rounds() {
        let mut ring = Ring::new();
        for i in 0..CAP as u64 * 10 {
            ring.push(cost(i));
        }
        assert_eq!(ring.capacity(), CAP);
        assert_eq!(ring.len(), CAP);
        assert_eq!(ring.slots.len(), CAP);
    }

    /// 未写入时快照为空：读侧不必为「还没成帧」准备特例。
    #[test]
    fn snapshot_is_empty_before_first_frame() {
        let ring = Ring::new();
        assert_eq!(ring.snapshot().count(), 0);
    }

    /// 快照从最旧到最新：聚合器与落盘按时间顺序消费，无需自行排序。
    #[test]
    fn snapshot_reads_oldest_first() {
        let mut ring = Ring::new();
        for i in 1..=5 {
            ring.push(cost(i));
        }
        assert_eq!(marks(&ring), [1, 2, 3, 4, 5]);
    }

    /// 溢出后窗口整体前移：长度封顶，首帧为最早一帧，末帧为最新一帧。
    #[test]
    fn snapshot_window_slides_after_overflow() {
        let mut ring = Ring::new();
        let total = CAP as u64 + 3;
        for i in 1..=total {
            ring.push(cost(i));
        }
        let marks = marks(&ring);
        assert_eq!(marks.len(), CAP);
        assert_eq!(marks.first().copied(), Some(total - CAP as u64 + 1));
        assert_eq!(marks.last().copied(), Some(total));
        assert!(
            marks.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "窗口须严格递增，覆盖不得造成乱序"
        );
    }

    /// 读快照是只读操作：反复读取不改写计数、不改槽区字节数。
    #[test]
    fn snapshot_is_read_only() {
        let mut ring = Ring::new();
        for i in 1..=CAP as u64 * 4 {
            ring.push(cost(i));
        }
        let footprint = std::mem::size_of_val(&*ring.slots);
        let written = ring.written();
        for _ in 0..32 {
            let samples: Vec<Costs> = ring.snapshot().collect();
            assert_eq!(samples.len(), ring.len());
        }
        assert_eq!(ring.written(), written);
        assert_eq!(std::mem::size_of_val(&*ring.slots), footprint);
    }

    /// 不增长内存：写满百万帧后槽区字节数仍与首帧时相同，快照长度恒为容量。
    #[test]
    fn footprint_stays_fixed_over_million_frames() {
        let mut ring = Ring::new();
        ring.push(cost(1));
        let base = std::mem::size_of_val(&*ring.slots);
        for i in 2..=1_000_000 {
            ring.push(cost(i));
        }
        assert_eq!(std::mem::size_of_val(&*ring.slots), base, "槽区不得扩张");
        assert_eq!(ring.snapshot().count(), CAP, "快照长度恒为容量");
        assert_eq!(ring.len(), CAP);
    }

    /// 档位随槽留存：读侧拿到的就是该帧写出档位（X2-30b 指标联动）。
    #[test]
    fn snapshot_carries_ladder() {
        let mut ring = Ring::new();
        let mut leveled = cost(1);
        leveled.set_ladder(2);
        ring.push(leveled);
        ring.push(cost(2));
        let samples: Vec<u64> = ring.snapshot().map(|costs| costs.ladder()).collect();
        assert_eq!(samples, [2, 0], "默认帧档位为 0，降级帧按实际档位留存");
    }
}
