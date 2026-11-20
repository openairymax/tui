// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 行日志落盘（0.1.21 X8-06b / X1-31a 共用）。
//
// 帧日志与事件录制都是旁路 NDJSON 通道：一行一条样本、按运行截断、按节拍
// 刷盘、触顶即停、写失败即停用。差别只在行的编码与文件名，故落盘机制收敛
// 于此一处，两个消费者各自只保留编码与校验，避免同一套打开/刷盘/熔断逻辑
// 各写一遍而逐渐分叉。
//
// 失败方向固定为「停止记录」：诊断通道不得因磁盘满或权限问题升级成渲染
// 错误，也不得每帧重复告警——首个错误 warn 一次即置 broken，此后 write()
// 直接返回假，调用方无需额外判断。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

/// 每 64 行刷一次盘：崩溃最多丢约一秒样本，又不让每行都等磁盘。
pub(crate) const FLUSH_EVERY: u32 = 64;

/// 单次运行行数上限：诊断通道是现场取证，不是长期遥测。
pub(crate) const MAX_LINES: u64 = 65_536;

/// 追加式行日志：一行一次写，按节拍刷盘。
pub(crate) struct LineLog {
    out: BufWriter<File>,
    pending: u32,
    written: u64,
    cap: u64,
    broken: bool,
}

impl LineLog {
    /// 建父目录并按运行截断打开；截断保证每次会话的行从首行连续。
    pub(crate) fn open(path: &Path, cap: u64) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            out: BufWriter::new(file),
            pending: 0,
            written: 0,
            cap,
            broken: false,
        })
    }

    /// 写入一行（自动补换行）；返回是否留存。触顶或已停用时返回假。
    pub(crate) fn write(&mut self, text: &str) -> bool {
        if self.broken || self.written >= self.cap {
            return false;
        }
        if let Err(e) = self.emit(text) {
            log::warn!("行日志写入失败，本进程停止记录：{e}");
            self.broken = true;
            return false;
        }
        self.written = self.written.saturating_add(1);
        self.pending += 1;
        if self.pending == FLUSH_EVERY {
            self.pending = 0;
            if let Err(e) = self.out.flush() {
                log::warn!("行日志刷盘失败，本进程停止记录：{e}");
                self.broken = true;
            }
        }
        true
    }

    /// 已留存行数：调用方据此给样本编号，无需自持计数器。
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    fn emit(&mut self, text: &str) -> std::io::Result<()> {
        self.out.write_all(text.as_bytes())?;
        self.out.write_all(b"\n")
    }
}

impl Drop for LineLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{LineLog, FLUSH_EVERY, MAX_LINES};
    use std::path::Path;

    fn body(path: &Path) -> String {
        std::fs::read_to_string(path).expect("日志文件已落盘")
    }

    /// 父目录不存在也能打开；按运行截断旧内容。
    #[test]
    fn open_creates_parents_and_truncates() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("a").join("b").join("x.jsonl");
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("预置目录");
        std::fs::write(&path, "旧内容\n").expect("预置旧文件");

        let mut log = LineLog::open(&path, MAX_LINES).expect("打开日志");
        assert!(log.write("新行"));
        drop(log);
        assert_eq!(body(&path), "新行\n", "旧内容须被截断");
    }

    /// 触顶即停：超出上限的行不留存，文件行数等于上限。
    #[test]
    fn write_stops_at_capacity() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("cap.jsonl");
        let mut log = LineLog::open(&path, 3).expect("打开日志");
        for i in 1..=5u32 {
            let kept = log.write(&format!("line{i}"));
            assert_eq!(kept, i <= 3, "第 {i} 行的留存判定");
        }
        assert_eq!(log.written(), 3);
        drop(log);
        assert_eq!(body(&path), "line1\nline2\nline3\n");
    }

    /// 刷盘节拍：达到 64 行时内容已落盘，未到下一节拍则维持上次刷盘量。
    /// 这是「崩溃最多丢约一秒」承诺的直接证据。
    #[test]
    fn flush_every_bounds_crash_loss() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("flush.jsonl");
        let mut log = LineLog::open(&path, MAX_LINES).expect("打开日志");
        for i in 1..=FLUSH_EVERY {
            assert!(log.write(&format!("n{i}")));
        }
        assert_eq!(
            body(&path).lines().count(),
            FLUSH_EVERY as usize,
            "到达节拍须刷盘"
        );
        assert!(log.write("n65"));
        assert_eq!(
            body(&path).lines().count(),
            FLUSH_EVERY as usize,
            "未到下一节拍仍是上次刷盘量"
        );
    }
}
