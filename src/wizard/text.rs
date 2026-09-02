// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 编辑态文本工具：字符边界步进（统一字节语义，防多字节切片 panic）、
// 字节→字符索引折算、按 unicode 显示宽度换行。

/// 字节位置 → 前一个字符边界（Backspace / ← 步进；编辑态统一字节语义，
/// 防多字节字符中间切片 panic）
pub(crate) fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    if pos == 0 {
        return 0;
    }
    match s[..pos].chars().next_back() {
        Some(c) => pos - c.len_utf8(),
        None => 0,
    }
}

/// 字节位置 → 后一个字符边界（→ 步进）
pub(crate) fn next_char_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    match s[pos..].chars().next() {
        Some(c) => pos + c.len_utf8(),
        None => s.len(),
    }
}

/// 字节索引 → 字符索引（渲染窗口计算用）
pub(crate) fn byte_to_char(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx.min(s.len())].chars().count()
}

/// 按 unicode 显示宽度换行（CJK 双列，空格处断行）
pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + w > width && !cur.is_empty() {
            out.push(cur);
            cur = String::new();
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_boundary_step_in_bytes() {
        let s = "智谱GLM"; // 智=3B 谱=3B G L M 各 1B → len 9
        assert_eq!(s.len(), 9);
        assert_eq!(prev_char_boundary(s, 9), 8);
        assert_eq!(prev_char_boundary(s, 8), 7);
        assert_eq!(prev_char_boundary(s, 7), 6);
        assert_eq!(prev_char_boundary(s, 6), 3, "回退整个 CJK 字符");
        assert_eq!(prev_char_boundary(s, 0), 0);
        assert_eq!(next_char_boundary(s, 0), 3);
        assert_eq!(next_char_boundary(s, 6), 7);
        assert_eq!(next_char_boundary(s, 9), 9, "末尾不再前进");
        assert_eq!(prev_char_boundary(s, 99), 8, "越界钳位");
        assert_eq!(next_char_boundary(s, 99), 9);
    }

    #[test]
    fn remove_at_boundary_never_panics() {
        let mut s = "智谱GLM".to_string();
        let mut pos = s.len();
        while pos > 0 {
            pos = prev_char_boundary(&s, pos);
            s.remove(pos);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn byte_to_char_counts() {
        let s = "智谱GLM";
        assert_eq!(byte_to_char(s, 0), 0);
        assert_eq!(byte_to_char(s, 3), 1);
        assert_eq!(byte_to_char(s, 6), 2);
        assert_eq!(byte_to_char(s, 9), 5);
        assert_eq!(byte_to_char(s, 99), 5);
    }

    #[test]
    fn wrap_respects_display_width() {
        assert_eq!(wrap_text("hello world", 5), vec!["hello", " worl", "d"]);
        let wrapped = wrap_text("上下文窗口", 4);
        assert_eq!(wrapped, vec!["上下", "文窗", "口"]);
        assert_eq!(wrap_text("", 10), Vec::<String>::new());
    }
}
