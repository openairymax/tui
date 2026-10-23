// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 应用状态的值类型：面板枚举、消息与角色、会话标签、粘贴折叠、退出守卫、
// 完成通知。纯数据定义，不含行为分派（行为分派在 App 的 impl 子模块）。

use std::collections::VecDeque;
use std::time::Instant;

use crate::budget::DOUBLE_ESCAPE_WINDOW_MS;

/// Maximum number of log entries to keep.
pub(super) const MAX_LOG_ENTRIES: usize = 200;

/// Active panel for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Chat,
    Help,
    Config,
    Logs,
    Memory,
    Plugins,
    /// 任务看板（hall.board：work_hall 执行实例 + 在线 agent，实时刷新）
    Board,
    /// 事件流（hall.stream：全局 gseq 因果序回放）
    Events,
    /// 思考链独立视图（0.1.18 B4）：模型推理原文默认不上屏、不落记忆，
    /// 仅在用户显式请求（Alt+E）时于本视图中按需取用。
    Think,
    /// 焦点视图（0.1.18 B11/W14）：最近一条回复的全屏只读覆盖层，
    /// 独立滚动，Esc 退出恢复原视口。
    Focus,
    /// 健康面板（0.1.19 U12，TUI 方案 §6.12 #3）：逐依赖端到端可达性、
    /// 最后成功时长与错误聚合，/health 打开。
    Health,
}

/// Represents a chat message in the conversation.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    /// 消息时间戳（HH:MM:SS），消息气泡头部展示
    pub timestamp: String,
    /// 稳定消息 id（0.1.9 W8）：虚拟滚动的行高缓存键，单调分配、永不复用
    pub id: u64,
    /// 裁剪前原始字节数（0.1.19 §5.3 originalBytes）：未裁剪时等于 content 长度
    pub original_bytes: usize,
}

impl ChatMessage {
    /// 流式哨兵 id：不进缓存、不参与身份判断
    pub const NO_ID: u64 = u64::MAX;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MessageRole {
    User,
    Agent,
    System,
    ToolCall,
    ToolResult,
}

/// Represents a log entry.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub daemon: Option<String>,
}

/// 会话 tab（2026-08-21 多会话）：对话核心状态的快照。
///
/// 轻量模型：App 主字段（messages/input/cursor/scroll）恒为"当前会话"；
/// 其他会话以快照存于 App.session_tabs。新建（Ctrl+T）/切换（Alt+1..9）
/// 时在快照与主字段间搬移，不触碰 GCCP/任务流等执行态（执行中不切换）。
pub struct SessionTab {
    pub title: String,
    pub messages: VecDeque<ChatMessage>,
    pub input: String,
    pub cursor: usize,
    pub scroll_offset: u16,
    /// 大粘贴折叠台账（0.1.19 L7 U3）：随输入缓冲一同存取，切 tab 不丢原文。
    pub pastes: Vec<PasteFold>,
}

/// 大粘贴折叠（0.1.19 L7 U3，TUI 方案 §6.4 #3）。
///
/// 输入缓冲只保留 `slot` 占位，原文存于 `body`；提交时由
/// [`App::take_input`] 展开。占位被用户删改时原文随之丢弃——输入框所见
/// 即所发，不做隐式补全。
#[derive(Debug, Clone)]
pub struct PasteFold {
    /// 输入框中的占位文本，形如「[已粘贴 32 KB · 900 行]」。
    pub slot: String,
    /// 粘贴原文（UTF-8 完整保留，不做任何切分）。
    pub body: String,
}

/// Ctrl+C 双击退出守卫（0.1.19 L7 U2，TUI 方案 §6.6 #3）。
///
/// 首次触发只记时刻（由上层给出提示），`DOUBLE_ESCAPE_WINDOW_MS` 内再次
/// 触发才确认退出；窗口外触发视为新一次误触并重新计时。空闲态与 busy
/// 态同规——守卫只裁决"是否确认"，不感知在途请求。
#[derive(Debug, Default)]
pub struct ExitGuard {
    armed_at: Option<Instant>,
}

impl ExitGuard {
    /// 记一次退出意图；窗口内二次触发返回 true 并复位。
    pub fn confirm(&mut self, now: Instant) -> bool {
        let hit = self.armed_at.is_some_and(|t| {
            now.saturating_duration_since(t).as_millis() < u128::from(DOUBLE_ESCAPE_WINDOW_MS)
        });
        self.armed_at = if hit { None } else { Some(now) };
        hit
    }
}

/// 会话完成通知（0.1.19 TUI 建设方案 §6.7 #4）。
///
/// App 只生产通知语义（标题/正文），终端写出与降级兜底归主循环 +
/// engine/term——避免状态层直接做 IO，也避免通知丢失于渲染路径。
#[derive(Debug, Clone)]
pub struct Notice {
    pub title: String,
    pub body: String,
}
