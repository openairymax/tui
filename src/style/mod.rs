// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 统一设计令牌（"暖石/羊皮纸" 3.0 主题）。
//
// 设计语言：
//   - 纸张感中性色：深色"暖石"（暖黑纸张 RGB(20,20,19)，参考 Claude
//     #141413）与浅色"羊皮纸"（米白 RGB(250,249,245)，参考 Claude
//     #faf9f5），告别纯黑/纯白的冷硬感；
//   - 语义色随主题分层：品牌/语义色不再双主题共用折中值，而是深浅各
//     自调优（同 minimax-code dark/light 双色板机制），浅色端语义色
//     全部达到 WCAG AA 4.5:1（2.x 浅色端 accent 仅 ~1.9:1 的根因
//     即共用折中）；
//   - 降饱和蓝相品牌主色：primary 深色雾蓝 (118,152,208)/浅色 (74,110,178)，
//     warning 采用 Crail 橙色相（参考 Anthropic 官方色板 #d97757 降调）；
//   - 分层表面体系：bg → surface → surface_2 → surface_3（顶部状态条
//     底色 bar 为 bg 深调，与内容区拉开纵深）；
//   - 语义色仅用于状态（成功/警告/危险），不做装饰性堆叠；
//   - 双主题适配：深色（默认，"暖石"）/ 浅色（"羊皮纸"，适配浅色
//     终端背景），中性色与语义色均随主题切换。
//
// 主题选择优先级（init_from_env）：
//   1. AIRY_TUI_THEME=dark|light 显式指定；
//   2. COLORFGBG 环境变量（终端导出的前景/背景色，bg>6 视为浅色背景）；
//   3. 默认深色。

use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

mod depth;

pub use depth::ColorDepth;
use depth::{detect_depth, mapped};

/// 主题模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    /// 深色背景（默认）
    Dark,
    /// 浅色背景（终端为浅色时自动切换）
    Light,
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

impl ThemeMode {
    pub fn set(self) {
        CURRENT.store(
            match self {
                ThemeMode::Dark => 0,
                ThemeMode::Light => 1,
            },
            Ordering::Relaxed,
        );
    }

    pub fn current() -> ThemeMode {
        match CURRENT.load(Ordering::Relaxed) {
            1 => ThemeMode::Light,
            _ => ThemeMode::Dark,
        }
    }
}

/// 根据环境初始化主题（main 启动早期调用一次）。
pub fn init_from_env() {
    // 0.1.6h：先定色深（乱色根因：256/16 色终端收到 38;2 序列错位解析）
    ColorDepth::set(detect_depth());
    log::info!("theme: color depth = {:?}", ColorDepth::current());
    // 1. 显式指定
    if let Ok(v) = std::env::var("AIRY_TUI_THEME") {
        match v.to_ascii_lowercase().as_str() {
            "light" => {
                log::info!("theme: explicit light (AIRY_TUI_THEME)");
                ThemeMode::Light.set();
                return;
            }
            "dark" => {
                log::info!("theme: explicit dark (AIRY_TUI_THEME)");
                ThemeMode::Dark.set();
                return;
            }
            _ => {}
        }
    }
    // 2. COLORFGBG 启发式（"fg;bg"，bg>6 为亮色 → 浅色背景）
    if let Ok(v) = std::env::var("COLORFGBG") {
        let parts: Vec<&str> = v.split(';').collect();
        if parts.len() >= 2 {
            if let Ok(n) = parts[1].trim().parse::<u16>() {
                if n > 6 {
                    log::info!("theme: light detected via COLORFGBG={}", v);
                    ThemeMode::Light.set();
                    return;
                }
            }
        }
    }
    ThemeMode::Dark.set();
}

// ─────────────────────── 品牌 / 语义色（随主题分层） ───────────────────────

/// 品牌主色：雾蓝（深）/ 靛蓝（浅）。用于品牌/焦点/主操作。
/// 降饱和蓝相保持 Airymax 品牌连续性，深浅各自调优至 WCAG AA。
pub fn primary() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(118, 152, 208),
        ThemeMode::Light => mapped(74, 110, 178),
    }
}
/// 品牌辅助强调：青瓷（深）/ 深青瓷（浅）。用于信息高亮。
pub fn accent() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(138, 190, 178),
        ThemeMode::Light => mapped(62, 120, 108),
    }
}
/// 成功 / 在线 / 用户消息：橄榄绿（Crail 降饱和体系，参考 Claude Olive）
pub fn success() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(150, 174, 108),
        ThemeMode::Light => mapped(96, 118, 62),
    }
}
/// 警告 / 等待 / 系统消息：Crail 橙降调（参考 Anthropic #d97757 色相）
pub fn warning() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(214, 138, 92),
        ThemeMode::Light => mapped(162, 92, 44),
    }
}
/// 危险 / 离线 / 错误：陶红
pub fn danger() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(208, 112, 100),
        ThemeMode::Light => mapped(168, 76, 66),
    }
}
/// 品红（工具 / GRAD 阶段）
pub fn magenta() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(172, 142, 200),
        ThemeMode::Light => mapped(118, 94, 156),
    }
}
/// 工具状态行（SSE tool_call/tool_result，Claude Code 风格工具执行提示）
pub fn tool_fg() -> Color {
    magenta()
}
/// 青（用户角色 [For Thee]，与 C 版 airy_cli CLR_CYAN 对齐）
pub fn cyan() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(110, 178, 186),
        ThemeMode::Light => mapped(52, 120, 130),
    }
}
/// 彩色徽章上的文字色：取主题背景色做反字，色底对比即语义色/背景对比
/// （深色端语义色 vs 暖黑、浅色端 vs 米白，均 ≥4.5:1，机制自洽）
pub fn on_color() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(20, 20, 19),
        ThemeMode::Light => mapped(250, 249, 245),
    }
}

// ─────────────────────────── 中性色（随主题切换） ───────────────────────────

/// 终端背景：暖黑纸张（深）/ 米白纸张（浅）。参考 Claude #141413/#faf9f5，
/// 告别纯黑/纯白，降低视觉疲劳（2.2.1.5.1 命令窗背景随主题明暗切换）
pub fn bg() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(20, 20, 19),
        ThemeMode::Light => mapped(250, 249, 245),
    }
}

/// 面板表面（略提亮一档，暖灰层次）
pub fn surface() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(30, 29, 27),
        ThemeMode::Light => mapped(243, 241, 234),
    }
}

/// 悬浮/激活表面
pub fn surface_active() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(42, 41, 38),
        ThemeMode::Light => mapped(233, 230, 220),
    }
}

/// 常规边框
pub fn border() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(72, 70, 64),
        ThemeMode::Light => mapped(208, 204, 192),
    }
}

/// 正文：暖白（深）/ 墨黑（浅）
pub fn text() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(233, 229, 220),
        ThemeMode::Light => mapped(38, 37, 33),
    }
}

/// 次要文字
pub fn dim() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(166, 162, 152),
        ThemeMode::Light => mapped(110, 106, 98),
    }
}

/// 极弱文字（时间戳/装饰）
pub fn faint() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(118, 115, 106),
        ThemeMode::Light => mapped(148, 144, 134),
    }
}

// ─────────────────────── 3.0 分层表面体系（随主题切换） ───────────────────────

/// 顶部状态条底色：背景深调，与内容区分层（暖石=炭黑 / 羊皮纸=亚麻白）。
/// 让系统状态条成为"纸张书脊"，内容区 surface 承接主体，纵深清晰。
pub fn bar() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(13, 13, 12),
        ThemeMode::Light => mapped(244, 242, 236),
    }
}

/// 次级表面（卡片/信息块，比 surface 再亮一档，用于会话 tab、chips 底）
pub fn surface_2() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(38, 37, 34),
        ThemeMode::Light => mapped(236, 233, 224),
    }
}

/// 三级表面（最高亮层：激活胶囊、悬浮块）
pub fn surface_3() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(47, 46, 42),
        ThemeMode::Light => mapped(228, 224, 213),
    }
}

/// 状态条分段分隔符（细竖线）：主色弱化蓝灰，比 border 更贴近品牌
pub fn separator() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(54, 66, 92),
        ThemeMode::Light => mapped(170, 182, 208),
    }
}

// ─────────────────────────── 测试 ───────────────────────────

/// WCAG 相对亮度（sRGB 线性化）。仅测试用（对比度门禁），不随产品构建。
#[cfg(test)]
fn wcag_luminance(r: u8, g: u8, b: u8) -> f64 {
    let f = |c: u8| {
        let s = c as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG 对比度（fg/bg 各通道 RGB）。仅测试用。
#[cfg(test)]
fn wcag_contrast(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
    let l1 = wcag_luminance(fg.0, fg.1, fg.2);
    let l2 = wcag_luminance(bg.0, bg.1, bg.2);
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// 取当前主题模式下某 token 的源 RGB（强制 TrueColor 深度，只取设计源值）。
/// 仅测试用（对比度门禁取设计源值，绕过终端降级映射）。
#[cfg(test)]
fn source_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => panic!("token must be source RGB under TrueColor, got {:?}", c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;

    /// H7 防复发：三档色深映射快照（TrueColor 保真 / 256 立方索引 / 16 色亮度加权）。
    #[test]
    fn color_depth_mapping_snapshots() {
        let _g = lock_env();
        ColorDepth::set(ColorDepth::TrueColor);
        ThemeMode::set(ThemeMode::Dark);
        assert_eq!(primary(), Color::Rgb(118, 152, 208));
        assert_eq!(bg(), Color::Rgb(20, 20, 19));

        ColorDepth::set(ColorDepth::Color256);
        // to_256(118,152,208) = 16 + 36*2 + 6*2 + 4 = 104；暖黑 ≈ 立方起点 16
        assert_eq!(primary(), Color::Indexed(104));
        assert_eq!(bg(), Color::Indexed(16));

        ColorDepth::set(ColorDepth::Basic);
        // to_basic(118,152,208)：亮度 ~148，主色相蓝 → 16 色亮蓝
        assert_eq!(primary(), Color::Indexed(12));
        // to_basic(233,229,220)：亮度 ~229 > 210 → 白
        assert_eq!(text(), Color::Indexed(15));
        ColorDepth::set(ColorDepth::TrueColor);
    }

    /// W3：文本/语义色对比度 ≥ WCAG AA 4.5:1。语义色常用于消息前缀与
    /// 状态文本（如 "warning:"），按正文级 AA 门禁（3.0 起双端全量纳管）。
    #[test]
    fn token_text_contrast_meets_wcag_aa() {
        let _g = lock_env();
        ColorDepth::set(ColorDepth::TrueColor);

        ThemeMode::set(ThemeMode::Dark);
        let dark_bg = source_rgb(bg());
        let cases_dark = [
            ("text/bg", text(), dark_bg, 4.5),
            ("text/surface", text(), source_rgb(surface()), 4.5),
            ("text/bar", text(), source_rgb(bar()), 4.5),
            ("dim/bg", dim(), dark_bg, 4.5),
            ("primary/bg", primary(), dark_bg, 4.5),
            ("accent/bg", accent(), dark_bg, 4.5),
            ("success/bg", success(), dark_bg, 4.5),
            ("warning/bg", warning(), dark_bg, 4.5),
            ("danger/bg", danger(), dark_bg, 4.5),
            ("magenta/bg", magenta(), dark_bg, 4.5),
            ("cyan/bg", cyan(), dark_bg, 4.5),
        ];
        for (name, fg, bgc, min) in cases_dark {
            let r = wcag_contrast(source_rgb(fg), bgc);
            assert!(
                r >= min,
                "dark theme {} contrast {:.2} < {:.1}",
                name,
                r,
                min
            );
        }

        ThemeMode::set(ThemeMode::Light);
        let light_bg = source_rgb(bg());
        let cases_light = [
            ("text/bg", text(), light_bg, 4.5),
            ("text/surface", text(), source_rgb(surface()), 4.5),
            ("text/bar", text(), source_rgb(bar()), 4.5),
            ("dim/bg", dim(), light_bg, 4.5),
            ("primary/bg", primary(), light_bg, 4.5),
            ("accent/bg", accent(), light_bg, 4.5),
            ("success/bg", success(), light_bg, 4.5),
            ("warning/bg", warning(), light_bg, 4.5),
            ("danger/bg", danger(), light_bg, 4.5),
            ("magenta/bg", magenta(), light_bg, 4.5),
            ("cyan/bg", cyan(), light_bg, 4.5),
        ];
        for (name, fg, bgc, min) in cases_light {
            let r = wcag_contrast(source_rgb(fg), bgc);
            assert!(
                r >= min,
                "light theme {} contrast {:.2} < {:.1}",
                name,
                r,
                min
            );
        }

        ColorDepth::set(ColorDepth::TrueColor);
    }

    #[test]
    fn theme_mode_switches_neutrals() {
        let _g = lock_env();
        ColorDepth::set(ColorDepth::TrueColor);
        ThemeMode::set(ThemeMode::Dark);
        assert_eq!(bg(), Color::Rgb(20, 20, 19));
        assert_eq!(text(), Color::Rgb(233, 229, 220));
        assert_eq!(primary(), Color::Rgb(118, 152, 208));
        ThemeMode::set(ThemeMode::Light);
        assert_eq!(bg(), Color::Rgb(250, 249, 245));
        assert_eq!(text(), Color::Rgb(38, 37, 33));
        // 3.0：语义色随主题分层（深浅各自调优，浅色端 accent 2.x 仅
        // ~1.9:1 的共用折中根因已移除）
        assert_eq!(primary(), Color::Rgb(74, 110, 178));
        ThemeMode::set(ThemeMode::Dark);
    }
}
