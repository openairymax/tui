// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 行内样式：**粗体** / `行内代码` / [链接](url) / ~~删除线~~ / 图片。
//
// 未闭合标记一律按普通文本回吐（绝不吞字），识别失败即字符级降级。

use crate::theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// 行内样式：**粗体** / `行内代码` / [链接](url) / ~~删除线~~（其余原样）。
pub(super) fn inline_styles(s: &str, base: Style) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let chars: Vec<char> = s.chars().collect();
    while i < chars.len() {
        // 行内代码 `...`（等宽底，优先于粗体识别）
        if chars[i] == '`' {
            flush_plain(&mut spans, &mut buf, base);
            let mut code = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1; // 跳过闭合 `
            }
            spans.push(Span::styled(
                format!(" {code} "),
                base.fg(theme::accent()).bg(theme::surface_active()),
            ));
            continue;
        }
        // 图片 ![alt](url)：终端不可渲染，降级为弱化占位
        if chars[i] == '!' && i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(consumed) = try_link(&chars, i + 1) {
                flush_plain(&mut spans, &mut buf, base);
                let (text, url) = consumed;
                spans.push(Span::styled(
                    format!("[图片: {text}]"),
                    base.fg(theme::faint()),
                ));
                // 整个 token = '!' + [text](url)
                i += 1 + text.chars().count() + url.chars().count() + 4;
                continue;
            }
        }
        // 链接 [text](url)：text 下划线 + 强调色
        if chars[i] == '[' {
            if let Some((text, url)) = try_link(&chars, i) {
                flush_plain(&mut spans, &mut buf, base);
                let text_len = text.chars().count();
                let url_len = url.chars().count();
                spans.push(Span::styled(
                    text,
                    base.fg(theme::accent()).add_modifier(Modifier::UNDERLINED),
                ));
                i += text_len + url_len + 4; // [text](url)
                continue;
            }
        }
        // **粗体**
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            flush_plain(&mut spans, &mut buf, base);
            let mut bold = String::new();
            i += 2;
            let mut closed = false;
            while i + 1 < chars.len() {
                if chars[i] == '*' && chars[i + 1] == '*' {
                    closed = true;
                    i += 2;
                    break;
                }
                bold.push(chars[i]);
                i += 1;
            }
            if !closed {
                // 未闭合：把已收集内容当普通文本（含开头的 **）
                buf.push_str("**");
                buf.push_str(&bold);
            } else {
                spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
            }
            continue;
        }
        // ~~删除线~~
        if chars[i] == '~' && i + 1 < chars.len() && chars[i + 1] == '~' {
            flush_plain(&mut spans, &mut buf, base);
            let mut strike = String::new();
            i += 2;
            let mut closed = false;
            while i + 1 < chars.len() {
                if chars[i] == '~' && chars[i + 1] == '~' {
                    closed = true;
                    i += 2;
                    break;
                }
                strike.push(chars[i]);
                i += 1;
            }
            if !closed {
                buf.push_str("~~");
                buf.push_str(&strike);
            } else {
                spans.push(Span::styled(
                    strike,
                    base.add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush_plain(&mut spans, &mut buf, base);
    Line::from(spans)
}

/// 尝试在 `chars[start] == '['` 处解析 `[text](url)`；成功返回 (text, url)。
/// 失败返回 None（按普通字符处理）。
fn try_link(chars: &[char], start: usize) -> Option<(String, String)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut close = start + 1;
    while close < chars.len() && chars[close] != ']' {
        close += 1;
    }
    if close >= chars.len() || close + 1 >= chars.len() || chars[close + 1] != '(' {
        return None;
    }
    let mut paren = close + 2;
    while paren < chars.len() && chars[paren] != ')' {
        paren += 1;
    }
    if paren >= chars.len() {
        return None;
    }
    let text: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..paren].iter().collect();
    if text.is_empty() || url.is_empty() {
        return None;
    }
    Some((text, url))
}

fn flush_plain(spans: &mut Vec<Span<'static>>, buf: &mut String, base: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), base));
    }
}
