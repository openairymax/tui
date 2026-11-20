// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧过滤（0.1.21 X1-04a/X1-04b，§23.1.5 事件入口的语法层）。
//
// 行读取器只保证「按行交付」，不保证每行都是帧：调试串、被截断的半截帧、
// 上游的杂散输出都可能混进同一条管道。本模块在语法层逐行判定——不是 JSON
// 对象即视为坏帧，跳过并计数，读取游标照常前进。一帧坏掉不许拖垮整条链路：
// 渲染进程一旦退出，用户连看见错误的机会都没有。
//
// 坏帧不整条留存（一条恶意长帧就足以把审计变成第二个内存缺口），只把长度与
// 首末各至多 8 字节放进定长审计：足以认出「是调试串还是半截帧」，又不会比
// 帧本身更大。审计对外只读（`audits()` 借出切片），消费方无法回写。
//
// 只判语法，不判语义：`schema`/`seq`/`type`/`payload` 的结构校验在 X1-05。
// 跳过的帧不会悄悄消失——总数由 `bad()` 暴露，明细由 `audits()` 给出。

#![cfg_attr(not(test), allow(dead_code))]

use std::io::{self, BufRead};

use serde_json::Value;

use super::lines::{Line, Lines};

/// 审计条目保留的首/末字节数。
const EDGE: usize = 8;

/// 审计环容量：只保留最近若干条坏帧。
const AUDIT_CAP: usize = 16;

/// 一条坏帧的审计记录：原始长度与首末字节（各至多 `EDGE` 字节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bad {
    pub(crate) len: usize,
    pub(crate) head: Vec<u8>,
    pub(crate) tail: Vec<u8>,
    pub(crate) truncated: bool,
}

impl Bad {
    /// 从原始行字节截取首末片段；截断行如实标记，避免把 1 MiB 处的断口
    /// 误当成帧尾。
    fn of(raw: &[u8], truncated: bool) -> Self {
        Self {
            len: raw.len(),
            head: raw[..raw.len().min(EDGE)].to_vec(),
            tail: raw[raw.len().saturating_sub(EDGE)..].to_vec(),
            truncated,
        }
    }
}

/// 帧过滤器：逐行判语法，坏帧跳过、计数并记审计。
pub(crate) struct Frames<R> {
    lines: Lines<R>,
    bad: u64,
    audit: Vec<Bad>,
}

impl<R: BufRead> Frames<R> {
    pub(crate) fn new(src: R) -> Self {
        Self {
            lines: Lines::new(src),
            bad: 0,
            audit: Vec::new(),
        }
    }

    /// 下一帧合法 JSON 对象；坏帧跳过并计数，上游结束返回 `None`。
    ///
    /// 顺序即上游顺序：过滤只做删除，不做重排、不补帧。
    pub(crate) fn next(&mut self) -> io::Result<Option<Value>> {
        while let Some(line) = self.lines.next()? {
            if let Some(frame) = parse(&line) {
                return Ok(Some(frame));
            }
            self.bad = self.bad.saturating_add(1);
            self.keep(&line);
        }
        Ok(None)
    }

    /// 已跳过的坏帧数（含被截断的行）。
    pub(crate) fn bad(&self) -> u64 {
        self.bad
    }

    /// 最近若干条坏帧的只读审计（旧在前）。
    pub(crate) fn audits(&self) -> &[Bad] {
        &self.audit
    }

    /// 追加审计条目，满则挤掉最旧的一条。
    fn keep(&mut self, line: &Line) {
        if self.audit.len() == AUDIT_CAP {
            self.audit.remove(0);
        }
        self.audit.push(Bad::of(&line.bytes, line.truncated));
    }
}

/// 单行 → 帧。空白行、非 JSON、非对象、被截断的行都不算帧。
fn parse(line: &Line) -> Option<Value> {
    if line.truncated {
        return None;
    }
    serde_json::from_str::<Value>(&line.text())
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::{Frames, AUDIT_CAP};
    use crate::engine::event::lines::MAX_LINE;
    use serde_json::Value;
    use std::io::Cursor;

    fn drain(data: &[u8]) -> (Vec<Value>, u64) {
        let mut src = Frames::new(Cursor::new(data.to_vec()));
        let mut out = Vec::new();
        while let Some(frame) = src.next().expect("读取不报错") {
            out.push(frame);
        }
        (out, src.bad())
    }

    /// 杂散输出、空行、非对象 JSON 都按坏帧跳过，好帧顺序不变。
    #[test]
    fn skips_noise_and_counts() {
        let (got, bad) = drain(b"{\"seq\":1}\nhello\n{\"seq\":2}\n\n[1,2]\n{\"seq\":3}");
        assert_eq!(got.len(), 3);
        assert_eq!(bad, 3, "杂散行 / 空行 / 数组均为坏帧");
        let seqs: Vec<_> = got
            .iter()
            .map(|v| v["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "过滤不得重排或丢好帧");
    }

    /// 干净流不产生坏帧计数。
    #[test]
    fn clean_stream_counts_nothing() {
        let (got, bad) = drain(b"{\"seq\":1}\n{\"seq\":2}\n");
        assert_eq!(got.len(), 2);
        assert_eq!(bad, 0);
    }

    /// 空流立即结束，且后续读取恒为 `None`。
    #[test]
    fn empty_stream_ends_at_once() {
        let mut src = Frames::new(Cursor::new(Vec::new()));
        assert!(src.next().expect("读取不报错").is_none());
        assert!(src.next().expect("读取不报错").is_none());
        assert_eq!(src.bad(), 0);
    }

    /// 超界行即便文本开头像帧也按坏帧处置，后续好帧照常交付。
    #[test]
    fn truncated_line_is_bad() {
        let mut data = b"{\"p\":\"".to_vec();
        data.resize(MAX_LINE + 10, b'x');
        data.push(b'\n');
        data.extend_from_slice(b"{\"seq\":7}\n");
        let (got, bad) = drain(&data);
        assert_eq!(bad, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["seq"], Value::from(7));
    }

    /// 帧前后的空白不影响解析（JSON 语法本身允许）。
    #[test]
    fn tolerates_surrounding_space() {
        let (got, bad) = drain(b"  {\"seq\":1}  \n");
        assert_eq!(got.len(), 1);
        assert_eq!(bad, 0);
    }

    /// 审计记下原始长度与首末各至多 8 字节，足以分辨「调试串」。
    #[test]
    fn audit_records_edges() {
        let mut src = Frames::new(Cursor::new(b"XYZ garbage\n".to_vec()));
        assert!(src.next().expect("读取不报错").is_none());
        let audit = src.audits();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].len, 11);
        assert_eq!(audit[0].head, b"XYZ garb".to_vec());
        assert_eq!(audit[0].tail, b" garbage".to_vec());
        assert!(!audit[0].truncated);
    }

    /// 审计是定长环：只留最近 `AUDIT_CAP` 条，总量仍按实数累计。
    #[test]
    fn audit_ring_keeps_latest() {
        let total = AUDIT_CAP + 4;
        let mut data = Vec::new();
        for i in 0..total {
            data.extend_from_slice(format!("bad-{i}\n").as_bytes());
        }
        let mut src = Frames::new(Cursor::new(data));
        assert!(src.next().expect("读取不报错").is_none());
        assert_eq!(src.bad(), total as u64);

        let audit = src.audits();
        assert_eq!(audit.len(), AUDIT_CAP);
        assert_eq!(audit[0].head, b"bad-4".to_vec(), "挤掉的应是最旧条目");
        assert_eq!(
            audit[AUDIT_CAP - 1].head,
            format!("bad-{}", total - 1).into_bytes()
        );
    }

    /// 被截断的行如实标记，长度按上界计，不把断口当帧尾。
    #[test]
    fn audit_marks_truncated() {
        let mut data = vec![b'{'; MAX_LINE + 4];
        data.push(b'\n');
        let mut src = Frames::new(Cursor::new(data));
        assert!(src.next().expect("读取不报错").is_none());
        let entry = &src.audits()[0];
        assert!(entry.truncated);
        assert_eq!(entry.len, MAX_LINE);
        assert_eq!(entry.tail.len(), 8);
    }

    /// 任意字节都不许 panic：全字节表（去掉换行与 CR）、NUL、半截 UTF-8
    /// 都在坏帧之列，之后的好帧照常交付。
    #[test]
    fn garbage_never_panics() {
        let mut data: Vec<u8> = (0..=255u8).filter(|b| *b != b'\n' && *b != b'\r').collect();
        data.extend_from_slice(b"\n\x00\x00\xFF\xFE\x80\n");
        data.extend_from_slice(&[0xF0, 0x9F, 0x92]);
        data.push(b'\n');
        data.extend_from_slice(b"{\"seq\":42}\n");

        let (got, bad) = drain(&data);
        assert_eq!(bad, 3);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["seq"], Value::from(42));
    }
}
