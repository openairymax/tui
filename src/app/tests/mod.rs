// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 应用状态单元测试：与 app 各职责域子模块共享同一模块视图。


// 单元测试按职责域分域：会话与记忆、IME 组合期、视口与渲染、交互流。
mod flow;
mod ime;
mod session;
mod viewport;

use super::*;

// ─────────── W7/W8：IME 组合期（preedit）行为回归 ───────────
// 0.1.18 W8：内置 IME 改为纯 Rust 引擎，恒编入且词典随源码树分发，
// 故不再需要 feature/链接门控。覆盖 CJK 组合期关键路径：
// Ctrl+1 激活 / 字母追加 / 空格·数字选字 / 退格 / Esc 取消 / Enter 提交。

fn app_with_ime() -> (crate::test_env::Home, App) {
    let home = crate::test_env::Home::new("ime");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.ime_engine.is_some(), "App 应加载内置 IME 引擎");
    app.ime_toggle();
    assert!(app.ime_active, "Ctrl+1 应进入拼音态");
    (home, app)
}

fn ime_type(app: &mut App, s: &str) {
    for ch in s.chars() {
        assert!(app.ime_input_char(ch), "拼音态下字母应被消费: {}", ch);
    }
}

// ─────────── U15（0.1.19 L7）：故障呈递语义（pub(super) 真实路径直调）───

fn approval_req(id: &str, tool: &str) -> crate::client::PendingApproval {
    crate::client::PendingApproval {
        request_id: id.to_string(),
        tool: tool.to_string(),
        agent_id: String::new(),
        params: "{}".to_string(),
        created_at: None,
    }
}

