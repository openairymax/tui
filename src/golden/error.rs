// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 错误、断线与自愈（TUI 建设方案 §6.6）。五条判据全部走产品真实路径：
// fail_card 生成可行动卡片并交给 System 消息折叠器、Link 三态机驱动状态灯、
// ExitGuard 裁决双击退出、冻结表翻译退出码、回放泵复用历史时间戳。
// 断言只读取屏上文案与状态机输出，不复制产品文案常量。

use std::time::{Duration, Instant};

use super::*;
use crate::budget::{DOUBLE_ESCAPE_WINDOW_MS, REPLAY_INTERVAL_MS};
use crate::engine::exit::Code;

/// §6.6 #1：错误先显摘要与修复命令，原始全文折叠；诊断附注不顶替原始错误。
#[test]
fn raw_error_is_folded_not_replaced() {
    let mut fx = Fixture::new("error-fold");
    let raw = "HTTP 429 rate limit exceeded\nretry-after: 30\nbody: {\"error\":\"quota\"}\n\
               x-req-id: abc123\nserver: llm_d\nhint: slow down";
    fx.app.fail_card(&anyhow::anyhow!(raw));

    let s = fx.draw();
    assert_has(&s, "请求失败：后端限流，请稍后重试");
    assert_has(&s, "修复：airymaxrt doctor");
    assert_has(&s, "原始错误（Alt+O 展开）：");
    assert_has(&s, "more lines · Alt+O 展开");
    assert_lacks(&s, "retry-after: 30");

    fx.app.browse_expanded = true;
    let full = fx.draw();
    assert_has(&full, "retry-after: 30");
    assert_has(&full, "server: llm_d");
    assert_lacks(&full, "more lines · Alt+O 展开");
}

/// §6.6 #1：同类错误再次发生只累加计数并指路 /health，不重复刷屏全文。
#[test]
fn repeated_cause_accumulates_count() {
    let mut fx = Fixture::new("error-count");
    fx.app.fail_card(&anyhow::anyhow!("HTTP 429 too many requests"));
    fx.app.fail_card(&anyhow::anyhow!("HTTP 429 too many requests"));
    let s = fx.draw();
    assert_has(&s, "同类错误已出现 2 次（/health 查看聚合）");
}

/// §6.6 #2：连接三态在 chrome 常显，断线自愈全过程可见。
#[test]
fn link_cycles_through_three_states() {
    let mut fx = Fixture::new("error-link");
    assert_has(&fx.draw(), "connecting");

    fx.app.link.observe(false, 0);
    assert_has(&fx.draw(), "reconnecting · 1");
    fx.app.link.observe(false, 0);
    assert_has(&fx.draw(), "reconnecting · 2");

    fx.app.link.observe(true, 3);
    assert_has(&fx.draw(), "replayed · 3");
    assert!(fx.app.link.is_online(), "补账态仍可承载交互");

    fx.app.link.observe(true, 3);
    let s = fx.draw();
    assert_has(&s, "ONLINE");
    assert_lacks(&s, "replayed");
}

/// §6.6 #3：Ctrl+C 双击才退出，500ms 窗口外重新计时。
#[test]
fn double_ctrl_c_guards_exit() {
    let mut fx = Fixture::new("error-exit");
    assert_eq!(DOUBLE_ESCAPE_WINDOW_MS, 500);
    let t0 = Instant::now();

    assert!(!fx.app.confirm_exit(t0), "首次按下只提示");
    assert_has(&fx.draw(), "再按一次 Ctrl+C 退出");
    assert!(
        fx.app.confirm_exit(t0 + Duration::from_millis(400)),
        "窗口内二次按下确认退出"
    );
    assert!(
        !fx.app.confirm_exit(t0 + Duration::from_secs(1)),
        "窗口外视为新的误触，重新计时"
    );
}

/// §6.6 #4：非零退出按冻结表映射人话，不裸抛数字。
#[test]
fn exit_codes_map_to_human_text() {
    assert_eq!(Code::Net.code(), 4);
    assert_eq!(Code::Deny.code(), 6);
    assert_eq!(Code::SigInt.code(), 130);
    assert_eq!(Code::Pipe.code(), 141);
    assert!(Code::Net.human().contains("网关"));
    assert!(Code::Deny.human().contains("权限"));
    assert!(Code::SigInt.human().contains("中断"));
    assert!(
        Code::Net.human().chars().any(|c| !c.is_ascii_digit()),
        "人话文案不得只剩数字"
    );
}

/// §6.6 #5：补账恢复复用历史时间戳，永不伪造当前时间。
#[test]
fn replay_reuses_original_stamps() {
    let mut fx = Fixture::new("error-replay");
    assert!(fx.app.memory.push("user", "上次的问题", "chat").is_ok());
    assert!(fx
        .app
        .memory
        .push("assistant", "上次的答复", "chat")
        .is_ok());
    let recs = fx.app.memory.recent(2);
    assert_eq!(recs.len(), 2);

    assert_eq!(fx.app.resume_session(), 2);
    for step in 1..=3 {
        fx.app.pump_replay(REPLAY_INTERVAL_MS * step);
    }

    let s = fx.draw();
    assert_has(&s, "已补账恢复 2 条历史。");
    assert_has(&s, "上次的问题");
    assert_has(&s, "上次的答复");

    let restored: Vec<&str> = fx
        .app
        .messages
        .iter()
        .filter(|m| m.content == "上次的问题" || m.content == "上次的答复")
        .map(|m| m.timestamp.as_str())
        .collect();
    assert_eq!(restored.len(), 2);
    for stamp in restored {
        assert!(
            recs.iter().any(|r| r.timestamp == stamp),
            "时间戳须原样复用历史记录：{stamp}"
        );
        assert_ne!(stamp, "—", "历史时间戳不得丢失");
    }
}
