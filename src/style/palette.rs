// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 主题色板（0.1.21 X7-02a）：一档主题 = 一张完整令牌表。
//
// 三档色板平铺在同一张表里，`style` 的取色函数只做「表 → 降级映射」，
// 不再各自写 `match ThemeMode`——此前每个令牌各带一份三臂 match，新增
// 高对比档要么复制 19 处分支、要么漏改其中几处造成档位半生效。色板集中
// 后，新增/调整一档只改本文件，取色面零改动。
//
// 高对比档（X7-02a）不是「深色的加强版」而是独立一档：背景压到纯黑、
// 正文纯白（21:1），语义色全部换成高亮端（均 ≥4.5:1，正文级 ≥7:1）。
// 语义辨识不依赖色相——时间线的八态仍以字形区分（X7-03b 的前置约定），
// 色弱/单色终端下形状是唯一可靠通道。

use super::ThemeMode;

/// 设计源 RGB（未经终端色深降级）。
pub(super) type Rgb = (u8, u8, u8);

/// 一档主题的完整令牌表。字段与 `style` 的公开取色函数一一对应，
/// 除 `tool_fg`（复用 magenta）外的每个令牌都在此有唯一来源。
pub(super) struct Tokens {
    pub primary: Rgb,
    pub accent: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub danger: Rgb,
    pub magenta: Rgb,
    pub cyan: Rgb,
    pub on_color: Rgb,
    pub bg: Rgb,
    pub surface: Rgb,
    pub surface_active: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub faint: Rgb,
    pub bar: Rgb,
    pub surface_2: Rgb,
    pub surface_3: Rgb,
    pub separator: Rgb,
}

/// 深色「暖石」：深灰纸张底 + 暖白正文，语义色降饱和。
pub(super) const DARK: Tokens = Tokens {
    primary: (118, 152, 208),
    accent: (138, 190, 178),
    success: (150, 174, 108),
    warning: (214, 138, 92),
    danger: (208, 112, 100),
    magenta: (172, 142, 200),
    cyan: (110, 178, 186),
    on_color: (20, 20, 19),
    bg: (20, 20, 19),
    surface: (30, 29, 27),
    surface_active: (42, 41, 38),
    border: (72, 70, 64),
    text: (233, 229, 220),
    dim: (166, 162, 152),
    faint: (118, 115, 106),
    bar: (13, 13, 12),
    surface_2: (38, 37, 34),
    surface_3: (47, 46, 42),
    separator: (54, 66, 92),
};

/// 浅色「羊皮纸」：米白纸张底 + 墨黑正文，语义色加深至 AA。
pub(super) const LIGHT: Tokens = Tokens {
    primary: (74, 110, 178),
    accent: (62, 120, 108),
    success: (96, 118, 62),
    warning: (162, 92, 44),
    danger: (168, 76, 66),
    magenta: (118, 94, 156),
    cyan: (52, 120, 130),
    on_color: (250, 249, 245),
    bg: (250, 249, 245),
    surface: (243, 241, 234),
    surface_active: (233, 230, 220),
    border: (208, 204, 192),
    text: (38, 37, 33),
    dim: (110, 106, 98),
    faint: (148, 144, 134),
    bar: (244, 242, 236),
    surface_2: (236, 233, 224),
    surface_3: (228, 224, 213),
    separator: (170, 182, 208),
};

/// 高对比档：纯黑底 + 纯白正文，语义色取高亮端保证形状之外颜色也读得出。
/// 档位固定深底——高对比用户的使用场景是强环境光/低视力，浅底高亮会
/// 把对比度推回 4.5 附近，与本档目的相反。
pub(super) const CONTRAST: Tokens = Tokens {
    primary: (130, 185, 255),
    accent: (130, 255, 225),
    success: (150, 255, 150),
    warning: (255, 215, 90),
    danger: (255, 140, 130),
    magenta: (235, 175, 255),
    cyan: (140, 245, 255),
    on_color: (0, 0, 0),
    bg: (0, 0, 0),
    surface: (10, 10, 10),
    surface_active: (26, 26, 26),
    border: (120, 120, 120),
    text: (255, 255, 255),
    dim: (205, 205, 205),
    faint: (160, 160, 160),
    bar: (0, 0, 0),
    surface_2: (16, 16, 16),
    surface_3: (30, 30, 30),
    separator: (110, 130, 175),
};

/// 当前档位的色板（取色函数的唯一分发点）。
pub(super) fn current() -> &'static Tokens {
    match ThemeMode::current() {
        ThemeMode::Dark => &DARK,
        ThemeMode::Light => &LIGHT,
        ThemeMode::Contrast => &CONTRAST,
    }
}
