// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 指标段骨架（0.1.21 X9-01a）：X 面实验的统一记录面。
//
// 只做「次数 + 最近一次观测」两件最小事实，供 X8 帧指标与 X9 体验门消费。
// 存储有界：固定槽位、栈上名字、无 HashMap、无堆增长；槽满后新名不登记，
// 只累计 dropped——指标本身绝不允许成为内存增长源（0.1.19 §5.3「一切有界」）。
//
// 名字规则与 feature::gate 对齐（[a-z0-9_]，≤ NAME_MAX）；非法名不登记，
// 计入 dropped，与槽满同口径，调用方可用 dropped() 观察丢失量。

/// 槽位上限：X 面全部实验共用，24 项足够覆盖当前登记面。
const SLOTS: usize = 24;

/// 名字上界，与 feature::gate::NAME_MAX 同口径。
const NAME_MAX: usize = 32;

/// 单槽：定长名字 + 计数 + 最近一次观测值（毫秒或微秒由调用方约定）。
#[derive(Clone, Copy)]
struct Slot {
    name: [u8; NAME_MAX],
    len: u8,
    count: u64,
    last: u64,
}

impl Slot {
    const EMPTY: Self = Self {
        name: [0; NAME_MAX],
        len: 0,
        count: 0,
        last: 0,
    };

    fn named(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        let legal = !bytes.is_empty()
            && bytes.len() <= NAME_MAX
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        if !legal {
            return None;
        }
        let mut slot = Self::EMPTY;
        slot.name[..bytes.len()].copy_from_slice(bytes);
        slot.len = bytes.len() as u8;
        Some(slot)
    }

    fn label(&self) -> &str {
        std::str::from_utf8(&self.name[..usize::from(self.len)]).unwrap_or("")
    }

    fn matches(&self, name: &str) -> bool {
        self.len != 0 && self.label() == name
    }
}

/// 有界指标段；`Default` 与 `new` 等价，便于嵌入上层结构。
pub(crate) struct Metrics {
    slots: [Slot; SLOTS],
    dropped: u64,
}

impl Metrics {
    pub(crate) fn new() -> Self {
        Self {
            slots: [Slot::EMPTY; SLOTS],
            dropped: 0,
        }
    }

    /// 计数加一；新名在有空槽时登记，否则计入 dropped。
    pub(crate) fn bump(&mut self, name: &str) {
        if let Some(at) = self.locate(name) {
            self.slots[at].count = self.slots[at].count.saturating_add(1);
            return;
        }
        match self.free() {
            Some(at) => match Slot::named(name) {
                Some(slot) => {
                    self.slots[at] = Slot {
                        count: 1,
                        ..slot
                    }
                }
                None => self.dropped = self.dropped.saturating_add(1),
            },
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    /// 计数加一并记录本次观测值；非法名与槽满同 bump 口径。
    pub(crate) fn observe(&mut self, name: &str, value: u64) {
        self.bump(name);
        if let Some(at) = self.locate(name) {
            self.slots[at].last = value;
        }
    }

    /// 累计次数；未登记返回 0。
    pub(crate) fn count(&self, name: &str) -> u64 {
        self.locate(name).map_or(0, |at| self.slots[at].count)
    }

    /// 最近一次观测值；未登记或从未 observe 返回 None。
    pub(crate) fn last(&self, name: &str) -> Option<u64> {
        self.locate(name)
            .filter(|&at| self.slots[at].last != 0)
            .map(|at| self.slots[at].last)
    }

    /// 被丢弃的登记请求总数（非法名 + 槽满）。
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    fn locate(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|slot| slot.matches(name))
    }

    fn free(&self) -> Option<usize> {
        self.slots.iter().position(|slot| slot.len == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_accumulates_per_name() {
        let mut m = Metrics::new();
        m.bump("frame_drop");
        m.bump("frame_drop");
        m.bump("input_wait");
        assert_eq!(m.count("frame_drop"), 2);
        assert_eq!(m.count("input_wait"), 1);
        assert_eq!(m.count("absent"), 0);
        assert_eq!(m.dropped(), 0);
    }

    #[test]
    fn observe_records_last_value() {
        let mut m = Metrics::new();
        assert_eq!(m.last("frame_p95"), None);
        m.observe("frame_p95", 12);
        m.observe("frame_p95", 9);
        assert_eq!(m.count("frame_p95"), 2);
        assert_eq!(m.last("frame_p95"), Some(9));
    }

    #[test]
    fn slots_are_bounded() {
        let mut m = Metrics::new();
        for i in 0..SLOTS {
            m.bump(&format!("slot_{i}"));
        }
        assert_eq!(m.dropped(), 0);
        m.bump("one_too_many");
        assert_eq!(m.dropped(), 1);
        assert_eq!(m.count("one_too_many"), 0);
        assert_eq!(m.count("slot_0"), 1);
    }

    #[test]
    fn illegal_names_count_as_dropped() {
        let mut m = Metrics::new();
        for name in ["", "Bad", "has space", &"b".repeat(NAME_MAX + 1)] {
            m.bump(name);
        }
        assert_eq!(m.dropped(), 4);
        assert_eq!(m.count(""), 0);
    }
}
