// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 焦点环与焦点记忆（0.1.21 X5-01/02）。同屏只有一个控件获焦，Tab
// 按环序前移、Shift-Tab 后移；换面板时把旧焦点记入该面板槽位，返回时
// 复原。环由「当前面板形态 + 是否有待决议审批」推导，不持久化多余状态。
//
// 纯数据：无 IO、无时钟、不引用 ratatui，也不认识 App 的面板枚举——
// 面板身份由调用方折算为 u8 槽位，引擎只负责环序与回位。

/// 同屏可聚焦的控件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Zone {
    /// 对话正文（滚动、拖选复制）。
    Conversation,
    /// 面板列表正文（看板/事件流/日志等只读列表）。
    List,
    /// 工具审批条（决议优先，故排在环首）。
    Approval,
    /// 输入栏（文本编辑）。
    Input,
}

impl Zone {
    /// 焦点名（页脚展示，中文短名）。
    pub(crate) fn cn(self) -> &'static str {
        match self {
            Zone::Conversation => "对话",
            Zone::List => "列表",
            Zone::Approval => "审批",
            Zone::Input => "输入",
        }
    }
}

/// 面板槽位数（与 `ActivePanel` 变体数上限一致）。
const PANELS: usize = 16;

/// 焦点环：当前控件 + 逐面板记忆。
#[derive(Debug, Clone)]
pub(crate) struct Focus {
    panel: u8,
    cur: Zone,
    memo: [Option<Zone>; PANELS],
}

/// 环序推导：审批条在场时最先可达（决议最急），正文其次，输入栏最后。
fn ring(chat: bool, approvals: bool) -> ([Zone; 4], usize) {
    let mut zones = [Zone::Input; 4];
    let mut n = 0;
    if approvals {
        zones[n] = Zone::Approval;
        n += 1;
    }
    zones[n] = if chat {
        Zone::Conversation
    } else {
        Zone::List
    };
    n += 1;
    zones[n] = Zone::Input;
    n += 1;
    (zones, n)
}

fn default_zone(chat: bool) -> Zone {
    if chat {
        Zone::Input
    } else {
        Zone::List
    }
}

impl Focus {
    pub(crate) fn new() -> Self {
        Self {
            panel: 0,
            cur: Zone::Input,
            memo: [None; PANELS],
        }
    }

    pub(crate) fn cur(&self) -> Zone {
        self.cur
    }

    pub(crate) fn is(&self, zone: Zone) -> bool {
        self.cur == zone
    }

    pub(crate) fn set(&mut self, zone: Zone) {
        self.cur = zone;
    }

    /// 面板与浮层状态同步（每次按键入口调用，幂等）。
    ///
    /// 换面板：旧焦点存入撤销记忆，新面板按其记忆复原（无记忆取默认）。
    /// 环外焦点（审批条消失后仍停留在 Approval）按环首收敛，避免焦点
    /// 指向不存在的控件。
    pub(crate) fn sync(&mut self, panel: u8, chat: bool, approvals: bool) {
        let slot = panel as usize % PANELS;
        if panel != self.panel {
            self.memo[self.panel as usize % PANELS] = Some(self.cur);
            self.panel = panel;
            self.cur = self.memo[slot].unwrap_or_else(|| default_zone(chat));
        }
        let (zones, n) = ring(chat, approvals);
        if !zones[..n].contains(&self.cur) {
            self.cur = zones[0];
        }
    }

    /// 焦点前移/后移（Tab / Shift-Tab），到尾回环。
    pub(crate) fn step(&mut self, back: bool, chat: bool, approvals: bool) {
        let (zones, n) = ring(chat, approvals);
        let pos = zones[..n].iter().position(|z| *z == self.cur);
        self.cur = match pos {
            Some(i) if back => zones[(i + n - 1) % n],
            Some(i) => zones[(i + 1) % n],
            None if back => zones[n - 1],
            None => zones[0],
        };
    }
}

impl Default for Focus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_ring_orders_body_then_input() {
        let (zones, n) = ring(true, false);
        assert_eq!(&zones[..n], &[Zone::Conversation, Zone::Input]);
        let (zones, n) = ring(false, false);
        assert_eq!(&zones[..n], &[Zone::List, Zone::Input]);
        let (zones, n) = ring(true, true);
        assert_eq!(
            &zones[..n],
            &[Zone::Approval, Zone::Conversation, Zone::Input]
        );
    }

    #[test]
    fn tab_wraps_and_shift_tab_inverts() {
        let mut f = Focus::new();
        f.set(Zone::Conversation);
        f.step(false, true, true);
        assert_eq!(f.cur(), Zone::Input);
        f.step(false, true, true);
        assert_eq!(f.cur(), Zone::Approval);
        f.step(false, true, true);
        assert_eq!(f.cur(), Zone::Conversation);
        f.step(true, true, true);
        assert_eq!(f.cur(), Zone::Approval);
    }

    #[test]
    fn panel_switch_restores_previous_control() {
        let mut f = Focus::new();
        f.sync(0, true, false);
        f.set(Zone::Conversation);
        // 切到面板 5（列表）：默认焦点在列表
        f.sync(5, false, false);
        assert_eq!(f.cur(), Zone::List);
        f.set(Zone::Input);
        // 回到对话：记忆里仍是对话正文
        f.sync(0, true, false);
        assert_eq!(f.cur(), Zone::Conversation);
        f.sync(5, false, false);
        assert_eq!(f.cur(), Zone::Input);
    }

    #[test]
    fn vanished_approval_falls_back_to_ring_head() {
        let mut f = Focus::new();
        f.sync(0, true, true);
        f.set(Zone::Approval);
        f.sync(0, true, false);
        assert_eq!(f.cur(), Zone::Conversation);
    }

    #[test]
    fn unknown_current_steps_from_ring_edges() {
        let mut f = Focus::new();
        f.set(Zone::Approval);
        f.step(false, true, false);
        assert_eq!(f.cur(), Zone::Conversation);
        f.set(Zone::Approval);
        f.step(true, true, false);
        assert_eq!(f.cur(), Zone::Input);
    }
}
