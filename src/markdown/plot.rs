// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 画板块（0.1.18 W7）：```plot fence → braille 点阵图表。
//
// 支持两种数据来源，同一画布上渲染：
//   表达式模式：`f: [名称 =] <expr>`（可多条）+ `x: <lo> .. <hi>` 采样区间；
//              求值走受限 PRN 解释器（plot/eval.rs），非任意代码执行。
//   数据模式：  `xs: <csv>` + `ys: <csv>`（`ys` 可重复，构成多序列）。
//
// 视觉要素：y 刻度标注与网格、x 刻度标注、零点轴、负值区背景区分、
// 多曲线图例。刻度步长取自 1/2/5×10ⁿ（plot/axis.rs）。
//
// 采样密度 = 画布像素宽（终端可用宽度 × 2），防混叠；y 域用固定探针数
// 估计，与画布几何解耦，避免"域依赖几何、几何依赖域"的循环。
//
// 解析失败/数据不足一律原样降级为代码块文本（绝不丢信息，绝不 panic）。

mod axis;
mod eval;

use crate::engine::grid;
use crate::theme;
use eval::{compile, eval, Tok};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// 画布高度（braille 字符行；每行 4 像素行）。
const CANVAS_ROWS: usize = 8;
/// 画布宽度下界（再窄则无法容纳刻度与曲线）。
const MIN_COLS: usize = 8;
/// 画布宽度上界（超宽画布在终端中难以一眼读完，且徒增采样成本）。
const MAX_COLS: usize = 60;
/// 可用宽度下界：再窄则刻度沟槽与画布无法同时容纳，直接降级为代码块文本。
const MIN_AVAIL: usize = 12;
/// y 域估计探针数（仅用于量程，不参与绘制）。
const PROBES: usize = 256;

/// 一条序列：名称（空则不进图例）+ 采样值（`None` 为断点）。
type Series = (String, Vec<Option<f64>>);

/// fence 内容解析结果。
struct Spec {
    title: String,
    xlabel: String,
    ylabel: String,
    lo: f64,
    hi: f64,
    fns: Vec<(String, Vec<Tok>)>,
    xs: Vec<f64>,
    data: Vec<Series>,
}

/// 已定型的图表：几何、像素层与标注。
struct Chart {
    title: String,
    xlabel: String,
    ylabel: String,
    legend: Vec<(String, Color)>,
    ylab: Vec<String>,
    gutter: usize,
    cols: usize,
    avail: usize,
    canvas: Canvas,
    mark: Vec<char>,
    neg: Vec<bool>,
    xlab: Vec<(usize, String)>,
}

/// braille 点阵写入：一个 braille 字符 = 2 像素列 × 4 像素行，点位按
/// Unicode braille 标准位序（U+2800 基址）。
pub(super) fn braille_set(grid: &mut [u8], cols: usize, rows: usize, px: usize, py: usize) {
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

/// 点阵画布：位图与序列归属两层同位，保证逐序列着色与图例一一对应。
struct Canvas {
    cols: usize,
    rows: usize,
    dots: Vec<u8>,
    owner: Vec<u8>,
}

impl Canvas {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            dots: vec![0u8; cols * rows],
            owner: vec![0u8; cols * rows],
        }
    }

    /// 写入一个点位并登记归属序列。
    fn dot(&mut self, px: usize, py: usize, o: u8) {
        braille_set(&mut self.dots, self.cols, self.rows, px, py);
        let (cx, cy) = (px / 2, py / 4);
        if cx < self.cols && cy < self.rows {
            self.owner[cy * self.cols + cx] = o;
        }
    }

    /// 整数 Bresenham 连线（相邻采样点之间补插值，保证曲线连续）。
    fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, o: u8) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x1 >= x0 { 1 } else { -1 };
        let sy = if y1 >= y0 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            if x >= 0 && y >= 0 {
                self.dot(x as usize, y as usize, o);
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
}

/// 渲染 ```plot 画板块。解析失败降级为代码块原样输出。
pub(super) fn render_plot(
    rows: &[String],
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let spec = parse_spec(rows);
    match prepare(&spec, indent, width) {
        Some(chart) => paint(&chart, indent, base),
        None => degrade(rows, indent, width, base),
    }
}

fn parse_spec(rows: &[String]) -> Spec {
    let mut spec = Spec {
        title: String::new(),
        xlabel: String::new(),
        ylabel: String::new(),
        lo: -10.0,
        hi: 10.0,
        fns: Vec::new(),
        xs: Vec::new(),
        data: Vec::new(),
    };
    for line in rows {
        if let Some(v) = line.strip_prefix("title:") {
            spec.title = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("xlabel:") {
            spec.xlabel = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("ylabel:") {
            spec.ylabel = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("x:") {
            if let Some((a, b)) = v.split_once("..") {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                    if a.is_finite() && b.is_finite() && b > a {
                        spec.lo = a;
                        spec.hi = b;
                    }
                }
            }
        } else if let Some(v) = line.strip_prefix("f:") {
            let body = v.trim();
            let (name, expr) = match body.split_once('=') {
                Some((n, e)) => (n.trim().to_string(), e.trim()),
                None => (String::new(), body),
            };
            if let Ok(prn) = compile(expr) {
                spec.fns.push((name, prn));
            }
        } else if let Some(v) = line.strip_prefix("xs:") {
            spec.xs = v
                .split(',')
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .collect();
        } else if let Some(v) = line.strip_prefix("ys:") {
            let body = v.trim();
            let (name, csv) = match body.split_once('=') {
                Some((n, e)) => (n.trim().to_string(), e.trim()),
                None => (String::new(), body),
            };
            let ys: Vec<Option<f64>> = csv
                .split(',')
                .map(|t| t.trim().parse::<f64>().ok())
                .collect();
            spec.data.push((name, ys));
        }
    }
    spec
}

fn prepare(spec: &Spec, indent: usize, width: usize) -> Option<Chart> {
    let avail = width.saturating_sub(indent + 2);
    if avail < MIN_AVAIL {
        return None;
    }
    let (xmin, xmax) = x_domain(spec)?;
    let (ymin, ymax) = y_domain(spec, xmin, xmax)?;
    let span_x = xmax - xmin;
    let span_y = ymax - ymin;
    if !span_x.is_finite() || span_x <= 0.0 || !span_y.is_finite() || span_y <= 0.0 {
        return None;
    }

    let yticks = axis::ticks(ymin, ymax, CANVAS_ROWS / 2);
    let (ylab, gutter) = y_labels(&yticks, ymin, ymax);
    let cols = avail.saturating_sub(gutter + 1).clamp(MIN_COLS, MAX_COLS);
    // 宽度不变量：行宽 = indent + 2 + gutter + 1 + cols ≤ width。
    // 刻度沟槽 + 画布放不下时降级，绝不画半张图。
    if cols + gutter + 1 > avail {
        return None;
    }

    // 采样密度 = 画布像素宽（终端可用宽度 × 2）
    let (xs, series) = samples(spec, xmin, xmax, cols * 2)?;
    if xs.len() < 2 || series.is_empty() {
        return None;
    }

    let pw = cols * 2;
    let ph = CANVAS_ROWS * 4;
    let mut canvas = Canvas::new(cols, CANVAS_ROWS);
    let mut mark = vec![' '; CANVAS_ROWS * cols];

    let x_px = |x: f64| -> i64 {
        (((x - xmin) / span_x * (pw - 1) as f64).round() as i64).clamp(0, pw as i64 - 1)
    };
    let y_px = |y: f64| -> i64 {
        (((ymax - y) / span_y * (ph - 1) as f64).round() as i64).clamp(0, ph as i64 - 1)
    };

    // 网格线（y 刻度）→ 零点轴压在其上
    for t in &yticks {
        let r = y_px(*t) as usize / 4;
        if r < CANVAS_ROWS {
            for c in 0..cols {
                mark[r * cols + c] = '·';
            }
        }
    }
    let straddle_y = ymin < 0.0 && ymax > 0.0;
    if straddle_y {
        let r = y_px(0.0) as usize / 4;
        if r < CANVAS_ROWS {
            for c in 0..cols {
                mark[r * cols + c] = '─';
            }
        }
        if xmin < 0.0 && xmax > 0.0 {
            let c = x_px(0.0) as usize / 2;
            if c < cols {
                for r in 0..CANVAS_ROWS {
                    if mark[r * cols + c] == ' ' {
                        mark[r * cols + c] = '│';
                    }
                }
            }
        }
    }

    // 曲线（null / 非有限值断线）
    for (si, (_, ys)) in series.iter().enumerate() {
        let o = (si + 1).min(u8::MAX as usize) as u8;
        let mut prev: Option<(i64, i64)> = None;
        for (i, y) in ys.iter().enumerate() {
            let cur = y
                .filter(|v| v.is_finite())
                .map(|v| (x_px(xs[i]), y_px(v)));
            match (prev, cur) {
                (Some((ax, ay)), Some((bx, by))) => canvas.line(ax, ay, bx, by, o),
                (None, Some((bx, by))) => canvas.dot(bx as usize, by as usize, o),
                _ => {}
            }
            prev = cur;
        }
    }

    // 负值区背景区分（以单元格中心的数据 y 为准）
    let mut neg = vec![false; CANVAS_ROWS * cols];
    if straddle_y {
        for r in 0..CANVAS_ROWS {
            let py = (r * 4 + 2) as f64;
            let y = ymax - py / (ph - 1) as f64 * span_y;
            let below = y < 0.0;
            for c in 0..cols {
                neg[r * cols + c] = below;
            }
        }
    }

    // x 刻度标注
    let target = (cols / 12).max(1) + 1;
    let mut xlab: Vec<(usize, String)> = Vec::new();
    for t in axis::ticks(xmin, xmax, target) {
        let c = (x_px(t) as usize / 2).min(cols.saturating_sub(1));
        xlab.push((c, axis::fmt(t)));
    }

    let legend: Vec<(String, Color)> = series
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| !n.is_empty())
        .map(|(i, (n, _))| (n.clone(), palette(i)))
        .collect();

    Some(Chart {
        title: spec.title.clone(),
        xlabel: spec.xlabel.clone(),
        ylabel: spec.ylabel.clone(),
        legend,
        ylab,
        gutter,
        cols,
        avail,
        canvas,
        mark,
        neg,
        xlab,
    })
}

fn x_domain(spec: &Spec) -> Option<(f64, f64)> {
    if !spec.data.is_empty() {
        let a = *spec.xs.first()?;
        let b = *spec.xs.last()?;
        if a.is_finite() && b.is_finite() && b > a {
            return Some((a, b));
        }
        return None;
    }
    if spec.fns.is_empty() {
        return None;
    }
    Some((spec.lo, spec.hi))
}

fn y_domain(spec: &Spec, xmin: f64, xmax: f64) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;

    if !spec.data.is_empty() {
        for (_, ys) in &spec.data {
            for v in ys.iter().flatten() {
                if v.is_finite() {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
    } else {
        for x in linspace(xmin, xmax, PROBES) {
            for (_, prn) in &spec.fns {
                if let Ok(v) = eval(prn, x) {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
    }

    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    if (hi - lo).abs() < 1e-12 {
        lo -= 1.0;
        hi += 1.0;
    }
    Some((lo, hi))
}

fn samples(spec: &Spec, xmin: f64, xmax: f64, n: usize) -> Option<(Vec<f64>, Vec<Series>)> {
    if !spec.data.is_empty() {
        let out: Vec<Series> = spec
            .data
            .iter()
            .filter(|(_, ys)| ys.len() == spec.xs.len())
            .cloned()
            .collect();
        if out.is_empty() {
            return None;
        }
        return Some((spec.xs.clone(), out));
    }
    if spec.fns.is_empty() {
        return None;
    }
    let xs = linspace(xmin, xmax, n.max(2));
    let mut out: Vec<Series> = Vec::new();
    for (name, prn) in &spec.fns {
        let ys: Vec<Option<f64>> = xs.iter().map(|x| eval(prn, *x).ok()).collect();
        if ys.iter().any(|v| v.is_some()) {
            out.push((name.clone(), ys));
        }
    }
    if out.is_empty() {
        return None;
    }
    Some((xs, out))
}

fn linspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    if n < 2 {
        return vec![a];
    }
    (0..n)
        .map(|i| a + (b - a) * i as f64 / (n - 1) as f64)
        .collect()
}

/// y 刻度标注：每个刻度落到其像素行对应的字符行，同行冲突时保留先到者。
fn y_labels(yticks: &[f64], ymin: f64, ymax: f64) -> (Vec<String>, usize) {
    let mut lab = vec![String::new(); CANVAS_ROWS];
    let span = ymax - ymin;
    if span > 0.0 {
        for t in yticks {
            let py = ((ymax - t) / span * (CANVAS_ROWS * 4 - 1) as f64).round();
            if !py.is_finite() {
                continue;
            }
            let r = (py as isize).clamp(0, CANVAS_ROWS as isize - 1) as usize;
            if lab[r].is_empty() {
                lab[r] = axis::fmt(*t);
            }
        }
    }
    let gutter = lab.iter().map(|s| grid::width(s)).max().unwrap_or(0);
    (lab, gutter)
}

fn paint(chart: &Chart, indent: usize, base: Style) -> Vec<Line<'static>> {
    let lead = " ".repeat(indent + 2);
    let mut out: Vec<Line<'static>> = Vec::new();

    if !chart.title.is_empty() {
        out.push(Line::from(vec![
            Span::styled(lead.clone(), Style::default()),
            Span::styled(
                grid::clip(&chart.title, chart.avail),
                base.fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !chart.ylabel.is_empty() {
        out.push(Line::from(vec![
            Span::styled(lead.clone(), Style::default()),
            Span::styled(
                grid::clip(&chart.ylabel, chart.avail),
                base.fg(theme::dim()).add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    if !chart.legend.is_empty() {
        let mut content: Vec<Span<'static>> = Vec::new();
        for (i, (name, color)) in chart.legend.iter().enumerate() {
            if i > 0 {
                content.push(Span::styled("  ".to_string(), Style::default()));
            }
            content.push(Span::styled("── ".to_string(), base.fg(*color)));
            content.push(Span::styled(name.clone(), base.fg(theme::dim())));
        }
        let mut spans = vec![Span::styled(lead.clone(), Style::default())];
        spans.extend(grid::clip_spans(&content, chart.avail));
        out.push(Line::from(spans));
    }

    for r in 0..CANVAS_ROWS {
        let mut spans = vec![
            Span::styled(lead.clone(), Style::default()),
            Span::styled(
                format!("{:>w$} ", chart.ylab[r], w = chart.gutter),
                base.fg(theme::dim()),
            ),
        ];
        let mut run = String::new();
        let mut cur: Option<(Color, Option<Color>)> = None;
        for c in 0..chart.cols {
            let i = r * chart.cols + c;
            let (glyph, fg) = glyph_at(chart, i);
            let bg = if chart.neg[i] {
                Some(theme::surface_2())
            } else {
                None
            };
            if cur != Some((fg, bg)) {
                if !run.is_empty() {
                    if let Some((f, b)) = cur {
                        spans.push(run_span(&run, f, b, base));
                    }
                }
                run.clear();
                cur = Some((fg, bg));
            }
            run.push(glyph);
        }
        if !run.is_empty() {
            if let Some((f, b)) = cur {
                spans.push(run_span(&run, f, b, base));
            }
        }
        out.push(Line::from(spans));
    }

    out.push(Line::from(vec![
        Span::styled(lead.clone(), Style::default()),
        Span::styled(format!("{:>w$} ", "", w = chart.gutter), Style::default()),
        Span::styled(x_row(chart), base.fg(theme::dim())),
    ]));

    if !chart.xlabel.is_empty() {
        out.push(Line::from(vec![
            Span::styled(lead, Style::default()),
            Span::styled(
                grid::clip(&chart.xlabel, chart.avail),
                base.fg(theme::dim()).add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    out
}

/// x 刻度行：标签居中于其列位，重叠者丢弃后到者（宁可少标，不可糊成一片）。
fn x_row(chart: &Chart) -> String {
    let mut row = vec![' '; chart.cols];
    let mut last = 0usize;
    for (c, s) in &chart.xlab {
        let n = s.chars().count();
        let start = c.saturating_sub(n / 2);
        if start < last {
            continue;
        }
        for (k, gc) in s.chars().enumerate() {
            if start + k < chart.cols {
                row[start + k] = gc;
            }
        }
        last = start + n + 1;
    }
    row.into_iter().collect()
}

fn glyph_at(chart: &Chart, i: usize) -> (char, Color) {
    let bits = chart.canvas.dots[i];
    if bits != 0 {
        let o = chart.canvas.owner[i].saturating_sub(1) as usize;
        let glyph = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
        return (glyph, palette(o));
    }
    match chart.mark[i] {
        '─' | '│' => (chart.mark[i], theme::dim()),
        '·' => ('·', theme::faint()),
        _ => (' ', theme::text()),
    }
}

fn run_span(s: &str, fg: Color, bg: Option<Color>, base: Style) -> Span<'static> {
    let mut st = base.fg(fg);
    if let Some(b) = bg {
        st = st.bg(b);
    }
    Span::styled(s.to_string(), st)
}

/// 序列调色板（与图例一一对应；颜色取自本仓设计令牌，无第二套调色板）。
fn palette(i: usize) -> Color {
    match i % 6 {
        0 => theme::primary(),
        1 => theme::accent(),
        2 => theme::success(),
        3 => theme::magenta(),
        4 => theme::cyan(),
        _ => theme::warning(),
    }
}

/// 降级：原样回吐 fence 内容（折行而非截断，语义零丢失），仍受宽度不变量约束。
fn degrade(rows: &[String], indent: usize, width: usize, base: Style) -> Vec<Line<'static>> {
    let avail = width.saturating_sub(indent + 1);
    let lead = " ".repeat(indent + 1);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in rows {
        let pieces = grid::wrap(line, avail);
        if pieces.is_empty() {
            out.push(Line::from(vec![Span::styled(lead.clone(), Style::default())]));
            continue;
        }
        for piece in pieces {
            out.push(Line::from(vec![
                Span::styled(lead.clone(), Style::default()),
                Span::styled(piece, base.bg(theme::surface())),
            ]));
        }
    }
    out
}

#[cfg(test)]
mod tests;
