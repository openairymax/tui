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
// 帧编码器（X2-10）在本文件；分片与同步块的纯计算在 frame_io；单批写出
// 与失败回退（X2-11/X2-12/X2-14/X2-15）在 backend_write。编码策略与写出
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
use super::backend_sgr::{color_param, mod_params};
use super::frame_io::SYNC_OFF;
use super::ladder::Ladder;

/// 写出策略子模块：与编码器共用私有字段，单独成文以控制单体行数。
#[path = "backend_write.rs"]
mod write;

/// 同一档位连续写失败达此数即降一档（X2-15a、X2-30a）。
const FAIL_LIMIT: u8 = 3;

/// 同一行内改用前移（CUF）的最大间距：超过即绝对定位。
///
/// X2-28b 的实测取舍：单数字前移 `ESC[nC` 恒 4 字节，双列绝对定位
/// `ESC[r;cH` 为 6~7 字节，三列起绝对定位还要多带一位列号。故间距在
/// 9 列（前移仍单数字）以内一律前移，跨过 9 列改绝对定位，避免两位数
/// 前移与列号长度相互追平后失去收益。同行目标的列号必然落在终端宽度
/// 之内，前移不会越过行尾折行。
const CUF_MAX: u16 = 9;

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
    /// 写出降级档位（X2-30a）：只降不升，重建后端即回默认档。
    level: Ladder,
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
            level: Ladder::Fused,
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

    /// 当前写出档位（X2-30a）。
    pub(crate) fn level(&self) -> Ladder {
        self.level
    }

    /// 是否已离开默认档（X2-15b 判据）。
    pub(crate) fn degraded(&self) -> bool {
        !self.level.is_fused()
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
    /// 用绝对定位。间距从**写入后的真实光标**起算（见 `draw` 的游程推进），
    /// 否则宽字符之后的空隙会多算一列，后续字符整体右移。光标已在目标
    /// 位置时不产生任何字节。
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
    ///
    /// X2-28b 快路径：三路与已生效状态完全一致时立即返回。逐格编码里这是
    /// 绝对多数路径，放在最前可免去每次新建参数表与拼接——样式未变时
    /// 不分配、不产生任何字节。
    fn emit_style(&mut self, cell: &Cell) {
        if self.fg == Some(cell.fg) && self.bg == Some(cell.bg) && self.mods == cell.modifier {
            return;
        }
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

    /// 本帧已交出：清空帧缓冲与失败计数，下一帧重新起算。
    fn settle(&mut self) {
        self.out.clear();
        self.marks.clear();
        self.fails = 0;
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
                } else {
                    // 续格不在本帧游程里：光标位置无从推断，下一个实格改
                    // 绝对定位，避免按陈旧位置前移而错位。
                    self.cursor = None;
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
            // 按符号真实显示宽度推进（宽字符 2 列，零宽按 1 列兜底），并把
            // 该位置登记为光标缓存：下一处空隙的前移距离由此起算。
            let width = super::grid::width(cell.symbol()).max(1) as u16;
            next = Some(Position {
                x: x.saturating_add(width),
                y,
            });
            self.cursor = next;
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
            other => Err(io::Error::other(format!(
                "clear_type [{other:?}] not supported with this backend"
            ))),
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

    /// 一帧一次写出（X2-12a/b）：默认档把分片按序拼进同一批次，同步开启
    /// 时每片自包一对 `h/l`（X2-14a）。空帧零写。
    ///
    /// 失败即计数，同一档连续失败达阈值降一档（X2-15a、X2-30a），并立即
    /// 用新档重投本次内容；档位只降不升，最弱档封顶。同步块已开而写途中
    /// 失败时，先补发关闭序列，避免残留在同步态。
    fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        match self.write_frame() {
            Ok(()) => {
                self.settle();
                Ok(())
            }
            Err(e) => {
                self.fails = self.fails.saturating_add(1);
                if self.sync {
                    let _ = self.writer.write_all(SYNC_OFF);
                    let _ = self.writer.flush();
                }
                if self.fails < FAIL_LIMIT {
                    return Err(e);
                }
                let next = self.level.next();
                if next == self.level {
                    return Err(e);
                }
                self.level = next;
                self.fails = 0;
                match self.write_frame() {
                    Ok(()) => {
                        self.settle();
                        Ok(())
                    }
                    Err(e) => {
                        self.fails = 1;
                        Err(e)
                    }
                }
            }
        }
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

/// 写出档位的只读出口（X2-30b）：帧指标按帧记录当前档位。
///
/// 与 `SyncCaps` 分开：能力施加是初始化动作，档位读取是每帧动作；测试
/// 后端无需实现它，也就不会为一个只读查询被迫接受同步包裹。
pub(crate) trait WriteLevel {
    fn write_level(&self) -> Ladder;
}

impl<W: Write> WriteLevel for BufferedBackend<W> {
    fn write_level(&self) -> Ladder {
        self.level
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

#[cfg(test)]
#[path = "backend_pos_tests.rs"]
mod pos_tests;

#[cfg(test)]
#[path = "backend_io_tests.rs"]
mod io_tests;

#[cfg(test)]
#[path = "backend_ladder_tests.rs"]
mod ladder_tests;
