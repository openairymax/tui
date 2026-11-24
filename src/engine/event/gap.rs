// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 断点台账（0.1.21 X1-09a，§23.1.5 / §19.1 D-15 判据②）。
//
// `seq::Watch` 只回答「这一帧是不是断点」，不留明细；明细在这里：每一段
// 缺失的帧号区间连同发现时刻记一条，供事后定位"哪几帧没到、什么时候断的"。
// 队列有界（`MAX_GAPS`），满后丢最旧并计数——断点台账是现场取证，不是
// 长期遥测，不能因长时间断网把内存拖垮。
//
// 时刻由调用方给出（毫秒），本模块不读时钟：判定与记录保持纯函数，
// 测试可用固定时刻复现同一段区间。
//
// 落盘走只读审计通道（X1-09b）：一行一段区间，格式固定为
// `{"gap":{"from":N,"to":N,"at":ms,"missed":N}}`，与事件录制共用
// `LineLog` 的截断/刷盘/熔断纪律，但落在独立的文件名下——录制文件是
// 可回放的帧流，混入审计行会让回放把它当成坏帧。
//
// `AIRY_TUI_GAP_AUDIT=1` 时启用落盘；未开启不影响内存台账，状态栏的
// 断点计数仍照常上报。

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;

use super::seq::Verdict;
use crate::engine::logfile::{LineLog, MAX_LINES};

/// 断点审计开关。
pub(crate) const SWITCH: &str = "AIRY_TUI_GAP_AUDIT";

/// 断点审计文件名（置于 AIRY_HOME/logs）。
pub(crate) const FILE_NAME: &str = "agentrt-tui-gaps.jsonl";

/// 按环境开关打开审计通道；未开启或路径不可用时返回 `None`（fail-closed）。
pub(crate) fn open_audit() -> Option<LineLog> {
    if !crate::engine::flag::on(SWITCH) {
        return None;
    }
    let path = crate::paths::log_dir().join(FILE_NAME);
    match LineLog::open(&path, MAX_LINES) {
        Ok(sink) => Some(sink),
        Err(e) => {
            log::warn!("断点审计未开启：{}（{e}）", path.display());
            None
        }
    }
}

/// 台账容量：足以覆盖一次长时间断网，又不会随会话时长增长。
pub(crate) const MAX_GAPS: usize = 256;

/// 一段缺失的帧号区间：from..=to 没有到达，发现于 at 毫秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) from: u64,
    pub(crate) to: u64,
    pub(crate) at: u64,
}

impl Gap {
    /// 缺失帧数（真空区间不出台账，故至少 1）。
    pub(crate) fn missed(&self) -> u64 {
        self.to.saturating_sub(self.from).saturating_add(1)
    }

    /// 审计行：一段区间一行 JSON，字段与内存结构一一对应。
    pub(crate) fn audit_line(&self) -> String {
        serde_json::json!({
            "gap": {
                "from": self.from,
                "to": self.to,
                "at": self.at,
                "missed": self.missed(),
            }
        })
        .to_string()
    }
}

/// 有界断点队列：顺序入、顺序出，满则丢最旧。
#[derive(Debug)]
pub(crate) struct Gaps {
    slots: VecDeque<Gap>,
    lost: u64,
}

impl Gaps {
    /// 空台账。
    pub(crate) fn new() -> Self {
        Self {
            slots: VecDeque::with_capacity(MAX_GAPS),
            lost: 0,
        }
    }

    /// 记一条断点事实：只收 gap，顺序帧与重复帧不进台账。
    ///
    /// 重复帧单独计数在 `Watch` 里；台账记的是"区间缺失"，两类事实口径不同，
    /// 混在一起会让 dropped 的统计失去意义。
    pub(crate) fn note(&mut self, verdict: Verdict, at: u64) -> bool {
        let (from, to) = match verdict {
            Verdict::Gap { from, to } => (from, to),
            _ => return false,
        };
        if self.slots.len() >= MAX_GAPS {
            self.slots.pop_front();
            self.lost = self.lost.saturating_add(1);
        }
        self.slots.push_back(Gap { from, to, at });
        true
    }

    /// 最早一段（定位断点起点用）。
    pub(crate) fn oldest(&self) -> Option<&Gap> {
        self.slots.front()
    }

    /// 最近一段（状态栏与告警用）。
    pub(crate) fn latest(&self) -> Option<&Gap> {
        self.slots.back()
    }

    /// 现存条数。
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否无断点。
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 因容量丢掉的条数：非零说明台账已被截断，定位须查审计落盘。
    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }

    /// 缺失帧总数：现存区间之和，不含已丢弃部分。
    pub(crate) fn missed(&self) -> u64 {
        self.slots.iter().map(Gap::missed).sum()
    }

    /// 命中查询：某个帧号是否落在已记录的缺失区间内（定位用）。
    pub(crate) fn covering(&self, seq: u64) -> Option<&Gap> {
        self.slots.iter().find(|g| g.from <= seq && seq <= g.to)
    }

    /// 把现存区间逐条写入只读审计，返回实际落盘条数。
    ///
    /// 写失败即停并保留未写部分：审计通道不可用时台账本身不能一起丢，
    /// 内存里仍可展示最近的断点。
    pub(crate) fn flush_to(&mut self, log: &mut LineLog) -> usize {
        let mut kept = 0;
        while let Some(g) = self.slots.pop_front() {
            if !log.write(&g.audit_line()) {
                self.slots.push_front(g);
                break;
            }
            kept += 1;
        }
        kept
    }
}

impl Default for Gaps {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{open_audit, Gaps, FILE_NAME, MAX_GAPS, SWITCH};
    use crate::engine::event::seq::{Tracker, Verdict};
    use crate::engine::logfile::LineLog;
    use crate::test_env::Home;

    /// 审计通道由开关控制：默认不建文件，开启后落到 AIRY_HOME/logs。
    #[test]
    fn audit_channel_follows_switch() {
        let home = Home::new("gap-audit");
        std::env::remove_var(SWITCH);
        assert!(open_audit().is_none(), "未开启时不建通道");

        std::env::set_var(SWITCH, "1");
        let mut log = open_audit().expect("开启且路径可建");
        let mut g = Gaps::new();
        g.note(gap(1, 2), 5);
        assert_eq!(g.flush_to(&mut log), 1);
        drop(log);
        std::env::remove_var(SWITCH);

        let path = home.path().join(crate::paths::LOG_SUBDIR).join(FILE_NAME);
        let body = std::fs::read_to_string(&path).expect("审计已落盘");
        assert!(body.contains("\"from\":1"), "区间须逐条落盘");
    }

    fn gap(from: u64, to: u64) -> Verdict {
        Verdict::Gap { from, to }
    }

    /// 只收区间缺失；顺序帧与重复帧不占台账。
    #[test]
    fn notes_gaps_only() {
        let mut g = Gaps::new();
        assert!(!g.note(Verdict::Ok, 1));
        assert!(!g.note(Verdict::Repeat { seq: 4 }, 2));
        assert!(g.note(gap(2, 3), 10));
        assert_eq!(g.len(), 1);
        assert_eq!(g.latest().map(|x| (x.from, x.to, x.at)), Some((2, 3, 10)));
        assert_eq!(g.missed(), 2);
    }

    /// 与断言器直连：整条流的断点按发生顺序落账。
    #[test]
    fn records_in_stream_order() {
        let mut t = Tracker::new();
        let mut g = Gaps::new();
        for (seq, at) in [(1u64, 100u64), (4, 130), (5, 140), (9, 170)] {
            g.note(t.accept(seq), at);
        }
        assert_eq!(g.len(), 2);
        assert_eq!(g.oldest().map(|x| (x.from, x.at)), Some((2, 130)));
        assert_eq!(g.latest().map(|x| (x.from, x.to)), Some((6, 8)));
        assert_eq!(g.missed(), 5);
    }

    /// 满后丢最旧并计数：内存不随断网时长增长。
    #[test]
    fn bounded_queue_drops_oldest() {
        let mut g = Gaps::new();
        for i in 0..MAX_GAPS as u64 + 3 {
            g.note(gap(i, i), i);
        }
        assert_eq!(g.len(), MAX_GAPS);
        assert_eq!(g.lost(), 3);
        assert_eq!(g.oldest().map(|x| x.from), Some(3));
        assert_eq!(g.latest().map(|x| x.from), Some(MAX_GAPS as u64 + 2));
    }

    /// 真空区间不计入缺失：from > to 只会出现在构造错误，饱和为 1 而不下溢。
    #[test]
    fn missed_never_underflows() {
        let mut g = Gaps::new();
        g.note(gap(7, 3), 1);
        assert_eq!(g.missed(), 1);
        assert!(Gaps::default().is_empty());
    }

    /// 定位：缺失帧落在区间内可命中，区间外的帧不误命中。
    #[test]
    fn covering_locates_missing_frame() {
        let mut g = Gaps::new();
        g.note(gap(3, 5), 10);
        g.note(gap(9, 9), 20);
        assert_eq!(g.covering(4).map(|x| x.at), Some(10));
        assert_eq!(g.covering(9).map(|x| x.at), Some(20));
        assert_eq!(g.covering(6), None);
        assert_eq!(g.covering(2), None);
    }

    /// 审计落盘：一段一行，坏帧区间与时刻可在文件里逐条定位。
    #[test]
    fn audit_locates_ranges_on_disk() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("gaps.jsonl");
        let mut log = LineLog::open(&path, 64).expect("打开审计");
        let mut g = Gaps::new();
        g.note(gap(3, 5), 900);
        g.note(gap(9, 9), 950);
        assert_eq!(g.flush_to(&mut log), 2);
        assert!(g.is_empty(), "落盘后内存台账不再重复持有");
        drop(log);

        let body = std::fs::read_to_string(&path).expect("审计已落盘");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("合法 JSON");
        assert_eq!(first["gap"]["from"], 3);
        assert_eq!(first["gap"]["to"], 5);
        assert_eq!(first["gap"]["at"], 900);
        assert_eq!(first["gap"]["missed"], 3);
    }

    /// 审计不可用（容量为零）时不必丢台账：未写部分留在内存。
    #[test]
    fn flush_keeps_gaps_when_audit_closed() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut log = LineLog::open(&dir.path().join("off.jsonl"), 0).expect("打开审计");
        let mut g = Gaps::new();
        g.note(gap(2, 4), 7);
        assert_eq!(g.flush_to(&mut log), 0);
        assert_eq!(g.len(), 1);
        assert_eq!(g.latest().map(|x| x.at), Some(7));
    }
}
