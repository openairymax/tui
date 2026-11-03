// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后端日志面板编排（0.1.20 批次五）：daemon 日志尾部快照拉取与消费。
//
// 与演化指标面板（evolution.rs）同构：打开即拉一轮，oneshot 回投不阻塞
// 主循环；落地后不再立即续拉，改由 poll_dlogs 的 BACKEND_REFRESH_MS 节拍
// 在面板激活期间续拉（本地文件读按节拍即可，不做紧密循环）。r122 批次四
// 在 executor_pool 与 llm_d 落的等待事件（排队 / 心跳 / 取件 / 等座弧线）
// 由此面板对用户可见。本层只发起与转交，不加工日志内容。

use std::time::Duration;

use crate::engine::sched::Lane;

use super::*;

/// 单次尾读超时：本地文件读，窗口很小，1s 足够。
const DLOG_TIMEOUT: Duration = Duration::from_secs(1);

/// 后端日志翻页步长（行）。
const DLOG_PAGE: usize = 10;

impl App {
    /// 发起一轮后端日志尾读（已在途则跳过）。
    pub fn start_dlogs(&mut self) {
        if self.backend_rx.is_some() {
            return;
        }
        let dir = crate::app::daemonlog::log_dir();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let entries = tokio::time::timeout(DLOG_TIMEOUT, crate::app::daemonlog::tail_all(&dir))
                .await
                .unwrap_or_default();
            let _ = tx.send(entries);
        });
        self.backend_rx = Some(rx);
    }

    /// 消费尾读结果；面板激活时按 BACKEND_REFRESH_MS 节拍续拉。
    pub fn poll_dlogs(&mut self, now_ms: u64) {
        let Some(mut rx) = self.backend_rx.take() else {
            if self.active_panel == ActivePanel::BackendLog
                && now_ms.saturating_sub(self.backend_at_ms) >= BACKEND_REFRESH_MS
            {
                self.start_dlogs();
            }
            return;
        };
        match rx.try_recv() {
            Ok(entries) => {
                self.backend_logs = entries;
                self.backend_at_ms = now_ms;
                self.mark_landed(Lane::Poll);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.backend_rx = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// 打开后端日志面板（/dlogs）：切面板、归零滚动、立即拉取。
    pub fn open_dlogs(&mut self) {
        self.active_panel = ActivePanel::BackendLog;
        self.backend_logs_scroll = 0;
        self.stop_hall_watch();
        self.start_dlogs();
    }

    pub fn dlog_up(&mut self) {
        self.backend_logs_scroll = self.backend_logs_scroll.saturating_sub(1);
    }

    pub fn dlog_down(&mut self) {
        self.backend_logs_scroll = self.backend_logs_scroll.saturating_add(1);
    }

    pub fn dlog_pgup(&mut self) {
        self.backend_logs_scroll = self.backend_logs_scroll.saturating_sub(DLOG_PAGE);
    }

    pub fn dlog_pgdn(&mut self) {
        self.backend_logs_scroll = self.backend_logs_scroll.saturating_add(DLOG_PAGE);
    }
}
