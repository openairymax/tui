// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 代码块层（0.1.18 W6）：语言徽章 + syntect 分词着色 + 超长行横向省略。
//
// 主题纪律：本层只消费 syntect 的 scope 分类（keyword/string/comment/number/
// function/type...），不消费其调色板——Cargo.toml 关掉 default-themes，着色一律
// 由本仓 theme::* 设计令牌裁决，随深浅主题与色深切换，无第二套颜色来源。
//
// 宽度纪律：超长行经 L2 唯一裁决点 `crate::engine::grid::clip_spans` 横向省略，
// 本模块不自行测量后截断。

use crate::engine::grid;
use crate::theme;
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
    let bg = base.bg(theme::surface());

    if !lang.is_empty() {
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent), Style::default()),
            Span::styled(
                format!("  {}  ", grid::clip(lang, budget)),
                base.fg(theme::accent())
                    .bg(theme::surface_active())
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
        "comment" => base.fg(theme::faint()).add_modifier(Modifier::ITALIC),
        "string" => base.fg(theme::success()),
        "constant" => {
            if name.starts_with("constant.numeric") {
                base.fg(theme::magenta())
            } else {
                base.fg(theme::warning())
            }
        }
        "keyword" => base.fg(theme::primary()).add_modifier(Modifier::BOLD),
        "storage" => base.fg(theme::primary()),
        "entity" => {
            if name.starts_with("entity.name.function") {
                base.fg(theme::cyan())
            } else {
                base.fg(theme::accent())
            }
        }
        "support" => {
            if name.starts_with("support.function") {
                base.fg(theme::cyan())
            } else {
                base.fg(theme::accent())
            }
        }
        "variable" => base.fg(theme::text()),
        "punctuation" => base.fg(theme::dim()),
        "invalid" => base.fg(theme::danger()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(lines: &[Line<'static>]) -> String {
        lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")
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
            .any(|s| s.content.contains("let") && s.style.fg == Some(theme::primary()));
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
}
