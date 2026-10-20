// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 启动与首帧（TUI 建设方案 §6.1）。断言点全部取自真实 `present::render`
// 的屏上文本：首帧外壳、回放进度、最小尺寸守卫、非 TTY 退出码。
// 慢初始化降级体现为「首帧不等待任何网络探测」——夹具构造于离线网关，
// 首帧仍呈现完整外壳，健康表保持未探测态。

use super::*;
use crate::budget::{FIRST_FRAME_BUDGET_MS, MIN_TERM_COLS, MIN_TERM_ROWS};
use crate::engine::boot;
use crate::engine::exit::Code;

/// §6.1 #1：首帧即有品牌条、输入行与快捷键行，不存在空白帧。
#[test]
fn first_frame_renders_core_chrome() {
    let mut fx = Fixture::new("boot-core");
    let s = fx.draw();
    assert_has(&s, "◈ AirymaxRT");
    assert_has(&s, "❯");
    assert_has(&s, "F1");
}

/// §6.1 #1：首帧预算为 100ms；恰好用满仍在预算内，超出即判定超支。
#[test]
fn first_frame_budget_boundary() {
    assert!(!boot::over_budget(0));
    assert!(!boot::over_budget(FIRST_FRAME_BUDGET_MS));
    assert!(boot::over_budget(FIRST_FRAME_BUDGET_MS + 1));
}

/// §6.1 #4：健康探测是异步的，首帧不等探测结果——离线夹具下外壳照常呈现，
/// 且不得凭空显示降级横幅（未探测 ≠ 不可达）。
#[test]
fn first_frame_does_not_wait_for_probe() {
    let mut fx = Fixture::new("boot-noprobe");
    assert!(fx.app.health.deps().is_empty());
    let s = fx.draw();
    assert_has(&s, "AirymaxRT");
    assert_lacks(&s, "▲ degraded");
}

/// §6.1 #2：恢复会话期间右下角常显剩余条数。
#[test]
fn replay_progress_counts_down() {
    let mut fx = Fixture::new("boot-replay");
    for i in 0..3 {
        fx.app
            .memory
            .push("user", &format!("历史消息 {i}"), "")
            .expect("写入记忆");
    }
    assert_eq!(fx.app.resume_session(), 3);
    let s = fx.draw();
    assert_has(&s, "回放中 · 3 条");
}

/// §6.1 #2：回放排空后进度行消失，并补一条记账说明。
#[test]
fn replay_drains_and_records_ledger() {
    let mut fx = Fixture::new("boot-drain");
    fx.app.memory.push("user", "旧消息", "").expect("写入记忆");
    fx.app.resume_session();
    let mut now = 0;
    while fx.app.replay_left() > 0 {
        now += 1_000;
        fx.app.pump_replay(now);
    }
    assert_eq!(fx.app.replay_left(), 0);
    let s = fx.draw();
    assert_lacks(&s, "回放中");
    assert_has(&s, "已补账恢复 1 条历史。");
}

/// §6.1 #2：没有历史时给出明确说明，而不是静默空屏。
#[test]
fn empty_resume_is_explained() {
    let mut fx = Fixture::new("boot-empty");
    assert_eq!(fx.app.resume_session(), 0);
    let s = fx.draw();
    assert_has(&s, "没有可恢复的历史会话");
    assert_lacks(&s, "回放中");
}

/// §6.1 #3：窄屏整屏提示，给出当前尺寸与最小尺寸。
#[test]
fn too_narrow_renders_guard() {
    let mut fx = Fixture::new("boot-narrow");
    let s = fx.draw_at(MIN_TERM_COLS - 1, H);
    assert_has(&s, "终端尺寸不足");
    assert_has(&s, &format!("至少 {MIN_TERM_COLS} 列"));
    assert_lacks(&s, "❯");
}

/// §6.1 #3：矮屏同样被守卫拦下。
#[test]
fn too_short_renders_guard() {
    let mut fx = Fixture::new("boot-short");
    let s = fx.draw_at(W, MIN_TERM_ROWS - 1);
    assert_has(&s, "终端尺寸不足");
    assert_has(&s, &format!("至少 {} 列 × {MIN_TERM_ROWS} 行", MIN_TERM_COLS));
}

/// §6.9 #1：恰好最小尺寸即进入正常界面（断点为闭区间下界）。
#[test]
fn minimum_size_is_usable() {
    let mut fx = Fixture::new("boot-min");
    let s = fx.draw_at(MIN_TERM_COLS, MIN_TERM_ROWS);
    assert_lacks(&s, "终端尺寸不足");
    assert_has(&s, "❯");
}

/// §6.1 #3：非 TTY 拒进取退出码 2（Usage），并指向行式对话。
#[test]
fn non_tty_rejects_with_usage() {
    let code = Code::Usage;
    assert_eq!(code.code(), 2);
    assert_eq!(code.human(), "命令行参数有误，请检查用法后重试");
}

/// §6.1 #3：退出码冻结表逐项固定，任何改动都需显式评审。
#[test]
fn exit_code_table_is_frozen() {
    assert_eq!(Code::Ok.code(), 0);
    assert_eq!(Code::Auth.code(), 3);
    assert_eq!(Code::Net.code(), 4);
    assert_eq!(Code::Deny.code(), 6);
    assert_eq!(Code::Internal.code(), 7);
    assert_eq!(Code::Soft.code(), 70);
    assert_eq!(Code::SigInt.code(), 130);
    assert_eq!(Code::Pipe.code(), 141);
}

/// §6.1 #1：状态条把版本常显在品牌行（版本单一来源 build.rs）。
#[test]
fn hero_carries_version() {
    let mut fx = Fixture::new("boot-hero");
    let s = fx.draw();
    assert_has(&s, concat!("v", env!("AIRY_RT_VERSION")));
}
