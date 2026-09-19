// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Chat panel rendering: 对话主面板（虚拟渲染入口）。
//
// 0.1.9 W8：929 行单文件拆为小模块（view 虚拟视图 / block 消息块 /
// flow 阶段头部与流式尾段 / welcome 空态）。大历史虚拟滚动——每帧只
// 物化与视口相交的行块，行高按稳定消息 id 缓存，对话规模与帧成本解耦。

mod block;
mod flow;
mod view;
mod welcome;

pub use view::ChatView;

use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::app::App;
use crate::theme;

/// 渲染对话主面板。
///
/// 参考 Claude Code 的简洁：无边框、内容直接铺开（靠留白分层），
/// 行级滚动 + 右侧滚动条。视口行的选取由 ChatView::layout 完成，
/// 本函数只做组件装配。
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    // 正文区宽：有对话且宽度允许时，先预留最右 1 列为滚动条轨道——正文
    // 按减 1 后的宽度换行/截断，滚动条画在预留列上，不再覆盖正文最后一列。
    // 预留与否只随"是否有消息"与面板宽度变化（跨帧稳定），滚动条随内容
    // 超出视口才出现，避免行高缓存随滚动抖动失效。
    let reserve_sb = area.width >= 44 && !app.messages.is_empty();
    let body_w = if reserve_sb {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let width = body_w as usize;
    let viewport = area.height as usize;
    // 缓存视图从 App 取出，避免 layout 的 &App 与 &mut chat_view 借用冲突
    let mut view = std::mem::take(&mut app.chat_view);
    let frame = view.layout(app, width, viewport, false);
    app.chat_view = view;

    // 0.1.18 B11（V11.2/V11.3）滚动契约回写：翻页步长 = 当前视口高度，
    // 可滚总量 = 总行数 - 视口高度。渲染每帧回写使 resize 后控制面的
    // 步长与钳位立即跟随，无需事件通知（SSoT 单向：渲染 → 控制面）。
    app.page_step = viewport as u16;
    app.chat_scroll_max = frame.total.saturating_sub(viewport) as u16;

    f.render_widget(Paragraph::new(Text::from(frame.lines)), area);

    // 滚动条：内容超出视口且有对话时显示；画在预留的最右 1 列上
    // （正文布局已按 width-1 排布，不再压住正文最后一列）；窄屏隐藏
    if reserve_sb && frame.total > viewport && area.width >= 44 {
        let sb_area = Rect {
            x: area.right().saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut state = ScrollbarState::new(frame.total)
            .position(frame.from_top)
            .viewport_content_length(viewport);
        let sb = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("█")
            .thumb_style(Style::default().fg(theme::primary()))
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(theme::faint()));
        f.render_stateful_widget(sb, sb_area, &mut state);
    }

    // 0.1.18 B11（V11.3）滚动位置指示：内容超出视口时给出位置反馈——
    // 已到顶显式提示；滚动途中显示 `视口末行/总行`；底部是默认阅读位
    // （最新消息），保持安静不常驻遮挡正文。
    if frame.total > viewport {
        let label = if frame.from_top == 0 {
            Some("已到顶".to_string())
        } else if frame.from_top + viewport < frame.total {
            Some(format!("{}/{}", frame.from_top + viewport, frame.total))
        } else {
            None
        };
        if let Some(text) = label {
            let w = 12.min(area.width);
            let hint = Rect {
                x: area.right().saturating_sub(w),
                y: area.bottom().saturating_sub(1),
                width: w,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(text)
                    .alignment(Alignment::Right)
                    .style(Style::default().fg(theme::faint()).bg(theme::surface())),
                hint,
            );
        }
    }
}

/// 渲染焦点视图（0.1.18 B11/W14，Alt+F）：最近一条回复的全屏只读覆盖层。
///
/// 复用对话消息块的展开态渲染（markdown 全宽重排），标题行给出键位提示；
/// 滚动独立于对话区（快照 + `focus_scroll_max` 每帧回写），位置指示契约
/// 与对话区一致；Esc 返回对话。
pub fn render_focus(f: &mut Frame, area: Rect, app: &mut App) {
    // 单条消息克隆成本可忽略；先取值再回写，避免借用交叠
    let Some(msg) = app.focus_msg.clone() else {
        return;
    };
    let width = area.width as usize;
    let viewport = area.height as usize;
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            " 焦点视图 · Esc 返回 · ↑↓ 滚动 · PgUp/PgDn 翻页",
            Style::default().fg(theme::faint()),
        )),
        Line::raw(""),
    ];
    block::render(&mut lines, &msg, width, true, true);
    app.focus_scroll_max = lines.len().saturating_sub(viewport);
    let first = app.focus_scroll.min(app.focus_scroll_max);
    let body: Vec<Line<'static>> = lines.into_iter().skip(first).take(viewport).collect();
    f.render_widget(Paragraph::new(Text::from(body)), area);

    if let Some(text) = focus_hint(first, app.focus_scroll_max, viewport) {
        let w = 12.min(area.width);
        let hint = Rect {
            x: area.right().saturating_sub(w),
            y: area.bottom().saturating_sub(1),
            width: w,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Right)
                .style(Style::default().fg(theme::faint()).bg(theme::surface())),
            hint,
        );
    }
}

/// 滚动位置文案（对话区与焦点视图共用契约）：已到顶显式提示、滚动途中
/// `末行/总行`、底部安静。
fn focus_hint(from_top: usize, scroll_max: usize, viewport: usize) -> Option<String> {
    if scroll_max == 0 {
        return None;
    }
    if from_top == 0 {
        Some("已到顶".to_string())
    } else if from_top < scroll_max {
        Some(format!("{}/{}", from_top + viewport, scroll_max + viewport))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B11（0.1.18）V11.3 位置指示契约四态：内容未超出视口无滚动量（安静）、
    /// 到顶显式提示、滚动途中显示 `末行/总行`、到底安静（默认阅读位为
    /// 最新消息，不常驻遮挡正文）。
    #[test]
    fn b11_hint_covers_position_states() {
        assert_eq!(focus_hint(0, 0, 10), None, "内容未超出视口：无滚动量");
        assert_eq!(focus_hint(0, 20, 10).as_deref(), Some("已到顶"));
        assert_eq!(focus_hint(5, 20, 10).as_deref(), Some("15/30"));
        assert_eq!(focus_hint(20, 20, 10), None, "到底：默认阅读位保持安静");
    }
}
