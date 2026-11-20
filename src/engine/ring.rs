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
// 槽内序号是「这一格当前装的是第几帧」，读侧据此判断哪些格子已写过、哪些
// 已被覆盖；写侧不读它，故本卡只落写入路径与覆盖语义，快照 API 归 X8-05b，
// 分位聚合归 X8-06a，落盘归 X8-06b。
//
// 序号与下标不随覆盖回退：计数只增（到 u64 上界后饱和，60fps 下约 97 亿年，
// 不作为可运行边界），覆盖只改槽内容，不改槽数量——这正是「不增长内存」
// 的结构前提。
//
// 读侧尚未落地，槽内序号与长度查询在非测试构建下暂无消费者；按 exit.rs 与
// frame.rs 的同例放宽 dead_code，快照 API 接线后此注记随之删除。

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
}

#[cfg(test)]
mod tests {
    use super::{Ring, CAP};
    use crate::engine::frame::{Costs, Stage};

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
}
