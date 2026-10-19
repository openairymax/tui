// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端色彩能力（0.1.19 TUI 建设方案 §4.4）：色深探测与降级映射。
//
// 令牌政策在 `style/mod.rs`，本模块只回答「当前终端能画多少色」以及
// 「一个设计源 RGB 在该能力下应映射到哪个索引」。探测顺序（COLORTERM →
// TERM → 最保守 16 色）与降级算法（256 立方近似、16 色亮度加权）在此
// 唯一实现，主题与组件都不得自行判断色深——否则 256/16 色终端会收到
// 38;2 序列并错位解析（0.1.6h 乱色根因）。

use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

/// 终端色彩深度能力（0.1.6h 修复乱色：不支持 truecolor 时回退 256/16 色）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24bit truecolor（COLORTERM=truecolor/24bit 或 TERM 含 direct）
    TrueColor,
    /// 256 色（TERM=*-256color）
    Color256,
    /// 基础 16 色（无任何信号时最保守）
    Basic,
}

/// 静态默认 Basic（2）：任何 init 之前先按最保守 16 色走，
/// 避免 256/16 色终端收到 38;2 序列错位解析（0.1.6h 乱色根因）。
static DEPTH: AtomicU8 = AtomicU8::new(2);

impl ColorDepth {
    pub(super) fn set(self) {
        DEPTH.store(self as u8, Ordering::Relaxed);
    }

    /// 判别值 ↔ 深度映射必须与 enum 声明顺序一致（TrueColor=0/256=1/
    /// Basic=2），曾与判别值倒置导致 truecolor 终端落到 16 色、Basic
    /// 终端收到 truecolor 序列（W3 测试捕获的潜伏映射 bug）。
    pub fn current() -> ColorDepth {
        match DEPTH.load(Ordering::Relaxed) {
            0 => ColorDepth::TrueColor,
            1 => ColorDepth::Color256,
            _ => ColorDepth::Basic,
        }
    }
}

/// 探测终端色深：COLORTERM=truecolor/24bit/direct → truecolor；
/// TERM 含 direct/truecolor → truecolor；TERM 含 256color → 256；
/// 否则 16 色。
pub(super) fn detect_depth() -> ColorDepth {
    if let Ok(ct) = std::env::var("COLORTERM") {
        let ct = ct.to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" || ct.contains("direct") {
            return ColorDepth::TrueColor;
        }
    }
    if let Ok(term) = std::env::var("TERM") {
        let t = term.to_ascii_lowercase();
        if t.contains("truecolor") || t.contains("direct") {
            return ColorDepth::TrueColor;
        }
        if t.contains("256color") || t.contains("256") {
            return ColorDepth::Color256;
        }
    }
    ColorDepth::Basic
}

/// Rgb → 256 色索引（Xterm 6x6x6 立方近似）
fn to_256(r: u8, g: u8, b: u8) -> u8 {
    let r = (r as u16 * 5 / 255).min(5);
    let g = (g as u16 * 5 / 255).min(5);
    let b = (b as u16 * 5 / 255).min(5);
    (16 + 36 * r + 6 * g + b) as u8
}

/// Rgb → ANSI 16 色（亮度加权 + 主色调，保持语义色）
/// 注意：亮度加权和 255*587 超过 u16 上限（149685 > 65535），必须用 u32
/// 累加，否则 debug 构建溢出 panic、release 构建静默回绕得错色（W3 测试捕获）。
fn to_basic(r: u8, g: u8, b: u8) -> u8 {
    let lum = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    if lum > 210 {
        15 // 白
    } else if lum < 40 {
        0 // 黑
    } else if r >= g && r >= b {
        if lum > 128 {
            9
        } else {
            1
        } // 红
    } else if g >= r && g >= b {
        if lum > 128 {
            10
        } else {
            2
        } // 绿
    } else if lum > 128 {
        12 // 蓝
    } else {
        4
    }
}

/// 按当前色深映射颜色（所有设计令牌统一入口）
pub(super) fn mapped(r: u8, g: u8, b: u8) -> Color {
    match ColorDepth::current() {
        ColorDepth::TrueColor => Color::Rgb(r, g, b),
        ColorDepth::Color256 => Color::Indexed(to_256(r, g, b)),
        ColorDepth::Basic => Color::Indexed(to_basic(r, g, b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;

    fn restore(ct: Option<String>, term: Option<String>) {
        match ct {
            Some(v) => std::env::set_var("COLORTERM", v),
            None => std::env::remove_var("COLORTERM"),
        }
        match term {
            Some(v) => std::env::set_var("TERM", v),
            None => std::env::remove_var("TERM"),
        }
    }

    #[test]
    fn detect_truecolor_from_colorterm() {
        let _g = lock_env();
        let (ct, term) = (std::env::var("COLORTERM").ok(), std::env::var("TERM").ok());
        std::env::set_var("COLORTERM", "truecolor");
        std::env::remove_var("TERM");
        assert_eq!(detect_depth(), ColorDepth::TrueColor);
        std::env::set_var("COLORTERM", "24bit");
        assert_eq!(detect_depth(), ColorDepth::TrueColor);
        restore(ct, term);
    }

    #[test]
    fn detect_256_from_term() {
        let _g = lock_env();
        let (ct, term) = (std::env::var("COLORTERM").ok(), std::env::var("TERM").ok());
        std::env::remove_var("COLORTERM");
        std::env::set_var("TERM", "xterm-256color");
        assert_eq!(detect_depth(), ColorDepth::Color256);
        std::env::set_var("TERM", "screen-256color");
        assert_eq!(detect_depth(), ColorDepth::Color256);
        restore(ct, term);
    }

    #[test]
    fn detect_fallback_to_basic() {
        let _g = lock_env();
        let (ct, term) = (std::env::var("COLORTERM").ok(), std::env::var("TERM").ok());
        std::env::remove_var("COLORTERM");
        std::env::set_var("TERM", "xterm");
        assert_eq!(detect_depth(), ColorDepth::Basic);
        std::env::remove_var("TERM");
        assert_eq!(detect_depth(), ColorDepth::Basic);
        restore(ct, term);
    }
}
