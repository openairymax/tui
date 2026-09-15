// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 轻量终端 markdown 渲染器（P2-A 修复：表格/标题/列表/引用/行内样式）。
//
// 设计原则（50 工程标准 A-1 极简主义）：自研零依赖，不引入重型 markdown
// crate——terminal 渲染需要显示宽度对齐（中文全角），通用库难以满足。
// 支持：
//   - 代码块（``` / ```lang，含语言徽章）
//   - 画板块（```plot：title/xs/ys → braille 点阵函数曲线，0.1.17）
//   - 表格（| a | b |，含分隔行对齐，P2-A 核心）
//   - 标题（# ~ ######）
//   - 列表（- / * / + / 1.，支持嵌套缩进；任务列表 [ ]/[x]，0.1.17）
//   - 引用（> 行，左边框线）
//   - 行内样式：**粗体** / `行内代码`
// 其余内容降级为纯文本（绝不出错，绝不截断语义）。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme;

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
        let mut pieces = wrap_line(&text, content_width);
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
            for piece in wrap_line(raw, width.saturating_sub(1).max(8)) {
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
            for piece in wrap_line(text, width.saturating_sub(indent).max(8)) {
                out.push(Line::from(vec![
                    Span::styled(" ".repeat(indent), Style::default()),
                    Span::styled(
                        piece,
                        base.fg(color).add_modifier(Modifier::BOLD),
                    ),
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
                let body_indent = indent + mark.width() + 2;
                for piece in wrap_line(rest, width.saturating_sub(body_indent).max(8)) {
                    out.push(Line::from(vec![
                        Span::styled(
                            format!("{0:width$}{1} ", "", mark, width = indent),
                            Style::default(),
                        ),
                        Span::styled(format!("{sym} "), sym_style),
                        Span::styled(piece, body_style),
                    ]));
                }
                continue;
            }
            // 列表正文的缩进 = 整体缩进 + 符号宽度（"• " 与 "12. " 对齐）
            let body_indent = indent + mark.width() + 2;
            let content_width = width.saturating_sub(body_indent).max(8);
            let lead = Span::styled(
                format!("{0:width$}{1} ", "", mark, width = indent),
                base.fg(theme::dim()),
            );
            for piece in wrap_line(body, content_width) {
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
            for piece in wrap_line(body, width.saturating_sub(indent + 2).max(8)) {
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
    let mark = if trimmed.len() != s.len() { "·" } else { "•" };
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
            if i < bytes.len()
                && bytes[i] == b'.'
                && i + 1 < bytes.len()
                && bytes[i + 1] == b' '
            {
                Some((
                    format!("{}.", &trimmed[..i]),
                    trimmed[i + 2..].trim(),
                ))
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

/// braille 点阵写入：一个 braille 字符 = 2 像素列 × 4 像素行，点位按
/// Unicode braille 标准位序（U+2800 基址）。
fn braille_set(grid: &mut [u8], cols: usize, rows: usize, px: usize, py: usize) {
    let cx = px / 2;
    let cy = py / 4;
    if cx >= cols || cy >= rows {
        return;
    }
    let bit = match (px % 2, py % 4) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        _ => 0x80,
    };
    grid[cy * cols + cx] |= bit;
}

/// 整数 Bresenham 连线（相邻采样点之间补插值，保证曲线连续）。
fn braille_line(grid: &mut [u8], cols: usize, rows: usize, x0: i64, y0: i64, x1: i64, y1: i64) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x1 >= x0 { 1 } else { -1 };
    let sy = if y1 >= y0 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        if x >= 0 && y >= 0 {
            braille_set(grid, cols, rows, x as usize, y as usize);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// 渲染 ```plot 画板块：解析 tool 输出（title/xs/ys），以 braille 点阵
/// 绘制 y=f(x) 曲线（null 断线）。解析失败降级为代码块原样输出（绝不
/// 出错，绝不截断语义）。
fn render_plot(rows: &[String], indent: usize, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut title = String::new();
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<Option<f64>> = Vec::new();
    for line in rows {
        if let Some(v) = line.strip_prefix("title:") {
            title = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("xs:") {
            xs = v
                .split(',')
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .collect();
        } else if let Some(v) = line.strip_prefix("ys:") {
            ys = v.split(',').map(|t| t.trim().parse::<f64>().ok()).collect();
        }
    }
    // 解析失败/数据不足 → 降级为代码块原样（保持信息不丢失）
    if xs.len() < 2 || ys.len() != xs.len() {
        let mut out: Vec<Line<'static>> = Vec::new();
        for line in rows {
            out.push(Line::from(vec![
                Span::styled(" ".repeat(indent + 1), Style::default()),
                Span::styled(line.clone(), base.bg(theme::surface())),
            ]));
        }
        return out;
    }

    // 画布尺寸：8 行 braille（32 像素行）× 可用宽度（braille 每字符 2 像素列）
    let canvas_rows = 8usize;
    let avail = width.saturating_sub(indent + 2).max(16);
    let canvas_cols = avail.min(60);
    let pw = canvas_cols * 2;
    let ph = canvas_rows * 4;
    let mut grid = vec![0u8; canvas_rows * canvas_cols];

    // y 域：忽略 null 与非有限值；退化区间（水平线）扩为 [-1,1]
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for v in ys.iter().flatten() {
        if v.is_finite() {
            y_min = y_min.min(*v);
            y_max = y_max.max(*v);
        }
    }
    if !y_min.is_finite() || !y_max.is_finite() {
        y_min = -1.0;
        y_max = 1.0;
    }
    if (y_max - y_min).abs() < 1e-12 {
        y_min -= 1.0;
        y_max += 1.0;
    }

    // 相邻有效采样点连线（null 断线）
    let n = xs.len();
    let to_px = |i: usize, y: f64| -> (i64, i64) {
        let fx = i as f64 * (pw - 1) as f64 / (n - 1) as f64;
        let fy = (y_max - y) / (y_max - y_min) * (ph - 1) as f64;
        (fx.round() as i64, fy.round() as i64)
    };
    let mut prev: Option<(i64, i64)> = None;
    for (i, y) in ys.iter().enumerate().take(n) {
        let cur = y.filter(|v| v.is_finite()).map(|v| to_px(i, v));
        if let Some((cx, cy)) = cur {
            if let Some((px0, py0)) = prev {
                braille_line(&mut grid, canvas_cols, canvas_rows, px0, py0, cx, cy);
            } else {
                braille_set(&mut grid, canvas_cols, canvas_rows, cx.max(0) as usize, cy.max(0) as usize);
            }
        }
        prev = cur;
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    if !title.is_empty() {
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent + 2), Style::default()),
            Span::styled(
                title,
                base.fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    for r in 0..canvas_rows {
        let s: String = grid[r * canvas_cols..(r + 1) * canvas_cols]
            .iter()
            .map(|b| char::from_u32(0x2800 + *b as u32).unwrap_or(' '))
            .collect();
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent + 2), Style::default()),
            Span::styled(s, base.fg(theme::primary())),
        ]));
    }
    // x 轴域标注（dim 色，提示可读的采样区间）
    out.push(Line::from(vec![
        Span::styled(" ".repeat(indent + 2), Style::default()),
        Span::styled(
            format!("x: {} … {}", xs[0], xs[n - 1]),
            base.fg(theme::dim()),
        ),
    ]));
    out
}

/// 行内样式：**粗体** / `行内代码` / [链接](url) / ~~删除线~~（其余原样）。
fn inline_styles(s: &str, base: Style) -> Line<'static> {
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
                spans.push(Span::styled(
                    bold,
                    base.add_modifier(Modifier::BOLD),
                ));
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

/// 尝试在 `chars[start] == '['` 处解析 `[text](url)`；成功返回 (text, url)
/// 与 text/url 长度（供调用方跳过）。失败返回 None（按普通字符处理）。
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

/// 渲染表格块：表头 + 分隔行 + 数据行，按列宽对齐（中文全角按 2 列计）。
fn render_table(rows: &[String], indent: usize, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    // 解析单元格（去掉首尾 |，按 | 分割）
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let trimmed = r.trim();
            let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
            inner
                .split('|')
                .map(|c| c.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    let cols = parsed.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return out;
    }
    // 列宽 = 各列最大显示宽度（表头/数据取最大；分隔行不参与）
    let mut col_w = vec![0usize; cols];
    for (ri, row) in parsed.iter().enumerate() {
        if is_separator_row(rows.get(ri).map(|s| s.as_str()).unwrap_or("")) {
            continue;
        }
        for (ci, cell) in row.iter().enumerate() {
            col_w[ci] = col_w[ci].max(cell.width());
        }
    }
    // 表格总宽超出可用宽度时等比收缩（极端窄屏容错）
    let gap = 1usize;
    let total = col_w.iter().map(|w| w + 2 * gap).sum::<usize>() + 1;
    let avail = width.saturating_sub(indent).max(8);
    if total > avail {
        shrink_cols(&mut col_w, total, avail);
    }

    for (ri, row) in parsed.iter().enumerate() {
        let is_sep = is_separator_row(rows.get(ri).map(|s| s.as_str()).unwrap_or(""));
        // 分隔行 → 水平线
        if is_sep {
            let line: String = col_w
                .iter()
                .map(|w| "─".repeat(w + 2 * gap))
                .collect::<Vec<_>>()
                .join("┼");
            out.push(Line::from(vec![
                Span::styled(" ".repeat(indent), Style::default()),
                Span::styled(format!("┌{line}┐"), base.fg(theme::dim())),
            ]));
            continue;
        }
        let mut spans = vec![Span::styled(" ".repeat(indent), Style::default())];
        let is_header = ri == 0 && !is_sep;
        for (ci, cw) in col_w.iter().enumerate() {
            let cell = row.get(ci).cloned().unwrap_or_default();
            let pad = cw.saturating_sub(cell.width());
            // 表头加粗 + 主色；数据行常规
            let style = if is_header {
                base.fg(theme::text()).add_modifier(Modifier::BOLD)
            } else {
                base.fg(theme::text())
            };
            spans.push(Span::styled(
                format!("│ {cell}{} ", " ".repeat(pad)),
                style,
            ));
        }
        spans.push(Span::styled("│", base.fg(theme::dim())));
        out.push(Line::from(spans));
    }
    out
}

/// 判断表格分隔行（如 |---|---|）。
fn is_separator_row(s: &str) -> bool {
    let trimmed = s.trim().trim_start_matches('|').trim_end_matches('|');
    if trimmed.is_empty() {
        return false;
    }
    trimmed.split('|').all(|c| {
        let t = c.trim();
        !t.is_empty()
            && t.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ' || ch == '=')
            && t.contains('-')
    })
}

/// 表格总宽超出可用宽度时，按比例收缩各列（极端窄屏容错，不截断单元格）。
fn shrink_cols(col_w: &mut [usize], total: usize, avail: usize) {
    let shrink = total.saturating_sub(avail);
    if shrink == 0 || col_w.is_empty() {
        return;
    }
    // 每列至少保留 1 列宽，其余按比例缩减
    let mut remain = shrink;
    let mut i = 0;
    while remain > 0 {
        if col_w[i] > 1 {
            col_w[i] -= 1;
            remain -= 1;
        }
        i = (i + 1) % col_w.len();
        if col_w.iter().all(|w| *w <= 1) {
            break;
        }
    }
}

/// 按显示宽度硬截断换行（中文等宽字符按 2 列计）。
pub fn wrap_line(s: &str, max_width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if s.is_empty() {
        return out;
    }
    if max_width < 2 {
        out.push(s.to_string());
        return out;
    }
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        // 单字符即超宽：先换行再放（避免死循环）
        if cur_w + w > max_width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
        let joined: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("fn main() {}"));
    }

    #[test]
    fn wrap_line_short_text_single_line() {
        assert_eq!(wrap_line("你好", 10), vec!["你好"]);
        assert_eq!(wrap_line("abc", 10), vec!["abc"]);
    }

    #[test]
    fn wrap_line_splits_by_display_width() {
        // 中文按 2 列计：宽度 5 时 "你好世" = 6 列超宽 → 拆行
        assert_eq!(wrap_line("你好世界", 5), vec!["你好", "世界"]);
        // 半角按 1 列计
        assert_eq!(wrap_line("abcdef", 3), vec!["abc", "def"]);
    }

    #[test]
    fn wrap_line_empty_and_narrow() {
        assert!(wrap_line("", 10).is_empty());
        // 极窄宽度兜底：整行返回，不产生空片段
        assert_eq!(wrap_line("abc", 1), vec!["abc"]);
    }

    #[test]
    fn wrap_line_mixed_widths() {
        // "a你好b" = 1+2+2+1 = 6 列，宽度 4 → "a你"（3 列）+ "好b"（3 列）
        assert_eq!(wrap_line("a你好b", 4), vec!["a你", "好b"]);
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
        assert!(
            !joined
                .chars()
                .any(|c| ('\u{2801}'..='\u{28FF}').contains(&c) && c != ' ')
        );
    }
}
