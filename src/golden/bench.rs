// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 长历史帧基准（0.1.21 X2-27a）：以预算上限的消息条数构造 5k 行量级的
// 会话历史，并在同态稳态下连续采样整帧耗时，供 X2-27b 按 p95 ≤16ms 判定。
//
// 采样口径：前若干帧只用于解析与高度缓存预热，不计入样本；样本帧驱动
// 完整的 `present::render` 与帧编码前的绘制路径，与生产主循环一致。
// 历史规模受 `MAX_CHAT_MESSAGES` 约束，行数由每条消息多行补足，不越过
// 预算界去构造无法真实持有的会话。

use super::{add, Fixture, H, W};
use crate::app::App;
use crate::app::MessageRole;
use crate::budget::{FRAME_P95_BUDGET_US, MAX_CHAT_MESSAGES};
use crate::engine::backend::BufferedBackend;
use ratatui::backend::Backend as _;
use ratatui::buffer::Buffer;
use std::time::Instant;

/// 历史行量级下限：基准必须覆盖的真实历史规模。
pub(crate) const HISTORY_LINES_MIN: usize = 5_000;
/// 预热帧数：冷缓存、首帧布局与解析成本不计入稳态样本。
pub(crate) const WARMUP_FRAMES: usize = 8;
/// 单条历史消息的行数：与消息条数相乘即历史行量级。
const LINES_PER_MSG: usize = 3;

/// 构造长历史并返回总行数；内容固定，基准可复跑。
pub(crate) fn long_history(app: &mut App) -> usize {
    let mut lines = 0;
    for i in 0..MAX_CHAT_MESSAGES {
        let body = format!("第{i}条\n细节行 {i}\n尾行 {i}");
        lines += LINES_PER_MSG;
        add(app, MessageRole::Agent, &body);
    }
    lines
}

/// 连续渲染 `frames` 个稳态帧，返回逐帧耗时（μs，已剔除预热帧）。
pub(crate) fn samples(fx: &mut Fixture, frames: usize) -> Vec<u64> {
    let mut out = Vec::with_capacity(frames);
    for i in 0..WARMUP_FRAMES + frames {
        let started = Instant::now();
        let _ = fx.draw_at(W, H);
        let elapsed = started.elapsed().as_micros() as u64;
        if i >= WARMUP_FRAMES {
            out.push(elapsed);
        }
    }
    out
}

/// 最近秩法取 p95：样本不足时退化为最大值，不隐藏单点尖峰。
pub(crate) fn p95(samples: &[u64]) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    assert!(n > 0, "p95 需要至少一个样本");
    let rank = (n * 95).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// 基准生成器可复跑：历史规模达标，样本条数与请求一致。
#[test]
fn bench_history_is_replayable() {
    let mut fx = Fixture::new("x2-27a-bench");
    let lines = long_history(&mut fx.app);
    assert!(
        lines >= HISTORY_LINES_MIN,
        "历史行数 {lines} 未达基准下限 {HISTORY_LINES_MIN}"
    );
    assert_eq!(
        fx.app.session_view().tabs()[0].messages(),
        MAX_CHAT_MESSAGES,
        "历史应填满保留上限"
    );
    let one = samples(&mut fx, 3);
    let two = samples(&mut fx, 3);
    assert_eq!(one.len(), 3);
    assert_eq!(two.len(), 3);
}

/// 判定接线（§23.9 #10）：5k 行历史稳态帧 p95 不超 16ms 预算。
#[test]
fn bench_p95_within_budget() {
    assert_eq!(FRAME_P95_BUDGET_US, 16_000);
    let mut fx = Fixture::new("x2-27b-p95");
    long_history(&mut fx.app);
    let sampled = samples(&mut fx, 60);
    let p = p95(&sampled);
    assert!(
        p <= FRAME_P95_BUDGET_US,
        "长历史帧 p95 {p}µs 超过预算 {FRAME_P95_BUDGET_US}µs（{} 个样本）",
        sampled.len()
    );
}

/// 编码相位剖析：整屏缓冲一次性编码（无 diff，作为上界）重复多轮，
/// 返回编码耗时中位数（μs）、帧字节数与定位次数。
pub(crate) fn encode_profile(buf: &Buffer, rounds: usize) -> (u64, usize, usize) {
    let w = buf.area.width as usize;
    let mut times = Vec::with_capacity(rounds);
    let mut bytes = 0;
    let mut moves = 0;
    for _ in 0..rounds {
        let mut backend = BufferedBackend::new(Vec::new());
        let started = Instant::now();
        backend
            .draw(
                buf.content
                    .iter()
                    .enumerate()
                    .map(|(i, cell)| ((i % w) as u16, (i / w) as u16, cell)),
            )
            .expect("整帧编码");
        let elapsed = started.elapsed().as_micros() as u64;
        backend.flush().expect("写出到内存");
        bytes = backend.frame_bytes().len();
        moves = backend.move_count();
        times.push(elapsed);
    }
    times.sort_unstable();
    (times[times.len() / 2], bytes, moves)
}

/// 剖析用例不参与常规回归：耗时数值随 runner 负载变化，只在显式
/// workflow_dispatch 步骤里取样，供 §16 留痕与优化前后对比。
#[test]
#[ignore = "剖析用例：由 workflow_dispatch 的剖析步骤显式执行"]
fn profile_frame_phases() {
    let mut fx = Fixture::new("x2-28a-profile");
    long_history(&mut fx.app);
    let render = samples(&mut fx, 30);
    let buf = super::frame_buffer(&mut fx.app, W, H);
    let (encode, bytes, moves) = encode_profile(&buf, 30);
    println!(
        "X2-28a profile: render_p95_us={} encode_p50_us={} frame_bytes={} moves={} cells={}",
        p95(&render),
        encode,
        bytes,
        moves,
        W as usize * H as usize
    );
}
