// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 键位分派共用判定：只读面板集合与日志用的安全截断。

use crate::app::ActivePanel;

pub(super) fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // T-02（P0-7 同族修复）：字节截断可能落在 UTF-8 多字节字符中间导致
    // panic（中文输入 > max 字节必现）。回退到最近的前序字符边界。
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// 只读展示面板：Help/Config/Logs/Memory/Plugins/Think/Focus 无输入语义，
/// Enter 与普通字符键不应写入或提交对话面板的共享输入（Board/Events 有
/// 各自的选中/过滤键位，不在此列）。
pub(super) fn read_only_panel(p: ActivePanel) -> bool {
    matches!(
        p,
        ActivePanel::Help
            | ActivePanel::Config
            | ActivePanel::Logs
            | ActivePanel::Memory
            | ActivePanel::Plugins
            | ActivePanel::Think
            | ActivePanel::Focus
            | ActivePanel::Health
    )
}

#[cfg(test)]
mod tests {
    use super::truncate_str;

    #[test]
    fn truncate_str_never_splits_multibyte_chars() {
        // T-02（P0-7 同族）：中文输入 80 字节截断窗——修复前第 80 字节
        // 落在多字节字符中间时 panic。
        let s = "汉".repeat(40); // 120 字节
        let out = truncate_str(&s, 80);
        assert_eq!(out, "汉".repeat(26)); // 78 字节处边界回退
        assert_eq!(out.len(), 78);

        // 短文本原样返回
        assert_eq!(truncate_str("hello", 80), "hello");
        assert_eq!(truncate_str("中文", 80), "中文");
        // 恰好在边界
        assert_eq!(truncate_str("中", 3), "中");
        // 截断点为 4 字节 emoji 中间时回退到 0
        assert_eq!(truncate_str("\u{1F600}", 2), "");
        // 空串
        assert_eq!(truncate_str("", 10), "");
    }
}
