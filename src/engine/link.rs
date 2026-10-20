// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 连接三态（0.1.19 §6.6 #2）：把「已连接」拆成 在线 / 重连中·n / 已补账·k
// 三个可分辨的呈递态。探测失败只累计次数；恢复成功且此前确实断过，先呈递
// 一次补账结果（k = 当前在线的依赖数），下一次成功才回到常态在线。
//
// 本模块是 L1 纯状态机：不做 IO、不读时钟、不触碰渲染，输入只有探测结果
// 与依赖快照，输出由上层取用。

/// 连接态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Link {
    /// 探测成功且无待补账。
    Online,
    /// 连续探测失败次数；0 表示首帧探测尚未落地。
    Retry(u32),
    /// 断线恢复后的补账呈递，携带已恢复在线的依赖数。
    Replay(u32),
}

impl Link {
    pub(crate) fn new() -> Self {
        Self::Retry(0)
    }

    /// 吸收一次探测结果。- `ok` 为本次探测是否成功；`resynced` 为当前在线
    /// 依赖数，仅在由断线恢复时取用。
    pub(crate) fn observe(&mut self, ok: bool, resynced: u32) {
        *self = match (*self, ok) {
            (_, false) => Self::Retry(match *self {
                Self::Retry(n) => n.saturating_add(1),
                _ => 1,
            }),
            (Self::Retry(n), true) if n > 0 => Self::Replay(resynced),
            _ => Self::Online,
        };
    }

    /// 是否可用于承载交互（在线或补账态）。
    pub(crate) fn is_online(self) -> bool {
        matches!(self, Self::Online | Self::Replay(_))
    }

    /// 呈递文案与告警属性：`(accent, label, warn)`。
    pub(crate) fn label(self) -> (&'static str, String, bool) {
        match self {
            Self::Online => ("●", "ONLINE".to_string(), false),
            Self::Retry(0) => ("◐", "connecting".to_string(), true),
            Self::Retry(n) => ("◐", format!("reconnecting · {n}"), true),
            Self::Replay(k) => ("●", format!("replayed · {k}"), false),
        }
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Link;

    #[test]
    fn probe_pending_is_not_online() {
        let link = Link::new();
        assert!(!link.is_online());
        assert_eq!(link.label().1, "connecting");
    }

    #[test]
    fn first_success_goes_online() {
        let mut link = Link::new();
        link.observe(true, 1);
        assert_eq!(link, Link::Online);
        assert!(link.is_online());
    }

    #[test]
    fn failures_accumulate() {
        let mut link = Link::Online;
        link.observe(false, 0);
        link.observe(false, 0);
        assert_eq!(link, Link::Retry(2));
        assert_eq!(link.label().1, "reconnecting · 2");
    }

    #[test]
    fn recovery_replays_once_then_online() {
        let mut link = Link::Retry(2);
        link.observe(true, 3);
        assert_eq!(link, Link::Replay(3));
        assert_eq!(link.label().1, "replayed · 3");
        link.observe(true, 3);
        assert_eq!(link, Link::Online);
    }
}
