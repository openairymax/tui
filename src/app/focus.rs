// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 焦点接线（0.1.21 X5-01/02）：把面板状态折算成焦点环入参，并把焦点名
// 交给呈现层。应用层只转述状态，环序与回位规则仍在 engine::focus。

use super::*;
use crate::engine::focus::Zone;

impl App {
    /// 面板/浮层状态 → 焦点环同步。按键入口与鼠标入口各调用一次，幂等。
    pub(crate) fn sync_focus(&mut self) {
        let chat = self.active_panel == ActivePanel::Chat;
        self.focus_ring
            .sync(self.active_panel as u8, chat, !self.approvals.is_empty());
    }

    /// 焦点前移/后移（Tab / Shift-Tab）。
    pub fn focus_step(&mut self, back: bool) {
        let chat = self.active_panel == ActivePanel::Chat;
        self.focus_ring.step(back, chat, !self.approvals.is_empty());
    }

    /// 显式聚焦输入栏（用户开始输入时调用，保证字符永不落空）。
    pub fn focus_input(&mut self) {
        self.focus_ring.set(Zone::Input);
    }

    /// 当前是否聚焦输入栏：Tab 的补全语义只在此成立。
    pub fn focus_is_input(&self) -> bool {
        self.focus_ring.is(Zone::Input)
    }

    /// 当前焦点名（页脚展示）。
    pub fn focus_name(&self) -> &'static str {
        self.focus_ring.cur().cn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_tab_cycles_body_and_input() {
        let _home = crate::test_env::Home::new("focus-app");
        let mut app = App::new("agents/main.agent.yaml");
        app.sync_focus();
        assert!(app.focus_is_input());
        app.focus_step(false);
        assert_eq!(app.focus_name(), "对话");
        app.focus_step(true);
        assert!(app.focus_is_input());
    }

    #[test]
    fn pending_approval_is_reachable_by_tab() {
        let _home = crate::test_env::Home::new("focus-approval");
        let mut app = App::new("agents/main.agent.yaml");
        app.approvals.push(crate::client::PendingApproval {
            request_id: "r1".to_string(),
            tool: "shell_exec".to_string(),
            agent_id: String::new(),
            params: "{}".to_string(),
            created_at: None,
        });
        app.sync_focus();
        app.focus_step(false);
        assert_eq!(app.focus_name(), "审批");
    }

    #[test]
    fn panel_switch_keeps_input_focus_on_return() {
        let _home = crate::test_env::Home::new("focus-panel");
        let mut app = App::new("agents/main.agent.yaml");
        app.sync_focus();
        app.toggle_panel(ActivePanel::Board);
        app.sync_focus();
        assert_eq!(app.focus_name(), "列表");
        app.toggle_panel(ActivePanel::Chat);
        app.sync_focus();
        assert!(app.focus_is_input());
    }
}
