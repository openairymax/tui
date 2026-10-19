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
/// TERM 含 direct/truecolor → truecolor；含 256color → 256；TERM=dumb 或
/// 无任何信号 → 16 色。
pub(super) fn detect_depth() -> ColorDepth {
    decide(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

/// 色深决策链（§5.5 #2）的纯函数形态：环境取值的语义裁决只在此实现，
/// 单测直接注入取值，不再改写进程环境。
fn decide(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
    if let Some(ct) = colorterm {
        let ct = ct.to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" || ct.contains("direct") {
            return ColorDepth::TrueColor;
        }
    }
    if let Some(term) = term {
        let t = term.to_ascii_lowercase();
        if t == "dumb" {
            return ColorDepth::Basic;
        }
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

/// Rgb → ANSI 16 色反查（§5.5 #3、§6.8 #6）。
///
/// 以 HSL 语义做最近色裁决，而非对 xterm 色板做 RGB 距离最近邻：设计令牌
/// 是低饱和暖色系，距离最近邻会把品牌蓝映射到灰档、把 warning/danger 压成
/// 同一档橙。规则：色度（chroma/max）低于 0.15 视为中性色走灰阶 ramp，
/// 否则按色相扇区取基色、按明度决定是否加亮档。
fn to_basic(r: u8, g: u8, b: u8) -> u8 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let luma = (u32::from(max) + u32::from(min)) as f32 / 510.0;
    let chroma = if max == 0 {
        0.0
    } else {
        f32::from(max - min) / f32::from(max)
    };
    if chroma < 0.15 {
        return match luma {
            l if l < 0.25 => 0,
            l if l < 0.60 => 8,
            l if l < 0.875 => 7,
            _ => 15,
        };
    }
    let hue = hue(r, g, b);
    let base = if !(15.0..345.0).contains(&hue) {
        1 // 红（danger 6.7°）
    } else if hue < 75.0 {
        3 // 黄/橙（warning 22.6°）
    } else if hue < 165.0 {
        2 // 绿（success 81.8°）
    } else if hue < 195.0 {
        6 // 青（cyan 186.3°）
    } else if hue < 255.0 {
        4 // 蓝（primary 217.3°）
    } else {
        5 // 品红（magenta 271.0°）
    };
    if luma >= 0.5 {
        base + 8
    } else {
        base
    }
}

/// 标准 HSL 色相角（0..360）。
fn hue(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d == 0.0 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = h * 60.0;
    if h < 0.0 {
        h + 360.0
    } else {
        h
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

    #[test]
    fn colorterm_truecolor_wins() {
        assert_eq!(decide(Some("truecolor"), None), ColorDepth::TrueColor);
        assert_eq!(decide(Some("24bit"), None), ColorDepth::TrueColor);
        assert_eq!(
            decide(Some("truecolor"), Some("xterm-256color")),
            ColorDepth::TrueColor
        );
    }

    #[test]
    fn term_256color_chain() {
        assert_eq!(decide(None, Some("xterm-256color")), ColorDepth::Color256);
        assert_eq!(decide(None, Some("screen-256color")), ColorDepth::Color256);
    }

    #[test]
    fn dumb_and_silent_fall_back_to_basic() {
        assert_eq!(decide(None, Some("dumb")), ColorDepth::Basic);
        assert_eq!(decide(None, Some("xterm")), ColorDepth::Basic);
        assert_eq!(decide(None, None), ColorDepth::Basic);
    }

    /// §5.5 #3：16 色反查须保住语义区分（danger≠warning、primary 蓝、
    /// 中性灰阶不染色）。
    #[test]
    fn ansi16_reverse_map_keeps_semantics() {
        assert_eq!(to_basic(118, 152, 208), 12); // primary dark
        assert_eq!(to_basic(233, 229, 220), 15); // text dark
        assert_eq!(to_basic(20, 20, 19), 0); // bg dark
        assert_eq!(to_basic(208, 112, 100), 9); // danger dark
        assert_eq!(to_basic(214, 138, 92), 11); // warning dark
        assert_eq!(to_basic(150, 174, 108), 10); // success dark
        assert_eq!(to_basic(110, 178, 186), 14); // cyan dark
        assert_eq!(to_basic(172, 142, 200), 13); // magenta dark
        assert_eq!(to_basic(74, 110, 178), 4); // primary light
        assert_eq!(to_basic(250, 249, 245), 15); // bg light
    }

    #[test]
    fn ansi16_gray_ramp() {
        assert_eq!(to_basic(0, 0, 0), 0);
        assert_eq!(to_basic(72, 70, 64), 8); // border dark
        assert_eq!(to_basic(166, 162, 152), 7); // dim dark
        assert_eq!(to_basic(255, 255, 255), 15);
    }
}
