// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AirymaxRT TUI 统一设计令牌（"极简蓝晶"主题）。
//
// 设计语言：
//   - 单一主色：晶蓝 RGB(56,102,250)，所有交互焦点/品牌元素统一使用；
//   - 语义色仅用于状态（成功/警告/危险），不做装饰性堆叠；
//   - 双主题适配：深色（默认）/ 浅色（适配浅色终端背景），中性色随主题切换。
//
// 主题选择优先级（init_from_env）：
//   1. AIRY_TUI_THEME=dark|light 显式指定；
//   2. COLORFGBG 环境变量（终端导出的前景/背景色，bg>6 视为浅色背景）；
//   3. 默认深色。

use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

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
pub const PRIMARY: Color = Color::Rgb(56, 102, 250);
/// 品牌辅助强调：青（用于信息高亮）
pub const ACCENT: Color = Color::Rgb(88, 205, 224);
/// 成功 / 在线 / 用户消息
pub const SUCCESS: Color = Color::Rgb(80, 200, 120);
/// 警告 / 等待 / 系统消息
pub const WARNING: Color = Color::Rgb(238, 178, 70);
/// 危险 / 离线 / 错误
pub const DANGER: Color = Color::Rgb(236, 92, 92);
/// 品红（工具 / GRAD 阶段）
pub const MAGENTA: Color = Color::Rgb(196, 124, 240);
/// 彩色徽章上的文字色（始终深色，保证彩色底对比度，不随主题切换）
pub const ON_COLOR: Color = Color::Rgb(15, 18, 24);

// ─────────────────────────── 中性色（随主题切换） ───────────────────────────

/// 终端背景（深：近黑深蓝；浅：近白）
pub fn bg() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(17, 19, 26),
        ThemeMode::Light => Color::Rgb(247, 248, 251),
    }
}

/// 面板表面（略亮于背景）
pub fn surface() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(26, 29, 39),
        ThemeMode::Light => Color::Rgb(238, 241, 246),
    }
}

/// 悬浮/激活表面
pub fn surface_active() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(33, 38, 52),
        ThemeMode::Light => Color::Rgb(224, 229, 238),
    }
}

/// 常规边框
pub fn border() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(64, 70, 88),
        ThemeMode::Light => Color::Rgb(203, 213, 225),
    }
}

/// 正文
pub fn text() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(226, 232, 240),
        ThemeMode::Light => Color::Rgb(30, 41, 59),
    }
}

/// 次要文字
pub fn dim() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(128, 136, 154),
        ThemeMode::Light => Color::Rgb(100, 116, 139),
    }
}

/// 极弱文字（时间戳/装饰）
pub fn faint() -> Color {
    match ThemeMode::current() {
        ThemeMode::Dark => Color::Rgb(88, 95, 112),
        ThemeMode::Light => Color::Rgb(156, 163, 175),
    }
}
