// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// health 模块单测：状态机迁移、横幅冷却、成因归类、聚合淘汰、时间线与
// 诊断脱敏。全部用注入时刻重放，不读墙钟。

use super::*;
use crate::budget::{MAX_ERROR_AGG_ENTRIES, MAX_HEALTH_PANEL_ITEMS};
use agentrt_rs::run_stream::gen;

/// §6.12 #3/§6.13 #3 状态机侧：last_ok 单调时刻随探测更新，
/// ok_age/down_age 按注入时刻投影，不下发墙钟。
#[test]
fn table_reports_transitions() {
    let mut t = HealthTable::new();
    assert_eq!(t.observe("llm", true, 1_000), Change::Same);
    assert_eq!(t.observe("llm", false, 2_000), Change::Down);
    assert_eq!(t.observe("llm", false, 3_000), Change::Same);
    assert_eq!(t.observe("llm", true, 4_000), Change::Up);
    let Some(dep) = t.get("llm") else {
        panic!("llm 未登记");
    };
    assert!(dep.up);
    assert_eq!(dep.flaps, 1);
    assert_eq!(dep.fails, 2);
    assert_eq!(dep.ok_age(7_000), Some(3_000));
    assert_eq!(dep.down_age(7_000), None);
}

#[test]
fn worst_picks_earliest_failure() {
    let mut t = HealthTable::new();
    t.observe("gateway", false, 5_000);
    t.observe("llm", false, 3_000);
    t.observe("tool", true, 2_000);
    assert_eq!(t.worst(), Some("llm"));
    assert!(t.degraded());
    assert!(t.is_down("gateway"));
    assert!(!t.is_down("tool"));
    assert!(!t.is_down("never-probed"));
}

#[test]
fn table_is_bounded() {
    let mut t = HealthTable::new();
    for i in 0..MAX_HEALTH_PANEL_ITEMS {
        t.observe(&format!("d{i}"), false, i as u64);
    }
    assert_eq!(t.observe("extra", false, 99), Change::Same);
    assert_eq!(t.deps().len(), MAX_HEALTH_PANEL_ITEMS);
    assert!(t.get("extra").is_none());
}

#[test]
fn classify_handles_daemon_chinese_digest() {
    // daemon 侧 fail_text 下发中文摘要，若分类只认英文字面量，用户看到的
    // 就是「未知错误」。这三条是 0.1.21 生产反馈的原始场景。
    let unreachable = "失败原因：模型服务不可达；下一步：可原样重试";
    assert_eq!(classify(unreachable).code, "unreachable");
    let rejected = "失败原因：模型服务拒绝了请求（检查 API Key / 模型名 / 配额）";
    assert_eq!(classify(rejected).code, "rejected");
    let config = "no model configured: set $AIRY_CONFIG_DIR/model.yaml";
    assert_eq!(classify(config).code, "config");
    assert_eq!(classify("请求超时").code, "timeout");
}

#[test]
fn wire_reason_outranks_summary() {
    // 摘要文案会被改写/本地化，归类只认载荷 reason（机制层权威）。
    let mut agg = ErrAgg::new();
    let c = agg.record_wire(Some(gen::AIRY_RS_REASON_PROVIDER_ERR), "请求超时", 1_000);
    assert_eq!(c.code, "rejected");
    // 未登记的原因回落关键词归类，新原因上线不被吞掉。
    let c = agg.record_wire(Some("vendor_new_reason"), "connection refused", 2_000);
    assert_eq!(c.code, "unreachable");
    // 旧引擎无载荷、或载荷 reason 为空串：与 record 的关键词路径一致。
    let c = agg.record_wire(Some(""), "未配置可用模型", 3_000);
    assert_eq!(c.code, "config");
}

#[test]
fn banner_shows_restores_and_cools() {
    let mut h = HealthTable::new();
    let mut b = Banner::default();
    h.observe("llm", false, 1_000);
    assert_eq!(b.sync(h.worst(), 1_000), BannerAct::Show);
    assert!(b.visible());
    assert!(b.text().contains("LLM"));
    assert_eq!(b.remedy(), "airymaxrt logs llm_d");

    assert_eq!(b.sync(h.worst(), 1_500), BannerAct::None);

    h.observe("llm", true, 2_000);
    assert_eq!(b.sync(h.worst(), 2_000), BannerAct::Restored);
    assert!(!b.visible());
    assert!(b.text().is_empty());

    // 冷却期（2s + 5s）内重弹被抑制，抖动计数递增。
    h.observe("llm", false, 3_000);
    assert_eq!(b.sync(h.worst(), 3_000), BannerAct::None);
    assert_eq!(b.flaps(), 1);

    h.observe("llm", true, 3_500);
    assert_eq!(b.sync(h.worst(), 3_500), BannerAct::None);
    h.observe("llm", false, 9_000);
    assert_eq!(b.sync(h.worst(), 9_000), BannerAct::Show);
}

#[test]
fn banner_switches_to_new_worst() {
    let mut h = HealthTable::new();
    let mut b = Banner::default();
    h.observe("llm", false, 1_000);
    assert_eq!(b.sync(h.worst(), 1_000), BannerAct::Show);
    assert_eq!(b.dep(), Some("llm"));
    h.observe("gateway", false, 500);
    assert_eq!(b.sync(h.worst(), 2_000), BannerAct::Show);
    assert_eq!(b.dep(), Some("gateway"));
    assert!(b.text().contains("网关"));
}

#[test]
fn err_agg_counts_same_code() {
    let mut a = ErrAgg::new();
    a.record("HTTP 429 Too Many Requests", 1_000);
    a.record("rate limit exceeded", 2_000);
    assert_eq!(a.len(), 1);
    assert_eq!(a.total(), 2);
    let Some(e) = a.entries().first() else {
        panic!("聚合条目缺失");
    };
    assert_eq!(e.code, "quota");
    assert_eq!(e.count, 2);
    assert_eq!(e.first_ms, 1_000);
    assert_eq!(e.last_ms, 2_000);
}

#[test]
fn err_agg_evicts_oldest_into_overflow() {
    let mut a = ErrAgg::new();
    let rounds = MAX_ERROR_AGG_ENTRIES + 8;
    for i in 0..rounds {
        let code: &'static str = Box::leak(format!("c{i}").into_boxed_str());
        a.record_cause(
            Cause {
                code,
                label: "x",
                remedy: "y",
            },
            i as u64,
        );
    }
    assert_eq!(a.len(), MAX_ERROR_AGG_ENTRIES - 1);
    assert_eq!(a.overflow() as usize, rounds - (MAX_ERROR_AGG_ENTRIES - 1));
    assert_eq!(a.total() as usize, rounds);
}

#[test]
fn classify_covers_known_causes() {
    assert_eq!(classify("HTTP 429").code, "quota");
    assert_eq!(classify("Unauthorized: invalid api key").code, "auth");
    assert_eq!(classify("model not found").code, "model");
    assert_eq!(classify("request timed out").code, "timeout");
    assert_eq!(classify("connection refused").code, "unreachable");
    assert_eq!(classify("HTTP 503 service unavailable").code, "backend");
    assert_eq!(classify("weird failure").code, "unknown");
}

#[test]
fn timeline_detects_backstep() {
    let mut t = Timeline::new();
    assert!(!t.observe(10_000, 1_000).corrected);
    let s = t.observe(8_000, 2_000);
    assert!(s.corrected);
    assert_eq!(s.order_ms, 2_000);
    let s = t.observe(8_500, 2_500);
    assert!(!s.corrected);
    assert_eq!(s.order_ms, 2_500);
    let Some(note) = t.note() else {
        panic!("回拨未记录");
    };
    assert!(note.contains("回拨"));
    assert!(note.contains("2s"));
}

#[test]
fn dur_cn_formats_scales() {
    assert_eq!(dur_cn(900), "0s");
    assert_eq!(dur_cn(5_000), "5s");
    assert_eq!(dur_cn(65_000), "1m5s");
    assert_eq!(dur_cn(3_725_000), "1h2m");
}

#[test]
fn diag_report_is_redacted() {
    let mut h = HealthTable::new();
    h.observe("llm", false, 10_000);
    h.observe("tool", true, 10_000);
    let mut a = ErrAgg::new();
    a.record("HTTP 503 upstream body=secret-token", 10_500);
    let tl = Timeline::new();
    let stamp = Stamp {
        order_ms: 11_000,
        corrected: false,
    };
    let out = diag_report(&stamp, &tl, &h, &a, "12:00:00", 11_000);
    assert!(out.contains("llm"));
    assert!(out.contains("tool"));
    assert!(out.contains("×1 backend"));
    assert!(out.contains("12:00:00"));
    assert!(!out.contains("secret-token"));
}
