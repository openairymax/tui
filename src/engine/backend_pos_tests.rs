// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-07a / X2-28b 的光标定位用例集：前移与绝对定位的取舍、间距起点。
//
// 与 backend_tests.rs 分文件存放，是为把「定位策略」与「样式/写出策略」
// 的判据各自收在一处——前移距离的口径必须只有一种，即写入后的真实光标；
// 否则宽字符之后的空隙会多算一列，后续字符整体右移。共享的构造器仍由
// backend_tests 提供，用例自身不复制实现细节。

use super::tests::{backend, cell};
use super::*;

/// 9 列以内的空隙仍走前移（X2-28b：单数字前移恒 4 字节）。
#[test]
fn medium_gap_stays_on_forward() {
    let (mut b, _) = backend();
    let a = cell("a");
    let g = cell("g");
    b.draw(vec![(0u16, 0u16, &a), (6u16, 0u16, &g)].into_iter())
        .unwrap();
    assert_eq!(b.move_count(), 2);
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[5C"), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 1, "frame = {s:?}");
}

/// 超过前移上限的空隙回到绝对定位，不出现两位数前移。
#[test]
fn gap_past_limit_uses_absolute() {
    let (mut b, _) = backend();
    let a = cell("a");
    let k = cell("k");
    b.draw(vec![(0u16, 0u16, &a), (11u16, 0u16, &k)].into_iter())
        .unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[1;12H"), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 2, "frame = {s:?}");
}

/// 等距 9 列的行：整行字节数低于全绝对定位（X2-28b 热点下降对比）。
///
/// 旧路径三处空隙各发一条 `ESC[1;cH`（6~7 字节），新路径各发一条
/// `ESC[8C`（4 字节）；反差直接体现在整帧字节数上。
#[test]
fn forward_moves_shrink_row_bytes() {
    let (mut b, _) = backend();
    let cells: Vec<Cell> = (0..4).map(|_| cell("x")).collect();
    let xs = [0u16, 9, 18, 27];
    let iter = xs.iter().zip(cells.iter()).map(|(x, c)| (*x, 0u16, c));
    b.draw(iter).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert_eq!(s.matches("\x1b[8C").count(), 3, "frame = {s:?}");
    // 复位 4 + 首发定位 6 + 首格符号 1 + 3×(前移 4 + 符号 1)。
    assert_eq!(b.frame_bytes().len(), 4 + 6 + 1 + 15, "frame = {s:?}");
}

/// 宽字符之后的空隙按真实光标起算，不把显示宽度当成空隙（回归）。
#[test]
fn wide_cell_gap_counts_from_real_cursor() {
    let (mut b, _) = backend();
    let wide = cell("中");
    let t = cell("t");
    b.draw(vec![(0u16, 0u16, &wide), (4u16, 0u16, &t)].into_iter())
        .unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[2C"), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 1, "frame = {s:?}");
}
