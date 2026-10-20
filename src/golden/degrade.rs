// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 降级与自愈能力矩阵（TUI 建设方案 §6.11 / §6.12）：依赖降级横幅全生命周期、
// 恢复撤横幅与提示、多依赖混合面板、空态组合、错误限流聚合、抖动冷却抑制与
// 冷却后重弹。断言以真实时钟注入驱动，不使用测试旁路改写渲染结果。

use super::*;
use crate::app::ActivePanel;
use crate::budget::DEGRADED_BANNER_COOLDOWN_MS;

/// §6.11 #1：后端降级先出横幅，持续超过 10s 才追加时长标注。
#[test]
fn banner_lifecycle_and_age() {
    let mut fx = Fixture::new("degrade-banner");
    fx.app.set_clock(1_000);
    fx.app.note_dep("llm", false, 1_000, None);
    fx.app.sync_banner(1_000);
    // 状态条右端的降级徽章在 100 列处被硬截断，故用宽视口覆盖其在场。
    let s = fx.draw_at(140, H);
    assert_has(&s, "LLM 后端未连接 · 已切换本地降级路径");
    assert_has(&s, "修复：airymaxrt logs llm_d");
    assert_has(&s, "▲ degraded: LLM 后端");
    assert_lacks(&s, "已持续");

    fx.app.set_clock(12_000);
    let s = fx.draw_at(140, H);
    assert_has(&s, "已持续 11s");
}

/// §6.11 #2：降级解除后横幅撤下，并以提示告知恢复。
#[test]
fn banner_restore_clears_and_notifies() {
    let mut fx = Fixture::new("degrade-restore");
    fx.app.set_clock(1_000);
    fx.app.note_dep("llm", false, 1_000, None);
    fx.app.sync_banner(1_000);
    let s = fx.draw();
    assert_has(&s, "LLM 后端未连接 · 已切换本地降级路径");

    fx.app.set_clock(2_000);
    fx.app.note_dep("llm", true, 2_000, None);
    fx.app.sync_banner(2_000);
    let s = fx.draw();
    assert_lacks(&s, "LLM 后端未连接");
    assert_has(&s, "LLM 后端已恢复");
}

/// §6.12 #1：多依赖混合时面板按依赖维度汇总，状态条只报存在降级。
#[test]
fn health_panel_mixed_deps() {
    let mut fx = Fixture::new("degrade-mixed");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.set_clock(1_000);
    fx.app.note_dep("gateway", false, 1_000, None);
    fx.app.note_dep("llm", true, 1_000, None);
    let s = fx.draw();
    assert_has(&s, "▲ 存在降级");
    assert_has(&s, "2 项依赖 · 1 项降级 · 1 次错误");
    assert_has(&s, "网关");
    assert_has(&s, "LLM 后端");
}

/// §6.12 #2：从未探测时面板给出空态指引，状态条仍报全部正常。
#[test]
fn health_panel_empty_state() {
    let mut fx = Fixture::new("degrade-empty");
    fx.app.active_panel = ActivePanel::Health;
    let s = fx.draw();
    assert_has(&s, "尚无依赖探测结果");
    assert_has(&s, "● 全部正常");
    assert_has(&s, "0 项依赖 · 0 项降级 · 0 次错误");
    assert_has(&s, "健康探测每 5s 自动执行一次");
}

/// §6.11 #3：限流错误在卡片与面板两处冗余编码，聚合计数一致。
#[test]
fn quota_error_aggregates_both_surfaces() {
    let mut fx = Fixture::new("degrade-quota");
    fx.app.note_dep("llm", true, 1_000, None);
    fx.app.set_clock(2_000);
    fx.app.fail_card(&anyhow::anyhow!("HTTP 429 too many requests"));
    fx.app.fail_card(&anyhow::anyhow!("HTTP 429 too many requests"));

    fx.app.active_panel = ActivePanel::Chat;
    let s = fx.draw();
    assert_has(&s, "请求失败：后端限流，请稍后重试");
    assert_has(&s, "同类错误已出现 2 次");

    fx.app.active_panel = ActivePanel::Health;
    let s = fx.draw();
    assert_has(&s, "错误聚合（1 类，共 2 次）");
    assert_has(&s, "×2");
    assert_has(&s, "后端限流");
    assert_has(&s, "修复：airymaxrt doctor");
}

/// §6.11 #4：冷却期内抖动不重弹横幅，只累计抑制计数。
#[test]
fn banner_cooldown_suppresses_flap() {
    let mut fx = Fixture::new("degrade-flap");
    fx.app.active_panel = ActivePanel::Health;
    fx.app.set_clock(1_000);
    fx.app.note_dep("llm", false, 1_000, None);
    fx.app.sync_banner(1_000);
    fx.app.set_clock(2_000);
    fx.app.note_dep("llm", true, 2_000, None);
    fx.app.sync_banner(2_000);
    fx.app.set_clock(3_000);
    fx.app.note_dep("llm", false, 3_000, None);
    fx.app.sync_banner(3_000);
    let s = fx.draw();
    assert_lacks(&s, "LLM 后端未连接");
    assert_has(&s, "降级横幅冷却期抑制重弹 1 次");
}

/// §6.11 #5：冷却期满后再次降级应正常重弹横幅。
#[test]
fn banner_reappears_after_cooldown() {
    let mut fx = Fixture::new("degrade-relapse");
    fx.app.set_clock(1_000);
    fx.app.note_dep("llm", false, 1_000, None);
    fx.app.sync_banner(1_000);
    fx.app.set_clock(2_000);
    fx.app.note_dep("llm", true, 2_000, None);
    fx.app.sync_banner(2_000);
    assert_eq!(DEGRADED_BANNER_COOLDOWN_MS, 5_000);

    fx.app.set_clock(8_000);
    fx.app.note_dep("llm", false, 8_000, None);
    fx.app.sync_banner(8_000);
    let s = fx.draw();
    assert_has(&s, "LLM 后端未连接 · 已切换本地降级路径");
}
