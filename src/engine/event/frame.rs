// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧过滤（0.1.21 X1-04a，§23.1.5 事件入口的语法层）。
//
// 行读取器只保证「按行交付」，不保证每行都是帧：调试串、被截断的半截帧、
// 上游的杂散输出都可能混进同一条管道。本模块在语法层逐行判定——不是 JSON
// 对象即视为坏帧，跳过并计数，读取游标照常前进。一帧坏掉不许拖垮整条链路：
// 渲染进程一旦退出，用户连看见错误的机会都没有。
//
// 只判语法，不判语义：`schema`/`seq`/`type`/`payload` 的结构校验在 X1-05，
// 首末字节的只读审计在 X1-04b。跳过的帧不会悄悄消失——计数由 `bad()` 暴露，
// 上层可在状态栏呈现「n 帧被丢弃」。

#![cfg_attr(not(test), allow(dead_code))]

use std::io::{self, BufRead};

use serde_json::Value;

use super::lines::{Line, Lines};

/// 帧过滤器：逐行判语法，坏帧跳过并累计。
pub(crate) struct Frames<R> {
    lines: Lines<R>,
    bad: u64,
}

impl<R: BufRead> Frames<R> {
    pub(crate) fn new(src: R) -> Self {
        Self {
            lines: Lines::new(src),
            bad: 0,
        }
    }

    /// 下一帧合法 JSON 对象；坏帧跳过并计数，上游结束返回 `None`。
    ///
    /// 顺序即上游顺序：过滤只做删除，不做重排、不补帧。
    pub(crate) fn next(&mut self) -> io::Result<Option<Value>> {
        while let Some(line) = self.lines.next()? {
            if let Some(frame) = parse(line) {
                return Ok(Some(frame));
            }
            self.bad = self.bad.saturating_add(1);
        }
        Ok(None)
    }

    /// 已跳过的坏帧数（含被截断的行）。
    pub(crate) fn bad(&self) -> u64 {
        self.bad
    }
}

/// 单行 → 帧。空白行、非 JSON、非对象、被截断的行都不算帧。
fn parse(line: Line) -> Option<Value> {
    if line.truncated {
        return None;
    }
    serde_json::from_str::<Value>(&line.text())
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::Frames;
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
}
