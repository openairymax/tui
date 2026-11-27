// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 键位模式（0.1.21 X5-03）。vi/emacs 双模只改「视口导航」这一类按键的
// 绑定，编辑键（写字符、删词、光标移动）不随模式变化——否则 vi 模下
// 用户会在输入框里打出 j/k。模式在启动期由 AIRY_TUI_KEYMAP 裁定，运行期
// Alt+K 经动作表切换（X5-03b），切换只改偏好、不改任何编辑语义。
//
// 本单元只做「按键 → 导航意图」的纯映射：不碰 App、不取时钟。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 键位模式。缺省 emacs：导航全落在修饰键和弦上，与字符输入零争键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Emacs,
    Vi,
}

/// 视口导航意图。落地方按当前视图选择滚动偏移，语义本身与视图无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    LineUp,
    LineDown,
    HalfUp,
    HalfDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl Mode {
    /// 解析配置值：`vi`/`vim` 与 `emacs`；其他值一律视为未识别。
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "emacs" => Some(Self::Emacs),
            "vi" | "vim" => Some(Self::Vi),
            _ => None,
        }
    }

    /// 启动期裁定：未设按 emacs；非法值告警后同样回落——一个拼写错误
    /// 不应把界面变成不可操作。
    pub(crate) fn from_env() -> Self {
        let Ok(raw) = std::env::var("AIRY_TUI_KEYMAP") else {
            return Self::default();
        };
        Self::parse(&raw).unwrap_or_else(|| {
            log::warn!("engine/keymap: 忽略未知 AIRY_TUI_KEYMAP={raw:?}，按 emacs 处理");
            Self::default()
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Emacs => "emacs",
            Self::Vi => "vi",
        }
    }

    /// 双模互切（X5-03b）：Alt+K 的唯一落地语义。
    pub(crate) fn toggle(self) -> Self {
        match self {
            Self::Emacs => Self::Vi,
            Self::Vi => Self::Emacs,
        }
    }
}

/// 按键 → 导航意图。`editing` 表示焦点在输入栏：vi 模的裸字符必须让位给
/// 输入（否则打不出 j/k），emacs 模全是修饰键和弦，不受影响。
pub(crate) fn nav(key: &KeyEvent, mode: Mode, editing: bool) -> Option<Nav> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match mode {
        Mode::Emacs => emacs(key.code, ctrl, alt),
        Mode::Vi if !editing => vi(key.code, ctrl, alt),
        Mode::Vi => None,
    }
}

fn emacs(code: KeyCode, ctrl: bool, alt: bool) -> Option<Nav> {
    match code {
        KeyCode::Char('p' | 'P') if ctrl && !alt => Some(Nav::LineUp),
        KeyCode::Char('n' | 'N') if ctrl && !alt => Some(Nav::LineDown),
        KeyCode::Char('v' | 'V') if ctrl && !alt => Some(Nav::PageDown),
        KeyCode::Char('v' | 'V') if alt && !ctrl => Some(Nav::PageUp),
        KeyCode::Home if ctrl && !alt => Some(Nav::Top),
        KeyCode::End if ctrl && !alt => Some(Nav::Bottom),
        _ => None,
    }
}

fn vi(code: KeyCode, ctrl: bool, alt: bool) -> Option<Nav> {
    if alt {
        return None;
    }
    match code {
        KeyCode::Char('j') if !ctrl => Some(Nav::LineDown),
        KeyCode::Char('k') if !ctrl => Some(Nav::LineUp),
        KeyCode::Char('g') if !ctrl => Some(Nav::Top),
        KeyCode::Char('G') if !ctrl => Some(Nav::Bottom),
        KeyCode::Char('d' | 'D') if ctrl => Some(Nav::HalfDown),
        KeyCode::Char('u' | 'U') if ctrl => Some(Nav::HalfUp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn emacs_chords_navigate_without_stealing_letters() {
        let n = KeyModifiers::CONTROL;
        let m = KeyModifiers::ALT;
        assert_eq!(
            nav(&ev(KeyCode::Char('p'), n), Mode::Emacs, true),
            Some(Nav::LineUp)
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('n'), n), Mode::Emacs, true),
            Some(Nav::LineDown)
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('v'), n), Mode::Emacs, true),
            Some(Nav::PageDown)
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('v'), m), Mode::Emacs, true),
            Some(Nav::PageUp)
        );
        assert_eq!(
            nav(&ev(KeyCode::Home, n), Mode::Emacs, true),
            Some(Nav::Top)
        );
        assert_eq!(
            nav(&ev(KeyCode::End, n), Mode::Emacs, true),
            Some(Nav::Bottom)
        );
        // 裸字符仍归输入编辑
        assert_eq!(
            nav(&ev(KeyCode::Char('j'), KeyModifiers::NONE), Mode::Emacs, false),
            None
        );
    }

    #[test]
    fn vi_letters_only_navigate_outside_the_input() {
        let none = KeyModifiers::NONE;
        assert_eq!(nav(&ev(KeyCode::Char('j'), none), Mode::Vi, false), Some(Nav::LineDown));
        assert_eq!(nav(&ev(KeyCode::Char('k'), none), Mode::Vi, false), Some(Nav::LineUp));
        assert_eq!(
            nav(&ev(KeyCode::Char('j'), none), Mode::Vi, true),
            None,
            "焦点在输入栏时 j 是普通字符"
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('j'), KeyModifiers::ALT), Mode::Vi, false),
            None,
            "带 Alt 的 j 不当作导航"
        );
    }

    #[test]
    fn vi_top_bottom_and_half_page() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(nav(&ev(KeyCode::Char('g'), none), Mode::Vi, false), Some(Nav::Top));
        assert_eq!(
            nav(&ev(KeyCode::Char('G'), KeyModifiers::SHIFT), Mode::Vi, false),
            Some(Nav::Bottom)
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('d'), ctrl), Mode::Vi, false),
            Some(Nav::HalfDown)
        );
        assert_eq!(
            nav(&ev(KeyCode::Char('u'), ctrl), Mode::Vi, false),
            Some(Nav::HalfUp)
        );
    }

    /// X5-03b 双模用例：同一个键在不同模式下解析不同，切换只换表。
    #[test]
    fn same_key_resolves_by_mode() {
        let j = ev(KeyCode::Char('j'), KeyModifiers::NONE);
        let ctrl_n = ev(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(nav(&j, Mode::Vi, false), Some(Nav::LineDown));
        assert_eq!(nav(&j, Mode::Emacs, false), None);
        assert_eq!(nav(&ctrl_n, Mode::Emacs, false), Some(Nav::LineDown));
        assert_eq!(nav(&ctrl_n, Mode::Vi, false), None);
        assert_eq!(Mode::Emacs.toggle(), Mode::Vi);
        assert_eq!(Mode::Vi.toggle(), Mode::Emacs);
        assert_eq!(Mode::Vi.name(), "vi");
        assert_eq!(Mode::Emacs.name(), "emacs");
    }

    #[test]
    fn parse_accepts_known_values_only() {
        assert_eq!(Mode::parse("vi"), Some(Mode::Vi));
        assert_eq!(Mode::parse(" VIM "), Some(Mode::Vi));
        assert_eq!(Mode::parse("emacs"), Some(Mode::Emacs));
        assert_eq!(Mode::parse("nano"), None);
    }
}
