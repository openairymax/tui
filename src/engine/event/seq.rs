// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧序号连续性断言（0.1.21 X1-08a，§23.1.5 / §19.1 D-15 判据①）。
//
// CLI 的 seq 自 1 严格递增，每一帧（含 heartbeat）占一个号；同一帧派生的
// 多条事件共用帧号，因此断言只吃信封序号。三种偏离各有名字：
//   gap    期望 N 却收到 N+k —— 中间 k 帧没有到达；
//   repeat 收到已消费过的号 —— 重连重放或上游重复写。
// gap 报一次后立即对齐到当前帧：若每帧都报同一个 gap，日志会被同一事实
// 淹没，真正的新断点反而看不见（一次不连续只算一次事故）。
//
// 本模块只做判定与上报：不保存断点历史（X1-09a 的 gap 队列）——明细留给
// 队列，这里只给状态栏一个总数。顺序帧不占用上报通道：`Watch::feed` 只在
// 断点处给出事实。

#![cfg_attr(not(test), allow(dead_code))]

/// 一次断言的结果：只有 Ok 表示按期望到达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// 恰好是期望的下一帧。
    Ok,
    /// 中间缺帧：from..=to 没有到达。
    Gap { from: u64, to: u64 },
    /// 已消费过的序号再次到达。
    Repeat { seq: u64 },
}

/// 期望序号跟踪器：只记录「下一个该来的号」。
#[derive(Debug, Clone)]
pub(crate) struct Tracker {
    next: u64,
}

impl Tracker {
    /// 从流首开始（D-15：seq 自 1 起）。
    pub(crate) fn new() -> Self {
        Self { next: 1 }
    }

    /// 断线重连：从 last_seq 的下一帧续收。
    pub(crate) fn resume(last_seq: u64) -> Self {
        Self {
            next: last_seq.saturating_add(1),
        }
    }

    /// 断言并推进：gap 只报一次，随后对齐到当前帧。
    pub(crate) fn accept(&mut self, seq: u64) -> Verdict {
        let want = self.next;
        if seq == want {
            self.next = want.saturating_add(1);
            return Verdict::Ok;
        }
        if seq < want {
            return Verdict::Repeat { seq };
        }
        self.next = seq.saturating_add(1);
        Verdict::Gap {
            from: want,
            to: seq - 1,
        }
    }

    /// 下一个期望序号（诊断与重连握手用）。
    pub(crate) fn next(&self) -> u64 {
        self.next
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

/// 断点上报接口：逐帧喂入，只在断点处给出事实并累计。
///
/// 与 `Tracker` 的分工：`Tracker` 只判「该来的号来了没有」，`Watch` 负责
/// 把异常从正常的逐帧流里挑出来，供状态栏读取总数。明细不在本层留存。
pub(crate) struct Watch {
    tracker: Tracker,
    gaps: u64,
    repeats: u64,
}

impl Watch {
    /// 从流首开始计。
    pub(crate) fn new() -> Self {
        Self::from_tracker(Tracker::new())
    }

    /// 重连续收：断点前的帧不计入本次上报。
    pub(crate) fn resume(last_seq: u64) -> Self {
        Self::from_tracker(Tracker::resume(last_seq))
    }

    /// 喂一帧：断点返回事实，顺序帧返回 `None`。
    pub(crate) fn feed(&mut self, seq: u64) -> Option<Verdict> {
        match self.tracker.accept(seq) {
            Verdict::Ok => None,
            v @ Verdict::Gap { .. } => {
                self.gaps = self.gaps.saturating_add(1);
                Some(v)
            }
            v @ Verdict::Repeat { .. } => {
                self.repeats = self.repeats.saturating_add(1);
                Some(v)
            }
        }
    }

    /// 下一个期望序号（续收握手的水位基）。
    pub(crate) fn next(&self) -> u64 {
        self.tracker.next()
    }

    /// 缺帧次数。
    pub(crate) fn gaps(&self) -> u64 {
        self.gaps
    }

    /// 重复次数。
    pub(crate) fn repeats(&self) -> u64 {
        self.repeats
    }

    /// 断点总数（状态栏直读）。
    pub(crate) fn breaks(&self) -> u64 {
        self.gaps.saturating_add(self.repeats)
    }

    fn from_tracker(tracker: Tracker) -> Self {
        Self {
            tracker,
            gaps: 0,
            repeats: 0,
        }
    }
}

impl Default for Watch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Tracker, Verdict, Watch};

    /// 顺序到达逐帧放行，期望随之前移。
    #[test]
    fn accepts_in_order() {
        let mut t = Tracker::new();
        assert_eq!(t.accept(1), Verdict::Ok);
        assert_eq!(t.accept(2), Verdict::Ok);
        assert_eq!(t.next(), 3);
    }

    /// 缺帧只报一次：对齐后下一帧照常放行，不重复报警。
    #[test]
    fn gap_reported_once() {
        let mut t = Tracker::new();
        assert_eq!(t.accept(3), Verdict::Gap { from: 1, to: 2 });
        assert_eq!(t.accept(4), Verdict::Ok);
        assert_eq!(t.accept(9), Verdict::Gap { from: 5, to: 8 });
        assert_eq!(t.accept(10), Verdict::Ok);
    }

    /// 重复到达可检出，且不破坏后续期望。
    #[test]
    fn repeat_detected() {
        let mut t = Tracker::new();
        assert_eq!(t.accept(1), Verdict::Ok);
        assert_eq!(t.accept(2), Verdict::Ok);
        assert_eq!(t.accept(2), Verdict::Repeat { seq: 2 });
        assert_eq!(t.accept(1), Verdict::Repeat { seq: 1 });
        assert_eq!(t.accept(3), Verdict::Ok);
    }

    /// 重连续收按断点起算；序号上界饱和而不溢出。
    #[test]
    fn resume_from_checkpoint() {
        let mut t = Tracker::resume(41);
        assert_eq!(t.next(), 42);
        assert_eq!(t.accept(42), Verdict::Ok);

        // 实际链路远达不到 2^64 帧：上界只保证不溢出，期望号饱和在 MAX。
        let mut edge = Tracker::resume(u64::MAX);
        assert_eq!(edge.accept(u64::MAX), Verdict::Ok);
        assert_eq!(edge.next(), u64::MAX);
    }

    /// 断点可检出：缺帧与重复都以事实上报，顺序帧不打扰。
    #[test]
    fn break_surfaces_once() {
        let mut w = Watch::new();
        assert_eq!(w.feed(1), None);
        assert_eq!(w.feed(4), Some(Verdict::Gap { from: 2, to: 3 }));
        assert_eq!(w.feed(5), None, "对齐后同一断点不再上报");
        assert_eq!(w.feed(4), Some(Verdict::Repeat { seq: 4 }));
        assert_eq!((w.gaps(), w.repeats(), w.breaks()), (1, 1, 2));
    }

    /// 重连续收：断点之前的帧不计入本次上报。
    #[test]
    fn watch_resumes() {
        let mut w = Watch::resume(9);
        assert_eq!(w.feed(10), None);
        assert_eq!(w.feed(12), Some(Verdict::Gap { from: 11, to: 11 }));
        assert_eq!((w.gaps(), w.repeats()), (1, 0));
    }
}
