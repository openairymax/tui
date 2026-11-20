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
// 本卡只回答「有没有断」，不保存断点历史（X1-09a 的 gap 队列）也不向外
// 上报（X1-08b 的上报接口）：判定与处置分开，测试才能只断言判定。

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

#[cfg(test)]
mod tests {
    use super::{Tracker, Verdict};

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
}
