// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 代码块层（0.1.18 W6）：语言徽章 + syntect 分词着色 + 超长行横向省略。
//
// 主题纪律：本层只消费 syntect 的 scope 分类（keyword/string/comment/number/
// function/type...），不消费其调色板——Cargo.toml 关掉 default-themes，着色一律
// 由本仓 style::* 设计令牌裁决，随深浅主题与色深切换，无第二套颜色来源。
//
// 宽度纪律：超长行经 L2 唯一裁决点 `crate::engine::grid::clip_spans` 横向省略，
// 本模块不自行测量后截断。

use crate::engine::grid;
use crate::style;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::sync::OnceLock;
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

/// 语法定义集：进程内一次加载。
///
/// packdump 反序列化在百毫秒量级，逐代码块重载不可接受；L1 解析缓存（key +
/// 内容指纹 + 宽度）保证同一段内容只走到这里一次。
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

/// 渲染代码块：语言徽章 + 高亮正文（超长行横向省略）。
pub(super) fn render_code(
    lang: &str,
    body: &str,
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // 正文每行另占 indent + 1 列前缀，预算须先扣除，否则窄屏下必然右溢
    let budget = width.saturating_sub(indent + 1);
    let bg = base.bg(style::surface());

    if !lang.is_empty() {
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent), Style::default()),
            Span::styled(
                format!("  {}  ", grid::clip(lang, budget)),
                base.fg(style::accent())
                    .bg(style::surface_active())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    for line in highlight(lang, body, bg) {
        let mut spans = vec![Span::styled(" ".repeat(indent + 1), bg)];
        spans.extend(grid::clip_spans(&line, budget));
        out.push(Line::from(spans));
    }
    out
}

/// 逐行分词着色。语言无法识别时退化为纯文本着色，绝不放弃整块内容。
fn highlight(lang: &str, body: &str, base: Style) -> Vec<Vec<Span<'static>>> {
    let set = syntaxes();
    let syntax = if lang.is_empty() {
        set.find_syntax_plain_text()
    } else {
        set.find_syntax_by_token(lang)
            .unwrap_or_else(|| set.find_syntax_plain_text())
    };

    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();

    for line in body.split('\n') {
        let mut spans: Vec<Span<'static>> = Vec::new();
        match state.parse_line(line, set) {
            Ok(ops) => {
                for (text, op) in ScopeRegionIterator::new(&ops, line) {
                    if stack.apply(op).is_err() {
                        stack = ScopeStack::new();
                    }
                    if !text.is_empty() {
                        spans.push(Span::styled(text.to_string(), scope_style(&stack, base)));
                    }
                }
            }
            Err(_) => {
                if !line.is_empty() {
                    spans.push(Span::styled(line.to_string(), base));
                }
            }
        }
        out.push(spans);
    }
    out
}

/// scope 栈 → 设计令牌样式。由最具体的作用域向外匹配，未分类作用域向上回退。
fn scope_style(stack: &ScopeStack, base: Style) -> Style {
    for scope in stack.as_slice().iter().rev() {
        let name = scope.build_string();
        if let Some(style) = classify(&name, base) {
            return style;
        }
    }
    base
}

/// 单条作用域名的令牌映射；`None` 表示不表态，交外层继续匹配。
///
/// 结构性包裹作用域（`source` / `meta`）一律不表态：它们覆盖整段文本，一旦
/// 着色会把整个函数体染成同一颜色。
fn classify(name: &str, base: Style) -> Option<Style> {
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "comment" => base.fg(style::faint()).add_modifier(Modifier::ITALIC),
        "string" => base.fg(style::success()),
        "constant" => {
            if name.starts_with("constant.numeric") {
                base.fg(style::magenta())
            } else {
                base.fg(style::warning())
            }
        }
        "keyword" => base.fg(style::primary()).add_modifier(Modifier::BOLD),
        "storage" => base.fg(style::primary()),
        "entity" => {
            if name.starts_with("entity.name.function") {
                base.fg(style::cyan())
            } else {
                base.fg(style::accent())
            }
        }
        "support" => {
            if name.starts_with("support.function") {
                base.fg(style::cyan())
            } else {
                base.fg(style::accent())
            }
        }
        "variable" => base.fg(style::text()),
        "punctuation" => base.fg(style::dim()),
        "invalid" => base.fg(style::danger()),
        _ => return None,
    })
}

/// 解析一行围栏（CommonMark 形态）：行首最多 3 个空格，连续 ≥3 个反引号
/// 或波浪号。返回（围栏字符、连续长度、围栏之后的剩余文本）。
fn fence(line: &str) -> Option<(u8, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = rest.bytes().take_while(|b| *b == ch).count();
    if run < 3 {
        return None;
    }
    Some((ch, run, &rest[run..]))
}

/// 提取最后一个完整围栏代码块的原文（TUI 方案 §6.3 #5：leader → y 复制）。
///
/// 只认完整块：未闭合的尾块不算（流式中途的半块不能被复制成残缺内容）。
/// 返回内容保持原文换行，去掉闭合围栏前的行尾换行符；空块不参与选取。
pub(crate) fn last_fenced(text: &str) -> Option<String> {
    let mut open: Option<(u8, usize, usize)> = None;
    let mut found: Option<String> = None;
    let mut pos = 0usize;

    for line in text.split_inclusive('\n') {
        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match open {
            Some((ch, want, start)) => {
                if let Some((c, n, tail)) = fence(raw) {
                    if c == ch && n >= want && tail.trim().is_empty() {
                        let body = &text[start..pos];
                        let body = body.strip_suffix('\n').unwrap_or(body);
                        let body = body.strip_suffix('\r').unwrap_or(body);
                        if !body.is_empty() {
                            found = Some(body.to_string());
                        }
                        open = None;
                    }
                }
            }
            None => {
                if let Some((c, n, tail)) = fence(raw) {
                    // 反引号围栏的信息串不得含反引号（CommonMark），波浪号不受限。
                    if c == b'~' || !tail.contains('`') {
                        open = Some((c, n, pos + line.len()));
                    }
                }
            }
        }
        pos += line.len();
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_badge_and_body() {
        let lines = render_code("rust", "fn main() {}", 0, 60, Style::default());
        assert_eq!(lines.len(), 2, "徽章行 + 正文行");
        assert!(joined(&lines).contains("rust"));
        assert!(joined(&lines).contains("fn main() {}"));
    }

    #[test]
    fn omits_badge_without_language() {
        let lines = render_code("", "plain", 0, 60, Style::default());
        assert_eq!(lines.len(), 1);
        assert!(!joined(&lines).contains("  "));
    }

    #[test]
    fn unknown_language_degrades_to_plain() {
        let lines = render_code("no-such-lang-xyz", "some text", 0, 60, Style::default());
        assert!(joined(&lines).contains("some text"));
    }

    #[test]
    fn keyword_is_highlighted() {
        let lines = render_code("rust", "let x = 1;", 0, 60, Style::default());
        let colored = lines[1]
            .spans
            .iter()
            .any(|s| s.content.contains("let") && s.style.fg == Some(style::primary()));
        assert!(colored, "关键字应取主色: {:?}", lines[1]);
    }

    #[test]
    fn long_line_is_clipped_with_ellipsis() {
        let long = "x".repeat(200);
        let lines = render_code("", &long, 0, 40, Style::default());
        assert!(joined(&lines).contains('…'), "超长行应横向省略");
        for line in &lines {
            assert!(grid::line_w(&line.spans) <= 40, "行宽越界: {line:?}");
        }
    }

    #[test]
    fn keeps_blank_lines() {
        let lines = render_code("", "a\n\nb", 0, 40, Style::default());
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn last_fenced_returns_latest_complete_block() {
        let md = "before\n```rust\nfn a() {}\n```\nmid\n~~~py\nprint(1)\n~~~\n";
        assert_eq!(last_fenced(md).as_deref(), Some("print(1)"));
    }

    #[test]
    fn last_fenced_ignores_unclosed_tail() {
        let md = "```sh\necho ok\n```\n```sh\necho half\n";
        assert_eq!(
            last_fenced(md).as_deref(),
            Some("echo ok"),
            "未闭合尾块不参与选取"
        );
    }

    #[test]
    fn last_fenced_accepts_indented_fence() {
        let md = "   ```\n   x\n   ```\n";
        assert_eq!(last_fenced(md).as_deref(), Some("   x"));
    }

    #[test]
    fn last_fenced_requires_matching_char_and_length() {
        let md = "````\na\n```\nb\n````\n";
        assert_eq!(
            last_fenced(md).as_deref(),
            Some("a\n```\nb"),
            "短围栏不能闭合长围栏"
        );
    }

    #[test]
    fn last_fenced_skips_inline_and_empty() {
        assert_eq!(last_fenced("no code `inline` here"), None);
        assert_eq!(last_fenced("```\n```\n"), None, "空块不参与选取");
    }
}
