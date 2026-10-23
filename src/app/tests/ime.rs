// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::*;

/// Ctrl+1 切回英文时拼音原文上屏、缓冲清空（与 CLI 语义一致）。
#[test]
fn ime_toggle_off_commits_raw_pinyin() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert_eq!(app.ime_buf, "zhongguo");
    app.ime_toggle();
    assert!(!app.ime_active, "切回英文应退出拼音态");
    assert!(app.ime_buf.is_empty());
    assert!(
        app.input.contains("zhongguo"),
        "拼音原文应上屏: {}",
        app.input
    );
}

/// 字母追加实时刷新候选；非 [a-z] 可见字符先上屏拼音原文再走正常路径。
#[test]
fn ime_pinyin_composition_refreshes_candidates() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert_eq!(app.ime_buf, "zhongguo");
    assert!(!app.ime_cands.is_empty(), "zhongguo 应有候选");
    assert_eq!(app.ime_cands[0], "中国", "词频最高者应为「中国」");
    // 任意可见非拼音字符：原文上屏并退出拼音态，按键放行
    let consumed = app.ime_input_char('你');
    assert!(!consumed, "非拼音字符不应被拼音态消费");
    assert!(!app.ime_active);
    assert!(app.input.contains("zhongguo"));
    assert!(app.ime_buf.is_empty());
}

/// 空格上屏高亮（默认首）候选，拼音态保持（连续词组输入不中断）。
#[test]
fn ime_space_commits_first_candidate_keeps_active() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert!(app.ime_input_char(' '), "空格应被消费");
    assert!(
        app.input.contains("中国"),
        "空格应上屏首候选: {}",
        app.input
    );
    assert!(app.ime_buf.is_empty(), "上屏后拼音缓冲应清空");
    assert!(app.ime_active, "选字后应保持拼音态以连续输入");
}

/// 数字键按页内下标选字（微信式分页）。
#[test]
fn ime_digit_selects_candidate() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert!(app.ime_input_char('1'), "数字应被消费");
    assert!(app.input.contains("中国"));
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_active);
}

/// 退格删拼音（候选随之刷新）；拼音删空后再次退格退出拼音态。
#[test]
fn ime_backspace_pops_then_exits() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongg");
    assert_eq!(app.ime_buf, "zhongg");
    assert!(app.ime_backspace());
    assert_eq!(app.ime_buf, "zhong");
    assert!(app.ime_active);
    for _ in 0..5 {
        assert!(app.ime_backspace());
    }
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_active, "缓冲空时拼音态仍在（首退格仅退态）");
    app.ime_backspace();
    assert!(!app.ime_active, "拼音缓冲为空时退格应退出拼音态");
}

/// Esc（ime_cancel）：放弃组合，不插入任何文本，退出拼音态。
#[test]
fn ime_cancel_discards_without_insert() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    app.ime_cancel();
    assert!(!app.ime_active);
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_cands.is_empty());
    assert!(!app.input.contains("zhongguo"), "Esc 不应上屏拼音原文");
    assert_eq!(app.input, "");
}

/// Enter：有候选上屏高亮候选并退出拼音态；无候选提交拼音原文退出。
///
/// 两个用例分属独立作用域：Home 持进程级 ENV_LOCK 直至作用域结束，
/// 同作用域内再建第二个 Home 会自死锁（首次运行暴露）。
#[test]
fn ime_enter_commits_candidate_or_raw() {
    {
        let (_d, mut app) = app_with_ime();
        ime_type(&mut app, "zhongguo");
        assert!(app.ime_commit_enter(), "拼音态 Enter 应由调用方先行提交");
        assert!(app.input.contains("中国"));
        assert!(!app.ime_active, "Enter 提交后退出拼音态");
    }

    {
        let (_d2, mut app2) = app_with_ime();
        ime_type(&mut app2, "zzzzz"); // 无候选拼音
        assert!(app2.ime_cands.is_empty(), "zzzzz 应无候选");
        assert!(app2.ime_commit_enter());
        assert!(app2.input.contains("zzzzz"), "无候选时 Enter 提交拼音原文");
        assert!(!app2.ime_active);
    }
}

