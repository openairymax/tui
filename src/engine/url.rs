// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 链接命中（0.1.21 X5-11b）：把正文可见行里的 http(s) 链接按「显示列区间」
// 建档，点击时用同一份几何反查并交给系统打开器。入参是已经落格的纯文本行，
// 因此宽字符、缩进与折行后的真实列位天然一致，不需要第二套坐标换算。
//
// 安全边界：链接来自模型输出/工具结果，只允许 http/https 交给打开器；
// file:// 等本地协议一律拒绝，长度超过上限也拒绝（避免超长参数注入）。

use std::io::{self, Stdio};
use std::process::Command;

use unicode_width::UnicodeWidthStr;

/// 单个链接在行内的显示列区间（右开）与原始地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: u16,
    pub end: u16,
    pub url: String,
}

/// 地址上限：超过即视为异常载荷，不建档也不打开。
const MAX_URL: usize = 2048;

/// 扫描一行的全部 http(s) 链接。起点必须是独立 token（前一字符为空白或
/// 标点），避免把 `xhttp://` 之类的内嵌串误判为链接。
pub(crate) fn scan(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some((off, scheme)) = next_scheme(&line[from..]) {
        let start_b = from + off;
        let joined = line[..start_b]
            .chars()
            .next_back()
            .map(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | ':' | '_'))
            .unwrap_or(false);
        if joined {
            from = start_b + scheme;
            continue;
        }
        let tail = &line[start_b..];
        let len = end_of_url(tail);
        let url = tail[..len].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if url.len() > scheme && url.len() <= MAX_URL {
            let start = UnicodeWidthStr::width(&line[..start_b]) as u16;
            let end = start.saturating_add(UnicodeWidthStr::width(url) as u16);
            out.push(Span {
                start,
                end,
                url: url.to_string(),
            });
        }
        from = start_b + len;
    }
    out
}

/// 反查点击落点：同一行内命中区间左闭右开。未命中返回 None（点击可能是
/// 普通文字/空白，调用方回落到焦点切换）。
pub(crate) fn find<'a>(rows: &'a [(u16, Span)], row: u16, col: u16) -> Option<&'a str> {
    rows.iter()
        .find(|(r, s)| *r == row && col >= s.start && col < s.end)
        .map(|(_, s)| s.url.as_str())
}

/// 用系统打开器打开链接。只接受 http(s) 且无空白字符；其余返回
/// InvalidInput，由调用方转成提示文案而不是静默失败。
pub(crate) fn open(url: &str) -> io::Result<()> {
    let target = allow(url)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "仅支持 http(s) 链接"))?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(opener)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // 打开器常驻到用户关闭页面，绝不能同步等待；但也不能完全撒手，否则
    // 子进程退出后成为僵尸。交给后台线程回收，主循环零阻塞。
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn allow(url: &str) -> Option<&str> {
    let t = url.trim();
    let scheme_ok = t.starts_with("http://") || t.starts_with("https://");
    (scheme_ok && t.len() <= MAX_URL && !t.chars().any(char::is_whitespace)).then_some(t)
}

/// 最早的 `http://` / `https://` 起点及其长度（用于跳过该前缀继续扫描）。
fn next_scheme(s: &str) -> Option<(usize, usize)> {
    match (s.find("http://").map(|o| (o, 7)), s.find("https://").map(|o| (o, 8))) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// 链接终止位置：空白与常见包裹/句读符号。返回值相对 `s` 起点。
fn end_of_url(s: &str) -> usize {
    s.find(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ')' | ']' | '}' | '>' | '"' | '\'' | ',' | ';' | '，' | '。' | '、' | '；' | '：'
            )
    })
    .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_plain_link_columns() {
        let spans = scan("see https://example.com/a?b=1 now");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 4);
        assert_eq!(spans[0].url, "https://example.com/a?b=1");
        assert_eq!(
            (spans[0].end - spans[0].start) as usize,
            "https://example.com/a?b=1".len()
        );
    }

    #[test]
    fn columns_count_wide_chars() {
        let spans = scan("链接：https://x.cn");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 6, "两个汉字 + 全角冒号 = 6 列");
    }

    #[test]
    fn trims_trailing_punctuation_and_wrappers() {
        let spans = scan("(见 https://a.cn/x)。");
        assert_eq!(spans[0].url, "https://a.cn/x");
        assert_eq!(scan("https://a.cn/x.")[0].url, "https://a.cn/x");
        assert_eq!(scan("http://a.cn/x,")[0].url, "http://a.cn/x");
    }

    #[test]
    fn scans_multiple_and_skips_embedded() {
        let spans = scan("https://a.cn 和 https://b.cn/p 以及 xhttps://c.cn");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].url, "https://b.cn/p");
    }

    #[test]
    fn hit_test_is_half_open() {
        let spans = scan("go https://a.cn now");
        let rows = vec![(3u16, spans[0].clone())];
        assert_eq!(find(&rows, 3, 9), Some("https://a.cn"));
        assert_eq!(find(&rows, 3, 3), Some("https://a.cn"));
        assert_eq!(find(&rows, 3, 2), None, "链接左侧空白不是链接");
        assert_eq!(find(&rows, 3, 17), None, "右边界开区间");
        assert_eq!(find(&rows, 4, 9), None, "别的行不命中");
    }

    #[test]
    fn opener_rejects_non_http() {
        assert!(open("file:///etc/passwd").is_err());
        assert!(open("https://a.cn/ x").is_err());
    }
}
