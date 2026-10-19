// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 大 cell 摘要（0.1.19 §5.3 / §6.10 #6）：单个渲染 cell 超预算时保留
// 头尾各半，中间插入截断标记，渲染树不随单条内容无界增长。
//
// 边界语义：按 UTF-8 字符边界收缩，绝不切裂多字节码点；未超预算时返回
// None（调用方零拷贝）。标记内 N 为原始字节数，即 originalBytes 记账。

/// 保留头尾各半的裁剪（超 `limit` 字节才发生）。
///
/// 返回 `Some` 时全文（含中置标记）不超过 `limit` 字节；头段与尾段平分
/// 扣除标记后的余额，各自按字符边界向内收缩。`None` 表示原文在预算内、
/// 无需改写。
pub(crate) fn cap(text: &str, limit: usize) -> Option<String> {
    let len = text.len();
    if len <= limit {
        return None;
    }
    let marker = format!("\n… [truncated: {len} bytes total] …\n");
    let keep = limit.saturating_sub(marker.len());
    let head_end = floor_boundary(text, keep / 2);
    let tail_start = ceil_boundary(text, len - (keep - keep / 2));
    let mut out = String::with_capacity(limit.max(marker.len()));
    out.push_str(&text[..head_end]);
    out.push_str(&marker);
    out.push_str(&text[tail_start..]);
    Some(out)
}

/// 不大于 `at` 的最近字符边界。
fn floor_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// 不小于 `at` 的最近字符边界。
fn ceil_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while at < s.len() && !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §6.10 #6：预算内不改写（调用方保持零拷贝），恰好等于预算亦然。
    #[test]
    fn within_limit_is_untouched() {
        assert!(cap("", 8).is_none());
        assert!(cap("abcdefgh", 8).is_none());
        assert!(cap("中文四字", 12).is_none());
    }

    /// §5.3 裁剪语义：头尾各半 + 中置标记，标记携带原始总字节数，
    /// 且标记计入预算（输出总长不超过 limit）。
    #[test]
    fn over_limit_keeps_both_ends() {
        let text = format!("HEAD{}TAIL", "x".repeat(1_000));
        let limit = 200;
        let marker = marker_for(text.len());
        let out = cap(&text, limit).expect("超预算应裁剪");
        assert!(out.starts_with("HEAD"), "头段保留: {out}");
        assert!(out.ends_with("TAIL"), "尾段保留: {out}");
        assert!(out.contains(&marker), "标记须记原始字节数: {out}");
        assert!(out.len() <= limit, "含标记的总长不超预算: {}", out.len());
        let keep = limit - marker.len();
        let head_bytes = out.find(&marker).expect("标记位置");
        let tail_bytes = out.len() - head_bytes - marker.len();
        assert_eq!(head_bytes, keep / 2, "头段 = 余额的一半");
        assert_eq!(tail_bytes, keep - keep / 2, "尾段 = 余额的其余部分");
    }

    /// 字节安全：多字节文本在任意奇数预算下都不切裂码点。
    #[test]
    fn multibyte_never_splits_char() {
        let text = "中".repeat(100);
        let marker = marker_for(text.len());
        for limit in [99, 101, 103] {
            let out = cap(&text, limit).expect("超预算应裁剪");
            assert!(out.len() <= limit, "含标记的总长不超预算: limit={limit}");
            let kept = out.replace(&marker, "");
            assert!(
                kept.chars().all(|c| c == '中'),
                "不得出现切裂产生的替换字符: limit={limit}"
            );
            assert!(
                kept.len() <= limit - marker.len(),
                "保留字节不超过余额: limit={limit}"
            );
        }
    }

    /// 标记全文（含前后换行）。
    fn marker_for(len: usize) -> String {
        format!("\n… [truncated: {len} bytes total] …\n")
    }
}
