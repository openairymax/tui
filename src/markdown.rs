// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端 markdown 渲染器（0.1.18 A 轨 W6：解析器归一 + 能力强化）。
//
// 分层（§5 W6 实现路线裁定）：
//   markdown.rs  —— 本文件。只做解析选项装配与派发，不含任何渲染逻辑。
//   block.rs     —— 块级事件流编排（段落/标题/列表/引用/代码/表格/脚注/分隔线）。
//   inline.rs    —— 行内样式（嵌套强调/链接/删除线/行内代码/图片/裸 URL）。
//   code.rs      —— 代码块语言徽章与 syntect 分词着色。
//   table.rs     —— 表格列宽对齐与超宽截断。
//   math.rs      —— 数学定界符归一与 LaTeX 终端降级。
//   plot.rs      —— ```plot 画板（0.1.17，本版未改动）。
//
// 解析层自 0.1.18 起换用 CommonMark 合规解析器：此前自研的行级扫描对未闭合
// 围栏、嵌套强调、转义、表格对齐四处存在能力盲区，直接造成"不能渲染的画面"。
//
// 宽度纪律（0.1.18 A 轨 W2，§3.3）：显示宽度的测量与截断一律委托 L2 唯一
// 裁决点 `crate::engine::grid`，本模块及其子模块不得自行测量后截断或填充。

mod block;
mod code;
mod inline;
mod math;
mod plot;
mod table;

use pulldown_cmark::{Options, Parser};
use ratatui::{
    style::Style,
    text::Line,
};

#[cfg(test)]
use plot::braille_set;

/// 渲染整段 markdown 内容为终端行序列。
///
/// `indent` 为内容整体左缩进（列数，UTF-8 空格按显示宽度对齐）。
/// `width` 为内容可用宽度。`base` 为普通文本样式（角色/工具消息沿用）。
pub fn render(content: &str, indent: usize, width: usize, base: Style) -> Vec<Line<'static>> {
    let src = math::norm_src(content);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    block::render(Parser::new_ext(&src, opts), indent, width, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::grid;

    fn joined(md: &str, width: usize) -> String {
        render(md, 0, width, Style::default())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
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

    /// 画板块集成：fence → 解析 → 画布（plot 层单测见 markdown/plot.rs）。
    #[test]
    fn render_renders_plot_fence() {
        let out = joined("```plot\ntitle: f(x)=x\nxs: 0,1,2\nys: 0,1,2\n```", 80);
        assert!(out.contains("f(x)=x"));
        assert!(
            out.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
            "应绘制 braille 画布: {out}"
        );
    }

    /// W7 集成：表达式模式经 fence 直达画布，且非法表达式不产出曲线。
    #[test]
    fn render_renders_expression_plot_fence() {
        let ok = joined("```plot\nx: -6.28 .. 6.28\nf: 正弦 = sin(x)\n```", 80);
        assert!(ok.contains("正弦"));
        assert!(ok.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));

        let bad = joined("```plot\nx: 0 .. 1\nf: exec(\"rm -rf /\")\n```", 80);
        assert!(!bad.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
        assert!(bad.contains("exec"), "非法表达式应原样降级: {bad}");
    }

    #[test]
    fn render_ignores_plot_like_content_outside_fence() {
        // plot 关键字出现在普通文本中不应触发画板模式
        let out = joined("plot is a word\ntitle: not a plot\nxs: 1,2\nys: 1,2", 60);
        assert!(out.contains("not a plot"));
        assert!(!out
            .chars()
            .any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)));
    }

    #[test]
    fn render_keeps_plain_text() {
        let lines = render("你好世界", 0, 40, Style::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("你好世界"));
    }

    #[test]
    fn render_fenced_code() {
        let out = joined("```rust\nfn main() {}\n```", 60);
        assert!(out.contains("fn main() {}"));
    }

    #[test]
    fn render_renders_task_list() {
        let out = joined("- [ ] 安装依赖\n- [x] 构建核心\n- 普通项", 60);
        assert!(out.contains('○'), "pending task symbol");
        assert!(out.contains('●'), "done task symbol");
        assert!(out.contains('•'), "plain bullet");
        assert!(out.contains("构建核心"));
    }

    #[test]
    fn render_renders_inline_capabilities() {
        let out = joined("**粗体** 与 *斜体* 与 ~~删除线~~", 60);
        assert!(out.contains("粗体"));
        assert!(out.contains("斜体"));
        assert!(out.contains("删除线"));
        // 标记符本身不得残留
        assert!(!out.contains("**"));
        assert!(!out.contains("~~"));
    }

    #[test]
    fn render_renders_link_with_weak_url() {
        let out = joined("[文档](https://example.com/doc)", 80);
        assert!(out.contains("文档"));
        assert!(out.contains("https://example.com/doc"));
    }

    #[test]
    fn render_normalizes_paren_delimiters() {
        let out = joined("公式 \\(a+b\\) 结束", 60);
        assert!(out.contains("a+b"));
        assert!(!out.contains("\\("), "反斜杠定界符不得透出: {out}");
    }

    #[test]
    fn render_degrades_latex_without_backslash_leak() {
        let cases = [
            "\\[\\frac{a}{b}\\]",
            "$$\\sum_{i=1}^{n} x_i$$",
            "$\\alpha + \\sqrt{y}$",
            "\\(\\log x \\leq 1\\)",
        ];
        for case in cases {
            let out = joined(case, 80);
            assert!(!out.contains('\\'), "LaTeX 反斜杠命令透出: {case} -> {out}");
        }
        assert!(joined("\\[\\frac{a}{b}\\]", 80).contains("a/b"));
        assert!(joined("$x^2$", 80).contains("x²"));
    }

    #[test]
    fn render_table_with_alignment() {
        let md = "| 左 | 中 | 右 |\n|:---|:---:|---:|\n| a | b | c |";
        let out = joined(md, 60);
        assert!(out.contains('│'));
        assert!(out.contains('a') && out.contains('b') && out.contains('c'));
    }

    /// R4 验收（§5 W2）：混合块型 + CJK + 全角标点，宽度全边界扫描右边界零溢出。
    #[test]
    fn render_never_overflows_right_edge() {
        let md = "## 标题（全角）\n\n段落 e\u{0301} 混合 mix 文字，含[链接](https://example.com)。\n\n- 列表项（中文）\n  - 嵌套项\n\n> 引用文字\n\n| 名称 | 值 |\n|:---|---:|\n| 上下文窗口 | １２３ |\n\n```rust\nlet s = \"字符串\";\n```\n\n---\n";
        for width in 8..=100usize {
            for indent in [0usize, 2, 6] {
                for line in render(md, indent, width, Style::default()) {
                    let w = grid::line_w(&line.spans);
                    assert!(
                        w <= width.max(indent + 8),
                        "右溢: width={width} indent={indent} line_w={w} line={line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn render_handles_empty_and_whitespace() {
        assert!(render("", 0, 40, Style::default()).is_empty());
        assert!(render("\n\n\n", 0, 40, Style::default()).is_empty());
    }
}
