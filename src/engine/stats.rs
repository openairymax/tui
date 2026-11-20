// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧耗时分位聚合（0.1.21 X8-06a，§23.9 #10 帧预算口径）。
//
// 输入是环形缓冲交出的原始单帧样本，输出是六段与整帧各自的 p50/p95/峰值。
// 聚合只做「排序 + 取位」，没有任何平滑、插值或跨窗口状态：同一批样本无论
// 顺序如何、分几次送入，结论都相同，因此判定可以离线用留存样本复算。
//
// 分位口径为最近秩（nearest-rank）：升序后取第 ceil(p·n/100) 个样本（秩从
// 1 起），n 个样本时 p50 即「至少一半样本不超过它」的最小值。整数运算、
// 无浮点，跨平台一致；样本数很少时 p95 直接落到峰值，不做区间外推。
//
// 未采样的段不参与聚合：该段结论为「无样本」而非 0。整帧口径同理，只统计
// 至少有一段已采样的帧——全空样本不是「0 耗时帧」，把它计入会拉低分位。
//
// 分段落盘与 golden 比对归 X8-06b；在落盘接线前，逐段查询暂无消费者，按
// exit.rs 与 frame.rs 的同例放宽 dead_code。

#![cfg_attr(not(test), allow(dead_code))]

use super::frame::{Costs, Stage};

/// 中位分位点（百分数）。
const P50: usize = 50;
/// 帧预算判据分位点（§23.9 #10：p95 ≤ 16ms）。
const P95: usize = 95;

/// 一组样本的分位结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) p50: u64,
    pub(crate) p95: u64,
    pub(crate) peak: u64,
    pub(crate) n: usize,
}

/// 一次聚合的完整结论：六段各自的分位（无样本为 `None`）与整帧分位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameStats {
    stages: [Option<Summary>; Stage::N],
    whole: Option<Summary>,
}

impl FrameStats {
    /// 指定段的分位结论；该段在窗口内从未被采样时为 `None`。
    pub(crate) fn stage(&self, stage: Stage) -> Option<Summary> {
        self.stages[stage.index()]
    }

    /// 整帧分位结论；窗口内没有任何已采样帧时为 `None`。
    pub(crate) fn whole(&self) -> Option<Summary> {
        self.whole
    }
}

/// 聚合一批单帧样本（顺序无关，可分批后合并使用同一口径离线复算）。
pub(crate) fn aggregate(samples: impl IntoIterator<Item = Costs>) -> FrameStats {
    let mut per: [Vec<u64>; Stage::N] = std::array::from_fn(|_| Vec::new());
    let mut whole: Vec<u64> = Vec::new();
    for costs in samples {
        if costs.sampled() == 0 {
            continue;
        }
        for stage in Stage::ALL {
            if let Some(us) = costs.get(stage) {
                per[stage.index()].push(us);
            }
        }
        whole.push(costs.total());
    }
    let mut stages: [Option<Summary>; Stage::N] = [None; Stage::N];
    for stage in Stage::ALL {
        stages[stage.index()] = summarize(&mut per[stage.index()]);
    }
    FrameStats {
        stages,
        whole: summarize(&mut whole),
    }
}

/// 升序排序后取分位；空样本不出结论。
fn summarize(values: &mut Vec<u64>) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let n = values.len();
    Some(Summary {
        p50: quantile(values, P50),
        p95: quantile(values, P95),
        peak: values[n - 1],
        n,
    })
}

/// 最近秩分位：秩 = ceil(p·n/100)，至少为 1（样本非空）。
fn quantile(sorted: &[u64], p: usize) -> u64 {
    let n = sorted.len();
    let rank = ((p * n).saturating_add(99) / 100).max(1);
    sorted[rank.min(n) - 1]
}

#[cfg(test)]
mod tests {
    use super::{aggregate, quantile, FrameStats, Summary};
    use crate::engine::frame::{Costs, Stage};

    /// 构造单帧：给 compose 与 present 两段赋值，其余留空（当前生产口径）。
    fn frame(compose: u64, present: u64) -> Costs {
        let mut costs = Costs::new();
        costs.set(Stage::Compose, compose);
        costs.set(Stage::Present, present);
        costs
    }

    /// 全空样本不是 0 耗时帧：不参与整帧聚合，也不产生任何段的结论。
    #[test]
    fn empty_input_yields_no_summary() {
        let stats = aggregate(std::iter::empty());
        for stage in Stage::ALL {
            assert_eq!(stats.stage(stage), None, "{} 无样本", stage.name());
        }
        assert_eq!(stats.whole(), None);

        let stats = aggregate([Costs::new()]);
        assert_eq!(stats.whole(), None, "全空样本不得计入整帧");
        assert_eq!(stats.stage(Stage::Compose), None);
    }

    /// 单样本：三个分位点都落在该样本上，计数为 1。
    #[test]
    fn single_sample_reports_its_own_value() {
        let stats = aggregate([frame(120, 300)]);
        let compose = stats.stage(Stage::Compose).expect("compose 有样本");
        assert_eq!(
            compose,
            Summary {
                p50: 120,
                p95: 120,
                peak: 120,
                n: 1
            }
        );
        let whole = stats.whole().expect("整帧有样本");
        assert_eq!(whole, Summary {
            p50: 420,
            p95: 420,
            peak: 420,
            n: 1
        });
        assert_eq!(stats.stage(Stage::Diff), None, "未采样段不出结论");
    }

    /// 最近秩口径：n=100 时 p50 取第 50 个、p95 取第 95 个。
    #[test]
    fn percentiles_follow_nearest_rank() {
        let samples: Vec<Costs> = (1..=100).map(|us| frame(us, 0)).collect();
        let compose = aggregate(samples).stage(Stage::Compose).expect("有样本");
        assert_eq!((compose.p50, compose.p95, compose.peak), (50, 95, 100));
        assert_eq!(compose.n, 100);
    }

    /// 奇偶样本数的取位规则一并锁定：偶数取偏低一秩，奇数按四舍五入上取。
    #[test]
    fn quantile_rank_is_locked() {
        assert_eq!(quantile(&[10, 20], 50), 10);
        assert_eq!(quantile(&[10, 20], 95), 20);
        assert_eq!(quantile(&[10, 20, 30], 50), 20);
        assert_eq!(quantile(&(1..=101).collect::<Vec<u64>>(), 50), 51);
        assert_eq!(quantile(&[7], 95), 7);
    }

    /// 交付顺序不影响结论：倒序输入与正序输入必须得到同一份汇总。
    #[test]
    fn aggregation_is_order_independent() {
        let samples: Vec<Costs> = (0..1_024)
            .map(|i| frame(100 + i % 37, 200 + i % 53))
            .collect();
        let forward: FrameStats = aggregate(samples.iter().copied());
        let backward: FrameStats = aggregate(samples.iter().rev().copied());
        assert_eq!(forward, backward, "离线复算须与在线聚合逐字段一致");
    }

    /// 整帧峰值是「最慢的那一帧」，不是各段峰值之和。
    #[test]
    fn whole_peak_is_slowest_frame() {
        let stats = aggregate([frame(100, 200), frame(300, 100)]);
        let whole = stats.whole().expect("有样本");
        assert_eq!(whole.peak, 400);
        assert_eq!(whole.p50, 300, "两帧时 p50 取偏低秩");
        assert_eq!(whole.n, 2);
    }

    /// 段计数各自独立：缺段只影响该段，不影响其它段与整帧的样本数。
    #[test]
    fn stage_counts_are_independent() {
        let mut only_compose = Costs::new();
        only_compose.set(Stage::Compose, 90);
        let stats = aggregate([only_compose, frame(110, 250)]);
        assert_eq!(stats.stage(Stage::Compose).expect("compose").n, 2);
        assert_eq!(stats.stage(Stage::Present).expect("present").n, 1);
        assert_eq!(stats.whole().expect("整帧").n, 2, "空样本不拉低计数");
    }
}
