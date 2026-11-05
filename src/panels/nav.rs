// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 面板页脚导航提示（缺口⑤）：观测三面板（Health/Evolution/Trace）此前只能
// 靠记忆斜杠命令打开，键位不可发现。页脚单点定义 F 键互达提示，三面板共用
// 一份文案——键位增删只改这里，不逐面板同步。

use ratatui::{style::Style, text::Span};

use crate::style;

/// 观测面板间互达的一行提示（追加到各面板页脚）。
pub fn observe_hint() -> Span<'static> {
    Span::styled(
        "  F10 健康 · F11 演化 · F12 轨迹 · F9 后端日志",
        Style::default().fg(style::faint()),
    )
}
