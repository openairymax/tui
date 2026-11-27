// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧写出策略（X2-11a/b、X2-14a、X2-15b、X2-30a）：把编码好的帧字节交给
// writer。
//
// 编码与写出分属两层：`backend` 只看单元格、只往帧缓冲里追加字节；本模块
// 只看帧缓冲、只决定「分几段、包不包同步块、写几次」。分片与同步块的纯计算
// 在 `frame_io`，此处只做「按策略取段并写出」，故写出策略的每一步都能被
// 记录式 writer 直接观测，无需真实终端。
//
// 四档策略与降级阶梯一一对应（`ladder::Ladder`）：合帧、单写、逐格、纯
// 文本。档位由 `backend` 的 flush 依失败计数推进，本模块只按档位分派，不在
// 此判断「何时降级」——判据属策略，路径属机制。
//
// 作为 `backend` 的子模块挂载（`#[path]`），与编码器共享同一组私有字段：
// 帧缓冲、单元格边界、失败计数与写出计数。分开成文件是为了让编码器与写出
// 器各自留在可读的行数内，而不是为了隔断状态。

use std::io::{self, Write};

use crate::engine::frame_io;
use crate::engine::ladder::Ladder;
use crate::budget::MAX_RENDER_WRITE_CHARS;

use super::BufferedBackend;

impl<W: Write> BufferedBackend<W> {
    /// 按当前档位写出本帧。
    pub(super) fn write_frame(&mut self) -> io::Result<()> {
        match self.level {
            Ladder::Fused => self.write_batch(),
            Ladder::Batch => self.write_single(),
            Ladder::Cell => self.write_cells(),
            Ladder::Text => self.write_plain(),
        }
    }

    /// 合帧（L1）：切分（X2-11a/b）后按片上同步块（X2-14a），一次写出。
    fn write_batch(&mut self) -> io::Result<()> {
        let parts = frame_io::split(&self.out, MAX_RENDER_WRITE_CHARS);
        let batch = frame_io::wrap(&self.out, &parts, self.sync);
        self.writer.write_all(&batch)?;
        self.writer.flush()?;
        self.writes += 1;
        Ok(())
    }

    /// 单写（L2）：整帧一次写出，去掉切片与同步包裹。
    ///
    /// 降级到此档说明失败点可能就在同步包裹或分片拼接上：每片多出的两个
    /// 写动作都是额外的失败面，去掉后只剩「把这段字节写出去」。
    fn write_single(&mut self) -> io::Result<()> {
        self.writer.write_all(&self.out)?;
        self.writer.flush()?;
        self.writes += 1;
        Ok(())
    }

    /// 逐格（L3）：按编码时的单元格边界逐段写出，段间不合并（X2-15b）。
    ///
    /// 单元格标记只记录单元格起点，首个标记之前（如清屏）与末个标记之后
    /// 的字节同样必须写出，故按「前缀 + 各格 + 尾段」遍历，空标记则整帧
    /// 一段。任何一段失败即返回，剩余内容留给下一次重试。
    fn write_cells(&mut self) -> io::Result<()> {
        let done = self.each_chunk(|b, start, end| {
            if b.sync {
                b.writer.write_all(frame_io::SYNC_ON)?;
            }
            b.writer.write_all(&b.out[start..end])?;
            if b.sync {
                b.writer.write_all(frame_io::SYNC_OFF)?;
            }
            Ok(())
        });
        done?;
        self.writer.flush()
    }

    /// 纯文本（L4）：逐格且剥掉样式 SGR，只留定位与字符。
    ///
    /// 样式是增强项，字符与位置是底线：终端在重压或异常态下对长序列的
    /// 解析更脆弱，剥掉 SGR 后每段的字节量最小，也最容易整段写出。此档
    /// 不再包裹同步块——同步块本身是额外的写动作，与「最小可写」相反。
    fn write_plain(&mut self) -> io::Result<()> {
        let done = self.each_chunk(|b, start, end| {
            let plain = frame_io::strip_style(&b.out[start..end]);
            b.writer.write_all(&plain)
        });
        done?;
        self.writer.flush()
    }

    /// 按单元格边界遍历帧字节，逐段交给 `emit`。
    ///
    /// `emit` 只负责写出一段，段内是否包裹同步块由各档自行决定：L3 保留
    /// 同步语义（每段自包一对 `h/l`），L4 直接写裸字节。段内失败即返回，
    /// 标记与帧缓冲原样保留，下一次 flush 仍可按同样的边界重投。
    fn each_chunk<F>(&mut self, emit: F) -> io::Result<()>
    where
        F: Fn(&mut Self, usize, usize) -> io::Result<()>,
    {
        let len = self.out.len();
        let mut start = 0;
        let mut i = 0;
        while i < self.marks.len() {
            let end = self.marks[i];
            i += 1;
            if end > start && end <= len {
                emit(self, start, end)?;
                self.writes += 1;
                start = end;
            }
        }
        if start < len {
            emit(self, start, len)?;
            self.writes += 1;
        }
        Ok(())
    }
}
