// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 面板状态：board 与 events 的光标、过滤，面板切换，记忆窗口翻页。

use super::*;

impl App {
    /// F6 看板光标下移（循环）。
    pub fn board_cursor_down(&mut self) {
        let n = self.board_visible_count();
        if n == 0 {
            self.board_cursor = 0;
            return;
        }
        self.board_cursor = (self.board_cursor + 1) % n;
    }

    /// F6 看板光标上移（循环）。
    pub fn board_cursor_up(&mut self) {
        let n = self.board_visible_count();
        if n == 0 {
            self.board_cursor = 0;
            return;
        }
        self.board_cursor = (self.board_cursor + n - 1) % n;
    }

    /// F6 看板状态过滤：空 = 全部；点按过滤后光标回零。
    pub fn board_set_filter(&mut self, filter: &str) {
        self.board_filter = filter.to_string();
        self.board_cursor = 0;
    }

    /// F6 看板当前可见条目数（应用过滤后；与面板渲染同序：最新在前）。
    pub fn board_visible_count(&self) -> usize {
        let Some(board) = &self.hall_board else {
            return 0;
        };
        let n = if self.board_filter.is_empty() {
            board.entries.len()
        } else {
            board
                .entries
                .iter()
                .filter(|e| e.state == self.board_filter)
                .count()
        };
        n
    }

    /// F6 看板当前选中条目的 execution_id（无则返回空；与渲染同序）。
    pub fn board_selected_exec(&self) -> String {
        let Some(board) = &self.hall_board else {
            return String::new();
        };
        let mut visible: Vec<&HallBoardEntry> = if self.board_filter.is_empty() {
            board.entries.iter().collect()
        } else {
            board
                .entries
                .iter()
                .filter(|e| e.state == self.board_filter)
                .collect()
        };
        visible.reverse(); // 与面板渲染一致：最新在前
        // 与渲染同序（board.rs 状态分组稳定排序）：此前仅 reverse 取索引，
        // 混合状态时高亮行与 Enter 详情错位（2.3.13 F6 看板选中错位）。
        visible.sort_by_key(|e| {
            crate::panels::board::state_rank(if e.state.is_empty() { "unknown" } else { &e.state })
        });
        visible
            .get(self.board_cursor % visible.len().max(1))
            .map(|e| e.execution_id.clone())
            .unwrap_or_default()
    }

    /// F6 看板选中行 → 切回对话并回放该任务决策链（复用 /chain 逻辑）。
    pub fn board_view_selected(&mut self) {
        let exec = self.board_selected_exec();
        if exec.is_empty() {
            return;
        }
        self.active_panel = ActivePanel::Chat;
        self.cmd_chain(&format!("/chain {}", exec));
    }

    /// F7 事件流光标下移（循环）。
    pub fn events_cursor_down(&mut self) {
        let n = self.events_visible_count();
        if n == 0 {
            self.events_cursor = 0;
            return;
        }
        self.events_cursor = (self.events_cursor + 1) % n;
    }

    /// F7 事件流光标上移（循环）。
    pub fn events_cursor_up(&mut self) {
        let n = self.events_visible_count();
        if n == 0 {
            self.events_cursor = 0;
            return;
        }
        self.events_cursor = (self.events_cursor + n - 1) % n;
    }

    /// F7 事件流类别过滤：空 = 全部；点按过滤后光标回零。
    pub fn events_set_filter(&mut self, filter: &str) {
        self.events_filter = filter.to_string();
        self.events_cursor = 0;
    }

    /// F7 事件流当前可见条数（应用过滤后）。
    pub fn events_visible_count(&self) -> usize {
        if self.events_filter.is_empty() {
            return self.hall_events.len();
        }
        self.hall_events
            .iter()
            .filter(|e| e.category == self.events_filter)
            .count()
    }

    /// F7 事件流选中行 → 对话区展示完整事件 JSON（方便深读）。
    pub fn events_view_selected(&mut self) {
        let Some(e) = self.events_selected() else {
            return;
        };
        self.active_panel = ActivePanel::Chat;
        let pretty = serde_json::to_string_pretty(&e.content).unwrap_or_else(|_| e.content.to_string());
        self.add_message(
            MessageRole::System,
            format!("[{}:{}] 事件详情（task={}）\n{}", events_category_cn(&e.category), e.gseq, e.task_id, pretty),
        );
    }

    /// F7 事件流当前选中事件（与面板渲染同序：最新在前）。
    pub fn events_selected(&self) -> Option<HallEvent> {
        let mut visible: Vec<&HallEvent> = if self.events_filter.is_empty() {
            self.hall_events.iter().collect()
        } else {
            self.hall_events
                .iter()
                .filter(|e| e.category == self.events_filter)
                .collect()
        };
        visible.reverse(); // 与面板渲染一致：最新在前
        visible
            .get(self.events_cursor % visible.len().max(1))
            .map(|e| (*e).clone())
    }

    /// Toggle a panel. If already active, go back to Chat.
    pub fn toggle_panel(&mut self, panel: ActivePanel) {
        if self.active_panel == panel {
            self.active_panel = ActivePanel::Chat;
            self.stop_hall_watch();
        } else {
            self.active_panel = panel;
            if panel == ActivePanel::Board || panel == ActivePanel::Events {
                self.start_hall_watch();
            } else {
                self.stop_hall_watch();
            }
            if panel == ActivePanel::Memory {
                self.memory_view.reset();
            }
        }
    }

    /// PgDn：记忆面板向更早方向翻一页（分组窗口懒加载，0.1.9 W8）。
    pub fn memory_page_down(&mut self) {
        self.memory_view.page_down(self.memory.len());
    }

    /// PgUp：记忆面板向更新方向翻回一页。
    pub fn memory_page_up(&mut self) {
        self.memory_view.page_up();
    }
}
