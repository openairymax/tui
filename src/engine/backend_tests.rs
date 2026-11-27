// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-06~X2-08 帧编码器的字节级用例集。
//
// 从 backend.rs 拆出：帧编码器本体会持续叠加分片与同步块逻辑，用例若与
// 实现同文件将很快越过单文件行数上限；此处按 `#[path]` 挂回同一模块，
// 仍可直接访问 crate 内可见的编码接口。

use super::*;
use std::sync::{Arc, Mutex};

/// 写次数与字节同时计量的探针 writer。
#[derive(Clone, Default)]
struct Sink {
    writes: Arc<Mutex<usize>>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        *self.writes.lock().unwrap() += 1;
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn backend() -> (BufferedBackend<Sink>, Sink) {
    let sink = Sink::default();
    (BufferedBackend::new(sink.clone()), sink)
}

fn cell(symbol: &str) -> Cell {
    let mut c = Cell::default();
    c.set_symbol(symbol);
    c
}

#[test]
fn new_frame_is_empty() {
    let (b, _) = backend();
    assert!(b.frame_bytes().is_empty());
    assert_eq!(b.write_count(), 0);
}

#[test]
fn draw_encodes_symbol_and_color() {
    let (mut b, _) = backend();
    let mut c = cell("A");
    c.fg = Color::Red;
    b.draw(vec![(0u16, 0u16, &c)].into_iter()).unwrap();
    let bytes = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(bytes.contains('A'), "frame = {bytes:?}");
    assert!(bytes.contains("\x1b[31m"), "frame = {bytes:?}");
}

#[test]
fn draw_skips_skipped_cells() {
    let (mut b, _) = backend();
    let mut c = cell("B");
    c.skip = true;
    b.draw(vec![(2u16, 1u16, &c)].into_iter()).unwrap();
    assert!(b.frame_bytes().is_empty());
}

#[test]
fn draw_emits_rgb_truecolor() {
    let (mut b, _) = backend();
    let mut c = cell("x");
    c.fg = Color::Rgb(1, 2, 3);
    c.bg = Color::Indexed(9);
    b.draw(vec![(0u16, 0u16, &c)].into_iter()).unwrap();
    let bytes = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(bytes.contains("38;2;1;2;3"), "frame = {bytes:?}");
    assert!(bytes.contains("48;5;9"), "frame = {bytes:?}");
}

#[test]
fn flush_writes_exactly_once_and_clears() {
    let (mut b, sink) = backend();
    let c = cell("Z");
    b.draw(vec![(0u16, 0u16, &c)].into_iter()).unwrap();
    assert_eq!(b.write_count(), 0, "draw 不得产生 IO");
    b.flush().unwrap();
    assert_eq!(b.write_count(), 1);
    assert_eq!(*sink.writes.lock().unwrap(), 1, "一帧一次 write");
    assert!(b.frame_bytes().is_empty());
    assert!(String::from_utf8_lossy(&sink.bytes.lock().unwrap()).contains('Z'));
}

#[test]
fn empty_flush_is_noop() {
    let (mut b, sink) = backend();
    b.flush().unwrap();
    assert_eq!(b.write_count(), 0);
    assert_eq!(*sink.writes.lock().unwrap(), 0);
}

#[test]
fn clear_encoding_and_reset_style_cache() {
    let (mut b, _) = backend();
    let mut c = cell("A");
    c.fg = Color::Blue;
    b.draw(vec![(0u16, 0u16, &c)].into_iter()).unwrap();
    b.clear().unwrap();
    let bytes = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(bytes.contains("\x1b[2J"), "frame = {bytes:?}");
    // 清屏后样式缓存必须失效：同色单元需重新发 SGR。
    let before = b.frame_bytes().len();
    b.draw(vec![(0u16, 0u16, &c)].into_iter()).unwrap();
    let after = String::from_utf8_lossy(&b.frame_bytes()[before..]).to_string();
    assert!(after.contains("\x1b[34m"), "frame tail = {after:?}");
}

#[test]
fn cursor_visibility_encoding() {
    let (mut b, _) = backend();
    b.hide_cursor().unwrap();
    b.show_cursor().unwrap();
    let bytes = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(bytes.contains("\x1b[?25l"));
    assert!(bytes.contains("\x1b[?25h"));
}

/// 横向无空隙的一行：整条游程只定位一次（X2-07a）。
#[test]
fn contiguous_run_merges_into_one_move() {
    let (mut b, _) = backend();
    let cells: Vec<Cell> = (0..5).map(|_| cell("x")).collect();
    let iter = cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
    b.draw(iter).unwrap();
    assert_eq!(b.move_count(), 1);
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert_eq!(s.matches('H').count(), 1, "frame = {s:?}");
    assert!(s.starts_with("\x1b[1;1H"), "frame = {s:?}");
}

/// 短空隙走前移（CUF），省掉行列两个数字（X2-07a）。
#[test]
fn short_gap_uses_forward() {
    let (mut b, _) = backend();
    let a = cell("a");
    let d = cell("d");
    b.draw(vec![(0u16, 0u16, &a), (3u16, 0u16, &d)].into_iter())
        .unwrap();
    assert_eq!(b.move_count(), 2);
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[3C"), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 1, "frame = {s:?}");
}

/// 远距跳转与跨行一律绝对定位，避免长前移在折行处越界。
#[test]
fn far_and_row_jumps_are_absolute() {
    let (mut b, _) = backend();
    let a = cell("a");
    let z = cell("z");
    b.draw(vec![(0u16, 0u16, &a), (40u16, 0u16, &z)].into_iter())
        .unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[1;41H"), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 2, "frame = {s:?}");

    let (mut b, _) = backend();
    b.draw(vec![(5u16, 0u16, &a), (0u16, 1u16, &z)].into_iter())
        .unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[2;1H"), "frame = {s:?}");
}

/// 宽字符与其 skip 占位格同属一条游程（X2-07a）。
#[test]
fn wide_cell_and_skip_share_one_move() {
    let (mut b, _) = backend();
    let wide = cell("中");
    let mut skip = Cell::default();
    skip.skip = true;
    let tail = cell("b");
    b.draw(
        vec![(0u16, 0u16, &wide), (1u16, 0u16, &skip), (2u16, 0u16, &tail)].into_iter(),
    )
    .unwrap();
    assert_eq!(b.move_count(), 1);
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains('中') && s.contains('b'), "frame = {s:?}");
    assert_eq!(s.matches('H').count(), 1, "frame = {s:?}");
}

fn styled(n: usize, fg: Color, modifier: Modifier) -> Vec<Cell> {
    (0..n)
        .map(|_| {
            let mut c = cell("s");
            c.fg = fg;
            c.modifier = modifier;
            c
        })
        .collect()
}

/// 同一样式的连续单元格只发一条 SGR（X2-08a）。
#[test]
fn identical_style_run_emits_one_sgr() {
    let (mut b, _) = backend();
    let cells = styled(4, Color::Red, Modifier::empty());
    let iter = cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
    b.draw(iter).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert_eq!(s.matches('m').count(), 1, "frame = {s:?}");
    assert!(s.contains("\x1b[31m"), "frame = {s:?}");
}

/// 修饰位跨游程只发差量（X2-08b）。
#[test]
fn modifier_transition_encodes_diff() {
    let (mut b, _) = backend();
    let bold = styled(1, Color::Red, Modifier::BOLD);
    let italic = styled(1, Color::Red, Modifier::ITALIC);
    b.draw(
        vec![(0u16, 0u16, &bold[0]), (1u16, 0u16, &italic[0])].into_iter(),
    )
    .unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[31;1m"), "frame = {s:?}");
    assert!(s.contains("\x1b[22;3m"), "frame = {s:?}");
}

/// 颜色与修饰位落在同一条 SGR（X2-08b）。
#[test]
fn color_and_bold_merge_into_one_sgr() {
    let (mut b, _) = backend();
    let c = styled(1, Color::LightBlue, Modifier::BOLD | Modifier::UNDERLINED);
    b.draw(vec![(0u16, 0u16, &c[0])].into_iter()).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[94;1;4m"), "frame = {s:?}");
}

/// 逐格定位的旧行为对 N 格需 N 次定位；游程合并后为 1 次（X2-07b）。
#[test]
fn move_count_drops_for_dense_row() {
    let (mut b, _) = backend();
    let cells: Vec<Cell> = (0..80).map(|_| cell(".")).collect();
    let iter = cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
    b.draw(iter).unwrap();
    assert_eq!(b.move_count(), 1, "80 格应合并为 1 次定位");
}
