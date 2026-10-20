// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 观测与诊断能力矩阵（TUI 建设方案 §6.12 / §6.13）：墙钟回拨标注、正常时间线
// 静默、面板统计聚合、诊断快照的头部/依赖/错误三段、错误条目淘汰溢出桶，以及
// 探测失败到错误聚合的真实接线。诊断快照断言只针对结构性字段，原始错误串与
// 会话正文一律不入报告。

use super::*;
use crate::app::ActivePanel;
use crate::engine::health::{diag_report, Cause, ErrAgg, HealthTable, Stamp, Timeline};

/// §6.13 #1：墙钟回拨按单调序修正并在面板标注回拨量。
#[test]
fn timeline_rollback_is_annotated() {
    let mut fx = Fixture::new("observe-skew");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.note_dep("llm", true, 1_000, None);
    fx.app.timeline.observe(100_000, 1);
    fx.app.timeline.observe(94_000, 2);
    fx.app.clock_ms = 2;
    let s = fx.draw();
    assert_has(&s, "时钟已校准（回拨 ≈6s）");
}

/// §6.13 #1：未发生回拨时不得出现校准标注。
#[test]
fn timeline_normal_stays_silent() {
    let mut fx = Fixture::new("observe-normal");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.set_clock(5_000);
    fx.app.note_dep("llm", true, 1_000, None);
    let s = fx.draw();
    assert_lacks(&s, "时钟已校准");
}

/// §6.12 #3：面板统计行按依赖数与降级数聚合。
#[test]
fn panel_stat_line_aggregates_deps() {
    let mut fx = Fixture::new("observe-stat");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.set_clock(1_000);
    fx.app.note_dep("llm", true, 1_000, None);
    let s = fx.draw();
    assert_has(&s, "● 全部正常");
    assert_has(&s, "1 项依赖 · 0 项降级 · 0 次错误");
    assert_has(&s, "LLM 后端");
}

/// §6.13 #5：诊断导出头部只含采集时刻、单调序与时间线状态（脱敏快照）。
#[test]
fn diag_report_header_segment() {
    let stamp = Stamp {
        order_ms: 42,
        corrected: false,
    };
    let tl = Timeline::new();
    let health = HealthTable::new();
    let errs = ErrAgg::new();
    let text = diag_report(&stamp, &tl, &health, &errs, "08:30:00", 0);
    assert!(text.contains("AirymaxRT 诊断快照"));
    assert!(text.contains("采集时刻：08:30:00"));
    assert!(text.contains("单调序：42ms"));
    assert!(text.contains("时间线：正常"));
}

/// §6.13 #2：诊断导出错误段按类聚合，同类限流错误合并计数（×N）。
#[test]
fn diag_report_error_segment() {
    let stamp = Stamp::default();
    let tl = Timeline::new();
    let health = HealthTable::new();
    let mut errs = ErrAgg::new();
    errs.record("HTTP 429 rate limited", 500);
    errs.record("too many requests, slow down", 1_000);
    let text = diag_report(&stamp, &tl, &health, &errs, "08:30:00", 2_000);
    assert!(text.contains("错误聚合（1 类，共 2 次）"));
    assert!(text.contains("×2 quota"));
    assert!(text.contains("后端限流"));
}

/// §6.13 #5：诊断导出依赖段（依赖健康快照）逐项标注可达性与失败/抖动计数。
#[test]
fn diag_report_dependency_segment() {
    let stamp = Stamp::default();
    let tl = Timeline::new();
    let mut health = HealthTable::new();
    health.observe("gateway", true, 1_000);
    health.observe("llm", false, 60_000);
    let errs = ErrAgg::new();
    let text = diag_report(&stamp, &tl, &health, &errs, "08:30:00", 60_000);
    assert!(text.contains("依赖健康（2 项，1 项降级）"));
    assert!(text.contains("✓ gateway"));
    assert!(text.contains("✗ llm"));
    assert!(text.contains("（失败 1 次 / 抖动 0 次）"));
}

/// §6.13 #4：错误聚合表有界，超限最旧条目并入溢出桶，计数不失真。
#[test]
fn diag_report_overflow_bucket() {
    let mut fx = Fixture::new("observe-overflow");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.note_dep("llm", true, 1_000, None);
    for i in 0..45u64 {
        let code: &'static str = Box::leak(format!("e{i:02}").into_boxed_str());
        fx.app.err_agg.record_cause(
            Cause {
                code,
                label: "测试错误",
                remedy: "airymaxrt doctor",
            },
            1_000 + i,
        );
    }
    fx.app.health_scroll = 0;
    let s = fx.draw();
    assert_has(&s, "错误聚合（31 类，共 45 次）");

    fx.app.health_scroll = 9_999;
    let s = fx.draw();
    assert_has(&s, "其他合并 14 次");
    assert_has(&s, "Esc 返回对话");
}

/// §6.12 #4：探测转为不可达时自动记入错误聚合并在面板可查。
#[test]
fn note_dep_down_records_unreachable() {
    let mut fx = Fixture::new("observe-unreach");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.set_clock(1_000);
    fx.app.note_dep("gateway", false, 1_000, None);
    let s = fx.draw();
    assert_has(&s, "▲ 存在降级");
    assert_has(&s, "1 项依赖 · 1 项降级 · 1 次错误");
    assert_has(&s, "×1");
    assert_has(&s, "无法连接后端服务");
    assert_has(&s, "修复：airymaxrt logs llm_d");
}
