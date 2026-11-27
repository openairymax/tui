// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 长会话内存上界（0.1.21 X8-09/X8-10）：100k 条消息逐条走真实
// `add_message`（净化、单条折叠、超限淘汰），并在进程内采样峰值常驻集
// （/proc/self/status 的 VmHWM）。判据：100k 相对 5k 基线的峰值 RSS
// 增幅不超过 20%——消息表若失去上限，本用例会直接变红，是 IS-19
// 「长会话内存持续上涨」的回归闸门。
//
// 采样口径：VmHWM 是进程启动以来的峰值，单调不减；同一进程内先取 5k
// 基线再取 100k，增幅只反映本轮新增分配。RSS 用例标记 `#[ignore]`，由
// Linux 流水线单线程显式执行，避免并行用例的临时分配污染数值；没有
// /proc 的平台采样返回 `None` 并跳过判定，不伪造跨平台数值。

use super::{add, Fixture};
use crate::app::{App, MessageRole, MAX_CHAT_MESSAGES};

/// 基准消息量（X8-09a）：100k 条，为保留上限的五十倍，足以暴露无界增长。
pub(crate) const MESSAGES_100K: usize = 100_000;
/// 基线消息量：与既有 5k 行长历史基准同一量级。
pub(crate) const MESSAGES_BASE: usize = 5_000;
/// 峰值 RSS 增幅红线（百分比，X8-10b）。
pub(crate) const RSS_GROWTH_LIMIT_PCT: u64 = 20;

/// 逐条落消息：内容带序号，避免相同字面量被合并而少算分配。
fn feed(app: &mut App, from: usize, to: usize) {
    for i in from..to {
        let body = format!("内存基准第 {i} 条：超出保留上限的旧消息应被淘汰。");
        add(app, MessageRole::Agent, &body);
    }
}

/// 峰值常驻集（KiB）。Linux 读 `/proc/self/status` 的 VmHWM，其他平台无此
/// 文件，返回 `None`。
#[cfg(target_os = "linux")]
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_kib() -> Option<u64> {
    None
}

/// X8-09b + §23.9 #13：100k 条逐条落库后只保留预算上限条数。
/// 全链走真实入口：`add_message` → 会话快照 `session_view`。
#[test]
fn hundred_thousand_messages_stay_bounded() {
    let mut fx = Fixture::new("x8-09b-100k");
    feed(&mut fx.app, 0, MESSAGES_100K);
    let kept = fx.app.session_view().tabs()[0].messages();
    assert_eq!(kept, MAX_CHAT_MESSAGES, "消息表应被淘汰到保留上限");
}

/// X8-10b：100k 相对 5k 的峰值 RSS 增幅不超过 20%。
#[test]
#[ignore = "内存基准：由 Linux 流水线单线程显式执行"]
fn rss_growth_stays_under_limit() {
    if peak_rss_kib().is_none() {
        eprintln!("X8-10b skip: 平台无 /proc，跳过 RSS 判定");
        return;
    }
    let mut fx = Fixture::new("x8-10b-rss");
    feed(&mut fx.app, 0, MESSAGES_BASE);
    let base = peak_rss_kib().expect("基线采样");
    feed(&mut fx.app, MESSAGES_BASE, MESSAGES_100K);
    let peak = peak_rss_kib().expect("峰值采样");
    assert!(base > 0, "基线 RSS 采样为零，无法判定增幅");
    let growth = peak.saturating_sub(base);
    let limit = base * RSS_GROWTH_LIMIT_PCT / 100;
    println!("X8-10b rss: base={base}KiB peak={peak}KiB growth={growth}KiB limit={limit}KiB");
    assert!(
        growth <= limit,
        "100k 消息峰值 RSS 增幅 {growth}KiB 超过红线 {limit}KiB（base={base}KiB peak={peak}KiB）"
    );
}
