// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话快照单测：空标题回退、当前会话标记与计数、/sessions 列表格式。
// 断言串与迁移前 app::list_tabs 的输出逐字符对齐。

use super::*;

fn view(rows: &[(&str, usize)], active: usize) -> SessionView {
    SessionView::new(
        rows.iter().map(|(t, n)| ((*t).to_string(), *n)).collect(),
        active,
        0,
    )
}

#[test]
fn empty_title_falls_back_to_seq() {
    let v = view(&[("", 3), ("部署排查", 2)], 0);
    assert_eq!(v.tab_title(0), "会话 1");
    assert_eq!(v.tab_title(1), "部署排查");
    assert_eq!(v.tab_title(9), "", "越界标题按空串回退");
}

#[test]
fn active_tab_is_marked() {
    let v = view(&[("主会话", 1), ("", 4)], 1);
    assert_eq!(v.tab_count(), 2);
    assert_eq!(v.active_index(), 1);
    let tabs = v.tabs();
    assert!(!tabs[0].is_active());
    assert!(tabs[1].is_active());
    assert_eq!(tabs[0].messages(), 1);
    assert_eq!(tabs[1].messages(), 4);
}

#[test]
fn render_list_keeps_legacy_format() {
    let v = view(&[("", 3), ("部署排查", 2), ("第三会话", 0)], 1);
    let want = "  [1] 会话 1（3 条）\n◂ [2] 部署排查（2 条）\n  [3] 第三会话（0 条）";
    assert_eq!(v.render_list(), want);
}

#[test]
fn replay_left_is_carried() {
    let v = SessionView::new(Vec::new(), 0, 5);
    assert_eq!(v.replay_left(), 5);
    assert_eq!(v.tab_count(), 0);
}
