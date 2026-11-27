// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 代码块层用例：徽章、作用域映射、八语言着色、缓存键与围栏选取。
// 用例单独成文件，避免生产代码文件在追加用例后越过单文件 SLOC 上限。

use super::*;
use ratatui::style::Color;

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

/// X4-07a：着色后折行——样式跨行延续，行宽不越界，字符不丢。
#[test]
fn highlighted_lines_wrap_without_breaking_sgr() {
    let literal = format!("let s = \"{}\";", "x".repeat(120));
    let lines = render_code("rust", &literal, 0, 30, Style::default());
    assert!(lines.len() > 3, "超长行应折行: {}", lines.len());
    let mut seen = 0usize;
    for line in lines.iter().skip(1) {
        assert!(grid::line_w(&line.spans) <= 30, "折行越界: {line:?}");
        for span in &line.spans {
            if span.content.contains('x') {
                seen += span.content.matches('x').count();
                assert_eq!(
                    span.style.fg,
                    Some(style::success()),
                    "字符串 token 跨折行续色失败: {span:?}"
                );
            }
        }
    }
    assert_eq!(seen, 120, "折行不得丢字符");
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

/// X4-06a：同一（文本 / 宽度 / 语言）三元组复用缓存，任一维变化即失配。
#[test]
fn code_cache_hits_on_same_triple() {
    reset();
    let body = "fn main() { let s = \"hi\"; }";
    let first = render_code("rust", body, 0, 60, Style::default());
    let again = render_code("rust", body, 0, 60, Style::default());
    assert_eq!(first, again, "命中缓存后输出必须与首次一致");
    let s = stats();
    assert_eq!((s.hits, s.misses), (1, 1), "第二次应命中: {s:?}");

    // 文本变化：内容指纹失配
    let edited = render_code("rust", "fn main() {}", 0, 60, Style::default());
    assert_ne!(first, edited);
    // 语言变化：词法不同，即使正文相同也必须失配
    let as_py = render_code("python", body, 0, 60, Style::default());
    assert_ne!(first, as_py);
    let s = stats();
    assert_eq!((s.hits, s.misses), (1, 3), "文本/语言变化不应命中: {s:?}");

    // 宽度变化：折行预算不同，必须失配
    let narrow = render_code("rust", body, 0, 20, Style::default());
    assert_ne!(first, narrow);
    let s = stats();
    assert_eq!((s.hits, s.misses), (1, 4), "宽度变化不应命中: {s:?}");
    assert_eq!(s.entries, 4, "四个不同键各占一条");
}

/// X4-06b（IS-08 回归）：稳定围栏块逐帧重绘的命中率必须达标。
///
/// 流式会话里围栏块生成完毕后，每帧仍会重绘整条消息；若此处不命中，
/// 词法分析会随代码块长度线性拖慢每一帧。
#[test]
fn stable_block_hit_rate_meets_target() {
    reset();
    let body = "fn main() { let s = \"hi\"; }";
    const FRAMES: u64 = 600;
    for _ in 0..FRAMES {
        let _ = render_code("rust", body, 2, 80, Style::default());
    }
    let s = stats();
    assert_eq!(s.misses, 1, "稳定块只应首帧未命中: {s:?}");
    assert_eq!(s.hits, FRAMES - 1, "其余帧都应命中: {s:?}");
    let rate = s.hits as f64 / (s.hits + s.misses) as f64;
    assert!(rate >= 0.95, "命中率未达 95% 红线: {rate:.3} {s:?}");
}

/// X4-06b：正文逐帧增长时全部失效，但条目与字节都不越双界。
///
/// 增长中的代码块无法复用，缓存必须靠淘汰兜底，否则长会话会被
/// 每帧一个新键撑爆内存。
#[test]
fn streaming_growth_stays_bounded() {
    reset();
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&format!("let v{i} = {i};\n"));
        let _ = render_code("rust", &body, 0, 80, Style::default());
    }
    let s = stats();
    assert_eq!(s.misses, 300, "每帧正文都在变，不应有命中: {s:?}");
    assert_eq!(s.hits, 0, "增长中的正文不得产生命中: {s:?}");
    assert!(
        s.entries <= CODE_CACHE_ENTRIES,
        "条目越界: {} > {CODE_CACHE_ENTRIES}",
        s.entries
    );
    assert!(
        s.bytes <= CODE_CACHE_BYTES,
        "字节越界: {} > {CODE_CACHE_BYTES}",
        s.bytes
    );
}
