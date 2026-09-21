// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// plot 层单元测试（0.1.18 W7）。与实现分离以维持 800 行纪律；
// 覆盖 A8 判据：表达式绘图、多曲线、坐标轴、负值区、宽度不变量。

use super::*;

fn rows(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn joined(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_braille(s: &str) -> bool {
    s.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
}

fn shaded(lines: &[Line<'static>]) -> bool {
    lines
        .iter()
        .any(|l| l.spans.iter().any(|sp| sp.style.bg == Some(theme::surface_2())))
}

#[test]
fn expression_mode_draws_curve() {
    let out = render_plot(
        &rows(&["title: 正弦", "x: -6.28 .. 6.28", "f: sin(x)"]),
        0,
        80,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("正弦"));
    assert!(has_braille(&s), "应绘制 braille 曲线: {s}");
}

#[test]
fn multi_series_produces_legend() {
    let out = render_plot(
        &rows(&["title: 对比", "x: -3 .. 3", "f: 线性 = x", "f: 平方 = x^2"]),
        0,
        80,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("线性") && s.contains("平方"), "图例缺失: {s}");
}

#[test]
fn unnamed_series_has_no_legend() {
    let out = render_plot(&rows(&["x: 0 .. 1", "f: x"]), 0, 60, Style::default());
    assert!(!joined(&out).contains("── "));
}

#[test]
fn axis_ticks_and_labels_are_rendered() {
    let out = render_plot(
        &rows(&["xlabel: 时间", "ylabel: 幅值", "x: 0 .. 10", "f: sin(x)"]),
        0,
        80,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("时间") && s.contains("幅值"));
    assert!(s.contains('·'), "应有网格线: {s}");
}

#[test]
fn zero_axis_appears_when_range_straddles_zero() {
    let out = render_plot(&rows(&["x: -2 .. 2", "f: x"]), 0, 80, Style::default());
    let s = joined(&out);
    assert!(s.contains('─'), "跨零量程应绘制零点轴: {s}");
    assert!(s.contains('│'), "跨零量程应绘制纵轴: {s}");
}

#[test]
fn negative_region_is_shaded() {
    let out = render_plot(&rows(&["x: -2 .. 2", "f: x"]), 0, 80, Style::default());
    assert!(shaded(&out), "负值区应有背景区分");
}

#[test]
fn positive_only_range_has_no_shading() {
    let out = render_plot(&rows(&["x: 1 .. 3", "f: x^2"]), 0, 80, Style::default());
    assert!(!shaded(&out), "全正量程不应有负值区背景");
}

#[test]
fn data_mode_still_works() {
    let out = render_plot(
        &rows(&["title: 采样", "xs: 0,1,2,3,4,5,6", "ys: 0,1,0,-1,0,1,0"]),
        0,
        80,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("采样"));
    assert!(has_braille(&s));
    // 标题 + 8 画布行 + x 刻度行
    assert_eq!(out.len(), 10);
}

#[test]
fn named_data_series_enters_legend() {
    let out = render_plot(
        &rows(&["xs: 0,1,2", "ys: 上界 = 1,2,3", "ys: 下界 = 3,2,1"]),
        0,
        80,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("上界") && s.contains("下界"), "图例缺失: {s}");
}

#[test]
fn null_breaks_the_line_without_panic() {
    let out = render_plot(
        &rows(&["xs: 0,1,2,3", "ys: 1,null,2,null"]),
        0,
        80,
        Style::default(),
    );
    // 无标题 → 8 画布行 + x 刻度行
    assert_eq!(out.len(), 9);
    assert!(has_braille(&joined(&out)));
}

#[test]
fn degenerate_y_range_is_expanded() {
    let out = render_plot(
        &rows(&["xs: 0,1,2,3,4", "ys: 2,2,2,2,2"]),
        0,
        80,
        Style::default(),
    );
    assert_eq!(out.len(), 9);
}

#[test]
fn malformed_input_degrades_verbatim() {
    let src = rows(&["title: bad", "xs: 1"]);
    let out = render_plot(&src, 0, 60, Style::default());
    assert_eq!(out.len(), 2);
    let s = joined(&out);
    assert!(s.contains("title: bad") && s.contains("xs: 1"));
}

#[test]
fn unknown_function_degrades_instead_of_panicking() {
    let out = render_plot(
        &rows(&["x: 0 .. 1", "f: system(\"rm -rf /\")"]),
        0,
        60,
        Style::default(),
    );
    let s = joined(&out);
    assert!(s.contains("system"), "非法表达式应原样降级: {s}");
}

#[test]
fn expression_out_of_domain_yields_gaps_not_failure() {
    // sqrt(x) 在 x<0 无定义：左半段应断线，右半段仍绘制
    let out = render_plot(&rows(&["x: -2 .. 2", "f: sqrt(x)"]), 0, 80, Style::default());
    assert!(has_braille(&joined(&out)));
}

#[test]
fn sampling_density_tracks_terminal_width() {
    // 采样点数 = 画布像素宽 = cols*2，故加宽终端不会让曲线变稀疏
    let narrow = render_plot(&rows(&["x: 0 .. 100", "f: sin(x)"]), 0, 30, Style::default());
    let wide = render_plot(&rows(&["x: 0 .. 100", "f: sin(x)"]), 0, 120, Style::default());
    let w = |ls: &[Line<'static>]| ls.iter().map(|l| grid::line_w(&l.spans)).max().unwrap_or(0);
    assert!(w(&wide) > w(&narrow), "画布应随终端宽度增长");
}

#[test]
fn every_line_respects_available_width() {
    let cases: [&[&str]; 4] = [
        &[
            "title: 标题（全角）",
            "xlabel: 横轴",
            "x: -6.28 .. 6.28",
            "f: 正弦 = sin(x)",
        ],
        &["x: -1 .. 1", "f: 甲 = x", "f: 乙 = x^2", "f: 丙 = x^3"],
        &["xs: 0,1,2,3", "ys: 1,null,2,null"],
        &["title: 极值", "x: 0 .. 1000", "f: exp(x)"],
    ];
    for width in 16..=100usize {
        for indent in [0usize, 4] {
            for case in cases {
                for line in render_plot(&rows(case), indent, width, Style::default()) {
                    let w = grid::line_w(&line.spans);
                    assert!(
                        w <= width,
                        "绘图右溢: width={width} indent={indent} line_w={w} line={line:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn expression_never_escapes_whitelist() {
    // 表外标识符一律编译失败 → 该条 f 被丢弃 → 无有效序列 → 降级
    for bad in ["x + y", "exec(x)", "x; x", "sin x", "x.__class__"] {
        let out = render_plot(
            &rows(&["x: 0 .. 1", &format!("f: {bad}")]),
            0,
            60,
            Style::default(),
        );
        let s = joined(&out);
        assert!(!has_braille(&s), "非法表达式不应产出曲线: {bad} -> {s}");
        assert!(s.contains("f: "), "应原样降级: {s}");
    }
}
