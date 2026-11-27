// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 代码块层（0.1.18 W6）：语言徽章 + 作用域着色 + 超长行横向省略。
//
// 词法与样式分两层：词法（lex.rs，X4-05a）只产出文本与作用域栈，本层只消费
// syntect 的 scope 分类（keyword/string/comment/number/function/type...）并映射
// 到本仓 style::* 设计令牌，不消费其调色板——Cargo.toml 关掉 default-themes，
// 着色随深浅主题与色深切换，无第二套颜色来源；同一段词法结果可按主题复染。
//
// 宽度纪律：超长行经 L2 唯一裁决点 `crate::engine::grid::clip_spans` 横向省略，
// 本模块不自行测量后截断。

use crate::engine::grid;
use crate::style;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{lang, lex};

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
    // 徽章展示探测后的规范名：```main.py 与 ```py 应显示同一个语言
    let shown = match lang::detect(lang) {
        Some(found) => found.id,
        None => lang,
    };

    if !shown.is_empty() {
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent), Style::default()),
            Span::styled(
                format!("  {}  ", grid::clip(shown, budget)),
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

/// 逐行分词着色：词法由 lex 层产出（X4-05a），本函数只做作用域 → 设计令牌
/// 的样式映射（X4-05b）。语言无法识别时 lex 退化为纯文本，绝不放弃整块内容。
fn highlight(lang: &str, body: &str, base: Style) -> Vec<Vec<Span<'static>>> {
    // 走探测表：syntect 的 token 查找认不出 `main.py`、`BASH` 这类写法
    let token = match lang::detect(lang) {
        Some(found) => found.token,
        None => lang,
    };
    lex::lines(token, body)
        .into_iter()
        .map(|row| token_spans(row, base))
        .collect()
}

/// token 行 → 带样式片段：token → SGR 的唯一映射点（X4-05b）。
///
/// 词法层只给作用域栈，颜色在此裁决；后续折行（X4-07）拿到的是已定型
/// 的片段序列，折行只需保持样式，不再回到作用域判定。
fn token_spans(row: Vec<lex::Token>, base: Style) -> Vec<Span<'static>> {
    row.into_iter()
        .map(|t| Span::styled(t.text, scope_style(&t.scopes, base)))
        .collect()
}

/// scope 栈 → 设计令牌样式。由最具体的作用域向外匹配，未分类作用域向上回退。
fn scope_style(scopes: &[String], base: Style) -> Style {
    for name in scopes.iter().rev() {
        if let Some(style) = classify(name, base) {
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
    fn path_tag_renders_canonical_badge() {
        let lines = render_code("src/main.py", "x = 1", 0, 60, Style::default());
        assert!(joined(&lines).contains("python"), "应显示探测后的规范名");
        assert!(!joined(&lines).contains("main.py"), "徽章不显示原始路径");
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

    /// X4-05b：token 作用域到设计令牌的映射表逐项钉死。
    #[test]
    fn scope_categories_map_to_design_tokens() {
        use ratatui::style::Color;
        let fg = |name: &str| classify(name, Style::default()).and_then(|s| s.fg);
        let table: &[(&str, Color)] = &[
            ("comment.line.double-slash", style::faint()),
            ("string.quoted.double", style::success()),
            ("constant.numeric.integer", style::magenta()),
            ("constant.language.boolean", style::warning()),
            ("keyword.control.flow", style::primary()),
            ("storage.type.function", style::primary()),
            ("entity.name.function", style::cyan()),
            ("entity.name.type", style::accent()),
            ("support.function.builtin", style::cyan()),
            ("support.type.primitive", style::accent()),
            ("variable.other.readwrite", style::text()),
            ("punctuation.terminator", style::dim()),
            ("invalid.illegal", style::danger()),
        ];
        for (name, want) in table {
            assert_eq!(fg(name), Some(*want), "作用域映射漂移: {name}");
        }
        // 结构性包裹作用域不表态，否则整块正文会被染成同一色
        for name in ["source.rust", "meta.block", "markup.heading"] {
            assert_eq!(classify(name, Style::default()), None, "{name}");
        }
        // 关键字加粗、注释斜体：颜色之外还有字形冗余，8 色档下仍可区分
        let kw = classify("keyword.control", Style::default()).unwrap();
        assert!(kw.add_modifier.contains(Modifier::BOLD));
        let cm = classify("comment.line", Style::default()).unwrap();
        assert!(cm.add_modifier.contains(Modifier::ITALIC));
    }

    /// X4-05b：八种语言都产出非默认取色的片段，且文本一字不丢。
    #[test]
    fn eight_languages_are_colored() {
        const SAMPLES: &[(&str, &str)] = &[
            ("rust", "fn main() { let s = \"hi\"; }"),
            ("python", "def f():\n    return \"hi\""),
            ("javascript", "const s = \"hi\";"),
            ("typescript", "const s: string = \"hi\";"),
            ("go", "var s = \"hi\""),
            ("c", "char *s = \"hi\";"),
            ("sh", "echo \"hi\""),
            ("json", "{\"s\": \"hi\"}"),
        ];
        for (tag, body) in SAMPLES {
            let token = lang::detect(tag).expect("白名单内语言应当可探测").token;
            let styled: Vec<Vec<Span<'static>>> = lex::lines(token, body)
                .into_iter()
                .map(|row| token_spans(row, Style::default()))
                .collect();
            assert!(
                styled
                    .iter()
                    .flatten()
                    .any(|s| { s.style.fg.is_some() && !s.content.trim().is_empty() }),
                "{tag} 未产出着色片段: {styled:?}"
            );
            let joined: String = styled
                .iter()
                .map(|row| row.iter().map(|s| s.content.as_ref()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(joined, *body, "{tag} 着色改了文本");
        }
    }
}
