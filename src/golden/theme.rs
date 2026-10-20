// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 主题与形态能力矩阵（TUI 建设方案 §6.7 / §6.10）：深/浅主题整屏配色差分、
// 环境自动探测链、显式覆盖优先级、品牌徽记对比度、网关降级双通道冗余编码、
// 色深与取色形态一致、链路指示三态。断言用自建 TestBackend 收集真实前景/
// 背景指纹，避免只看符号文本而漏掉配色差异。

use super::*;
use crate::engine::link::Link;
use crate::style::{apply_scheme, init_from_env, text, ColorDepth, ThemeMode};
use ratatui::style::Color;

/// 整屏配色指纹：逐格收集（前景, 背景），供主题切换做逐格差分。
fn color_fingerprint(app: &mut App) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(W, H)).expect("测试终端");
    term.draw(|f| crate::present::render(f, app))
        .expect("渲染一帧");
    let buf = term.backend().buffer();
    let mut out = Vec::with_capacity((W as usize) * (H as usize));
    for y in 0..H {
        for x in 0..W {
            let cell = &buf[(x, y)];
            out.push(format!("{:?}|{:?}", cell.fg, cell.bg));
        }
    }
    out
}

/// §6.7 #1：深/浅主题切换必须改变整屏配色，且回到深色逐格还原。
#[test]
fn theme_switch_differs_then_restores() {
    let mut fx = Fixture::new("theme-diff");
    ThemeMode::Dark.set();
    let dark1 = color_fingerprint(&mut fx.app);
    ThemeMode::Light.set();
    let light = color_fingerprint(&mut fx.app);
    ThemeMode::Dark.set();
    let dark2 = color_fingerprint(&mut fx.app);
    assert_ne!(dark1, light, "浅色主题应改变整屏配色");
    assert_eq!(dark1, dark2, "回到深色应逐格还原");
}

/// §6.7 #2：显式环境变量优先，探测链按「显式 → COLORFGBG → 默认深色」收敛。
#[test]
fn theme_env_probe_chain() {
    let _fx = Fixture::new("theme-env");
    ThemeMode::Dark.set();
    std::env::set_var("AIRY_TUI_THEME", "dark");
    assert!(!apply_scheme(true), "显式深色下浅色探测必须被拒");
    assert!(!apply_scheme(false));
    assert_eq!(ThemeMode::current(), ThemeMode::Dark);

    init_from_env();
    assert_eq!(ThemeMode::current(), ThemeMode::Dark);

    std::env::set_var("AIRY_TUI_THEME", "light");
    init_from_env();
    assert_eq!(ThemeMode::current(), ThemeMode::Light);

    std::env::remove_var("AIRY_TUI_THEME");
    std::env::remove_var("COLORFGBG");
    init_from_env();
    assert_eq!(ThemeMode::current(), ThemeMode::Dark, "无线索时回落默认深色");

    ThemeMode::Dark.set();
}

/// §6.7 #3：无显式覆盖时，探测结果真实生效且同值重设被识别为无变化。
#[test]
fn theme_probe_applies_without_override() {
    let _fx = Fixture::new("theme-probe");
    std::env::remove_var("AIRY_TUI_THEME");
    std::env::remove_var("COLORFGBG");
    ThemeMode::Dark.set();
    assert!(apply_scheme(true), "浅色探测应生效");
    assert_eq!(ThemeMode::current(), ThemeMode::Light);
    assert!(apply_scheme(false), "深色探测应生效");
    assert_eq!(ThemeMode::current(), ThemeMode::Dark);
    assert!(!apply_scheme(false), "同值重设不应触发重绘");
}

/// §6.10 #1：品牌徽记以对比色块呈现，前景与背景必须可区分。
#[test]
fn brand_badge_contrast() {
    let mut fx = Fixture::new("theme-brand");
    ThemeMode::Dark.set();
    let mut term = Terminal::new(TestBackend::new(W, H)).expect("测试终端");
    term.draw(|f| crate::present::render(f, &mut fx.app))
        .expect("渲染一帧");
    let buf = term.backend().buffer();
    let mut found = false;
    for y in 0..H {
        for x in 0..W {
            let cell = &buf[(x, y)];
            if cell.symbol() == "◈" {
                assert_ne!(
                    format!("{:?}", cell.fg),
                    format!("{:?}", cell.bg),
                    "品牌徽记需前景背景可区分"
                );
                found = true;
            }
        }
    }
    assert!(found, "品牌徽记 ◈ 未渲染");
}

/// §6.10 #2：网关降级在横幅与状态条两处冗余编码，语义一致。
#[test]
fn gateway_degrade_redundant_encoding() {
    let mut fx = Fixture::new("theme-degrade");
    fx.app.set_clock(1_000);
    fx.app.note_dep("gateway", false, 1_000, None);
    fx.app.sync_banner(1_000);
    // 状态条右端的降级徽章在 100 列处被硬截断，故用宽视口断言双通道冗余编码。
    let s = fx.draw_at(140, H);
    assert_has(&s, "网关不可达 · 正在重连");
    assert_has(&s, "▲ degraded: 网关");
}

/// §6.7 #4：色深与取色形态必须一致（真彩深下全局取色即为 RGB）。
#[test]
fn depth_and_palette_agree() {
    assert_eq!(
        matches!(ColorDepth::current(), ColorDepth::TrueColor),
        matches!(text(), Color::Rgb(..)),
        "取色形态应与色深一致"
    );
}

/// §6.10 #3：链路指示灯三态对应「在线/重连中/已续传」文案。
#[test]
fn link_indicator_three_states() {
    let mut fx = Fixture::new("theme-link");
    fx.app.link = Link::Online;
    let s = fx.draw();
    assert_has(&s, "● ONLINE");

    fx.app.link = Link::Retry(2);
    let s = fx.draw();
    assert_has(&s, "reconnecting · 2");

    fx.app.link = Link::Replay(3);
    let s = fx.draw();
    assert_has(&s, "replayed · 3");
}
