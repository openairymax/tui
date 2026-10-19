// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 首帧预算度量（0.1.19 §6.1 #1）：进程启动基准至首次成帧的耗时。
//
// 只读度量：超预算仅告警，不重试、不阻塞首绘，也不改变后续帧路径。启动期
// 能力探测（网关健康、主题能力）与首帧并发，晚到结果经事件热修正。

use std::time::Instant;

use crate::budget::FIRST_FRAME_BUDGET_MS;

/// 启动基准（进程进入 main 的时刻）。
pub(crate) struct Boot {
    at: Instant,
}

impl Boot {
    pub(crate) fn mark() -> Self {
        Self { at: Instant::now() }
    }

    /// 自启动基准至此刻的毫秒数。
    pub(crate) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// 首帧是否超出预算（纯函数：两界可确定性测试）。
pub(crate) fn over_budget(ms: u64) -> bool {
    ms > FIRST_FRAME_BUDGET_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §6.1 #1 首帧预算：上界含于预算内，超出 1ms 即判超。
    #[test]
    fn budget_boundary_is_inclusive() {
        assert!(!over_budget(0));
        assert!(!over_budget(FIRST_FRAME_BUDGET_MS));
        assert!(over_budget(FIRST_FRAME_BUDGET_MS + 1));
    }
}
