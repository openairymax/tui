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
// 游程合并（X2-07）、样式游程与最短 SGR（X2-08/X2-09）、三流合一的
// 帧编码器（X2-10）、分片与同步块（X2-11/X2-13/X2-14）、单批写出与
// 失败回退（X2-12/X2-15）均在本文件与 frame_io 落位；编码策略与写出
// 策略分离，后续卡只换内部实现，不动 Backend 接口。
//
// 接线（X2-12a）：entry 的终端构造已换成
// `Terminal::new(BufferedBackend::new(stdout))`，能力探测结论经
// `SyncCaps::apply_sync` 落到同步开关上；原 `term::Bounded` 包装已删除，
// 它按任意字节数截断会切断同步块。测量访问器（写次数、定位次数、失败
// 计数、降级态）只供用例断言，故本模块允许它们在生产构建中未被读取。

#![allow(dead_code)] // 测量访问器仅测试读取

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use super::backend_sgr::{color_param, mod_params};
use super::frame_io::{self, SYNC_OFF};
use crate::budget::MAX_RENDER_WRITE_CHARS;

/// 连续写失败达此数即回退逐格写（X2-15a）。
const FAIL_LIMIT: u8 = 3;

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
    /// 同步输出开关（X2-13a）：由能力探测结论在接线时设置。
    sync: bool,
    /// 连续写失败次数（X2-15a）。
    fails: u8,
    /// 逐格回退模式（X2-15b）：连续失败达阈值后置位。
    degraded: bool,
    /// 每个单元格编码起点在 `out` 中的偏移，是逐格回退的分片边界。
    marks: Vec<usize>,
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
            sync: false,
            fails: 0,
            degraded: false,
            marks: Vec::new(),
        }
    }

    /// 开启/关闭同步输出包裹（X2-13a）；结论来自能力探测。
    pub(crate) fn set_sync(&mut self, on: bool) {
        self.sync = on;
    }

    /// 当前是否包裹同步块。
    pub(crate) fn sync_enabled(&self) -> bool {
        self.sync
    }

    /// 连续写失败次数（X2-15a 判据用）。
    pub(crate) fn fail_count(&self) -> u8 {
        self.fails
    }

    /// 是否已回退逐格写。
    pub(crate) fn degraded(&self) -> bool {
        self.degraded
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
    /// 帧编码器主干（X2-10a）：移动、样式、内容三流按单元格顺序合入
    /// 同一缓冲，写出顺序即终端解析顺序；本函数不做任何 IO。
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
            // 逐格回退的分片边界：本格的全部字节从当前偏移开始。
            self.marks.push(self.out.len());
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
        self.marks.clear();
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

    /// 一帧 = 一次 `write_all`（X2-12a/b）：全部分片按序拼进同一批次，
    /// 同步开启时每片自包一对 `h/l`（X2-14a）。空帧零写。
    ///
    /// 失败计数达阈值即切逐格回退（X2-15a/b），并立即用逐格路径重投本次
    /// 内容；同步块已开而写途中失败时，先补发关闭序列，避免残留在同步态。
    fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        let first = if self.degraded {
            self.write_cells()
        } else {
            self.write_batch()
        };
        match first {
            Ok(()) => {
                self.out.clear();
                self.marks.clear();
                self.fails = 0;
                Ok(())
            }
            Err(e) => {
                self.fails = self.fails.saturating_add(1);
                if self.sync {
                    let _ = self.writer.write_all(SYNC_OFF);
                    let _ = self.writer.flush();
                }
                if self.fails >= FAIL_LIMIT && !self.degraded {
                    self.degraded = true;
                    if self.write_cells().is_ok() {
                        self.out.clear();
                        self.marks.clear();
                        self.fails = 0;
                        return Ok(());
                    }
                }
                Err(e)
            }
        }
    }
}

impl<W: Write> BufferedBackend<W> {
    /// 整帧批次：切分（X2-11a/b）后按片上同步块（X2-14a），一次写出。
    fn write_batch(&mut self) -> io::Result<()> {
        let parts = frame_io::split(&self.out, MAX_RENDER_WRITE_CHARS);
        let batch = frame_io::wrap(&self.out, &parts, self.sync);
        self.writer.write_all(&batch)?;
        self.writer.flush()?;
        self.writes += 1;
        Ok(())
    }

    /// 逐格回退（X2-15b）：按编码时的单元格边界逐段写出，段间不合并。
    ///
    /// 单元格标记只记录单元格起点，首个标记之前（如清屏）与末个标记之后
    /// 的字节同样必须写出，故按「前缀 + 各格 + 尾段」遍历，空标记则整帧
    /// 一段。任何一段失败即返回，剩余内容留给下一次重试。
    fn write_cells(&mut self) -> io::Result<()> {
        let len = self.out.len();
        let mut start = 0;
        for i in 0..self.marks.len() {
            let end = self.marks[i];
            if end > start && end <= len {
                self.write_chunk(start, end)?;
                start = end;
            }
        }
        self.write_chunk(start, len)?;
        self.writer.flush()
    }

    /// 写出 `out[start..end]` 一段，同步开启时自包一对 `h/l`。
    fn write_chunk(&mut self, start: usize, end: usize) -> io::Result<()> {
        if start >= end || end > self.out.len() {
            return Ok(());
        }
        if self.sync {
            self.writer.write_all(frame_io::SYNC_ON)?;
        }
        self.writer.write_all(&self.out[start..end])?;
        if self.sync {
            self.writer.write_all(SYNC_OFF)?;
        }
        self.writes += 1;
        Ok(())
    }
}

/// 同步输出能力施加点（X2-13a）：终端能力只有落到后端开关才真正生效。
///
/// 单独成 trait 而不并进 `Backend`：`Backend` 是对 ratatui 的契约，同步
/// 包裹是本项目对帧写出的增强，二者生命周期不同——测试后端无需实现它。
pub(crate) trait SyncCaps {
    fn apply_sync(&mut self, on: bool);
}

impl<W: Write> SyncCaps for BufferedBackend<W> {
    fn apply_sync(&mut self, on: bool) {
        self.set_sync(on);
    }
}

impl<W: Write> Drop for BufferedBackend<W> {
    /// 异常路径兜底（X2-13b）：带未写内容退出时补发关闭序列，终端不残留
    /// 同步态。正常 flush 成功后 `out` 为空，此路径不产生任何字节。
    fn drop(&mut self) {
        if self.sync && !self.out.is_empty() {
            let _ = self.writer.write_all(SYNC_OFF);
            let _ = self.writer.flush();
        }
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
