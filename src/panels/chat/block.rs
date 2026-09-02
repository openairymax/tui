// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话消息块渲染（0.1.9 W8）：单条消息 → 行块、回合分隔线、思考链块内折叠。
//
// 折叠从旧「全局行区间折叠」改为「块内折叠」：每条 System 消息的行区间
// 互不重叠且恰为一个块，块内截断与旧全局区间折叠严格等价。

use ratatui::{
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::app::{App, ChatMessage, MessageRole};
use crate::theme;

/// 长回复折叠（2026-08-17，与 C 版 airy_cli 对齐）：折叠仅作用于 System
/// 思考链消息（Agent 最终答复不折叠，完整展示——用户诉求「折叠思考链，
/// 完整展示结果」）。渲染行数超过 FOLD_MAX_LINES 时，live 视口只显示前
/// FOLD_KEEP_LINES 行 + 折叠尾；Alt+E 浏览展开（0.1.7：与滚动解耦，
/// 展开会改变总行数导致视口跳变）。阈值与 C 版 CLI_REPLY_FOLD_MAX=6 一致。
pub(super) const FOLD_MAX_LINES: usize = 6;
pub(super) const FOLD_KEEP_LINES: usize = 3;

/// 工具角色（调用/结果）：连续工具消息紧凑呈现（Claude Code 风格，间不留空行）。
pub(super) fn is_tool(role: MessageRole) -> bool {
    matches!(role, MessageRole::ToolCall | MessageRole::ToolResult)
}

/// 回合分隔线：`── Worked for Ns · N tok · $x ──`（Claude Code 惯例，C 版 airy_cli 同款）。
///
/// 展示上一回合耗时 + 会话累计用量；无已结算回合（首回合）时降级为细分隔线。
/// 行数恒 2（分隔线 + 空行），虚拟化按固定行数计（view::SEP_LINES）。
pub(super) fn push_turn_separator(out: &mut Vec<Line<'static>>, app: &App) {
    let (elapsed, metrics) = match app.last_turn_elapsed {
        Some(started) => {
            let secs = started.elapsed().as_secs_f64();
            (
                format!(" Worked for {:.1}s", secs),
                format!(" · {} tok · ${:.4}", app.tokens, app.cost),
            )
        }
        None => (String::new(), String::new()),
    };
    let sep = "─".repeat(6);
    let text = format!("{}{}{}{}", sep, elapsed, metrics, sep);
    out.push(Line::from(Span::styled(
        text,
        Style::default().fg(theme::faint()),
    )));
    out.push(Line::raw(""));
}

/// 渲染一条消息为一个行块：角色头部 + 内容 +（非 compact 时）尾部空行；
/// System 思考链长块块内折叠。
///
/// 角色命名规范（与 C 版 airy_cli / 文档 03-cli-reference 对齐）：
///   [For Thee]       青   用户（操作者）输入
///   [Super Agent]    绿   agentrt 本体：最终答复与决策
///   [Dual Think]     黄   系统级思考：GCCP / 双思考 / 蓝图路由轨迹
///   [Sub xxx Agent]  品红 子代理与执行体（xxx = 代理类型，取自工具名）
/// 时间戳展示在头部行；内容固定缩进 4 列；用户消息以气泡背景色块区分，
/// 其余角色左对齐流式。工具调用/结果单行紧凑预览（截断 + …）。
pub(super) fn render(
    out: &mut Vec<Line<'static>>,
    msg: &ChatMessage,
    width: usize,
    compact: bool,
    expanded: bool,
) {
    let start = out.len();
    push_header(out, msg);
    push_content(out, msg, width);
    // 消息间留白（替代粗分隔线，视觉更轻）；连续工具消息（compact）不插空行
    if !compact {
        out.push(Line::raw(""));
    }
    if msg.role == MessageRole::System {
        fold_block(out, start, expanded);
    }
}

/// 头部行：角色名（加粗语义色）+ 状态图标 + 时间戳（极弱色）。
fn push_header(out: &mut Vec<Line<'static>>, msg: &ChatMessage) {
    let (name, color) = match msg.role {
        MessageRole::User => ("[For Thee]".to_string(), theme::primary()),
        MessageRole::Agent => ("[Super Agent]".to_string(), theme::success()),
        MessageRole::System => ("[Dual Think]".to_string(), theme::dim()),
        MessageRole::ToolCall | MessageRole::ToolResult => {
            // [Sub <tag> Agent]：tag 取工具名（ToolCall 首 token）；ToolResult
            // 是工具结果（JSON 等），无工具名可识别时回退 "exec"
            let first = msg.content.split_whitespace().next().unwrap_or("");
            let is_jsonish =
                first.starts_with('{') || first.starts_with('[') || first.starts_with('"');
            let tag = if first.is_empty() || is_jsonish { "exec" } else { first };
            let tag: String = tag.chars().take(12).collect();
            // 工具调用品红（调用侧）· 工具结果青（回传侧），Claude Code 风格
            let c = if msg.role == MessageRole::ToolCall {
                theme::magenta()
            } else {
                theme::cyan()
            };
            (format!("[Sub {} Agent]", tag), c)
        }
    };

    // 状态图标（头部行右侧，语义色）：调用 ✓ 成功 / ✗ 失败（content 含
    // 「（失败）」标记判定，2026-08-17 适配过程化格式）；结果 ▸ 回传。
    let (icon, icon_color) = match msg.role {
        MessageRole::ToolCall => {
            if msg.content.contains("（失败）") {
                (" ✗", theme::danger())
            } else {
                (" ✓", theme::success())
            }
        }
        MessageRole::ToolResult => (" ▸", theme::faint()),
        _ => ("", theme::faint()),
    };

    out.push(Line::from(vec![
        Span::styled(
            name,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            icon.to_string(),
            Style::default().fg(icon_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", msg.timestamp),
            Style::default().fg(theme::faint()),
        ),
    ]));
}

/// 内容段：统一固定缩进 4 列（Claude 式克制排版）。工具行为单行紧凑
/// 预览；其余走 markdown 渲染；用户消息叠加气泡背景色块。
fn push_content(out: &mut Vec<Line<'static>>, msg: &ChatMessage, width: usize) {
    const INDENT: usize = 4;

    // 工具调用/结果与思考链为次要信息：轻量 dim 色
    let base = match msg.role {
        MessageRole::ToolCall | MessageRole::ToolResult | MessageRole::System => {
            Style::default().fg(theme::dim())
        }
        _ => Style::default().fg(theme::text()),
    };

    if msg.content.is_empty() {
        out.push(Line::from(Span::styled(
            format!("{:width$}（空）", "", width = INDENT),
            Style::default().fg(theme::faint()),
        )));
    } else if is_tool(msg.role) {
        let max = width.saturating_sub(INDENT + 3).max(8);
        let first_line = msg.content.lines().next().unwrap_or("").trim();
        let mut preview: String = first_line.chars().take(max).collect();
        if first_line.chars().count() > max || msg.content.lines().count() > 1 {
            preview.push('…');
        }
        out.push(Line::from(Span::styled(
            format!("{:width$}{}", "", preview, width = INDENT),
            base,
        )));
    } else {
        let mut rendered = crate::markdown::render(&msg.content, INDENT, width, base);
        // 用户消息：气泡背景色块（Claude 式左右分层）；Airymax 回复左对齐流式
        if msg.role == MessageRole::User {
            for line in rendered.iter_mut() {
                let bg = theme::surface_active();
                let spans: Vec<Span<'static>> = line.spans.iter().map(|s| s.clone().bg(bg)).collect();
                *line = Line::from(spans);
            }
        }
        out.extend(rendered);
    }
}

/// 块内折叠：`[start, out.len())` 为一个 System 块，超阈值截断为
/// FOLD_KEEP_LINES 行 + 折叠尾（展开态/短块原样保留）。
fn fold_block(out: &mut Vec<Line<'static>>, start: usize, expanded: bool) {
    let len = out.len() - start;
    if expanded || len <= FOLD_MAX_LINES {
        return;
    }
    let more = len - FOLD_KEEP_LINES;
    out.truncate(start + FOLD_KEEP_LINES);
    out.push(Line::from(Span::styled(
        format!("  └ … {more} more lines · Alt+E 展开"),
        Style::default().fg(theme::faint()),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatMessage;

    fn msg(role: MessageRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            timestamp: "00:00:00".to_string(),
            id: ChatMessage::NO_ID,
        }
    }

    /// 角色命名规范：头部行必须为 [For Thee]/[Super Agent]/[Dual Think]/[Sub xxx Agent]。
    #[test]
    fn role_headers_follow_naming_scheme() {
        let cases = [
            (MessageRole::User, "[For Thee]", "hi"),
            (MessageRole::Agent, "[Super Agent]", "hello!"),
            (MessageRole::System, "[Dual Think]", "GCCP 提示"),
            (MessageRole::ToolCall, "[Sub web_fetch Agent]", "web_fetch {\"url\":\"x\"}"),
            (MessageRole::ToolResult, "[Sub exec Agent]", "{\"ok\":1}"),
        ];
        for (role, expect_head, content) in cases.iter() {
            let mut lines: Vec<Line<'static>> = Vec::new();
            render(&mut lines, &msg(*role, content), 80, false, false);
            assert!(!lines.is_empty(), "role {:?}: 应至少渲染头部行", role);
            let head = lines[0].to_string();
            assert!(
                head.starts_with(expect_head),
                "role {:?}: 头部应为 '{}'，实际 '{}'",
                role,
                expect_head,
                head
            );
        }
    }

    /// 子代理 tag 兜底：空内容 / 无工具名时回退 "exec"。
    #[test]
    fn sub_agent_tag_fallback() {
        let mut lines: Vec<Line<'static>> = Vec::new();
        render(&mut lines, &msg(MessageRole::ToolResult, ""), 80, false, false);
        assert!(lines[0].to_string().starts_with("[Sub exec Agent]"));
    }

    /// 超长工具名 tag 截断到 12 字符，保持 "[Sub xxx Agent]" 结构完整。
    #[test]
    fn sub_agent_tag_truncated() {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let m = msg(MessageRole::ToolCall, "very_long_tool_name_that_exceeds_budget args");
        render(&mut lines, &m, 80, false, false);
        assert!(lines[0].to_string().starts_with("[Sub very_long_to Agent]"));
    }

    /// 块内折叠：长思考链截断为 KEEP 行 + 折叠尾；展开态/短块/Agent 长文不折叠。
    #[test]
    fn fold_block_collapses_long_system() {
        // 段落以空行分隔（markdown 软换行会合并单 \n 行，须逐段独立成块）
        let long: String = (0..12)
            .map(|i| format!("para {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut folded: Vec<Line<'static>> = Vec::new();
        render(&mut folded, &msg(MessageRole::System, &long), 80, false, false);
        assert_eq!(folded.len(), FOLD_KEEP_LINES + 1, "折叠后 = KEEP 行 + 折叠尾");
        let tail = folded.last().unwrap().to_string();
        assert!(tail.contains("more lines") && tail.contains("展开"), "折叠尾: {tail}");

        let mut expanded: Vec<Line<'static>> = Vec::new();
        render(&mut expanded, &msg(MessageRole::System, &long), 80, false, true);
        assert!(expanded.len() > FOLD_MAX_LINES, "展开态显示全量");
        let joined: String = expanded.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("para 11"), "展开态含末段");

        // 短思考链（≤ 阈值）不折叠（行数 = 头部 + 段落 + 段落留白 + 块留白）
        let mut short: Vec<Line<'static>> = Vec::new();
        render(&mut short, &msg(MessageRole::System, "one line"), 80, false, false);
        assert!(short.len() <= FOLD_MAX_LINES, "短块不应触发折叠: {}", short.len());
        assert!(!short.last().unwrap().to_string().contains("more lines"));

        // Agent 最终答复不折叠（完整展示）
        let mut agent: Vec<Line<'static>> = Vec::new();
        render(&mut agent, &msg(MessageRole::Agent, &long), 80, false, false);
        assert!(!agent.last().unwrap().to_string().contains("more lines"), "答复不折叠");
    }

    /// 折叠块与后续块级联：折叠仅作用于自身区间，不侵蚀相邻块。
    #[test]
    fn fold_block_scoped_to_own_range() {
        let long: String = (0..10)
            .map(|i| format!("seg {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut lines: Vec<Line<'static>> = Vec::new();
        render(&mut lines, &msg(MessageRole::User, "q"), 80, false, false);
        let user_len = lines.len();
        render(&mut lines, &msg(MessageRole::System, &long), 80, false, false);
        assert_eq!(lines.len(), user_len + FOLD_KEEP_LINES + 1, "System 块折叠后总行数");
        // 折叠尾之后继续追加块不受影响
        render(&mut lines, &msg(MessageRole::Agent, "a"), 80, false, false);
        let text: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(text.contains("[Super Agent]"), "后续块完整");
    }
}
