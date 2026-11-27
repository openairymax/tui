// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-29a/b（§23.5.2 序 109/110）：超 1MiB 大帧的写出路径与完整性。
//
// 剖析与常规用例的帧都在 8KiB 量级，`frame_io` 的分片函数虽已被纯函数用例
// 覆盖，但「真实编码器编出 >1MiB 帧 → flush 分片 → 每片自包同步块 → 写出」
// 这条链路没有用例穿过。本文件用记录式写入口接住写出流，按同步块边界把
// 流重新拆回分片，与编码器持有的帧字节逐段比对：既验证片数不少于两片，
// 也验证关闭同步后写出流与源帧逐字节相同、首末字节不丢。

use super::*;
use std::sync::{Arc, Mutex};

/// 单个符号的字节数：取 64KiB，使 24 格即越过 1MiB 分片上限。
const BIG: usize = 64 * 1024;
const CELLS: u16 = 24;

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<u8>>>);

impl Rec {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for Rec {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn find(buf: &[u8], pat: &[u8]) -> Option<usize> {
    buf.windows(pat.len()).position(|w| w == pat)
}

/// 按同步块边界把写出流拆回分片；任何一片未闭合即为配对失败。
fn parts_of(out: &[u8]) -> Vec<Vec<u8>> {
    let mut parts = Vec::new();
    let mut rest = out;
    while !rest.is_empty() {
        assert!(rest.starts_with(frame_io::SYNC_ON), "分片缺少起始序列");
        rest = &rest[frame_io::SYNC_ON.len()..];
        let end = find(rest, SYNC_OFF).expect("分片未闭合");
        parts.push(rest[..end].to_vec());
        rest = &rest[end + SYNC_OFF.len()..];
    }
    parts
}

/// 超过 1MiB 的帧经 flush 后切成多片，逐片自包同步块且内容无缺失。
#[test]
fn megabyte_frame_splits_into_synced_parts() {
    let rec = Rec::default();
    let mut b = BufferedBackend::new(rec.clone());
    b.set_sync(true);
    let mut c = Cell::default();
    c.set_symbol(&"x".repeat(BIG));
    let iter = (0..CELLS).map(|i| (i, 0u16, &c));
    b.draw(iter).unwrap();
    let frame = b.frame_bytes().to_vec();
    assert!(
        frame.len() > MAX_RENDER_WRITE_CHARS,
        "帧仅 {} 字节，未越过分片上限",
        frame.len()
    );

    b.flush().unwrap();
    let parts = parts_of(&rec.bytes());
    assert!(parts.len() >= 2, "分片数 = {}", parts.len());
    assert!(
        parts.iter().all(|p| p.len() <= MAX_RENDER_WRITE_CHARS),
        "存在超过上限的分片"
    );
    assert_eq!(
        parts.iter().map(Vec::len).sum::<usize>(),
        frame.len(),
        "分片总长与帧长不等"
    );
    assert_eq!(parts.concat(), frame, "分片拼接后与源帧不一致");
}

/// 关闭同步时写出流与源帧逐字节相同，首末字节均不丢（X2-29b）。
#[test]
fn megabyte_frame_written_intact() {
    let rec = Rec::default();
    let mut b = BufferedBackend::new(rec.clone());
    let mut c = Cell::default();
    c.set_symbol(&"x".repeat(BIG));
    let iter = (0..CELLS).map(|i| (i, 0u16, &c));
    b.draw(iter).unwrap();
    let frame = b.frame_bytes().to_vec();
    b.flush().unwrap();

    let out = rec.bytes();
    assert_eq!(out.len(), frame.len(), "写出长度与帧长不等");
    assert_eq!(out, frame, "写出流与源帧不一致");
    assert!(out.starts_with(b"\x1b[1;1H"), "首段 = {:?}", &out[..4]);
    assert_eq!(out.last(), Some(&b'x'), "末字节丢失");
}
