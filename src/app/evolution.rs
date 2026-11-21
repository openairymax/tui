// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 演化指标编排（0.1.20 W8）：调度 gateway `airy_sys_telemetry_metrics` 的
// 只读拉取，把结果交给 engine::evolution 解析。本层只做查询帧的发起与
// 结果转交；失败原因原样保留，不合成指标也不猜测成因。时刻由主循环注入。

use crate::engine::evolution::{snapshot, unavailable};
use crate::engine::sched::Lane;

use super::*;

/// 演化面板翻页步长（行）。
const EVO_PAGE: usize = 10;

impl App {
    /// 发起一轮演化指标拉取（已在途则跳过）。单次查询的应答时限由 CLI
    /// 侧 3s 窗口保证，客户端不另设超时。
    pub fn start_evo(&mut self) {
        if self.evo_q {
            return;
        }
        match self.send_query(
            "evo.metrics",
            "airy_sys_telemetry_metrics",
            &serde_json::json!({}),
        ) {
            Ok(()) => self.evo_q = true,
            Err(e) => self.evo = Some(unavailable(&e)),
        }
    }

    /// 摘取拉取结果；面板仍打开时续拉下一轮（指标随时间变化）。
    pub fn poll_evo(&mut self, now_ms: u64) {
        if !self.evo_q {
            return;
        }
        let snap = match self.take_query("evo.metrics") {
            Some(Ok(v)) => snapshot(&v),
            Some(Err(e)) => unavailable(&e),
            None if !self.pipe_alive() => unavailable("事件管道不可用"),
            None => return,
        };
        self.evo_q = false;
        self.evo = Some(snap);
        self.evo_at_ms = now_ms;
        self.mark_landed(Lane::Poll);
        if self.active_panel == ActivePanel::Evolution {
            self.start_evo();
        }
    }

    /// 打开演化指标面板（/evolution）：切面板、归零滚动、立即拉取。
    pub fn open_evo(&mut self) {
        self.active_panel = ActivePanel::Evolution;
        self.evo_scroll = 0;
        self.start_evo();
    }

    pub fn evo_scroll_up(&mut self) {
        self.evo_scroll = self.evo_scroll.saturating_sub(1);
    }

    pub fn evo_scroll_down(&mut self) {
        self.evo_scroll = self.evo_scroll.saturating_add(1);
    }

    pub fn evo_page_up(&mut self) {
        self.evo_scroll = self.evo_scroll.saturating_sub(EVO_PAGE);
    }

    pub fn evo_page_down(&mut self) {
        self.evo_scroll = self.evo_scroll.saturating_add(EVO_PAGE);
    }
}
