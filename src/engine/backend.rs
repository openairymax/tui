// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-06a/b（§23.5.2 序 83/84，WP X2-5「原子帧单写」）：BufferedBackend 骨架。
//
// 分层铁律：本文件属 L5 合成层的后端设施，是全仓唯一允许触碰终端字节流的
// 新落点（原 L5 只经 `terminal.draw` 间接写出）。职责单一——把 ratatui 交来
// 的 diff 帧**编码进内存 `Vec<u8>`**，不在此刻做任何 IO；真正的写出收敛在
// `flush()` 的单次 `write_all`（X2-13a），因此一帧的字节天然是一个不可分割
// 的批次，为后续 `\e[?2026h/l` 同步块（X2-14a）与 >1MiB 分片（X2-11a）提供
// 唯一落点。
//
// 本卡只做「骨架 + 最小编码通路」：
//   - X2-06a：`BufferedBackend` 被 `CrosstermBackend` 包裹，可实例化；
//   - X2-06b：`draw` 期把单元格 diff 编码进 `Vec<u8>`，可产出字节用例。
// 游程合并（X2-07）、样式游程与 SGR 最短化（X2-08）、整帧 diff 编码器
// （X2-09/X2-10）在后续卡落位；本卡按「位置游标 + 前景/背景色最小 SGR +
// 符号字节」编码，够用且可被后续卡替换内部策略而不动接口。
//
// 接线（entry.rs 把 `Terminal::new(CrosstermBackend::new(stdout))` 换成
// `Terminal::new(BufferedBackend::new(CrosstermBackend::new(stdout)))`）属
// X2-09，本卡不动运行期路径，故模块暂以 `allow(dead_code)` 挂起。

#![allow(dead_code)] // X2-06 骨架，运行期接线在 X2-09

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

/// 同一行内改用前移（CUF）的最大间距：超过即绝对定位。
///
/// 3 列的取舍：`ESC[nC` 与 `ESC[r;cH` 字节数在 3 附近持平，更短的间距
/// 用前移能省下行列两个数字，更长的间距用绝对定位一次到位更稳。
const CUF_MAX: u16 = 3;

/// 帧字节缓冲后端：包裹 `CrosstermBackend<W>`，把一帧的 diff 编码进
/// `out`，`flush()` 时一次 `write_all` 写出。
pub(crate) struct BufferedBackend<W: Write> {
    inner: CrosstermBackend<W>,
    /// 当前帧待写出的字节（已编码的 SGR/移动/符号序列）。
    out: Vec<u8>,
    /// 上一次显式定位后的光标位置；用于省略连续的重复定位。
    cursor: Option<Position>,
    /// 已生效的前景/背景色，用于最小化 SGR 输出。
    fg: Option<Color>,
    bg: Option<Color>,
    /// `flush()` 实际发生的 `write_all` 次数（判据：一帧恰好 1 次）。
    writes: usize,
    /// 累计定位次数（X2-07b 判据：游程合并后应显著低于单元格数）。
    moves: usize,
}

impl<W: Write> BufferedBackend<W> {
    pub(crate) const fn new(inner: CrosstermBackend<W>) -> Self {
        Self {
            inner,
            out: Vec::new(),
            cursor: None,
            fg: None,
            bg: None,
            writes: 0,
            moves: 0,
        }
    }

    /// 当前帧已编码但未写出的字节（只读）。
    pub(crate) fn frame_bytes(&self) -> &[u8] {
        &self.out
    }

    /// 取走当前帧字节（调用方自持写出，不经过 `flush`）。
    pub(crate) fn take_frame(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// `flush()` 累计写次数（判据用）。
    pub(crate) fn write_count(&self) -> usize {
        self.writes
    }

    /// 累计定位次数（X2-07b 判据用）。
    pub(crate) fn move_count(&self) -> usize {
        self.moves
    }

    /// 归还内层后端（测试与 X2-09 接线用）。
    pub(crate) fn into_inner(self) -> CrosstermBackend<W> {
        self.inner
    }

    /// 绝对定位（CUP）：跨行或远距跳转的唯一落点。
    fn emit_move(&mut self, pos: Position) {
        let _ = write!(self.out, "\x1b[{};{}H", pos.y + 1, pos.x + 1);
        self.cursor = Some(pos);
        self.moves += 1;
    }

    /// 把光标送到 `pos`（X2-07a）。
    ///
    /// 同游程内的相邻格由 `draw` 的 `next` 预测直接跳过，根本不进此函数；
    /// 这里只处理真正的空隙：同一行且间距不超过 `CUF_MAX` 列用前移，其余
    /// 用绝对定位。光标已在目标位置时不产生任何字节。
    fn seek(&mut self, pos: Position) {
        match self.cursor {
            Some(cur) if cur == pos => {}
            Some(cur) if cur.y == pos.y && cur.x < pos.x && pos.x - cur.x <= CUF_MAX => {
                let _ = write!(self.out, "\x1b[{}C", pos.x - cur.x);
                self.cursor = Some(pos);
                self.moves += 1;
            }
            _ => self.emit_move(pos),
        }
    }

    /// 前景/背景色最小 SGR：只对发生变化的通道发参数。
    fn emit_style(&mut self, cell: &Cell) {
        if self.fg == Some(cell.fg) && self.bg == Some(cell.bg) {
            return;
        }
        let mut parts: Vec<String> = Vec::with_capacity(2);
        if self.fg != Some(cell.fg) {
            parts.push(match sgr_channel(cell.fg, true) {
                Some(s) => s,
                None => "39".to_string(),
            });
            self.fg = Some(cell.fg);
        }
        if self.bg != Some(cell.bg) {
            parts.push(match sgr_channel(cell.bg, false) {
                Some(s) => s,
                None => "49".to_string(),
            });
            self.bg = Some(cell.bg);
        }
        let _ = write!(self.out, "\x1b[{}m", parts.join(";"));
    }
}

impl<W: Write> Backend for BufferedBackend<W> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // 帧内预测位置：与已写出的符号跨度一致，命中即免去定位。
        let mut next: Option<Position> = None;
        for (x, y, cell) in content {
            let pos = Position { x, y };
            if cell.skip {
                // 宽字符占用的第二格：预测命中时只推进预测，不产生字节。
                if next == Some(pos) {
                    next = Some(Position {
                        x: x.saturating_add(1),
                        y,
                    });
                }
                continue;
            }
            if next != Some(pos) {
                self.seek(pos);
            }
            self.emit_style(cell);
            self.out.extend_from_slice(cell.symbol().as_bytes());
            // 按符号真实显示宽度推进（宽字符 2 列，零宽按 1 列兜底）。
            let width = UnicodeWidthStr::width(cell.symbol().as_str()).max(1) as u16;
            next = Some(Position {
                x: x.saturating_add(width),
                y,
            });
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.out.extend_from_slice(b"\x1b[?25l");
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.out.extend_from_slice(b"\x1b[?25h");
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.seek(position.into());
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.out.extend_from_slice(b"\x1b[2J");
        self.cursor = None;
        self.fg = None;
        self.bg = None;
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        match clear_type {
            ClearType::All => self.clear(),
            other => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("clear_type [{other:?}] not supported with this backend"),
            )),
        }
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    /// X2-13a：一帧 = 一次 `write_all`（`out` 非空时）。空帧零写。
    fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        let writer = self.inner.writer_mut();
        writer.write_all(&self.out)?;
        writer.flush()?;
        self.writes += 1;
        self.out.clear();
        Ok(())
    }
}

/// 把 `Color` 映射到 SGR 参数串；`Reset` 返回 `None`（调用方回落到 39/49）。
fn sgr_channel(color: Color, fg: bool) -> Option<String> {
    let base: u8 = if fg { 30 } else { 40 };
    let bright: u8 = if fg { 90 } else { 100 };
    let (r#type, n): (u8, u8) = match color {
        Color::Reset => return None,
        Color::Black => (base, 0),
        Color::Red => (base, 1),
        Color::Green => (base, 2),
        Color::Yellow => (base, 3),
        Color::Blue => (base, 4),
        Color::Magenta => (base, 5),
        Color::Cyan => (base, 6),
        Color::Gray => (base, 7),
        Color::DarkGray => (bright, 0),
        Color::LightRed => (bright, 1),
        Color::LightGreen => (bright, 2),
        Color::LightYellow => (bright, 3),
        Color::LightBlue => (bright, 4),
        Color::LightMagenta => (bright, 5),
        Color::LightCyan => (bright, 6),
        Color::White => (bright, 7),
        Color::Indexed(i) => {
            return Some(format!("{};5;{}", if fg { 38 } else { 48 }, i));
        }
        Color::Rgb(r, g, b) => {
            return Some(format!("{};2;{};{};{}", if fg { 38 } else { 48 }, r, g, b));
        }
    };
    Some((r#type + n).to_string())
}

#[cfg(test)]
mod tests {
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
        (BufferedBackend::new(CrosstermBackend::new(sink.clone())), sink)
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

    /// 逐格定位的旧行为对 N 格需 N 次定位；游程合并后为 1 次（X2-07b）。
    #[test]
    fn move_count_drops_for_dense_row() {
        let (mut b, _) = backend();
        let cells: Vec<Cell> = (0..80).map(|_| cell(".")).collect();
        let iter = cells.iter().enumerate().map(|(i, c)| (i as u16, 0u16, c));
        b.draw(iter).unwrap();
        assert_eq!(b.move_count(), 1, "80 格应合并为 1 次定位");
    }
}
