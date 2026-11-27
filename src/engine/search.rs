// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全文会话检索（0.1.21 X8-04b）：按显示行做子串匹配，纯文本、无 IO。
//
// 检索的对象是「已物化的渲染行」而不是消息原文：命中位置要能直接换算成
// 滚动行号，否则跳转还得二次反推折行。匹配按字符边界推进，ASCII 大小写
// 不敏感、非 ASCII 精确匹配——`char::eq_ignore_ascii_case` 不会像整体
// 小写化那样改变字节长度，行内切片因此始终落在合法的 UTF-8 边界上。

/// 行内命中区间（字节偏移，左闭右开）。
pub(crate) type Hit = (usize, usize);

/// 找出 `hay` 中所有 `needle` 命中区间；空针返回空表。
///
/// 命中后跳过整段针长，避免「aa 在 aaa 中命中两次」这类重叠结果让高亮
/// 与跳转计数翻倍。
pub(crate) fn find_all(hay: &str, needle: &str) -> Vec<Hit> {
    if needle.is_empty() {
        return Vec::new();
    }
    let n: Vec<char> = needle.chars().collect();
    let h: Vec<(usize, char)> = hay.char_indices().collect();
    let mut hits = Vec::new();
    let mut i = 0usize;
    while i + n.len() <= h.len() {
        let same = (0..n.len()).all(|k| h[i + k].1.eq_ignore_ascii_case(&n[k]));
        if same {
            let start = h[i].0;
            let end = match h.get(i + n.len()) {
                Some((off, _)) => *off,
                None => hay.len(),
            };
            hits.push((start, end));
            i += n.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// 在行表里找命中行号（自顶 0 起），超过 `cap` 即截断。
///
/// 截断而非拒绝：搜索是浏览辅助，命中量本身不承载语义，但行表来自整段
/// 会话，结果数必须有界，否则一次查询就能把待跳转表撑到与历史同阶。
pub(crate) fn hit_lines(lines: &[String], query: &str, cap: usize) -> Vec<u32> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if find_all(line, query).is_empty() {
            continue;
        }
        out.push(i as u32);
        if out.len() >= cap {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{find_all, hit_lines};

    #[test]
    fn ascii_match_is_case_insensitive() {
        assert_eq!(find_all("Error: ERROR", "error"), vec![(0, 5), (7, 12)]);
    }

    #[test]
    fn cjk_match_is_exact_and_offsets_are_bytes() {
        let hits = find_all("第 12 条 · 第 12 条", "第 12");
        assert_eq!(hits.len(), 2);
        assert_eq!(&"第 12 条 · 第 12 条"[hits[0].0..hits[0].1], "第 12");
    }

    #[test]
    fn overlapping_needle_advances_by_full_length() {
        assert_eq!(find_all("aaa", "aa"), vec![(0, 2)]);
        assert!(find_all("abc", "abd").is_empty());
        assert!(find_all("abc", "").is_empty());
    }

    #[test]
    fn no_hit_returns_empty() {
        assert!(find_all("abc", "abd").is_empty());
        assert!(find_all("abc", "").is_empty());
    }

    #[test]
    fn hit_lines_skips_blank_lines() {
        let lines = vec!["".to_string(), "有内容".to_string()];
        assert_eq!(hit_lines(&lines, "有", 8), vec![1]);
        assert!(hit_lines(&lines, "", 8).is_empty());
    }

    #[test]
    fn hit_lines_respects_cap() {
        let lines: Vec<String> = (0..10).map(|i| format!("命中 {i}")).collect();
        assert_eq!(hit_lines(&lines, "命中", 3), vec![0, 1, 2]);
    }
}
