// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 两段式 leader 前缀状态机（0.1.19 TUI 建设方案 §5.4 #3、§6.4 #7）。
//
// 语义只有三条：按下前缀键进入待续键态；`LEADER_HINT_DELAY_MS` 内未续键
// 即浮出可接键提示（只浮出一次）；续键或取消即回到空闲。时刻一律由调用方
// 注入，本模块不取时钟——边界行为因此可在单测里用虚拟时刻确定性重放。
//
// 提示浮出后前缀态继续保留：用户看到提示再按键仍属同一次 leader 序列，
// 取消权交给显式的取消键（Esc）或下一条被消费的按键。

use std::time::{Duration, Instant};

use crate::budget::LEADER_HINT_DELAY_MS;

/// leader 待续键状态。
#[derive(Debug, Default)]
pub(crate) struct Leader {
    /// 前缀键按下时刻；None 表示不在 leader 序列中。
    armed_at: Option<Instant>,
    /// 提示浮层是否已显示（每个序列只浮出一次）。
    hint: bool,
}

impl Leader {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 记录一次前缀键按下，开启新的 leader 序列。
    pub(crate) fn press(&mut self, now: Instant) {
        self.armed_at = Some(now);
        self.hint = false;
    }

    /// 是否处于待续键态。
    pub(crate) fn pending(&self) -> bool {
        self.armed_at.is_some()
    }

    /// 提示浮层当前是否可见。
    pub(crate) fn hint_visible(&self) -> bool {
        self.hint
    }

    /// 到期裁决：满延迟即置提示可见并返回真（调用方据此补帧）。
    /// 已显示过或不在序列中返回假，保证一次序列只报告一次。
    pub(crate) fn hint_due(&mut self, now: Instant) -> bool {
        let Some(armed) = self.armed_at else {
            return false;
        };
        if self.hint {
            return false;
        }
        if now.saturating_duration_since(armed) < Duration::from_millis(LEADER_HINT_DELAY_MS) {
            return false;
        }
        self.hint = true;
        true
    }

    /// 距提示到期还剩多久（主循环据此收窄事件等待）；无需等待时为 None。
    pub(crate) fn wait(&self, now: Instant) -> Option<Duration> {
        if self.hint {
            return None;
        }
        self.armed_at.map(|armed| {
            Duration::from_millis(LEADER_HINT_DELAY_MS)
                .saturating_sub(now.saturating_duration_since(armed))
        })
    }

    /// 退出 leader 序列（续键消费或显式取消）。
    pub(crate) fn cancel(&mut self) {
        self.armed_at = None;
        self.hint = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn press_arms_without_hint() {
        let t0 = Instant::now();
        let mut l = Leader::new();
        l.press(t0);
        assert!(l.pending());
        assert!(!l.hint_visible());
        assert!(!l.hint_due(t0), "按下瞬间不浮出提示");
    }

    #[test]
    fn hint_waits_full_delay() {
        let t0 = Instant::now();
        let mut l = Leader::new();
        l.press(t0);
        assert!(
            !l.hint_due(at(t0, LEADER_HINT_DELAY_MS - 1)),
            "界内不提前浮出"
        );
        assert!(!l.hint_visible());
        assert!(
            l.hint_due(at(t0, LEADER_HINT_DELAY_MS)),
            "满延迟即浮出并报告一次"
        );
        assert!(l.hint_visible());
        assert!(
            !l.hint_due(at(t0, LEADER_HINT_DELAY_MS + 1)),
            "同一序列只报告一次"
        );
    }

    #[test]
    fn wait_tracks_remaining_delay() {
        let t0 = Instant::now();
        let mut l = Leader::new();
        assert_eq!(l.wait(t0), None, "空闲态不参与等待收窄");
        l.press(t0);
        assert_eq!(
            l.wait(at(t0, 200)),
            Some(Duration::from_millis(LEADER_HINT_DELAY_MS - 200)),
            "剩余时长由常量派生"
        );
        l.hint_due(at(t0, LEADER_HINT_DELAY_MS));
        assert_eq!(l.wait(at(t0, LEADER_HINT_DELAY_MS)), None, "已浮出无需再等");
    }

    #[test]
    fn cancel_clears_sequence() {
        let t0 = Instant::now();
        let mut l = Leader::new();
        l.press(t0);
        l.hint_due(at(t0, LEADER_HINT_DELAY_MS));
        l.cancel();
        assert!(!l.pending());
        assert!(!l.hint_visible());
        assert!(!l.hint_due(at(t0, LEADER_HINT_DELAY_MS + 1)));
    }

    #[test]
    fn repress_restarts_sequence() {
        let t0 = Instant::now();
        let mut l = Leader::new();
        l.press(t0);
        l.hint_due(at(t0, LEADER_HINT_DELAY_MS));
        l.press(at(t0, LEADER_HINT_DELAY_MS + 10));
        assert!(!l.hint_visible(), "新一轮序列从无提示开始");
        assert_eq!(
            l.wait(at(t0, LEADER_HINT_DELAY_MS + 10)),
            Some(Duration::from_millis(LEADER_HINT_DELAY_MS))
        );
    }
}
