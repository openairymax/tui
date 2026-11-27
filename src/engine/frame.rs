// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 单帧渲染的六阶段词汇表（0.1.21 X2-25a，§23.9 #10 帧预算口径）。
//
// 一帧从排版到上屏分六段：compose（排版）→ diff（缓冲差异）→ encode（转义
// 编码）→ write（写出通道）→ flush（冲刷）→ present（换帧提交）。六段之和
// 即整帧耗时，p95 ≤ 16ms 的判据按段归因。名字与下标是跨模块契约：名字即
// JSONL 的键，下标即环形缓冲的列，故集中在此一处定义，杜绝各处自写一套。
//
// 本模块只定义词汇与容器，不取时钟、不做 IO——采样时刻由调用方注入（与 L4
// 同一口径），逐帧留存归 X8-05a 环形缓冲，分位聚合归 X8-06a。
//
// 未采样的段以 `None` 表示，与「耗时 0」严格区分：P2 原子帧后端落地前，
// diff/encode/write/flush 尚无打点，不得以 0 冒充已采集，否则 p95 与按段
// 归因会得到「一切正常」的假象。
//
// 六阶段表是 X8-05a 环形缓冲与 X8-06a 聚合器的跨卡契约（名字即 JSONL 键、
// 下标即环槽列），先于消费者落地；故非测试构建下按 exit.rs 同例放宽
// dead_code，消费者接线后此注记随之删除。

#![cfg_attr(not(test), allow(dead_code))]

use std::time::Duration;

/// 帧内阶段的执行顺序，判别值即列下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Compose,
    Diff,
    Encode,
    Write,
    Flush,
    Present,
}

impl Stage {
    /// 阶段总数：环形缓冲列宽、聚合器遍历次数均由此导出。
    pub(crate) const N: usize = 6;

    /// 定长阶段表：顺序即执行顺序，供遍历与完整性校验。
    pub(crate) const ALL: [Stage; Self::N] = [
        Stage::Compose,
        Stage::Diff,
        Stage::Encode,
        Stage::Write,
        Stage::Flush,
        Stage::Present,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    /// 稳态名字：日志与 JSONL 的键。改名等于改样本格式，须同步 golden。
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Stage::Compose => "compose",
            Stage::Diff => "diff",
            Stage::Encode => "encode",
            Stage::Write => "write",
            Stage::Flush => "flush",
            Stage::Present => "present",
        }
    }
}

/// 阶段耗时汇聚口：采样点把本段耗时交出，由容器决定留存形态。
///
/// 单帧容器（`Costs`）按帧留存，环形缓冲按槽留存；采样点无需感知两者差异。
pub(crate) trait StageSink {
    fn stage_cost(&mut self, stage: Stage, cost: Duration);
}

/// 一帧的六段耗时（μs，`None` = 该段未采样）与写出档位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Costs {
    us: [Option<u64>; Stage::N],
    /// 本帧的写出档位编码（X2-30b）：0 为默认档，取值见 `ladder::Ladder`。
    ///
    /// 它不是耗时，不参与合计与完整性判据；与耗时同槽留存，是因为判读
    /// p95 时若不看该帧由哪一档写出，会把降级帧的「快」误当成优化成果。
    ladder: u8,
}

impl Costs {
    pub(crate) const fn new() -> Self {
        Self {
            us: [None; Stage::N],
            ladder: 0,
        }
    }

    pub(crate) fn set(&mut self, stage: Stage, us: u64) {
        self.us[stage.index()] = Some(us);
    }

    pub(crate) fn get(&self, stage: Stage) -> Option<u64> {
        self.us[stage.index()]
    }

    /// 记录本帧写出档位（X2-30b）：编码由 `Ladder::code` 给出。
    pub(crate) fn set_ladder(&mut self, code: u8) {
        self.ladder = code;
    }

    /// 本帧写出档位编码，0 为默认档。
    pub(crate) fn ladder(&self) -> u8 {
        self.ladder
    }

    /// 整帧耗时：已采样段之和，饱和相加不因异常值回绕。
    pub(crate) fn total(&self) -> u64 {
        self.us
            .iter()
            .flatten()
            .fold(0u64, |sum, us| sum.saturating_add(*us))
    }

    /// 六段齐全：X2-25b 的完整性判据，亦是 X8-06a 按段聚合的前提。
    pub(crate) fn complete(&self) -> bool {
        self.us.iter().all(Option::is_some)
    }

    /// 已采样段数。
    pub(crate) fn sampled(&self) -> usize {
        self.us.iter().filter(|us| us.is_some()).count()
    }
}

impl StageSink for Costs {
    fn stage_cost(&mut self, stage: Stage, cost: Duration) {
        self.set(stage, cost.as_micros() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::{Costs, Stage, StageSink};
    use std::time::Duration;

    /// 六阶段表须覆盖全部变体且下标即判别值（否则环槽列与聚合串档）。
    #[test]
    fn stage_table_covers_all_variants() {
        assert_eq!(Stage::ALL.len(), Stage::N);
        for (i, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), i, "下标须与判别值一致");
        }
        // 逐臂穷举、无通配：新增变体而忘登记时此处编译失败。
        for stage in Stage::ALL {
            match stage {
                Stage::Compose
                | Stage::Diff
                | Stage::Encode
                | Stage::Write
                | Stage::Flush
                | Stage::Present => {}
            }
        }
    }

    /// 名字是 JSONL 键与日志字段，顺序与拼写一并锁定。
    #[test]
    fn names_are_stable_tokens() {
        let names: Vec<&str> = Stage::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            ["compose", "diff", "encode", "write", "flush", "present"]
        );
    }

    /// 未采样不是 0：新容器的每段皆为未知，整帧合计为 0 且不完整。
    #[test]
    fn unsampled_stage_is_unknown_not_zero() {
        let costs = Costs::new();
        for stage in Stage::ALL {
            assert_eq!(costs.get(stage), None, "{} 未采样须为 None", stage.name());
        }
        assert_eq!(costs.total(), 0);
        assert_eq!(costs.sampled(), 0);
        assert!(!costs.complete());
    }

    /// 合计只累已采样段：缺段不拖累整帧口径，也不虚构 0。
    #[test]
    fn total_sums_only_sampled_stages() {
        let mut costs = Costs::new();
        costs.set(Stage::Compose, 120);
        costs.set(Stage::Present, 300);
        assert_eq!(costs.total(), 420);
        assert_eq!(costs.sampled(), 2);
        assert!(!costs.complete(), "缺四段不得判齐全");
    }

    /// 六段全部可采集：汇聚口落满即判齐全，合计为各段之和。
    #[test]
    fn sink_collects_every_stage() {
        let mut costs = Costs::new();
        for (i, stage) in Stage::ALL.iter().enumerate() {
            costs.stage_cost(*stage, Duration::from_micros(10 * (i as u64 + 1)));
        }
        assert!(costs.complete());
        assert_eq!(costs.sampled(), Stage::N);
        assert_eq!(costs.total(), 10 + 20 + 30 + 40 + 50 + 60);
    }

    /// 异常大值不得回绕：饱和相加保证整帧口径单调不减。
    #[test]
    fn total_saturates_on_extreme_costs() {
        let mut costs = Costs::new();
        costs.set(Stage::Compose, u64::MAX);
        costs.set(Stage::Present, u64::MAX);
        assert_eq!(costs.total(), u64::MAX);
    }

    /// 档位不是阶段：它不参与合计、不参与完整性，只随帧留存（X2-30b）。
    #[test]
    fn ladder_is_not_a_stage() {
        let mut costs = Costs::new();
        assert_eq!(costs.ladder(), 0, "默认档编码为 0");
        costs.set(Stage::Compose, 100);
        costs.set_ladder(3);
        assert_eq!(costs.ladder(), 3);
        assert_eq!(costs.total(), 100, "档位不得计入耗时");
        assert_eq!(costs.sampled(), 1);
        assert!(!costs.complete(), "档位不得影响完整性");
    }
}
