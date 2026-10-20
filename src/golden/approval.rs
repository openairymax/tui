// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 审批与确认流（TUI 建设方案 §6.5）。审批卡由 `present::render` 真实投影
// 到输入栏上方；队列、信息完备、三态直达、决议回显均走产品公开入口。
// 渠道故障（fail-closed，§6.5 #4）与降级角标（§6.12 #5）的呈递语义涉及
// `pub(super)` 入口，按「不为测试放宽可见性」纪律在 `app::tests` 内以
// 真实路径覆盖（approval_unavailable_fails_closed /
// fallback_badge_is_transient_and_once）——golden 不复制产品文案。

use super::*;
use crate::client::PendingApproval;

fn approval(id: &str, tool: &str, agent: &str, params: &str) -> PendingApproval {
    PendingApproval {
        request_id: id.to_string(),
        tool: tool.to_string(),
        agent_id: agent.to_string(),
        params: params.to_string(),
        created_at: Some(1_700_000_000),
    }
}

/// §6.5 #1：多条待决议排队串行出队，卡头给出「1 / N」位置徽标。
#[test]
fn queue_shows_position_badge() {
    let mut fx = Fixture::new("approval-queue");
    fx.app.approvals.push(approval("r1", "file_write", "", "{}"));
    fx.app.approvals.push(approval("r2", "shell_exec", "", "{}"));
    fx.app.approvals.push(approval("r3", "net_get", "", "{}"));
    let s = fx.draw();
    assert_has(&s, "工具审批请求");
    assert_has(&s, "1 / 3");
    assert_has(&s, "工具: file_write");
    assert_lacks(&s, "shell_exec");
}

/// §6.5 #2：审批卡信息完备——参数预览、主体、工具与请求时刻同屏。
#[test]
fn card_carries_full_context() {
    let mut fx = Fixture::new("approval-card");
    fx.app.approvals.push(approval(
        "r1",
        "file_write",
        "agents/coder",
        r#"{"path":"src/main.rs","op":"overwrite"}"#,
    ));
    let s = fx.draw();
    assert_has(&s, "主体: agents/coder");
    assert_has(&s, "工具: file_write");
    assert_has(&s, "\"path\":\"src/main.rs\"");
    assert_has(&s, "时间:");
}

/// §6.5 #3：三态决议键同屏常显，Esc 明确等价于拒绝。
#[test]
fn tri_state_keys_are_visible() {
    let mut fx = Fixture::new("approval-keys");
    fx.app.approvals.push(approval("r1", "file_write", "", "{}"));
    let s = fx.draw();
    assert_has(&s, "[y] 允许本次");
    assert_has(&s, "[a] 本会话总是");
    assert_has(&s, "[n] 拒绝（Esc）");
}

/// §6.5 #1/#3：决议后立即出队并回显决议结果，下一张卡顶到队首。
#[tokio::test]
async fn decision_clears_head_and_echoes() {
    let mut fx = Fixture::new("approval-allow");
    fx.app.approvals.push(approval("r1", "file_write", "", "{}"));
    fx.app.approvals.push(approval("r2", "shell_exec", "", "{}"));
    fx.app.approve_request("allow");
    assert_eq!(fx.app.approvals.len(), 1);

    let s = fx.draw();
    assert_has(&s, "已允许本次执行工具「file_write」");
    assert_has(&s, "工具: shell_exec");
    assert_lacks(&s, "1 / 2");
}

/// §6.5 #3/#4：用户拒绝以 `rejected` 语义呈递，与渠道故障的 `unavailable`
/// 分属不同标注——两者都绝不放行。
#[tokio::test]
async fn deny_marks_rejected() {
    let mut fx = Fixture::new("approval-deny");
    fx.app.approvals.push(approval("r1", "shell_exec", "", "{}"));
    fx.app.approve_request("deny");
    assert!(fx.app.approvals.is_empty());

    let s = fx.draw();
    assert_has(&s, "已拒绝工具「shell_exec」（rejected：用户拒绝）");
    assert_lacks(&s, "工具审批请求");
}

/// §6.5 #5：审批等待不阻塞后台输出，也不遮挡输入行。
#[test]
fn approval_does_not_block_output() {
    let mut fx = Fixture::new("approval-flow");
    add(&mut fx.app, MessageRole::Agent, "后台日志仍在滚动");
    fx.app.approvals.push(approval("r1", "file_write", "", "{}"));
    let s = fx.draw();
    assert_has(&s, "后台日志仍在滚动");
    assert_has(&s, "工具审批请求");
    assert_has(&s, "❯");
}
