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
// 本文件持有 writer 本体而不是包裹 `CrosstermBackend`：后者不对外暴露
// writer，无法在 flush 时交出一帧字节；尺寸、窗口与光标查询直接委托
// crossterm 的终端函数，语义与原后端一致。
//
//   - X2-06a：`BufferedBackend` 可实例化；
//   - X2-06b：`draw` 期把单元格 diff 编码进 `Vec<u8>`，可产出字节用例。
// 游程合并（X2-07）、样式游程（X2-08）、SGR 最短化与整帧编码器
// （X2-09/X2-10）在后续卡落位；本卡按「位置游标 + 样式差量 + 符号字节」
// 编码，够用且可被后续卡替换内部策略而不动接口。
//
// 接线（entry.rs 把 `Terminal::new(CrosstermBackend::new(stdout))` 换成
// `Terminal::new(BufferedBackend::new(stdout))`）属 X2-09，本卡不动运行期
// 路径，故模块暂以 `allow(dead_code)` 挂起。

#![allow(dead_code)] // X2-06 骨架，运行期接线在 X2-09

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use super::backend_sgr::{color_param, mod_params};

/// 同一行内改用前移（CUF）的最大间距：超过即绝对定位。
///
/// 3 列的取舍：`ESC[nC` 与 `ESC[r;cH` 字节数在 3 附近持平，更短的间距
/// 用前移能省下行列两个数字，更长的间距用绝对定位一次到位更稳。
const CUF_MAX: u16 = 3;

/// 帧字节缓冲后端：持有 writer 本体，把一帧的 diff 编码进 `out`，
/// `flush()` 时一次 `write_all` 写出。
pub(crate) struct BufferedBackend<W: Write> {
    writer: W,
    /// 当前帧待写出的字节（已编码的 SGR/移动/符号序列）。
    out: Vec<u8>,
    /// 上一次显式定位后的光标位置；用于省略连续的重复定位。
    cursor: Option<Position>,
    /// 已生效的前景/背景色与修饰位，用于最小化 SGR 输出。
    fg: Option<Color>,
    bg: Option<Color>,
    mods: Modifier,
    /// `flush()` 实际发生的 `write_all` 次数（判据：一帧恰好 1 次）。
    writes: usize,
    /// 累计定位次数（X2-07b 判据：游程合并后应显著低于单元格数）。
    moves: usize,
}

impl<W: Write> BufferedBackend<W> {
    pub(crate) const fn new(writer: W) -> Self {
        Self {
            writer,
            out: Vec::new(),
            cursor: None,
            fg: None,
            bg: None,
            mods: Modifier::empty(),
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

    /// 归还 writer（测试与 X2-09 接线用）。
    pub(crate) fn into_inner(self) -> W {
        self.writer
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

    /// 样式游程（X2-08a/b）：前景、背景、修饰位三路差量合并成一条 SGR。
    ///
    /// 同一游程内后续单元格三路均未变化时直接返回，不产生字节；跨游程
    /// 只发差量，避免整条样式重发。
    ///
    /// X2-09a 最短化：目标为全默认时用单条 `SGR 0` 合并复位——它同时清
    /// 颜色与修饰位，字节数也少于 `39;49` 加逐位关闭。
    fn emit_style(&mut self, cell: &Cell) {
        if is_plain(cell) {
            if self.fg != Some(Color::Reset)
                || self.bg != Some(Color::Reset)
                || !self.mods.is_empty()
            {
                self.out.extend_from_slice(b"\x1b[0m");
                self.fg = Some(Color::Reset);
                self.bg = Some(Color::Reset);
                self.mods = Modifier::empty();
            }
            return;
        }
        let mut parts: Vec<String> = Vec::with_capacity(4);
        if self.fg != Some(cell.fg) {
            parts.push(color_param(cell.fg, true).unwrap_or_else(|| "39".to_string()));
            self.fg = Some(cell.fg);
        }
        if self.bg != Some(cell.bg) {
            parts.push(color_param(cell.bg, false).unwrap_or_else(|| "49".to_string()));
            self.bg = Some(cell.bg);
        }
        if self.mods != cell.modifier {
            mod_params(self.mods, cell.modifier, &mut parts);
            self.mods = cell.modifier;
        }
        if parts.is_empty() {
            return;
        }
        let _ = write!(self.out, "\x1b[{}m", parts.join(";"));
    }
}

/// 单元格是否为全默认样式（默认前景、默认背景、无修饰位）。
fn is_plain(cell: &Cell) -> bool {
    cell.fg == Color::Reset && cell.bg == Color::Reset && cell.modifier.is_empty()
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
            let width = UnicodeWidthStr::width(cell.symbol()).max(1) as u16;
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
        let (x, y) = crossterm::cursor::position()?;
        Ok(Position { x, y })
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
        self.mods = Modifier::empty();
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
        let (width, height) = crossterm::terminal::size()?;
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let ws = crossterm::terminal::window_size()?;
        Ok(WindowSize {
            columns_rows: Size {
                width: ws.columns,
                height: ws.rows,
            },
            pixels: Size {
                width: ws.width,
                height: ws.height,
            },
        })
    }

    /// X2-13a：一帧 = 一次 `write_all`（`out` 非空时）。空帧零写。
    fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        self.writer.write_all(&self.out)?;
        self.writer.flush()?;
        self.writes += 1;
        self.out.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
