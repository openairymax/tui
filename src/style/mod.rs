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
// 主题选择优先级：
//   1. AIRY_TUI_THEME=dark|light|contrast 显式指定（contrast 为无障碍高对比档，
//      探测结论不得覆盖）；
//   2. OSC11（背景 RGB）/ DEC2031（明暗偏好）探测结论，首帧后晚到热修正；
//   3. COLORFGBG 环境变量（终端导出的前景/背景色，bg>6 视为浅色背景）；
//   4. 默认深色。

use ratatui::style::Color;
#[cfg(not(test))]
use std::sync::atomic::{AtomicU8, Ordering};

mod depth;
mod palette;

pub use depth::ColorDepth;
use depth::{detect_depth, mapped};
use palette::Rgb;

/// 主题档位。
///
/// 前两档描述终端背景，第三档是显式无障碍选择（X7-02a）：高对比不是
/// 「深色加强」，而是独立色板（纯黑底/纯白字），故与背景探测并列为一档，
/// 由 `AIRY_TUI_THEME=contrast` 显式进入；探测结论不得把用户从该档拉走。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    /// 深色背景（默认）
    Dark,
    /// 浅色背景（终端为浅色时自动切换）
    Light,
    /// 高对比（纯黑底/纯白字，语义色高亮端）
    Contrast,
}

/// 主题态的进程内单一权威：生产期由探测线程写入、渲染线程读取，故用
/// 原子；单测并行执行时进程级可变状态会互相污染（一个用例切主题就会
/// 改变另一个用例的渲染输出），因此测试期退化为线程本地，让每个用例
/// 只见自己设置的视图。
#[cfg(not(test))]
static CURRENT: AtomicU8 = AtomicU8::new(0);
#[cfg(test)]
thread_local! {
    static CURRENT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(not(test))]
fn store(code: u8) {
    CURRENT.store(code, Ordering::Relaxed);
}

#[cfg(test)]
fn store(code: u8) {
    CURRENT.with(|slot| slot.set(code));
}

#[cfg(not(test))]
fn load() -> u8 {
    CURRENT.load(Ordering::Relaxed)
}

#[cfg(test)]
fn load() -> u8 {
    CURRENT.with(std::cell::Cell::get)
}

impl ThemeMode {
    pub fn set(self) {
        store(match self {
            ThemeMode::Dark => 0,
            ThemeMode::Light => 1,
            ThemeMode::Contrast => 2,
        });
    }

    pub fn current() -> ThemeMode {
        match load() {
            1 => ThemeMode::Light,
            2 => ThemeMode::Contrast,
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
    if let Some(mode) = explicit() {
        log::info!("theme: explicit {mode:?} (AIRY_TUI_THEME)");
        mode.set();
        return;
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

/// 显式主题（唯一裁决点，探测结果不得覆盖）。
fn explicit() -> Option<ThemeMode> {
    match std::env::var("AIRY_TUI_THEME")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        "contrast" | "high" | "hc" | "high-contrast" => Some(ThemeMode::Contrast),
        _ => None,
    }
}

/// 探测结论热修正主题（§5.5 #1、§6.8 #3）：首帧后才拿到应答，
/// 与启动期 COLORFGBG 推断相比更权威。返回是否发生变更，
/// 调用方据此补一帧（不重建终端，不打断输入）。
pub(crate) fn apply_scheme(light: bool) -> bool {
    if explicit().is_some() {
        return false;
    }
    let want = if light {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    if want == ThemeMode::current() {
        return false;
    }
    want.set();
    log::info!("theme: probe switched to {want:?}");
    true
}

// ─────────────────────── 品牌 / 语义色（色板唯一来源） ───────────────────────
//
// 每个令牌只做「当前色板取值 → 终端色深降级」两步；档位差异全部落在
// `palette`，取色面不再出现 `match ThemeMode`——新增一档不必复制 19 处
// 分支，也不会漏改其中几处造成档位半生效。

/// 取色板字段并交给色深映射（降级唯一实现仍在 `depth`）。
fn token(pick: fn(&palette::Tokens) -> Rgb) -> Color {
    let (r, g, b) = pick(palette::current());
    mapped(r, g, b)
}

/// 品牌主色：雾蓝（深）/ 靛蓝（浅）/ 亮蓝（高对比）。用于品牌/焦点/主操作。
pub fn primary() -> Color {
    token(|t| t.primary)
}
/// 品牌辅助强调：青瓷（深）/ 深青瓷（浅）。用于信息高亮。
pub fn accent() -> Color {
    token(|t| t.accent)
}
/// 成功 / 在线 / 用户消息：橄榄绿（Crail 降饱和体系，参考 Claude Olive）
pub fn success() -> Color {
    token(|t| t.success)
}
/// 警告 / 等待 / 系统消息：Crail 橙降调（参考 Anthropic #d97757 色相）
pub fn warning() -> Color {
    token(|t| t.warning)
}
/// 危险 / 离线 / 错误：陶红
pub fn danger() -> Color {
    token(|t| t.danger)
}
/// 品红（工具 / GRAD 阶段）
pub fn magenta() -> Color {
    token(|t| t.magenta)
}
/// 工具状态行（SSE tool_call/tool_result，Claude Code 风格工具执行提示）
pub fn tool_fg() -> Color {
    magenta()
}
/// 青（用户角色 [For Thee]，与 C 版 airy_cli CLR_CYAN 对齐）
pub fn cyan() -> Color {
    token(|t| t.cyan)
}
/// 彩色徽章上的文字色：取主题背景色做反字，色底对比即语义色/背景对比
/// （各档语义色 vs 底色均 ≥4.5:1，机制自洽）
pub fn on_color() -> Color {
    token(|t| t.on_color)
}

// ─────────────────────────── 中性色（随档位切换） ───────────────────────────

/// 终端背景：暖黑纸张（深）/ 米白纸张（浅）/ 纯黑（高对比）。参考
/// Claude #141413/#faf9f5，告别纯黑/纯白降低视觉疲劳；高对比档例外。
pub fn bg() -> Color {
    token(|t| t.bg)
}

/// 面板表面（略提亮一档，暖灰层次）
pub fn surface() -> Color {
    token(|t| t.surface)
}

/// 悬浮/激活表面
pub fn surface_active() -> Color {
    token(|t| t.surface_active)
}

/// 常规边框
pub fn border() -> Color {
    token(|t| t.border)
}

/// 正文：暖白（深）/ 墨黑（浅）/ 纯白（高对比）
pub fn text() -> Color {
    token(|t| t.text)
}

/// 次要文字
pub fn dim() -> Color {
    token(|t| t.dim)
}

/// 极弱文字（时间戳/装饰）
pub fn faint() -> Color {
    token(|t| t.faint)
}

// ─────────────────────── 3.0 分层表面体系（随档位切换） ───────────────────────

/// 顶部状态条底色：背景深调，与内容区分层（暖石=炭黑 / 羊皮纸=亚麻白）。
/// 让系统状态条成为"纸张书脊"，内容区 surface 承接主体，纵深清晰。
pub fn bar() -> Color {
    token(|t| t.bar)
}

/// 次级表面（卡片/信息块，比 surface 再亮一档，用于会话 tab、chips 底）
pub fn surface_2() -> Color {
    token(|t| t.surface_2)
}

/// 三级表面（最高亮层：激活胶囊、悬浮块）
pub fn surface_3() -> Color {
    token(|t| t.surface_3)
}

/// 状态条分段分隔符（细竖线）：主色弱化蓝灰，比 border 更贴近品牌
pub fn separator() -> Color {
    token(|t| t.separator)
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

/// WCAG 对比度（fg/bg 各通道 RGB）。测试与 golden 对比度门禁共用。
#[cfg(test)]
pub(crate) fn wcag_contrast(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
    let l1 = wcag_luminance(fg.0, fg.1, fg.2);
    let l2 = wcag_luminance(bg.0, bg.1, bg.2);
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// 取当前主题模式下某 token 的源 RGB（强制 TrueColor 深度，只取设计源值）。
/// 测试与 golden 对比度门禁共用；调用方须先置 TrueColor。
#[cfg(test)]
pub(crate) fn source_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => panic!("token must be source RGB under TrueColor, got {:?}", c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;

    /// H7 防复发（§6.8 #2）：三档色深映射快照（TrueColor 保真 / 256 立方索引 / 16 色语义反查）。
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

    /// X2-23b：命中高亮的前景（反字）与背景（主色/强调色）在三档色深下
    /// 都不得落到同一索引——16 色反查把品牌蓝压成亮蓝、反字压成黑/白，
    /// 若两者同索引，命中文本会在降级终端上"消失"。
    #[test]
    fn highlight_pair_stays_distinct_in_all_depths() {
        let _g = lock_env();
        let depths = [
            (ColorDepth::TrueColor, "truecolor"),
            (ColorDepth::Color256, "256"),
            (ColorDepth::Basic, "16"),
        ];
        for (depth, name) in depths {
            ColorDepth::set(depth);
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                ThemeMode::set(mode);
                assert_ne!(
                    primary(),
                    on_color(),
                    "{name}/{mode:?}: 当前命中底与反字不可同色"
                );
                assert_ne!(
                    accent(),
                    on_color(),
                    "{name}/{mode:?}: 其他命中底与反字不可同色"
                );
            }
        }
        ColorDepth::set(ColorDepth::TrueColor);
        ThemeMode::set(ThemeMode::Dark);
    }
}
