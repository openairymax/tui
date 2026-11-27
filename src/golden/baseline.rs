// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 渲染基线矩阵（0.1.21 X2-26）：把一帧的可复跑度量收敛为「逐行文本 +
// 逐行显示宽度 + 整屏符号指纹 + 整屏配色指纹」，先立 40 列档，再扩
// 80/120 列档。断言只依赖度量本身，不落任何人工维护的期望文本，因此
// 任何一档失配都能定位到具体行、具体格。
//
// 40 列低于预算下限（46 列），基线的期望是「守卫可见、无溢出」：整屏
// 转为尺寸提示，Ctrl+C 退出路径必须可达；宽度合规保证后续 X2-21 把
// 40 列做成真实堆叠布局时，同一套判据可以直接复用。

use super::{assert_has, assert_lacks, rows_of, Fixture, H, W};
use crate::app::App;
use crate::budget::MIN_TERM_COLS;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

/// 一帧基线：逐行文本、逐行显示宽度、整屏符号与配色指纹。
pub(crate) struct Baseline {
    cols: u16,
    rows: Vec<String>,
    widths: Vec<usize>,
    symbols: Vec<String>,
    colors: Vec<String>,
}

impl Baseline {
    /// 渲染一帧并采集全部度量；渲染路径与生产完全一致。
    pub(crate) fn capture(app: &mut App, w: u16, h: u16) -> Self {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("测试终端");
        term.draw(|f| crate::present::render(f, app))
            .expect("渲染一帧");
        let buf = term.backend().buffer();
        let rows = rows_of(buf, w, h);
        let widths = rows
            .iter()
            .map(|r| UnicodeWidthStr::width(r.as_str()))
            .collect();
        let mut symbols = Vec::with_capacity((w as usize) * (h as usize));
        let mut colors = Vec::with_capacity((w as usize) * (h as usize));
        for y in 0..h {
            for x in 0..w {
                let cell = &buf[(x, y)];
                symbols.push(cell.symbol().to_string());
                colors.push(format!("{:?}|{:?}", cell.fg, cell.bg));
            }
        }
        Self {
            cols: w,
            rows,
            widths,
            symbols,
            colors,
        }
    }

    pub(crate) fn rows(&self) -> &[String] {
        &self.rows
    }

    /// 超出视口列数的行：(行号, 显示宽度)。
    fn overflow(&self) -> Vec<(usize, usize)> {
        self.widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > self.cols as usize)
            .map(|(y, w)| (y, *w))
            .collect()
    }

    /// 同态同帧：文本、符号、配色三者逐格相等才算一致。
    fn same_frame(&self, other: &Self) -> bool {
        self.rows == other.rows && self.symbols == other.symbols && self.colors == other.colors
    }
}

/// 断言视口内无溢出；失败时同时给出基准行文本便于定位。
fn assert_fits(base: &Baseline) {
    let bad = base.overflow();
    assert!(
        bad.is_empty(),
        "存在超宽行 {:?}（视口 {} 列）\n---屏幕---\n{}",
        bad,
        base.cols,
        base.rows().join("\n")
    );
}

/// 40 列档即低于最小可用宽度：守卫接管整屏，宽度合规且退出路径可达。
#[test]
fn baseline_40col_fits_guard() {
    assert!(40 < MIN_TERM_COLS, "40 列档须落在最小可用宽度之下");
    let mut fx = Fixture::new("x2-26a-40");
    let base = Baseline::capture(&mut fx.app, 40, H);
    assert_fits(&base);
    assert_has(&fx.draw_at(40, H), "终端尺寸不足");
    assert_has(&fx.draw_at(40, H), "Ctrl+C 退出");
    assert_lacks(&fx.draw_at(40, H), "❯");
}

/// 同态同帧：同一状态连续两帧的文本、符号、配色逐格一致。
#[test]
fn baseline_40col_reproducible() {
    let mut fx = Fixture::new("x2-26a-idem");
    let a = Baseline::capture(&mut fx.app, 40, H);
    let b = Baseline::capture(&mut fx.app, 40, H);
    assert!(a.same_frame(&b), "40 列档同态两帧应逐格一致");
}

/// 40 列档与 100 列档可区分：守卫态与常规界面不得同形。
#[test]
fn baseline_40col_differs_from_wide() {
    let mut fx = Fixture::new("x2-26a-diff");
    let narrow = Baseline::capture(&mut fx.app, 40, H);
    let wide = Baseline::capture(&mut fx.app, W, H);
    assert_eq!(narrow.rows().len(), wide.rows().len(), "两档行数应同高");
    assert_ne!(
        narrow.symbols, wide.symbols,
        "40 列守卫态不应与 100 列常规界面同形"
    );
    assert_has(&fx.draw_at(W, H), "❯");
}
