// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 单帧阶段计时器（0.1.21 X2-25b）：把「段边界」折算成六阶段耗时。
//
// 采样点只声明「某段到此结束」，不自行读时钟——时钟由调用方注入（与 L4
// 同一口径），因此单测可用合成时刻精确断言边界，不必真睡。计时器产出的
// `Costs` 是 X8-05a 环形缓冲与 X8-06a 聚合器的唯一输入。
//
// 一帧的形状是 begin → lap(Compose) → lap(Present) → finish；P2 原子帧
// 后端落地后，diff/encode/write/flush 在各自边界补 lap，调用方无需改法。
// 未打点的段保持 unknown，不得以 0 冒充已采集。

use std::time::{Duration, Instant};

use super::frame::{Costs, Stage};

/// 帧内阶段计时器：`lap` 关闭上一段并落账，重复打点取首次。
pub(crate) struct Timer<C> {
    now: C,
    costs: Costs,
    last: Option<Instant>,
}

impl<C: FnMut() -> Instant> Timer<C> {
    pub(crate) const fn new(now: C) -> Self {
        Self {
            now,
            costs: Costs::new(),
            last: None,
        }
    }

    /// 开帧：清空上一帧的段账并落下起点。未成帧的轮次不得调用。
    pub(crate) fn begin(&mut self) {
        self.costs = Costs::new();
        self.last = Some((self.now)());
    }

    /// 收束当前段并记入 `stage`。
    ///
    /// 未开帧（无起点）或该段已记过时丢弃本次采样：段边界只能前进，
    /// 迟到的补记不得覆盖先到的真值，也不得凭空造出耗时。
    pub(crate) fn lap(&mut self, stage: Stage) {
        let now = (self.now)();
        let Some(last) = self.last.replace(now) else {
            return;
        };
        if self.costs.get(stage).is_none() {
            self.costs.set(stage, micros(now.saturating_duration_since(last)));
        }
    }

    /// 收帧：交出本帧段账（未采样段保持 unknown）。
    pub(crate) fn finish(self) -> Costs {
        self.costs
    }
}

fn micros(span: Duration) -> u64 {
    u64::try_from(span.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Timer, Stage};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// 合成时钟：每次读取推进到预设偏移，边界消耗与真实时间无关。
    fn clock(offsets: &[u64]) -> (impl FnMut() -> Instant, Rc<Cell<usize>>) {
        let ticks = Rc::new(Cell::new(0));
        let seen = ticks.clone();
        let base = Instant::now();
        let mut pending: Vec<u64> = offsets.iter().rev().copied().collect();
        let now = move || {
            ticks.set(ticks.get() + 1);
            let off = pending.pop().unwrap_or(0);
            base + Duration::from_micros(off)
        };
        (now, seen)
    }

    /// 六段全部可采集：各段耗时等于相邻两次打点的间隔，合计等于整帧跨度。
    #[test]
    fn six_laps_fill_every_stage() {
        let (now, ticks) = clock(&[0, 10, 30, 60, 100, 150, 210]);
        let mut timer = Timer::new(now);
        timer.begin();
        for stage in Stage::ALL {
            timer.lap(stage);
        }
        let costs = timer.finish();
        assert!(costs.complete(), "六段齐全");
        assert_eq!(costs.sampled(), Stage::N);
        assert_eq!(ticks.get(), 7, "begin + 六次 lap 各读一次时钟");
        let expect = [10, 20, 30, 40, 50, 60];
        for (stage, want) in Stage::ALL.iter().zip(expect) {
            assert_eq!(costs.get(*stage), Some(want), "{}", stage.name());
        }
        assert_eq!(costs.total(), 210);
    }

    /// 未开帧时打点丢弃：没有起点就没有间隔，不得造出 0 耗时。
    #[test]
    fn lap_without_begin_is_dropped() {
        let (now, _) = clock(&[100]);
        let mut timer = Timer::new(now);
        timer.lap(Stage::Compose);
        assert_eq!(timer.finish().sampled(), 0);
    }

    /// 同一段重复打点取首次：段边界只能前进，补记不得覆盖真值。
    #[test]
    fn duplicate_lap_keeps_first_measurement() {
        let (now, _) = clock(&[0, 10, 25, 40]);
        let mut timer = Timer::new(now);
        timer.begin();
        timer.lap(Stage::Compose);
        timer.lap(Stage::Compose);
        timer.lap(Stage::Present);
        let costs = timer.finish();
        assert_eq!(costs.get(Stage::Compose), Some(10));
        assert_eq!(costs.get(Stage::Present), Some(15));
    }

    /// 跨帧复用：begin 清空上一帧段账，旧帧数据不得渗入新帧。
    #[test]
    fn begin_discards_previous_frame() {
        let (now, _) = clock(&[0, 10, 20, 30, 45]);
        let mut timer = Timer::new(now);
        timer.begin();
        timer.lap(Stage::Compose);
        timer.lap(Stage::Present);
        assert_eq!(timer.finish().sampled(), 2);

        let (now, _) = clock(&[0, 7, 20]);
        let mut timer = Timer::new(now);
        timer.begin();
        timer.lap(Stage::Compose);
        let costs = timer.finish();
        assert_eq!(costs.get(Stage::Compose), Some(7));
        assert_eq!(costs.get(Stage::Present), None, "新帧不得带上旧帧的段");
        assert!(!costs.complete());
    }
}
