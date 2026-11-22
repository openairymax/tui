// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话布局/历史回归（0.1.21 D-14）：sessions.json 只存布局，正文经 mem_d
// 还原。断言读回文件与还原后的 tab 状态，不断言"面板提示了成功"——落盘类
// 功能最容易出现"提示成功、文件没动"。

use super::*;

fn app() -> App {
    App::new("agents/main.agent.yaml")
}

/// 以纯内存记忆后端构造（单测无 tokio 运行时，记忆不触网）。
fn app_mem() -> App {
    let mut a = app();
    a.memory = Box::new(crate::memory::MirrorMemory::volatile());
    a
}

fn seed(a: &mut App, conv: &str, turns: &[(&str, &str)]) {
    for (role, text) in turns {
        a.memory.seed(role, text, "chat", Some(conv));
    }
}

#[test]
fn sess_layout_roundtrip_has_no_messages() {
    let _home = crate::test_env::Home::new("sess-layout");
    let mut a = app();
    a.add_message(MessageRole::User, "第一轮".to_string());
    a.new_session_tab();
    a.rename_tab("部署排查");
    a.persist_tabs();

    let mut b = app();
    b.restore_tabs();
    assert_eq!(b.session_view().tab_count(), 2, "布局应整体还原");
    assert_eq!(b.session_view().tab_title(1), "部署排查", "标题属于布局");
    assert!(
        b.session_tabs.iter().all(|t| t.messages.is_empty()),
        "sessions.json 不得再存消息正文"
    );
    let raw = std::fs::read_to_string(crate::paths::airy_home_path(&[
        "data",
        "agentrt",
        "tui",
        "sessions.json",
    ]))
    .expect("sessions.json");
    assert!(!raw.contains("第一轮"), "正文只存 mem_d，文件内不得出现");
}

#[test]
fn sess_empty_tab_layout_persisted() {
    let _home = crate::test_env::Home::new("sess-empty");
    let mut a = app();
    a.new_session_tab();
    a.persist_tabs();
    let mut b = app();
    b.restore_tabs();
    assert_eq!(
        b.session_view().tab_count(),
        2,
        "空会话的布局也应保留（内容在 mem_d）"
    );
    assert!(b.messages.is_empty());
}

#[test]
fn sess_history_restored_from_memory_by_conv() {
    let _home = crate::test_env::Home::new("sess-history");
    let mut a = app_mem();
    a.persist_tabs();
    let conv = a.session_tabs[0].conv.clone();
    seed(&mut a, &conv, &[("user", "第一轮"), ("assistant", "回应")]);
    a.persist_tabs();

    let mut b = app_mem();
    b.restore_tabs();
    assert!(b.messages.is_empty(), "布局恢复后内容需经 mem_d");
    seed(&mut b, &conv, &[("user", "第一轮"), ("assistant", "回应")]);
    assert!(b.poll_history_restore(), "水合就绪后应触发历史还原");
    assert_eq!(b.messages.len(), 2);
    assert_eq!(b.messages[0].content, "第一轮");
    assert_eq!(b.messages[1].role, MessageRole::Agent);
    assert!(!b.poll_history_restore(), "历史还原只执行一次");
}

#[test]
fn sess_conv_stable_across_turns() {
    let _home = crate::test_env::Home::new("sess-conv-stable");
    let mut a = app();
    let first = a.current_conv();
    assert!(first.starts_with("sess_"), "会话标识即请求 session_id");
    assert_eq!(first, a.current_conv(), "同一会话多轮不换标识");
    a.new_session_tab();
    let other = a.current_conv();
    assert_ne!(first, other, "新会话不复用标识");
}

#[test]
fn sess_history_recovered_after_file_deleted() {
    let _home = crate::test_env::Home::new("sess-deleted");
    let mut a = app_mem();
    let conv = crate::app::session_store::new_conv();
    seed(
        &mut a,
        &conv,
        &[("user", "删档也要在"), ("assistant", "收到")],
    );
    // 不落盘即模拟 sessions.json 丢失：布局没了，对话仍在 mem_d
    let mut b = app_mem();
    seed(
        &mut b,
        &conv,
        &[("user", "删档也要在"), ("assistant", "收到")],
    );
    assert!(b.poll_history_restore());
    assert_eq!(b.messages.len(), 2, "无布局时采用最近会话组");
    assert_eq!(b.messages[0].content, "删档也要在");
    assert_eq!(b.session_tabs[0].conv, conv, "会话标识随之接管");
}

#[test]
fn sess_close_removes_tab_and_switches() {
    let _home = crate::test_env::Home::new("sess-close");
    let mut a = app();
    a.add_message(MessageRole::User, "keep".to_string());
    a.new_session_tab();
    a.add_message(MessageRole::User, "gone".to_string());
    assert_eq!(a.session_view().tab_count(), 2);
    a.close_tab();
    assert_eq!(a.session_view().tab_count(), 1, "关闭后只剩主会话");
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
    assert_eq!(a.session_view().tab_count(), 1);
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
    assert_eq!(a.session_view().tab_title(0), "部署排查");
    let mut b = app();
    b.restore_tabs();
    assert_eq!(b.session_view().tab_title(0), "部署排查", "重命名必须随落盘持久");
}
