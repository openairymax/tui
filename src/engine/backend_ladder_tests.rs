// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 写出降级阶梯的触发用例（0.1.21 X2-30a/b，§23.5.2 序 111/112）。
//
// 四档必须各自可达，判据是「档位 + 写出内容」两件事实同时成立：档位证明
// 策略确实降到了该档，内容证明降档没有丢帧。三类被控 writer 各司其职——
// 按次数失败（逐档推进）、按宽度拒绝（证明逐格能救回超宽写）、拒绝同步
// 序列（证明单写能绕开被拒的包裹）。

use crate::engine::ladder::Ladder;
use super::tests::{cell, FailSink};
use super::*;
use std::sync::{Arc, Mutex};

/// 画出两格内容：任一路径都必须把这两个字符送达。
fn draw_pair<W: Write>(b: &mut BufferedBackend<W>) {
    let a = cell("A");
    let d = cell("B");
    b.draw(vec![(0u16, 0u16, &a), (1u16, 0u16, &d)].into_iter())
        .unwrap();
}

/// 同档连续三次失败降一档；级次只降不升（X2-30a 判据：四级可枚举）。
#[test]
fn ladder_steps_down_per_three_failures() {
    let sink = FailSink::new(9);
    let mut b = BufferedBackend::new(sink.clone());
    draw_pair(&mut b);
    let expect = [
        (Ladder::Fused, Err(())),
        (Ladder::Fused, Err(())),
        (Ladder::Batch, Err(())),
        (Ladder::Batch, Err(())),
        (Ladder::Cell, Err(())),
        (Ladder::Cell, Err(())),
        (Ladder::Text, Ok(())),
    ];
    for (i, (level, want)) in expect.iter().enumerate() {
        let got = b.flush();
        assert_eq!(got.is_ok(), want.is_ok(), "第 {} 次 flush", i + 1);
        assert_eq!(b.level(), *level, "第 {} 次 flush 后档位", i + 1);
    }
    assert!(b.degraded());
    let text = String::from_utf8_lossy(&sink.bytes()).to_string();
    assert_eq!(text, "\x1b[1;1HAB", "纯文本档须剥掉样式、保留定位与字符");
}

/// 单写档绕开同步包裹：拒绝 2026 序列的通道在降档后仍能送达内容。
#[test]
fn batch_path_bypasses_rejected_sync() {
    let sink = SyncHater::default();
    let mut b = BufferedBackend::new(sink.clone());
    b.set_sync(true);
    draw_pair(&mut b);
    for _ in 0..2 {
        assert!(b.flush().is_err());
        assert_eq!(b.level(), Ladder::Fused);
    }
    assert!(b.flush().is_ok(), "降档后单写应成功");
    assert_eq!(b.level(), Ladder::Batch);
    let bytes = sink.bytes();
    assert!(!bytes.windows(4).any(|w| w == b"2026"), "单写不得带同步包裹");
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains('A') && text.contains('B'), "bytes = {text:?}");
}

/// 逐格档救回「单次写超长即失败」的通道：整帧过不了、单格能过。
#[test]
fn cell_path_recovers_capped_writes() {
    let sink = Capped::new(10);
    let mut b = BufferedBackend::new(sink.clone());
    let a = cell("A");
    let d = cell("B");
    let e = cell("C");
    b.draw(
        vec![(0u16, 0u16, &a), (1u16, 0u16, &d), (2u16, 0u16, &e)].into_iter(),
    )
    .unwrap();
    let whole = b.frame_bytes().len();
    assert!(whole > 10, "整帧须超过通道上限，{whole} 字节");
    // Fused 三次失败 → 降 Batch 并立即重投（仍超限，记 Batch 首次失败）；
    // Batch 再三次失败 → 降 Cell 并重投成功，故前四次 flush 均失败。
    for _ in 0..4 {
        assert!(b.flush().is_err());
    }
    assert!(b.flush().is_ok(), "逐格后每段都不超上限");
    assert_eq!(b.level(), Ladder::Cell);
    let (bytes, max) = sink.snapshot();
    assert_eq!(bytes.len(), whole, "逐格不得丢字节");
    assert!(max <= 10, "任何一次写都不得超上限，实际 {max}");
}

/// 纯文本档封顶：继续失败不再下沉，且档位对指标出口一致（X2-30b）。
#[test]
fn text_level_is_terminal() {
    let sink = FailSink::new(usize::MAX);
    let mut b = BufferedBackend::new(sink);
    draw_pair(&mut b);
    for _ in 0..12 {
        let _ = b.flush();
    }
    assert_eq!(b.level(), Ladder::Text);
    assert_eq!(b.write_level(), Ladder::Text, "指标出口须反映实际档位");
}

/// 拒绝包含 2026 序列的写：模拟终端或中间层不认同步块。
#[derive(Clone, Default)]
struct SyncHater {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SyncHater {
    fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

impl Write for SyncHater {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.windows(4).any(|w| w == b"2026") {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "sync rejected"));
        }
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 按字节宽度拒绝的通道：超过 `cap` 的写一律失败，模拟极小的原子写窗口。
#[derive(Clone)]
struct Capped {
    cap: usize,
    state: Arc<Mutex<(Vec<u8>, usize)>>,
}

impl Capped {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            state: Arc::new(Mutex::new((Vec::new(), 0))),
        }
    }

    /// 已接收字节与单次写最大长度。
    fn snapshot(&self) -> (Vec<u8>, usize) {
        let guard = self.state.lock().unwrap();
        (guard.0.clone(), guard.1)
    }
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.cap {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "too wide"));
        }
        let mut guard = self.state.lock().unwrap();
        guard.0.extend_from_slice(buf);
        guard.1 = guard.1.max(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
