// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 演化指标编排（0.1.20 W8）：调度 gateway `airy_sys_telemetry_metrics` 的
// 只读拉取，把结果交给 engine::evolution 解析。本层只做真实 RPC 的发起与
// 结果转交；失败原因原样保留，不合成指标也不猜测成因。时刻由主循环注入。

use std::time::Duration;

use crate::engine::evolution::{snapshot, Snapshot};
use crate::engine::sched::Lane;

use super::*;

/// 单次拉取超时：gateway 需转发 monit，留出比健康探测更宽的窗口。
const EVO_TIMEOUT: Duration = Duration::from_secs(3);

/// 演化面板翻页步长（行）。
const EVO_PAGE: usize = 10;

impl App {
    /// 发起一轮演化指标拉取（已在途则跳过）。
    pub fn start_evo(&mut self) {
        if self.evo_rx.is_some() {
            return;
        }
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let r = tokio::time::timeout(
                EVO_TIMEOUT,
                gw.rpc_call("airy_sys_telemetry_metrics", serde_json::json!({})),
            )
            .await;
            let snap = match r {
                Ok(Ok(v)) => snapshot(&v),
                Ok(Err(e)) => Snapshot {
                    available: false,
                    reason: e.to_string(),
                    items: Vec::new(),
                },
                Err(_) => Snapshot {
                    available: false,
                    reason: format!("请求超时（{}s）", EVO_TIMEOUT.as_secs()),
                    items: Vec::new(),
                },
            };
            let _ = tx.send(snap);
        });
        self.evo_rx = Some(rx);
    }

    /// 消费拉取结果；面板仍打开时续拉下一轮（指标随时间变化）。
    pub fn poll_evo(&mut self, now_ms: u64) {
        let Some(mut rx) = self.evo_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(snap) => {
                self.evo = Some(snap);
                self.evo_at_ms = now_ms;
                self.mark_landed(Lane::Poll);
                if self.active_panel == ActivePanel::Evolution {
                    self.start_evo();
                }
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.evo_rx = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// 打开演化指标面板（/evolution）：切面板、归零滚动、立即拉取。
    pub fn open_evo(&mut self) {
        self.active_panel = ActivePanel::Evolution;
        self.evo_scroll = 0;
        self.stop_hall_watch();
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
