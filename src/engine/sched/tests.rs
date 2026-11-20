// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 调度层回归（0.1.18 A 轨 §5A.3 W4；0.1.21 X8-05/X8-06 环与分位）。
//
// 时钟一律注入：合帧窗口、上报节拍、空闲等待都以显式毫秒驱动，测试不真睡；
// 帧耗时以合成 Costs 填充，只断言调度层自己的口径（合并、丢弃、降级、留存、
// 分位窗口），不重复渲染层的断言。

use super::{Beat, Beats, Cfg, Costs, Lane, Sched, Stage, BEATS, BEAT_N, METRIC_EVERY};
use crate::budget::{FRAME_QUEUE_CAP, IDLE_WAIT_MS};

/// 60fps：合帧窗口为区间下限 16ms。
fn fixture() -> Sched {
    Sched::new(Cfg::from_fps(60))
}

/// 当前生产打点只覆盖排版与呈现两段，其余阶段在 P2 前保持未采样。
fn cost(layout_us: u64, present_us: u64) -> Costs {
    let mut costs = Costs::new();
    costs.set(Stage::Compose, layout_us);
    costs.set(Stage::Present, present_us);
    costs
}

/// 节拍表须覆盖全部变体且下标即判别值（否则 advance 会串档）。
#[test]
fn beat_table_covers_all_variants() {
    assert_eq!(BEATS.len(), BEAT_N);
    for (i, beat) in BEATS.iter().enumerate() {
        assert_eq!(*beat as usize, i, "节拍表下标须与判别值一致");
    }
}

/// 帧率上限收敛到 16–33ms 窗口，越界取值不得退化。
#[test]
fn coalesce_window_follows_fps_within_bounds() {
    assert_eq!(Cfg::from_fps(60).coalesce_ms, 16);
    assert_eq!(Cfg::from_fps(240).coalesce_ms, 16);
    assert_eq!(Cfg::from_fps(30).coalesce_ms, 33);
    assert_eq!(Cfg::from_fps(0).coalesce_ms, 33, "越界下限收敛");
    assert_eq!(Cfg::from_fps(u32::MAX).coalesce_ms, 16, "越界上限收敛");
}

/// 帧率上限的用户配置入口：合法取值生效，非法/未设置退回默认。
#[test]
fn fps_env_configures_coalesce_window() {
    let _lock = crate::test_env::lock_env();
    std::env::set_var("AIRY_TUI_MAX_FPS", "30");
    assert_eq!(Cfg::from_env().coalesce_ms, 33, "合法取值生效");
    std::env::set_var("AIRY_TUI_MAX_FPS", "not-a-number");
    assert_eq!(Cfg::from_env().coalesce_ms, 16, "非法取值退回默认 60fps");
    std::env::remove_var("AIRY_TUI_MAX_FPS");
    assert_eq!(Cfg::from_env().coalesce_ms, 16, "未设置退回默认 60fps");
}

/// §3.5 合帧：窗口内的增量整批合并为一帧。
#[test]
fn coalesces_stream_burst_into_one_frame() {
    let mut sched = fixture();
    sched.advance(0);
    for _ in 0..10 {
        assert!(sched.push(Lane::Stream), "队列未满须入队");
    }
    assert!(!sched.take_frame(), "窗口未到不得成帧");
    sched.advance(15);
    assert!(!sched.take_frame(), "窗口内不得成帧");
    sched.advance(16);
    assert!(sched.take_frame(), "窗口到期成帧");
    assert_eq!(sched.merges, 10, "10 个增量合并为一帧");
    assert_eq!(sched.wait_max_us, 16_000, "首条事件的合帧等待被记录");
    assert!(!sched.take_frame(), "同一帧不得重复取走");
}

/// §6.10 #1（输入抢占渲染，机制见 §3.5 优先级）：输入回显不被帧率
/// 闸拖住，抢占合帧窗口立即成帧。
#[test]
fn input_preempts_coalesce_window() {
    let mut sched = fixture();
    sched.advance(0);
    sched.push(Lane::Stream);
    assert!(!sched.take_frame(), "流式档须等窗口");
    sched.push(Lane::Input);
    assert!(sched.take_frame(), "输入档抢占并立即成帧");
}

/// §3.5 帧度量：帧率由窗口成帧数与注入时长导出，冻结时钟不得除零发散。
#[test]
fn frame_rate_derives_from_injected_window() {
    assert_eq!(
        Sched::fps_in(METRIC_EVERY, 8_000),
        64,
        "512 帧 / 8s = 64fps"
    );
    assert_eq!(
        Sched::fps_in(METRIC_EVERY, 16_000),
        32,
        "窗口拉长则帧率下降"
    );
    assert_eq!(Sched::fps_in(METRIC_EVERY, 0), 0, "冻结时钟报 0 而非发散");
    assert_eq!(Sched::fps_in(0, 1_000), 0, "零帧窗口为 0");
}

/// §3.5 帧度量：上报节拍即窗口边界，起点随上报推进（窗口不重叠不遗漏）。
#[test]
fn metric_report_advances_window_origin() {
    let mut sched = fixture();
    for i in 0..METRIC_EVERY {
        sched.advance(i * 16);
        sched.note_frame(&cost(100, 200));
    }
    assert_eq!(sched.frames, METRIC_EVERY);
    assert_eq!(
        sched.rate_ms,
        (METRIC_EVERY - 1) * 16,
        "窗口起点须推进到本次上报时刻"
    );
}

/// §3.5 背压：超上限丢中间态、保首尾。
#[test]
fn sheds_middle_hiding_head_and_tail() {
    let mut sched = fixture();
    let total = FRAME_QUEUE_CAP + 36;
    for i in 0..total {
        sched.advance(i as u64);
        assert!(sched.push(Lane::Stream));
        assert!(sched.queue.len() <= FRAME_QUEUE_CAP, "队列须恒以 64 为上限");
    }
    assert_eq!(sched.queue.front().map(|(_, at)| *at), Some(0), "保留首条");
    assert_eq!(
        sched.queue.back().map(|(_, at)| *at),
        Some((total - 1) as u64),
        "保留末条"
    );
    assert_eq!(sched.shed, 36, "溢出部分计为丢弃");
}

/// §3.5 背压：输入档不参与丢弃，满队列时拒收而非丢业务事件。
#[test]
fn input_lane_is_never_shed() {
    let mut sched = fixture();
    sched.advance(0);
    for _ in 0..FRAME_QUEUE_CAP {
        assert!(sched.push(Lane::Input));
    }
    assert!(!sched.push(Lane::Input), "仅输入档占满时拒收");
    assert_eq!(sched.shed, 0, "业务事件不得丢弃");
    assert_eq!(sched.queue.len(), FRAME_QUEUE_CAP);
}

/// §6.10 #4（卡顿自愈降帧的机制侧，呈递由 golden render_degrade_toggle
/// 护航）：超预算置降级态，新帧起算清零即恢复回升。
#[test]
fn over_budget_frame_degrades_and_resets() {
    let mut sched = fixture();
    sched.advance(0);
    sched.push(Lane::Input);
    assert!(sched.take_frame());
    assert!(!sched.degraded());

    sched.note_frame(&cost(4_000, 4_000));
    assert!(!sched.degraded(), "预算内不降级");

    sched.note_frame(&cost(5_000, 5_000));
    assert!(sched.degraded(), "超预算降级");

    sched.advance(1);
    sched.push(Lane::Input);
    assert!(sched.take_frame());
    assert!(!sched.degraded(), "降级态随新帧清零");
}

/// §3.5 空闲降频：无事件无到期事项时降到空闲节拍。
#[test]
fn idles_at_upper_bound_when_nothing_pending() {
    let mut sched = fixture();
    sched.advance(0);
    assert_eq!(sched.wait_ms(), IDLE_WAIT_MS, "静止界面按空闲节拍等待");

    sched.push(Lane::Stream);
    assert_eq!(sched.wait_ms(), 16, "有待办时等待收窄到合帧窗口");

    sched.advance(16);
    assert!(sched.take_frame(), "窗口到期成帧");
    assert_eq!(sched.wait_ms(), IDLE_WAIT_MS, "清算后回到空闲节拍");
}

/// §5A.3 W4 统一节拍表：启用即到期，每周期至多一次，停用即静默。
#[test]
fn beats_fire_once_per_period_and_stop() {
    let mut sched = fixture();
    sched.advance(0);
    assert_eq!(sched.wait_ms(), IDLE_WAIT_MS, "无启用节拍时不介入等待");

    sched.beat_set(Beat::Hall, true);
    assert_eq!(sched.wait_ms(), 0, "启用即到期，须立即处理");
    let due: Beats = sched.advance(0);
    assert!(due.has(Beat::Hall), "启用后立即到期");
    assert!(!sched.advance(0).has(Beat::Hall), "同周期不重复");

    assert!(!sched.advance(999).has(Beat::Hall));
    assert!(sched.advance(1_000).has(Beat::Hall), "满周期后再次到期");
    assert_eq!(
        sched.wait_ms(),
        IDLE_WAIT_MS,
        "下周期余量超出空闲上限，等待按上限封顶"
    );

    sched.beat_set(Beat::Hall, false);
    assert!(!sched.advance(9_999).has(Beat::Hall), "停用后不再到期");
    assert_eq!(sched.wait_ms(), IDLE_WAIT_MS, "停用后退出等待计算");
}

/// 显式刷新请求：未启用的节拍不因强制而介入。
#[test]
fn forced_beat_requires_active_beat() {
    let mut sched = fixture();
    sched.advance(0);
    sched.beat_now(Beat::Hall);
    assert!(!sched.advance(0).has(Beat::Hall), "未启用不得被强制到期");

    sched.beat_set(Beat::Hall, true);
    sched.advance(0);
    sched.advance(100);
    sched.beat_now(Beat::Hall);
    assert!(sched.advance(100).has(Beat::Hall), "启用后强制立即到期");
}

/// 逐帧留存：聚合口径与环内原始样本出自同一次上报。
#[test]
fn note_frame_retains_raw_sample_in_ring() {
    let mut sched = fixture();
    sched.note_frame(&cost(120, 300));
    sched.note_frame(&cost(80, 200));
    assert_eq!(sched.ring.len(), 2);
    assert_eq!(sched.ring.written(), 2);
    assert_eq!(sched.frames, 2);
}

/// 分位汇总取自环内窗口：未写满时报全部样本，写满后只报最近一个窗口。
#[test]
fn stats_follow_the_retained_window() {
    let mut sched = fixture();
    for i in 1..=100u64 {
        sched.note_frame(&cost(i, 0));
    }
    let compose = sched.stats().stage(Stage::Compose).expect("有样本");
    assert_eq!((compose.p50, compose.p95, compose.peak), (50, 95, 100));
    assert_eq!(sched.stats().whole().expect("整帧").n, 100);

    let mut sched = fixture();
    let total = crate::engine::ring::CAP as u64 + 5;
    for i in 1..=total {
        sched.note_frame(&cost(i, 0));
    }
    let compose = sched.stats().stage(Stage::Compose).expect("有样本");
    assert_eq!(compose.n, crate::engine::ring::CAP, "样本数封顶于容量");
    assert_eq!(compose.peak, total, "峰值即最新一帧");
}

/// 帧度量：按帧数节拍自计数，不因跳帧漂移。
#[test]
fn metrics_count_only_presented_frames() {
    let mut sched = fixture();
    sched.advance(0);
    for i in 0..8u64 {
        sched.advance(i * 100);
        assert!(!sched.take_frame(), "无待办不得成帧");
        sched.note_frame(&cost(100, 200));
    }
    assert_eq!(sched.frames, 8);
    assert_eq!(sched.layout_sum_us, 800);
    assert_eq!(sched.present_sum_us, 1_600);
    assert_eq!(sched.over, 0);
}
