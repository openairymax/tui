// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT TUI 二进制入口：启动编排与主循环在库的 `entry` 模块，本文件
// 保持单行转发，使 golden 测试能以库形式驱动同一套 App/渲染代码。

fn main() {
    agentrt_tui::entry::run();
}
