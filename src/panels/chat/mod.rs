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
    layout::Rect,
    style::Style,
    text::Text,
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
    let width = area.width as usize;
    let viewport = area.height as usize;
    // 缓存视图从 App 取出，避免 layout 的 &App 与 &mut chat_view 借用冲突
    let mut view = std::mem::take(&mut app.chat_view);
    let frame = view.layout(app, width, viewport, false);
    app.chat_view = view;

    f.render_widget(Paragraph::new(Text::from(frame.lines)), area);

    // 滚动条：内容超出视口且有对话时显示；窄屏（<44 列）隐藏避免挤占
    if frame.total > viewport && !app.messages.is_empty() && area.width >= 44 {
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
}
