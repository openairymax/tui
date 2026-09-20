// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 轻量终端 markdown 渲染器（P2-A 修复：表格/标题/列表/引用/行内样式）。
//
// 设计原则（50 工程标准 A-1 极简主义）：自研零依赖，不引入重型 markdown
// crate——terminal 渲染需要显示宽度对齐（中文全角），通用库难以满足。
//
// 宽度纪律（0.1.18 A 轨 W2，§3.3）：显示宽度的测量与截断一律委托 L2 唯一
// 裁决点 `crate::engine::grid`，本模块不得自行测量后截断或填充。
// 支持：
//   - 代码块（``` / ```lang，含语言徽章）
//   - 画板块（```plot：title/xs/ys → braille 点阵函数曲线，0.1.17）
//   - 表格（| a | b |，含分隔行对齐，P2-A 核心）
//   - 标题（# ~ ######）
//   - 列表（- / * / + / 1.，支持嵌套缩进；任务列表 [ ]/[x]，0.1.17）
//   - 引用（> 行，左边框线）
//   - 行内样式：**粗体** / `行内代码`
// 其余内容降级为纯文本（绝不出错，绝不截断语义）。

mod inline;
mod plot;
mod table;

use crate::engine::grid;
use crate::theme;
use inline::inline_styles;
use plot::render_plot;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use table::render_table;

#[cfg(test)]
use plot::braille_set;
#[cfg(test)]
use table::is_separator_row;

/// 渲染整段 markdown 内容为终端行序列。
///
/// `indent` 为内容整体左缩进（列数，UTF-8 空格按显示宽度对齐）。
/// `width` 为内容可用宽度。`base` 为普通文本样式（角色/工具消息沿用）。
///
/// 0.1.7 段落重排：连续普通文本行（源内无空行分隔）合并为一个段落，
/// 段落间自动留白——此前逐行直出导致长回复"文字挤在一起"，阅读困难。
pub fn render(content: &str, indent: usize, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let mut code_lang: String = String::new();
    // plot 画板块缓冲：```plot fence 内的 title/xs/ys 数据行
    let mut in_plot = false;
    let mut plot_lines: Vec<String> = Vec::new();
    // 表格块缓冲：连续以 | 开头的行（表头+分隔+数据）收集后统一对齐渲染
    let mut table: Vec<String> = Vec::new();
    // 段落缓冲：连续普通文本行（软换行）合并为一段
    let mut para: Vec<String> = Vec::new();

    let flush_para = |out: &mut Vec<Line<'static>>, para: &mut Vec<String>| {
        if para.is_empty() {
            return;
        }
        let text = para.join(" ");
        para.clear();
        let content_width = width.saturating_sub(indent).max(8);
        let mut pieces = grid::wrap(&text, content_width);
        if pieces.is_empty() {
            pieces.push(String::new());
        }
        for piece in pieces {
            let mut spans = vec![Span::styled(" ".repeat(indent), Style::default())];
            spans.extend(inline_styles(&piece, base).spans);
            out.push(Line::from(spans));
        }
        // 段落间留白（阅读呼吸感；消息间另有整体留白）
        out.push(Line::raw(""));
    };

    let push_table = |out: &mut Vec<Line<'static>>, table: &mut Vec<String>| {
        if !table.is_empty() {
            let rendered = render_table(table, indent, width, base);
            out.extend(rendered);
            table.clear();
        }
    };

    for raw in content.lines() {
        let trimmed = raw.trim();
        // ── 代码块 fence ──
        if trimmed.starts_with("```") {
            flush_para(&mut out, &mut para);
            push_table(&mut out, &mut table);
            if in_code {
                // 闭合 fence → 空行留白
                in_code = false;
                code_lang.clear();
                out.push(Line::raw(""));
            } else if in_plot {
                // 闭合 plot 块 → braille 画板渲染
                in_plot = false;
                out.extend(render_plot(&plot_lines, indent, width, base));
                plot_lines.clear();
                out.push(Line::raw(""));
            } else {
                // 开启 fence：```plot 进入画板模式，其余显示语言徽章
                let lang = trimmed.trim_matches('`').trim().to_string();
                if lang == "plot" {
                    in_plot = true;
                    plot_lines.clear();
                } else {
                    in_code = true;
                    code_lang = lang;
                    if !code_lang.is_empty() {
                        out.push(Line::from(vec![
                            Span::styled(" ".repeat(indent), Style::default()),
                            Span::styled(
                                format!("  {}  ", code_lang),
                                Style::default()
                                    .fg(theme::accent())
                                    .bg(theme::surface_active())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                    }
                }
            }
            continue;
        }
        // ── plot 块内：收集数据行（title/xs/ys），闭合时统一渲染 ──
        if in_plot {
            plot_lines.push(trimmed.to_string());
            continue;
        }
        // ── 代码块内：原样（等宽底色） ──
        if in_code {
            for piece in grid::wrap(raw, width.saturating_sub(1).max(8)) {
                out.push(Line::from(vec![
                    Span::styled(" ".repeat(indent + 1), Style::default()),
                    Span::styled(piece, base.bg(theme::surface())),
                ]));
            }
            continue;
        }
        // ── 表格块：收集连续 | 行 ──
        if trimmed.starts_with('|') {
            flush_para(&mut out, &mut para);
            table.push(trimmed.to_string());
            continue;
        }
        if !table.is_empty() {
            push_table(&mut out, &mut table);
        }
        // ── 标题（# ~ ######） ──
        if let Some(level) = heading_level(trimmed) {
            flush_para(&mut out, &mut para);
            let text = trimmed[level..].trim();
            if text.is_empty() {
                out.push(Line::raw(""));
                continue;
            }
            // 大标题加粗主色，小节用次强调色（Claude 的层次化排版）
            let color = if level == 1 {
                theme::primary()
            } else if level == 2 {
                theme::accent()
            } else {
                theme::text()
            };
            for piece in grid::wrap(text, width.saturating_sub(indent).max(8)) {
                out.push(Line::from(vec![
                    Span::styled(" ".repeat(indent), Style::default()),
                    Span::styled(piece, base.fg(color).add_modifier(Modifier::BOLD)),
                ]));
            }
            out.push(Line::raw(""));
            continue;
        }
        // ── 分隔线（--- / *** / ___，≥3 个） ──
        if is_hr(trimmed) {
            flush_para(&mut out, &mut para);
            let n = width.saturating_sub(indent).clamp(6, 40);
            out.push(Line::from(vec![
                Span::styled(" ".repeat(indent), Style::default()),
                Span::styled("─".repeat(n), base.fg(theme::separator())),
            ]));
            continue;
        }
        // ── 列表（- / * / + / 1. 及嵌套缩进） ──
        if let Some((mark, body)) = list_item(trimmed) {
            flush_para(&mut out, &mut para);
            // 任务列表（[ ] / [x]）：○ 待办 / ● 已完成（弱化+删除线）
            if let Some((done, rest)) = task_item(body) {
                let sym = if done { "●" } else { "○" };
                let sym_style = if done {
                    base.fg(theme::accent())
                } else {
                    base.fg(theme::dim())
                };
                let body_style = if done {
                    base.fg(theme::faint()).add_modifier(Modifier::CROSSED_OUT)
                } else {
                    base
                };
                // 前缀实测宽度（缩进 + 标记 + 空格 + 符号 + 空格）——正文预算由此推出，
                // 不手写常数：此前按 indent+标记宽+2 估算，较实际前缀少 1 列，任务列表
                // 正文折行后必然右溢 1 列。
                let prefix_w = indent + grid::width(&mark) + grid::width(sym) + 2;
                let lead = format!("{}{} ", " ".repeat(indent), mark);
                for piece in grid::wrap(rest, width.saturating_sub(prefix_w).max(8)) {
                    out.push(Line::from(vec![
                        Span::styled(lead.clone(), Style::default()),
                        Span::styled(format!("{sym} "), sym_style),
                        Span::styled(piece, body_style),
                    ]));
                }
                continue;
            }
            // 列表正文的缩进 = 整体缩进 + 标记与前导空格实测宽度
            let prefix_w = indent + grid::width(&mark) + 1;
            let content_width = width.saturating_sub(prefix_w).max(8);
            let lead = Span::styled(
                format!("{}{} ", " ".repeat(indent), mark),
                base.fg(theme::dim()),
            );
            for piece in grid::wrap(body, content_width) {
                // lead（Span）+ 行内样式（Line.spans）合并为同一行
                let mut spans = vec![lead.clone()];
                spans.extend(inline_styles(&piece, base).spans);
                out.push(Line::from(spans));
            }
            continue;
        }
        // ── 引用（> 行） ──
        if trimmed.starts_with('>') {
            flush_para(&mut out, &mut para);
            let body = trimmed.trim_start_matches('>').trim();
            if body.is_empty() {
                out.push(Line::raw(""));
                continue;
            }
            for piece in grid::wrap(body, width.saturating_sub(indent + 2).max(8)) {
                let mut spans = vec![
                    Span::styled(" ".repeat(indent), Style::default()),
                    Span::styled("▏", base.fg(theme::dim())),
                    Span::styled(" ", Style::default()),
                ];
                spans.extend(inline_styles(&piece, base).spans);
                out.push(Line::from(spans));
            }
            continue;
        }
        // ── 普通段落：软换行合并 + 段落间留白（0.1.7） ──
        if trimmed.is_empty() {
            flush_para(&mut out, &mut para);
        } else {
            para.push(trimmed.to_string());
        }
    }
    // 收尾：清空残留段落、表格块与未闭合的 plot 块
    flush_para(&mut out, &mut para);
    push_table(&mut out, &mut table);
    if in_plot {
        out.extend(render_plot(&plot_lines, indent, width, base));
    }
    out
}

/// 分隔线判定：全为 - / * / _ 且长度 ≥3。
fn is_hr(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    c.len() >= 3 && c.iter().all(|ch| matches!(ch, '-' | '*' | '_'))
}

/// 识别标题行：返回 # 的数量（1-6），非标题返回 None。
fn heading_level(s: &str) -> Option<usize> {
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    // 必须紧跟空格（"#标题" 视为普通文本）
    if s.as_bytes().get(hashes).copied() != Some(b' ') {
        return None;
    }
    Some(hashes)
}

/// 识别列表项：返回 (符号 mark, 正文)。支持 - / * / + / 1. / 1) 及嵌套缩进。
fn list_item(s: &str) -> Option<(String, &str)> {
    let trimmed = s.trim_start();
    // 有前导空白 → 嵌套列表（符号以 · 展示，与一级 • 区分）
    let mark = if trimmed.len() != s.len() {
        "·"
    } else {
        "•"
    };
    let bytes = trimmed.as_bytes();
    match bytes.first()? {
        b'-' | b'*' | b'+' => {
            if bytes.len() > 1 && bytes[1] == b' ' {
                Some((mark.to_string(), trimmed[2..].trim()))
            } else {
                None
            }
        }
        b'0'..=b'9' => {
            let mut i = 0;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
                Some((format!("{}.", &trimmed[..i]), trimmed[i + 2..].trim()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 识别任务列表体：返回 (是否完成, 剩余文本)。支持 [ ] / [x] / [X]。
fn task_item(body: &str) -> Option<(bool, &str)> {
    let b = body.as_bytes();
    if b.len() < 3 || b[0] != b'[' || b[2] != b']' {
        return None;
    }
    let done = match b[1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    let rest = body[3..].strip_prefix(' ').unwrap_or(&body[3..]);
    Some((done, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_detection() {
        assert_eq!(heading_level("# 标题"), Some(1));
        assert_eq!(heading_level("### 三级"), Some(3));
        assert_eq!(heading_level("#标题"), None); // 缺空格
        assert_eq!(heading_level("####### 七个"), None);
        assert_eq!(heading_level("普通文本"), None);
    }

    #[test]
    fn list_detection() {
        assert_eq!(list_item("- 项目"), Some(("•".to_string(), "项目")));
        assert_eq!(list_item("* 星号"), Some(("•".to_string(), "星号")));
        assert_eq!(list_item("1. 编号"), Some(("1.".to_string(), "编号")));
        assert_eq!(list_item("  - 嵌套"), Some(("·".to_string(), "嵌套")));
        assert_eq!(list_item("普通行"), None);
    }

    #[test]
    fn separator_detection() {
        assert!(is_separator_row("| --- | --- |"));
        assert!(is_separator_row("|:--|--:|"));
        assert!(!is_separator_row("| a | b |"));
        assert!(!is_separator_row("| x |"));
    }

    #[test]
    fn table_render_produces_lines() {
        let rows = vec![
            "| 名称 | 数值 |".to_string(),
            "| --- | --- |".to_string(),
            "| 苹果 | 12 |".to_string(),
        ];
        let lines = render_table(&rows, 2, 60, Style::default());
        // 表头 + 分隔线 + 数据 = 3 行
        assert_eq!(lines.len(), 3);
        // 数据行包含单元格内容
        assert!(lines.iter().any(|l| l.to_string().contains("苹果")));
    }

    #[test]
    fn render_keeps_plain_text() {
        let lines = render("你好世界", 0, 40, Style::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("你好世界"));
    }

    #[test]
    fn render_fenced_code() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render(md, 0, 60, Style::default());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("fn main() {}"));
    }

    /// R4 验收（§5 W2）：CJK + 全角标点混排表格，宽度全边界扫描右边界零溢出。
    #[test]
    fn table_right_edge_never_overflows() {
        let rows = vec![
            "| 名称 | 数值 | 备注 |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 上下文窗口（全角标点） | １２３ | 混合 mix 文字 |".to_string(),
            "| a你b好c | 456 | 组合字符 e\u{0301} 结束 |".to_string(),
        ];
        for width in 8..=90usize {
            for indent in [0usize, 2, 6] {
                for line in render_table(&rows, indent, width, Style::default()) {
                    let w: usize = line
                        .spans
                        .iter()
                        .map(|s| grid::width(s.content.as_ref()))
                        .sum();
                    assert!(
                        w <= width.max(indent + 8),
                        "表格右溢: width={width} indent={indent} line_w={w} line={line:?}"
                    );
                }
            }
        }
    }

    /// 降级路径：可用宽度连"每列 1 列内容"都放不下时输出纯文本行，不画半个表格。
    #[test]
    fn table_degrades_to_plain_text_when_too_narrow() {
        let rows = vec![
            "| a | b | c | d |".to_string(),
            "| --- | --- | --- | --- |".to_string(),
        ];
        let lines = render_table(&rows, 0, 6, Style::default());
        assert!(!lines.is_empty());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains('┌'), "过窄时应降级为纯文本: {joined:?}");
        assert!(joined.contains('a') && joined.contains('d'));
    }

    #[test]
    fn task_item_detection() {
        assert_eq!(task_item("[ ] 待办"), Some((false, "待办")));
        assert_eq!(task_item("[x] 完成"), Some((true, "完成")));
        assert_eq!(task_item("[X] 大写"), Some((true, "大写")));
        // 缩进体已由 list_item trim，无空格分隔也应识别
        assert_eq!(task_item("[x]无空格"), Some((true, "无空格")));
        assert_eq!(task_item("普通文本"), None);
        assert_eq!(task_item("[y] 非法"), None);
        assert_eq!(task_item("[ 缺右括号"), None);
    }

    #[test]
    fn braille_set_bit_layout() {
        // 2 像素列 × 4 像素行 = 8 个点，逐点校验位序
        let cases: [(usize, usize, u8); 8] = [
            (0, 0, 0x01),
            (0, 1, 0x02),
            (0, 2, 0x04),
            (0, 3, 0x40),
            (1, 0, 0x08),
            (1, 1, 0x10),
            (1, 2, 0x20),
            (1, 3, 0x80),
        ];
        for (px, py, bit) in cases {
            let mut grid = [0u8; 1];
            braille_set(&mut grid, 1, 1, px, py);
            assert_eq!(grid[0], bit, "pixel ({px},{py})");
        }
        // 越界写入静默忽略
        let mut grid = [0u8; 1];
        braille_set(&mut grid, 1, 1, 2, 0);
        braille_set(&mut grid, 1, 1, 0, 4);
        assert_eq!(grid[0], 0);
    }

    #[test]
    fn render_plot_draws_braille_canvas() {
        let rows = vec![
            "title: sin(x)".to_string(),
            "xs: 0,1,2,3,4,5,6".to_string(),
            "ys: 0,1,0,-1,0,1,0".to_string(),
        ];
        let lines = render_plot(&rows, 0, 80, Style::default());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        // 标题行 + 8 行画布 + x 域标注 = 10 行
        assert_eq!(lines.len(), 10);
        assert!(joined.contains("sin(x)"));
        assert!(joined.contains("x: 0 … 6"));
        // 画布行应含 braille 字符（U+2800..=U+28FF）
        let has_braille = lines[1..9].iter().any(|l| {
            l.to_string()
                .chars()
                .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
        });
        assert!(has_braille, "canvas should contain braille dots");
    }

    #[test]
    fn render_plot_degrades_on_malformed_input() {
        let rows = vec!["title: bad".to_string(), "xs: 1".to_string()];
        let lines = render_plot(&rows, 0, 60, Style::default());
        // 数据不足 → 原样降级为代码块文本
        assert_eq!(lines.len(), 2);
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("title: bad"));
        assert!(joined.contains("xs: 1"));
    }

    #[test]
    fn render_plot_null_breaks_line() {
        // ys 含 null：null 断线，渲染不 panic 且正常产出画布
        let rows = vec![
            "title: gapped".to_string(),
            "xs: 0,1,2,3".to_string(),
            "ys: 1,null,2,null".to_string(),
        ];
        let lines = render_plot(&rows, 0, 80, Style::default());
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn render_plot_degenerate_y_domain() {
        // 水平线（y 全相等）→ y 域扩为 [-1,1]，不 panic
        let rows = vec![
            "title: flat".to_string(),
            "xs: 0,1,2,3,4".to_string(),
            "ys: 2,2,2,2,2".to_string(),
        ];
        let lines = render_plot(&rows, 0, 80, Style::default());
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn render_renders_plot_fence() {
        let md = "```plot\ntitle: f(x)=x\nxs: 0,1,2\nys: 0,1,2\n```";
        let lines = render(md, 0, 80, Style::default());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("f(x)=x"));
        assert!(joined.contains("x: 0 … 2"));
    }

    #[test]
    fn render_renders_task_list() {
        let md = "- [ ] 安装依赖\n- [x] 构建核心\n- 普通项";
        let lines = render(md, 0, 60, Style::default());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains('○'), "pending task symbol");
        assert!(joined.contains('●'), "done task symbol");
        assert!(joined.contains("•"), "plain bullet");
        assert!(joined.contains("构建核心"));
    }

    #[test]
    fn render_ignores_plot_like_content_outside_fence() {
        // plot 关键字出现在普通文本中不应触发画板模式
        let md = "plot is a word\ntitle: not a plot\nxs: 1,2\nys: 1,2";
        let lines = render(md, 0, 60, Style::default());
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("not a plot"));
        // 未进画板模式 → 无 braille 画布输出
        assert!(!joined
            .chars()
            .any(|c| ('\u{2801}'..='\u{28FF}').contains(&c) && c != ' '));
    }
}
