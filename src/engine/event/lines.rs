// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// NDJSON 行读取器（0.1.21 X1-02a，§23.1.5 事件入口的读侧）。
//
// D-15 约定 stdout 一行一帧，但管道读到的字节块与行边界无关：一次 read 可能
// 只拿到半行，也可能一口气带回多行。本模块把字节流切成整行——残行保存在
// `pend` 里跨块拼接，只有见到换行（或上游 EOF）才产出，绝不把半行交给下游。
// 行尾 CR 一律剥掉：CLI 只写 LF，但 Windows 侧若经文本模式中转会出现 CRLF，
// 剥除对合法帧无损，对非法帧的判断留给 X1-04。
//
// 只做切分，不做语义：不判 JSON、不限长度、不计序号。逐行上限与超限丢弃在
// X1-03（因此这里用逐块扫描而非 read_until：后者无上界，遇恶意长行会一路吃到
// OOM）；UTF-8 处置见下。

#![cfg_attr(not(test), allow(dead_code))]

use std::io::{self, BufRead};

/// 逐行切分器：包装任意 `BufRead`，产出不含行尾的整行字节。
pub(crate) struct Lines<R> {
    src: R,
    pend: Vec<u8>,
}

impl<R: BufRead> Lines<R> {
    pub(crate) fn new(src: R) -> Self {
        Self {
            src,
            pend: Vec::new(),
        }
    }

    /// 产出下一行文本：非法或截断的 UTF-8 序列以 U+FFFD 顶替，不 panic、
    /// 不吞行，后续行照常产出。是否为坏帧归 X1-04 判（它拿到的是同一行的
    /// 文本，不再关心编码细节）。
    ///
    /// 「半字符回推」发生在字节层：换行是单字节 ASCII，字符边界永远不会与
    /// 行边界冲突；跨读被切开的半个字符由 `pend` 原样保留，整行到齐后再解码，
    /// 因此无需再维护一份文本解码器的暂存状态。对端真的把字符写残时，唯一
    /// 正确的处置就是顶替——把残字节推到下一行只会污染下一帧。
    pub(crate) fn text(&mut self) -> io::Result<Option<String>> {
        Ok(self
            .next()?
            .map(|raw| String::from_utf8_lossy(&raw).into_owned()))
    }

    /// 产出下一整行；上游结束且无残行时返回 `None`。
    ///
    /// EOF 前最后一行即使没有换行也照常产出（对端可能写完就退出），空白行
    /// 产出空切片而不是被跳过——空行与「流结束」是两种不同的事实。
    pub(crate) fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let chunk = self.src.fill_buf()?;
            if chunk.is_empty() {
                return if self.pend.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(strip_cr(std::mem::take(&mut self.pend))))
                };
            }
            match chunk.iter().position(|b| *b == b'\n') {
                Some(at) => {
                    self.pend.extend_from_slice(&chunk[..at]);
                    self.src.consume(at + 1);
                    return Ok(Some(strip_cr(std::mem::take(&mut self.pend))));
                }
                None => {
                    let len = chunk.len();
                    self.pend.extend_from_slice(chunk);
                    self.src.consume(len);
                }
            }
        }
    }
}

/// 剥掉行尾的 CR（CRLF 归一为 LF 语义）。
fn strip_cr(mut line: Vec<u8>) -> Vec<u8> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::Lines;
    use std::io::{BufReader, Cursor, Read};

    /// 每次 read 只交出一个字节的源：逼出「半行跨块拼接」的最坏形态。
    struct Drip {
        data: Vec<u8>,
        pos: usize,
    }

    impl Read for Drip {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.data[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    fn lines(data: &[u8]) -> Vec<Vec<u8>> {
        let mut src = Lines::new(BufReader::with_capacity(3, Cursor::new(data.to_vec())));
        let mut out = Vec::new();
        while let Some(line) = src.next().expect("读取不报错") {
            out.push(line);
        }
        out
    }

    /// 行长大于读缓冲：多行依次完整产出，顺序与原文一致。
    #[test]
    fn splits_lines_across_reads() {
        let got = lines(b"{\"seq\":1}\n{\"seq\":2,\"long\":\"0123456789\"}\n");
        assert_eq!(
            got,
            vec![
                b"{\"seq\":1}".to_vec(),
                b"{\"seq\":2,\"long\":\"0123456789\"}".to_vec()
            ]
        );
    }

    /// 一字节一读：跨块拼接与缓冲容量无关。
    #[test]
    fn joins_lines_from_single_byte_reads() {
        let mut src = Lines::new(BufReader::new(Drip {
            data: b"first\nsecond\n".to_vec(),
            pos: 0,
        }));
        assert_eq!(src.next().expect("可读"), Some(b"first".to_vec()));
        assert_eq!(src.next().expect("可读"), Some(b"second".to_vec()));
        assert_eq!(src.next().expect("可读"), None);
    }

    /// 末行无换行也要产出（对端写完即退），且其后恒为 None。
    #[test]
    fn yields_tail_without_newline() {
        let mut src = Lines::new(Cursor::new(b"a\nb".to_vec()));
        assert_eq!(src.next().expect("可读"), Some(b"a".to_vec()));
        assert_eq!(src.next().expect("可读"), Some(b"b".to_vec()));
        assert_eq!(src.next().expect("可读"), None);
        assert_eq!(src.next().expect("可读"), None, "EOF 须可重复判定");
    }

    /// 空行是事实而非结束：保留为空切片，空输入才是 None。
    #[test]
    fn keeps_blank_lines() {
        let got = lines(b"\n\nx\n");
        assert_eq!(got, vec![Vec::new(), Vec::new(), b"x".to_vec()]);
        assert!(lines(b"").is_empty(), "空输入不产出行");
    }

    /// CRLF 归一：行尾 CR 不进入下游。
    #[test]
    fn strips_carriage_return() {
        let got = lines(b"a\r\nb\r\n");
        assert_eq!(got, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    /// 多字节字符被读边界切断时，整行仍逐字节还原（拆分点不吞字节）。
    #[test]
    fn keeps_multibyte_bytes_intact() {
        let text = "中文：完成\n".as_bytes();
        let got = lines(text);
        assert_eq!(got, vec![text[..text.len() - 1].to_vec()]);
    }

    /// 读缓冲（3 字节）正好把「流」切开：文本层仍拿到完整字符。
    #[test]
    fn text_survives_multibyte_split() {
        let mut src = Lines::new(BufReader::with_capacity(
            3,
            Cursor::new("流式：完成\n".as_bytes().to_vec()),
        ));
        assert_eq!(src.text().expect("可读").as_deref(), Some("流式：完成"));
        assert_eq!(src.text().expect("可读"), None);
    }

    /// 行尾字符被对端写残（只有前导字节）：顶替为 U+FFFD，下一行不受牵连。
    #[test]
    fn text_replaces_truncated_tail() {
        let raw = vec![b'a', 0xE4, 0xB8, b'\n', b'b', b'\n'];
        let mut src = Lines::new(Cursor::new(raw));
        assert_eq!(src.text().expect("可读").as_deref(), Some("a\u{FFFD}"));
        assert_eq!(src.text().expect("可读").as_deref(), Some("b"));
    }

    /// 非法字节（非 UTF-8 前导）：整行顶替而不中断流。
    #[test]
    fn text_replaces_invalid_bytes() {
        let raw = vec![0xFF, b'\n', b'x', b'\n'];
        let mut src = Lines::new(Cursor::new(raw));
        assert_eq!(src.text().expect("可读").as_deref(), Some("\u{FFFD}"));
        assert_eq!(src.text().expect("可读").as_deref(), Some("x"));
    }
}
