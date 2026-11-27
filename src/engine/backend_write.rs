// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧写出策略（X2-11a/b、X2-14a、X2-15b）：把编码好的帧字节交给 writer。
//
// 编码与写出分属两层：`backend` 只看单元格、只往帧缓冲里追加字节；本模块
// 只看帧缓冲、只决定「分几段、包不包同步块、写几次」。分片与同步块的纯计算
// 在 `frame_io`，此处只做「按策略取段并写出」，故写出策略的每一步都能被
// 记录式 writer 直接观测，无需真实终端。
//
// 作为 `backend` 的子模块挂载（`#[path]`），与编码器共享同一组私有字段：
// 帧缓冲、单元格边界、失败计数与写出计数。分开成文件是为了让编码器与写出
// 器各自留在可读的行数内，而不是为了隔断状态。

use std::io::{self, Write};

use super::frame_io;
use super::BufferedBackend;
use crate::budget::MAX_RENDER_WRITE_CHARS;

impl<W: Write> BufferedBackend<W> {
    /// 整帧批次：切分（X2-11a/b）后按片上同步块（X2-14a），一次写出。
    pub(super) fn write_batch(&mut self) -> io::Result<()> {
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
    pub(super) fn write_cells(&mut self) -> io::Result<()> {
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
            self.writer.write_all(frame_io::SYNC_OFF)?;
        }
        self.writes += 1;
        Ok(())
    }
}
