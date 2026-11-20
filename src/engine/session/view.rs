// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话呈现视图：把 tab 槽位与回放计数收敛为不可变的按值快照。
// 空标题回退与 /sessions 列表拼串在此一次完成，渲染侧只读快照字段。

/// 单个会话 tab 的只读呈现视图：标题已按「会话 N」回退。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabView {
    title: String,
    messages: usize,
    is_active: bool,
}

impl TabView {
    pub(crate) fn new(title: String, messages: usize, is_active: bool) -> Self {
        Self {
            title,
            messages,
            is_active,
        }
    }

    /// 展示标题（构造时已完成空标题回退）。
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// 会话内消息数（当前会话为 live 长度，其余为槽位快照）。
    pub(crate) fn messages(&self) -> usize {
        self.messages
    }

    pub(crate) fn is_active(&self) -> bool {
        self.is_active
    }
}

/// 会话列表的不可变快照：构造后不再变化，只读消费。
///
/// `total_tabs` 与 `tabs.len()` 同源，保留独立字段是为了让「总数」与
/// 「可见条目」在后续过滤场景下仍可分别表达，且与旧 `tab_count()` 语义一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionView {
    tabs: Vec<TabView>,
    active: usize,
    replay_left: usize,
    total_tabs: usize,
}

impl SessionView {
    /// 由 app 层收拢的原始行（标题, 消息数）构造快照。
    ///
    /// 当前会话的消息数由 app 层填入 live 长度；`active` 越界时只影响
    /// 当前标记，标题查询按空串回退（与旧访问器一致）。
    pub(crate) fn new(rows: Vec<(String, usize)>, active: usize, replay_left: usize) -> Self {
        let total_tabs = rows.len();
        let tabs = rows
            .into_iter()
            .enumerate()
            .map(|(i, (title, messages))| {
                TabView::new(title_of(title, i), messages, i == active)
            })
            .collect();
        Self {
            tabs,
            active,
            replay_left,
            total_tabs,
        }
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.total_tabs
    }

    /// 当前会话索引（None 主会话归 0 的约定由 app 层完成）。
    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    /// 回放剩余条数（渲染骨架行「回放中 · N 条」）。
    pub(crate) fn replay_left(&self) -> usize {
        self.replay_left
    }

    pub(crate) fn tabs(&self) -> &[TabView] {
        &self.tabs
    }

    /// 指定 tab 的展示标题；越界为空串。
    pub(crate) fn tab_title(&self, i: usize) -> &str {
        self.tabs.get(i).map(TabView::title).unwrap_or("")
    }

    /// /sessions 列表：序号 · 标题 · 消息数，当前会话标 ◂。
    /// 逐行拼接格式与迁移前的 app::list_tabs 一字不差。
    pub(crate) fn render_list(&self) -> String {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mark = if t.is_active() { "◂" } else { " " };
                format!("{} [{}] {}（{} 条）", mark, i + 1, t.title(), t.messages())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 空标题回退为「会话 N」（N 为 1 起的展示序号）。
fn title_of(title: String, i: usize) -> String {
    if title.is_empty() {
        format!("会话 {}", i + 1)
    } else {
        title
    }
}
