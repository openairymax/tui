// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-06~X2-08 帧编码器的字节级用例集。
//
// 从 backend.rs 拆出：帧编码器本体会持续叠加分片与同步块逻辑，用例若与
// 实现同文件将很快越过单文件行数上限；此处按 `#[path]` 挂回同一模块，
// 仍可直接访问 crate 内可见的编码接口。

use crate::engine::frame_io;
use crate::engine::ladder::Ladder;
use super::*;
use std::sync::{Arc, Mutex};

/// 写次数与字节同时计量的探针 writer。
#[derive(Clone, Default)]
pub(super) struct Sink {
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

pub(super) fn backend() -> (BufferedBackend<Sink>, Sink) {
    let sink = Sink::default();
    (BufferedBackend::new(sink.clone()), sink)
}

pub(super) fn cell(symbol: &str) -> Cell {
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
    assert!(bytes.contains("\x1b[31;49m"), "frame = {bytes:?}");
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
    assert!(after.contains("\x1b[34;49m"), "frame tail = {after:?}");
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
///
/// 间距从写入 `a` 后的真实光标（第 2 列）起算，故到第 4 列是前移 2 格。
#[test]
fn short_gap_uses_forward() {
    let (mut b, _) = backend();
    let a = cell("a");
    let d = cell("d");
    b.draw(vec![(0u16, 0u16, &a), (3u16, 0u16, &d)].into_iter())
        .unwrap();
    assert_eq!(b.move_count(), 2);
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[2C"), "frame = {s:?}");
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
    assert!(s.contains("\x1b[31;49m"), "frame = {s:?}");
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
    assert!(s.contains("\x1b[31;49;1m"), "frame = {s:?}");
    assert!(s.contains("\x1b[22;3m"), "frame = {s:?}");
}

/// 颜色与修饰位落在同一条 SGR（X2-08b）。
#[test]
fn color_and_bold_merge_into_one_sgr() {
    let (mut b, _) = backend();
    let c = styled(1, Color::LightBlue, Modifier::BOLD | Modifier::UNDERLINED);
    b.draw(vec![(0u16, 0u16, &c[0])].into_iter()).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.contains("\x1b[94;49;1;4m"), "frame = {s:?}");
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

/// 全默认目标用单条 `SGR 0` 合并复位，不发 39;49（X2-09a）。
#[test]
fn plain_style_uses_single_reset() {
    let (mut b, _) = backend();
    let decorated = styled(1, Color::Red, Modifier::BOLD);
    b.draw(vec![(0u16, 0u16, &decorated[0])].into_iter()).unwrap();
    let plain = Cell::default();
    b.draw(vec![(1u16, 0u16, &plain)].into_iter()).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert!(s.ends_with("\x1b[0m "), "frame = {s:?}");
    assert!(!s.contains("39;49"), "frame = {s:?}");
}

/// 黄金字节流：2×2 帧的完整编码序列逐字节比对（X2-10b）。
#[test]
fn golden_two_by_two_frame() {
    let (mut b, _) = backend();
    let mut a = cell("A");
    a.fg = Color::Red;
    let mut bcell = cell("B");
    bcell.fg = Color::Red;
    let c = cell("C");
    let d = cell("D");
    b.draw(
        vec![
            (0u16, 0u16, &a),
            (1u16, 0u16, &bcell),
            (0u16, 1u16, &c),
            (1u16, 1u16, &d),
        ]
        .into_iter(),
    )
    .unwrap();
    let got = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert_eq!(got, "\x1b[1;1H\x1b[31;49mAB\x1b[2;1H\x1b[0mCD");
}

/// 字节预算：整行默认样式的定位与复位各只一次（X2-09b）。
#[test]
fn plain_row_stays_within_byte_budget() {
    let (mut b, _) = backend();
    let cells: Vec<Cell> = (0..40).map(|_| Cell::default()).collect();
    let iter = cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
    b.draw(iter).unwrap();
    let s = String::from_utf8_lossy(b.frame_bytes()).to_string();
    assert_eq!(s.matches('m').count(), 1, "frame = {s:?}");
    // 绝对定位 6 字节 + 复位 4 字节 + 40 个空格。
    assert_eq!(b.frame_bytes().len(), 6 + 4 + 40, "frame = {s:?}");
}

/// 前 N 次写注入失败，之后正常接收字节（X2-15a 判据）。
#[derive(Clone, Default)]
pub(super) struct FailSink {
    fails: Arc<Mutex<usize>>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl FailSink {
    pub(super) fn new(fails: usize) -> Self {
        let sink = Self::default();
        *sink.fails.lock().unwrap() = fails;
        sink
    }

    pub(super) fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

impl Write for FailSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut left = self.fails.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected"));
        }
        drop(left);
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn on_count(buf: &[u8]) -> usize {
    buf.windows(frame_io::SYNC_ON.len())
        .filter(|w| *w == frame_io::SYNC_ON)
        .count()
}

fn off_count(buf: &[u8]) -> usize {
    buf.windows(SYNC_OFF.len()).filter(|w| *w == SYNC_OFF).count()
}

/// 同步开启时每次 flush 自包一对 `h/l`（X2-13a、X2-14a 判据：配对率 100%）。
#[test]
fn sync_block_pairs_each_flush() {
    let (mut b, sink) = backend();
    b.set_sync(true);
    let a = cell("A");
    b.draw(vec![(0u16, 0u16, &a)].into_iter()).unwrap();
    b.flush().unwrap();
    b.draw(vec![(1u16, 0u16, &a)].into_iter()).unwrap();
    b.flush().unwrap();
    let bytes = sink.bytes.lock().unwrap().clone();
    assert_eq!(b.write_count(), 2, "每帧恰一次写出");
    assert_eq!(on_count(&bytes), 2, "bytes = {bytes:?}");
    assert_eq!(off_count(&bytes), 2, "bytes = {bytes:?}");
    assert!(bytes.ends_with(SYNC_OFF), "bytes = {bytes:?}");
}

/// 携带未写内容退出时补发关闭序列，终端不残留同步态（X2-13b）。
#[test]
fn drop_closes_pending_sync_block() {
    let sink = Sink::default();
    {
        let mut b = BufferedBackend::new(sink.clone());
        b.set_sync(true);
        let a = cell("A");
        b.draw(vec![(0u16, 0u16, &a)].into_iter()).unwrap();
    }
    let bytes = sink.bytes.lock().unwrap().clone();
    assert!(bytes.ends_with(SYNC_OFF), "bytes = {bytes:?}");
}

/// 断管（EPIPE）异常：失败路径同样补发关闭序列，终端不残留同步态（X2-14b）。
#[test]
fn broken_pipe_closes_sync_block() {
    let sink = FailSink::new(1);
    let mut b = BufferedBackend::new(sink.clone());
    b.set_sync(true);
    let a = cell("A");
    b.draw(vec![(0u16, 0u16, &a)].into_iter()).unwrap();
    assert!(b.flush().is_err());
    assert_eq!(sink.bytes(), SYNC_OFF.to_vec(), "只应留下关闭序列");
    assert_eq!(b.fail_count(), 1);
    assert!(!b.degraded());
}

/// 连续失败达阈值即降一档，内容仍然送达（X2-15a/b、X2-30a）。
#[test]
fn repeated_failures_step_down_one_level() {
    let sink = FailSink::new(3);
    let mut b = BufferedBackend::new(sink.clone());
    let a = cell("A");
    let c = cell("B");
    b.draw(vec![(0u16, 0u16, &a), (1u16, 0u16, &c)].into_iter())
        .unwrap();
    assert!(b.flush().is_err());
    assert_eq!(b.fail_count(), 1);
    assert!(!b.degraded(), "单次失败不降级");
    assert!(b.flush().is_err());
    assert_eq!(b.fail_count(), 2);
    // 第三次失败达阈值：降一档后立即重投，本次 flush 即成功。
    assert!(b.flush().is_ok());
    assert!(b.degraded());
    assert_eq!(b.level(), Ladder::Batch, "三次失败只降一档");
    let bytes = sink.bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains('A') && text.contains('B'), "bytes = {text:?}");
}

/// 降级路径同样写出首个单元格标记之前的字节（如清屏），不丢内容。
#[test]
fn degraded_write_keeps_prefix_bytes() {
    let sink = FailSink::new(3);
    let mut b = BufferedBackend::new(sink.clone());
    let a = cell("A");
    b.clear().unwrap();
    b.draw(vec![(0u16, 0u16, &a)].into_iter()).unwrap();
    for _ in 0..2 {
        assert!(b.flush().is_err());
    }
    assert!(b.flush().is_ok());
    let text = String::from_utf8_lossy(&sink.bytes()).to_string();
    assert!(text.contains("\x1b[2J"), "bytes = {text:?}");
    assert!(text.contains('A'), "bytes = {text:?}");
}

/// 跨帧样式未变：第二帧不再重发任何 SGR（X2-28b 快路径）。
#[test]
fn repeated_draw_reuses_style_cache() {
    let (mut b, _) = backend();
    let cells = styled(2, Color::Red, Modifier::empty());
    let iter = || cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
    b.draw(iter()).unwrap();
    let first = b.frame_bytes().len();
    assert!(first > 0);
    b.draw(iter()).unwrap();
    let tail = String::from_utf8_lossy(&b.frame_bytes()[first..]).to_string();
    assert!(!tail.contains('m'), "frame tail = {tail:?}");
}
