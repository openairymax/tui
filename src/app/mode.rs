// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 模式标记协议：解析 LLM 返回的 [MODE:*] 标记，判定对话/任务/大任务集。

/// LLM 判定的模式（任务集判定结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeMarker {
    /// 普通对话
    Chat,
    /// 任务集（简单任务，无需 GCCP）
    Task,
    /// 大任务集（需先任务事实确认 GCCP）
    TaskGccp,
}

/// 解析 LLM 返回的模式标记详情（区分普通任务集与大任务集 GCCP）。
///
/// 容错（2026-08-26 修复）：此前要求响应严格以 `[MODE:XXX]` 开头，LLM 输出
/// 「好的，[MODE:TASK]…」等带前导文本时判定失败，任务集无法进入。现在只在
/// 响应开头 64 字符窗口内定位 `[MODE:` 标记（前导文本被 trim 后仍可能残留
/// 简短客套语），避免正文中提及标记造成的误判。
pub fn parse_mode_detail(resp: &str) -> (ModeMarker, String) {
    let t = resp.trim_start();
    if t.is_empty() {
        return (ModeMarker::Chat, resp.to_string());
    }
    let win = t.len().min(64);
    let head = &t[..win];
    if let Some(idx) = head.find("[MODE:") {
        if let Some(end_rel) = head[idx..].find(']') {
            let end = idx + end_rel;
            let marker = &head[idx..=end];
            let mode = match marker {
                "[MODE:TASK:GCCP]" => ModeMarker::TaskGccp,
                "[MODE:TASK]" => ModeMarker::Task,
                "[MODE:CHAT]" => ModeMarker::Chat,
                _ => return (ModeMarker::Chat, resp.to_string()),
            };
            let rest = t[end + 1..].trim_start().to_string();
            return (mode, rest);
        }
    }
    (ModeMarker::Chat, resp.to_string())
}
