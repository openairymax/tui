// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 动作表（0.1.21 X5-05/08）。键盘与鼠标不是两套逻辑：两侧分别解析出
// 同一个 `Action`，再由应用层唯一入口落地。表是常量单源，启动期查重，
// 冲突即拒绝启动（fail-closed），不在运行中静默覆盖某个绑定。
//
// 本单元只做「输入 → 语义」的纯映射，不碰 App、不读时钟。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 跨输入源的语义动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// 允许本次调用。
    Allow,
    /// 本会话总是允许（待后端支持时由网关裁决）。
    AllowAlways,
    /// 拒绝调用。
    Deny,
    /// 上半页。
    HalfUp,
    /// 下半页。
    HalfDown,
    /// 鼠标滚轮捕获开关。
    MouseToggle,
    /// vi/emacs 键位模式互切（X5-03b）。
    KeymapToggle,
}

/// 动作生效条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    /// 恒可用。
    Any,
    /// 仅当存在待决议审批时可用（避免普通输入被吞字）。
    Approval,
}

/// 上下文：由调用方从应用状态折算，本单元不依赖 App。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ctx {
    pub approvals: bool,
    pub ime: bool,
}

/// 绑定表（常量单源）。修饰键只比较 Ctrl/Alt，Shift 随字符大小写归一，
/// 因此 `Y` 与 `y` 同键，避免 Caps Lock/Shift 造成「同一个键时灵时不灵」。
pub(crate) const CHORDS: &[(KeyCode, KeyModifiers, Need, Action)] = &[
    (KeyCode::Char('y'), KeyModifiers::NONE, Need::Approval, Action::Allow),
    (KeyCode::Char('a'), KeyModifiers::NONE, Need::Approval, Action::AllowAlways),
    (KeyCode::Char('n'), KeyModifiers::NONE, Need::Approval, Action::Deny),
    (KeyCode::Char('d'), KeyModifiers::NONE, Need::Approval, Action::Deny),
    (KeyCode::Esc, KeyModifiers::NONE, Need::Approval, Action::Deny),
    (KeyCode::Char('u'), KeyModifiers::ALT, Need::Any, Action::HalfUp),
    (KeyCode::Char('d'), KeyModifiers::ALT, Need::Any, Action::HalfDown),
    (KeyCode::Char('m'), KeyModifiers::CONTROL, Need::Any, Action::MouseToggle),
    (KeyCode::Char('k'), KeyModifiers::ALT, Need::Any, Action::KeymapToggle),
];

fn norm(key: &KeyEvent) -> (KeyCode, KeyModifiers) {
    let mods = key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
    let code = match key.code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    };
    (code, mods)
}

/// 按键 → 动作。IME 拼音态下 Esc 归输入法取消，不当作拒绝。
pub(crate) fn key_action(key: &KeyEvent, ctx: Ctx) -> Option<Action> {
    let (code, mods) = norm(key);
    if ctx.ime && code == KeyCode::Esc {
        return None;
    }
    CHORDS.iter().find_map(|(c, m, need, action)| {
        if *c != code || *m != mods {
            return None;
        }
        if *need == Need::Approval && !ctx.approvals {
            return None;
        }
        Some(*action)
    })
}

/// 表内查重：同和弦（归一后）且生效条件有交集即冲突。
pub(crate) fn conflicts(
    chords: &[(KeyCode, KeyModifiers, Need, Action)],
) -> Option<(usize, usize)> {
    for (i, a) in chords.iter().enumerate() {
        for (j, b) in chords.iter().enumerate().skip(i + 1) {
            if a.0 == b.0 && a.1 == b.1 && (a.2 == b.2 || a.2 == Need::Any || b.2 == Need::Any) {
                return Some((i, j));
            }
        }
    }
    None
}

/// 启动期自检：内置表必须无冲突。冲突时给出可定位的种类下标。
pub(crate) fn validate() -> Result<(), String> {
    match conflicts(CHORDS) {
        None => Ok(()),
        Some((i, j)) => Err(format!(
            "动作表冲突：{:?} / {:?}（下标 {i} 与 {j}）",
            CHORDS[i].0, CHORDS[j].0
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn ctx(approvals: bool) -> Ctx {
        Ctx {
            approvals,
            ime: false,
        }
    }

    #[test]
    fn builtin_table_has_no_conflicts() {
        assert!(validate().is_ok());
    }

    #[test]
    fn approval_letters_map_only_with_pending_request() {
        assert_eq!(
            key_action(&ev(KeyCode::Char('y'), KeyModifiers::NONE), ctx(true)),
            Some(Action::Allow)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT), ctx(true)),
            Some(Action::AllowAlways)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('D'), KeyModifiers::SHIFT), ctx(true)),
            Some(Action::Deny)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Esc, KeyModifiers::NONE), ctx(true)),
            Some(Action::Deny)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('y'), KeyModifiers::NONE), ctx(false)),
            None
        );
        assert_eq!(
            key_action(&ev(KeyCode::Esc, KeyModifiers::NONE), ctx(false)),
            None
        );
    }

    #[test]
    fn plain_letters_stay_typing_without_approval() {
        for c in ['y', 'a', 'n', 'd', 'u'] {
            assert_eq!(
                key_action(&ev(KeyCode::Char(c), KeyModifiers::NONE), ctx(false)),
                None
            );
        }
    }

    #[test]
    fn half_page_and_mouse_toggle_are_modified_chords() {
        assert_eq!(
            key_action(&ev(KeyCode::Char('u'), KeyModifiers::ALT), ctx(false)),
            Some(Action::HalfUp)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('D'), KeyModifiers::ALT), ctx(false)),
            Some(Action::HalfDown)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('m'), KeyModifiers::CONTROL), ctx(false)),
            Some(Action::MouseToggle)
        );
        assert_eq!(
            key_action(&ev(KeyCode::Char('k'), KeyModifiers::ALT), ctx(false)),
            Some(Action::KeymapToggle)
        );
    }

    #[test]
    fn ime_esc_cancels_composition_not_request() {
        let ime = Ctx {
            approvals: true,
            ime: true,
        };
        assert_eq!(key_action(&ev(KeyCode::Esc, KeyModifiers::NONE), ime), None);
    }

    #[test]
    fn duplicate_chords_are_reported() {
        let dup = [
            (KeyCode::Char('y'), KeyModifiers::NONE, Need::Any, Action::Allow),
            (KeyCode::Char('y'), KeyModifiers::NONE, Need::Any, Action::Deny),
        ];
        assert_eq!(conflicts(&dup), Some((0, 1)));
        let disjoint = [
            (KeyCode::Char('y'), KeyModifiers::NONE, Need::Approval, Action::Allow),
            (KeyCode::Char('u'), KeyModifiers::ALT, Need::Any, Action::HalfUp),
        ];
        assert_eq!(conflicts(&disjoint), None);
    }
}
