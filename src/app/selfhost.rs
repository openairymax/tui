// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 自托管面板编排（D-1）：调度 llm_d `llm.get_stats` 的只读拉取，并把本机
// 显存（nvidia-smi）附在同一快照上。失败原因原样保留——面板据此显示降级，
// 不回显上一轮端点数值。时刻由主循环注入，本层不自行读时钟。

use crate::engine::sched::Lane;
use crate::engine::selfhost::{snapshot, unavailable};

use super::*;

/// 自托管面板翻页步长（行）。
const SH_PAGE: usize = 10;

/// 翻页步长须为正：零步长会让翻页键变成空操作，负值在 usize 下无处可去。
const _: () = assert!(SH_PAGE >= 5);

impl App {
    /// 发起一轮自托管快照拉取（已在途则跳过）。应答时限由 CLI 侧 3s
    /// 查询窗口保证，客户端不另设超时。
    pub fn start_selfhost(&mut self) {
        if self.sh_q {
            return;
        }
        match self.send_query("sh.stats", "llm.get_stats", &serde_json::json!({})) {
            Ok(()) => self.sh_q = true,
            Err(e) => self.selfhost = Some(unavailable(format!("llm.get_stats 失败：{e}"))),
        }
    }

    /// 摘取快照；面板仍打开时续拉下一轮（在飞/排队随时间变化）。
    pub fn poll_selfhost(&mut self, now_ms: u64) {
        if !self.sh_q {
            return;
        }
        let mut snap = match self.take_query("sh.stats") {
            Some(Ok(v)) => snapshot(&v),
            Some(Err(e)) => unavailable(format!("llm.get_stats 失败：{e}")),
            None if !self.pipe_alive() => {
                unavailable("llm.get_stats 未应答（事件管道不可用）".to_string())
            }
            None => return,
        };
        self.sh_q = false;
        if snap.live {
            snap.vram = vram_usage();
        }
        self.selfhost = Some(snap);
        self.selfhost_at_ms = now_ms;
        self.mark_landed(Lane::Poll);
        if self.active_panel == ActivePanel::SelfHost {
            self.start_selfhost();
        }
    }

    /// 打开自托管面板（/selfhost 或 Alt+S）：切面板、归零滚动、立即拉取。
    pub fn open_selfhost(&mut self) {
        self.active_panel = ActivePanel::SelfHost;
        self.selfhost_scroll = 0;
        self.start_selfhost();
    }

    pub fn sh_scroll_up(&mut self) {
        self.selfhost_scroll = self.selfhost_scroll.saturating_sub(1);
    }

    pub fn sh_scroll_down(&mut self) {
        self.selfhost_scroll = self.selfhost_scroll.saturating_add(1);
    }

    pub fn sh_page_up(&mut self) {
        self.selfhost_scroll = self.selfhost_scroll.saturating_sub(SH_PAGE);
    }

    pub fn sh_page_down(&mut self) {
        self.selfhost_scroll = self.selfhost_scroll.saturating_add(SH_PAGE);
    }
}

/// 本机显存（已用 MiB，总量 MiB）。无 GPU / 无 nvidia-smi 时返回 None，
/// 面板据此显示「未检测到 GPU」而不是 0。
fn vram_usage() -> Option<(u64, u64)> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.lines().next()?.split(',').map(str::trim);
    let used: u64 = it.next()?.parse().ok()?;
    let total: u64 = it.next()?.parse().ok()?;
    if total == 0 {
        return None;
    }
    Some((used, total))
}
