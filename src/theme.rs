// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 统一设计令牌（"蓝晶深空/晨光" 2.0 主题）。
//
// 设计语言：
//   - 单一主色：晶蓝 RGB(56,102,250)，所有交互焦点/品牌元素统一使用；
//   - 分层表面体系：bg → surface → surface_2 → surface_3（顶部状态条
//     底色 bar 为品牌色深调，与内容区拉开纵深）；
//   - 语义色仅用于状态（成功/警告/危险），不做装饰性堆叠；
//   - 双主题适配：深色（默认，"蓝晶深空"）/ 浅色（"蓝晶晨光"，适配
//     浅色终端背景），中性色随主题切换。
//
// 主题选择优先级（init_from_env）：
//   1. AIRY_TUI_THEME=dark|light 显式指定；
//   2. COLORFGBG 环境变量（终端导出的前景/背景色，bg>6 视为浅色背景）；
//   3. 默认深色。

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

static DEPTH: AtomicU8 = AtomicU8::new(2);

impl ColorDepth {
    fn set(self) {
        DEPTH.store(self as u8, Ordering::Relaxed);
    }
    pub fn current() -> ColorDepth {
        match DEPTH.load(Ordering::Relaxed) {
            1 => ColorDepth::Color256,
            0 => ColorDepth::Basic,
            _ => ColorDepth::TrueColor,
        }
    }
}

/// 探测终端色深：COLORTERM=truecolor/24bit/direct → truecolor；
/// TERM 含 direct/truecolor → truecolor；TERM 含 256color → 256；
/// 否则 16 色。无检测时输出 38;2 序列会被 256/16 色终端错位解析
///（对话框/头部变绿的根因）。
fn detect_depth() -> ColorDepth {
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
fn to_basic(r: u8, g: u8, b: u8) -> u8 {
    let lum = (r as u16 * 299 + g as u16 * 587 + b as u16 * 114) / 1000;
    if lum > 210 {
        15 // 白
    } else if lum < 40 {
        0 // 黑
    } else if r >= g && r >= b {
        if lum > 128 { 9 } else { 1 } // 红
    } else if g >= r && g >= b {
        if lum > 128 { 10 } else { 2 } // 绿
    } else if lum > 128 {
        12 // 蓝
    } else {
        4
    }
}

/// 按当前色深映射颜色（所有设计令牌统一入口）
fn mapped(r: u8, g: u8, b: u8) -> Color {
    match ColorDepth::current() {
        ColorDepth::TrueColor => Color::Rgb(r, g, b),
        ColorDepth::Color256 => Color::Indexed(to_256(r, g, b)),
        ColorDepth::Basic => Color::Indexed(to_basic(r, g, b)),
    }
}

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

// ─────────────────────────── 品牌主色（双主题共用） ───────────────────────────

/// 品牌主色：晶蓝（Airymax 主题色，用于品牌/焦点/主操作）
pub fn primary() -> Color { mapped(56, 102, 250) }
/// 品牌辅助强调：青（用于信息高亮）
pub fn accent() -> Color { mapped(88, 205, 224) }
/// 成功 / 在线 / 用户消息
pub fn success() -> Color { mapped(80, 200, 120) }
/// 警告 / 等待 / 系统消息
pub fn warning() -> Color { mapped(238, 178, 70) }
/// 危险 / 离线 / 错误
pub fn danger() -> Color { mapped(236, 92, 92) }
/// 品红（工具 / GRAD 阶段）
pub fn magenta() -> Color { mapped(196, 124, 240) }
/// 工具状态行（SSE tool_call/tool_result，Claude Code 风格工具执行提示）
pub fn tool_fg() -> Color { mapped(196, 124, 240) }
/// 青（用户角色 [For Thee]，与 C 版 airy_cli CLR_CYAN 对齐）
pub fn cyan() -> Color { mapped(70, 190, 220) }
/// 彩色徽章上的文字色（始终深色，保证彩色底对比度，不随主题切换）
pub fn on_color() -> Color { mapped(15, 18, 24) }

// ─────────────────────────── 中性色（随主题切换） ───────────────────────────

/// 终端背景（深：纯黑；浅：纯白。2.2.1.5.1 命令窗背景随主题明暗切换）
pub fn bg() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(0, 0, 0),
        ThemeMode::Light => mapped(255, 255, 255),
    }
}

/// 面板表面（略亮于背景；纯黑/纯白下保留一档层次）
pub fn surface() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(13, 15, 21),
        ThemeMode::Light => mapped(239, 242, 247),
    }
}

/// 悬浮/激活表面
pub fn surface_active() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(33, 38, 52),
        ThemeMode::Light => mapped(224, 229, 238),
    }
}

/// 常规边框
pub fn border() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(64, 70, 88),
        ThemeMode::Light => mapped(203, 213, 225),
    }
}

/// 正文
pub fn text() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(226, 232, 240),
        ThemeMode::Light => mapped(30, 41, 59),
    }
}

/// 次要文字
pub fn dim() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(128, 136, 154),
        ThemeMode::Light => mapped(100, 116, 139),
    }
}

/// 极弱文字（时间戳/装饰）
pub fn faint() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(88, 95, 112),
        ThemeMode::Light => mapped(156, 163, 175),
    }
}

// ─────────────────────── 2.0 分层表面体系（随主题切换） ───────────────────────

/// 顶部状态条底色：品牌色深调，与内容区分层（深空=深蓝黑 / 晨光=淡蓝白）。
/// 让系统状态条成为"品牌头"，内容区 surface 承接主体，纵深清晰。
pub fn bar() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(10, 13, 26),
        ThemeMode::Light => mapped(236, 241, 255),
    }
}

/// 次级表面（卡片/信息块，比 surface 再亮一档，用于会话 tab、chips 底）
pub fn surface_2() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(21, 26, 38),
        ThemeMode::Light => mapped(230, 235, 247),
    }
}

/// 三级表面（最高亮层：激活胶囊、悬浮块）
pub fn surface_3() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(31, 37, 54),
        ThemeMode::Light => mapped(219, 226, 242),
    }
}

/// 状态条分段分隔符（细竖线）：主色弱化，比 border 更贴近品牌
pub fn separator() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => mapped(52, 66, 110),
        ThemeMode::Light => mapped(165, 185, 232),
    }
}
