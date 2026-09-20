// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 画板块：```plot fence 内 title/xs/ys 数据行 → braille 点阵曲线。
//
// 画布几何由 `width` 参数导出，越界点静默忽略；解析失败/数据不足一律原样
// 降级为代码块文本（绝不丢信息，绝不 panic）。

use crate::theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

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
pub(super) fn render_plot(
    rows: &[String],
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
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
                braille_set(
                    &mut grid,
                    canvas_cols,
                    canvas_rows,
                    cx.max(0) as usize,
                    cy.max(0) as usize,
                );
            }
        }
        prev = cur;
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    if !title.is_empty() {
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent + 2), Style::default()),
            Span::styled(title, base.fg(theme::accent()).add_modifier(Modifier::BOLD)),
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
