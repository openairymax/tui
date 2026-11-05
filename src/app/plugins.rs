// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Plugins 面板状态：技能库整表滚动。
//
// 0.1.20 TUI 缺口④：此前渲染层硬编码 take(10)，第 11 条起的技能在 UI 上
// 不可见——"技能沉淀了但看不见"与没有沉淀等价。整表渲染 + PgUp/PgDn
// 滚动后，展示不再受常量截断；滚动位置是纯视图状态，随面板重开复位。

use super::App;

/// PgUp/PgDn 一次翻过的行数（与 Config/Health 面板翻页粒度一致）。
const PLUGINS_PAGE: u16 = 6;

impl App {
    pub fn plugins_pgup(&mut self) {
        self.plugins_scroll = self.plugins_scroll.saturating_sub(PLUGINS_PAGE);
    }

    pub fn plugins_pgdn(&mut self) {
        self.plugins_scroll = self.plugins_scroll.saturating_add(PLUGINS_PAGE);
    }
}
