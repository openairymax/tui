// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话落盘/删除/重命名回归：断言读回 sessions.json 与还原后的 tab 状态，
// 不断言"面板提示了成功"——落盘类功能最容易出现"提示成功、文件没动"。

use super::*;

fn app() -> App {
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    App::new("agents/main.agent.yaml", gw)
}

#[test]
fn sess_persist_restore_roundtrip() {
    let _home = crate::test_env::Home::new("sess-roundtrip");
    let mut a = app();
    a.add_message(MessageRole::User, "第一轮".to_string());
    a.new_session_tab();
    a.add_message(MessageRole::User, "第二轮".to_string());
    a.persist_tabs();

    let mut b = app();
    b.restore_tabs();
    assert_eq!(b.tab_count(), 2, "两个有内容的会话应全部还原");
    let first: Vec<String> = b
        .session_tabs
        .get(0)
        .map(|t| t.messages.iter().map(|m| m.content.clone()).collect())
        .unwrap_or_default();
    assert!(
        first.iter().any(|c| c == "第一轮"),
        "槽 0 应含主会话消息"
    );
    assert!(
        b.messages.iter().any(|m| m.content == "第二轮"),
        "当前会话（槽 1）内容应还原到主字段"
    );
}

#[test]
fn sess_empty_tab_not_persisted() {
    let _home = crate::test_env::Home::new("sess-empty");
    let mut a = app();
    a.persist_tabs();
    let mut b = app();
    b.restore_tabs();
    assert_eq!(b.tab_count(), 1, "空会话不落盘，还原后保持初始单 tab");
    assert!(b.messages.is_empty());
}

#[test]
fn sess_close_removes_tab_and_switches() {
    let _home = crate::test_env::Home::new("sess-close");
    let mut a = app();
    a.add_message(MessageRole::User, "keep".to_string());
    a.new_session_tab();
    a.add_message(MessageRole::User, "gone".to_string());
    assert_eq!(a.tab_count(), 2);
    a.close_tab();
    assert_eq!(a.tab_count(), 1, "关闭后只剩主会话");
    assert_eq!(a.active_tab, None, "关闭末 tab 后回落到槽 0");
    assert!(
        a.messages.iter().any(|m| m.content == "keep"),
        "主会话内容不受影响"
    );
}

#[test]
fn sess_close_main_clears_in_place() {
    let _home = crate::test_env::Home::new("sess-close-main");
    let mut a = app();
    a.add_message(MessageRole::User, "x".to_string());
    a.close_tab();
    assert_eq!(a.tab_count(), 1);
    assert!(
        !a.messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "x"),
        "主会话关闭语义 = 清空对话"
    );
}

#[test]
fn sess_rename_updates_title_and_persists() {
    let _home = crate::test_env::Home::new("sess-rename");
    let mut a = app();
    a.add_message(MessageRole::User, "hi".to_string());
    a.rename_tab("部署排查");
    assert_eq!(a.tab_title(0), "部署排查");
    let mut b = app();
    b.restore_tabs();
    assert_eq!(b.tab_title(0), "部署排查", "重命名必须随落盘持久");
}
