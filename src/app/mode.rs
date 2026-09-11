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

/// 返回 `s` 中不超过 `max` 字节且落在 UTF-8 字符边界上的最大偏移。
///
/// T-01（P0-7 修复）：64 字节窗口此前直接按字节切片截断，当第 64 字节
/// 落在多字节字符（中文等）中间时 `&t[..win]` panic，中文长回复流式必现。
/// 回退到最近的前序字符边界，窗口语义最多损失 3 字节。
fn boundary_floor(s: &str, max: usize) -> usize {
    let mut idx = s.len().min(max);
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// 解析 LLM 返回的模式标记详情（区分普通任务集与大任务集 GCCP）。
///
/// 容错（2026-08-26 修复）：此前要求响应严格以 `[MODE:XXX]` 开头，LLM 输出
/// 「好的，[MODE:TASK]…」等带前导文本时判定失败，任务集无法进入。现在只在
/// 响应开头 64 字节窗口（字符安全，见 `boundary_floor`）内定位 `[MODE:`
/// 标记（前导文本被 trim 后仍可能残留简短客套语），避免正文中提及标记
/// 造成的误判。
pub fn parse_mode_detail(resp: &str) -> (ModeMarker, String) {
    let t = resp.trim_start();
    if t.is_empty() {
        return (ModeMarker::Chat, resp.to_string());
    }
    let head = &t[..boundary_floor(t, 64)];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_floor_never_splits_multibyte_chars() {
        // ASCII：不超过 max 即可
        assert_eq!(boundary_floor("abc", 10), 3);
        assert_eq!(boundary_floor("abcdef", 4), 4);
        // 汉字 3 字节：截断落在第 2 个汉字中间时回退到前序边界
        assert_eq!(boundary_floor("中中中", 4), 3);
        assert_eq!(boundary_floor("中中中", 5), 3);
        assert_eq!(boundary_floor("中中中", 6), 6);
        // 4 字节 emoji：任何非边界截断回退
        assert_eq!(boundary_floor("\u{1F600}", 2), 0);
        assert_eq!(boundary_floor("\u{1F600}", 4), 4);
        // 空串
        assert_eq!(boundary_floor("", 64), 0);
    }

    #[test]
    fn window_truncation_on_multibyte_char_does_not_panic() {
        // P0-7 复现向量：21 个汉字（63 字节）+ 4 字节 emoji，64 字节窗口
        // 恰好落在 emoji 中间。修复前 `&t[..64]` 在此 panic。
        let body = format!("{}\u{1F600}", "汉".repeat(21));
        let _ = parse_mode_detail(&body); // 不 panic 即通过
    }

    #[test]
    fn multibyte_leading_text_still_recognizes_markers() {
        // 中文前导 + 标记在窗口内：功能不回归
        assert_eq!(
            parse_mode_detail("好的，[MODE:TASK:GCCP]\n先做任务事实确认"),
            (ModeMarker::TaskGccp, "先做任务事实确认".to_string())
        );
        assert_eq!(
            parse_mode_detail("这是中文前导，[MODE:TASK] 开始"),
            (ModeMarker::Task, "开始".to_string())
        );
    }

    #[test]
    fn marker_outside_window_is_ignored() {
        // 标记位于 64 字节窗口之外：不触发模式切换，原样返回
        let prefix = "汉".repeat(30); // 90 字节 > 64
        let body = format!("{prefix}[MODE:TASK] 后续内容");
        assert_eq!(parse_mode_detail(&body), (ModeMarker::Chat, body));
    }

    #[test]
    fn unclosed_or_unknown_marker_falls_back_to_chat() {
        let body = "[MODE:TASK 未闭合".to_string();
        assert_eq!(parse_mode_detail(&body), (ModeMarker::Chat, body));
        let body2 = "[MODE:WHAT] 未知标记".to_string();
        assert_eq!(parse_mode_detail(&body2), (ModeMarker::Chat, body2));
    }

    #[test]
    fn empty_and_whitespace_inputs_return_original() {
        assert_eq!(parse_mode_detail(""), (ModeMarker::Chat, String::new()));
        assert_eq!(
            parse_mode_detail("   \n\t "),
            (ModeMarker::Chat, "   \n\t ".to_string())
        );
    }
}
