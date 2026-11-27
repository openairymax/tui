// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全量会话视图（X8-04a）：Ctrl+O 打开的全屏只读回放。
//
// 与对话区共用同一虚拟视图与行高缓存，只有「展开态固定为全展开」与
// 「独立距底偏移」两处不同——回看长会话不另起一套测量，也不改写对话
// 区视口；偏移为 0 时持续跟随最新，流式输出到结束都能原地阅读。
//
// `/` 打开增量检索（X8-04b）：命中行由整段会话物化行一次性索引，逐键只
// 重跑匹配；检索框贴正文底部居中（X2-20a 底部居中锚点），压住正文与
// 滚动位置浮标（X2-22a z 序），命中以高亮样式层覆写（X2-23a/24a）。

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::engine::grid;
use crate::engine::overlay::{self, Rect as GeoRect};
use crate::engine::search::find_all;
use crate::style;

use super::chat::{focus_hint, ChatView};

/// 顶部键位提示行高度。
const HINT_ROWS: u16 = 1;

/// 键位提示文案：与焦点视图同一「单行常显」体例，Esc 为唯一退出。
const HINT: &str = " 全文会话 · ↑↓ 滚动 · PgUp/PgDn 翻页 · Home/End 顶底 · / 搜索 · Esc 返回";

/// 检索框最大宽度（列）：再宽就失去「浮动条」形态，窄视口取正文宽度。
const BAR_COLS: u16 = 64;

/// 渲染全量会话视图：顶部提示 + 正文（虚拟视口）+ 右下滚动位置。
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height <= HINT_ROWS {
        return;
    }
    let head = Rect {
        height: HINT_ROWS,
        ..area
    };
    let body = Rect {
        y: area.y + HINT_ROWS,
        height: area.height - HINT_ROWS,
        ..area
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            HINT,
            Style::default().fg(style::faint()).bg(style::bar()),
        ))),
        head,
    );

    let viewport = body.height as usize;
    // 检索索引与待兑现跳转先于布局：命中行表要参与本帧高亮，跳转要改变
    // 本帧起始行——放到正文落格之后再算，只能等下一帧才看见目标。
    if app.search.open {
        sync_index(app, body.width);
        if app.search.pending {
            app.search.pending = false;
            app.search_jump();
        }
    }
    // 缓存视图从 App 取出，避免 layout 的 &App 与 &mut chat_view 借用冲突
    let mut view = std::mem::take(&mut app.chat_view);
    let frame = view.layout_from(
        app,
        body.width as usize,
        viewport,
        false,
        app.transcript_scroll,
        true,
    );
    app.chat_view = view;
    // 可滚总量每帧回写（与对话区同一单向契约），偏移越界即钳位。
    app.transcript_scroll_max = frame.total.saturating_sub(viewport) as u16;
    app.transcript_scroll = app.transcript_scroll.min(app.transcript_scroll_max);
    app.page_step = viewport as u16;

    let query = app.search.query.clone();
    let from_top = frame.from_top;
    let marked = if query.is_empty() {
        frame.lines
    } else {
        let cur = app.search.anchor();
        frame
            .lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let idx = (from_top + i) as u32;
                let hit = app.search.hits.binary_search(&idx).is_ok();
                mark(line, &query, hit && cur == Some(idx))
            })
            .collect()
    };

    f.render_widget(
        Paragraph::new(Text::from(marked)).style(Style::default().bg(style::bg())),
        body,
    );

    // 检索框压住滚动位置浮标：同一底部区域出现两个浮层时，获焦者在上。
    // 焦点遮罩（X2-22b）：检索框获焦即正文降一档，视觉焦点收在浮条上；
    // 浮条随后绘制并清除底层降亮修饰，保持满对比。
    if app.search.open {
        crate::present::dim(f, body);
        render_bar(f, body, app);
    } else if let Some(text) = focus_hint(from_top, app.transcript_scroll_max as usize, viewport) {
        render_hint(f, body, &text);
    }
}

/// 检索框：贴正文底部居中（底部居中锚点），单行展示查询与命中计数。
fn render_bar(f: &mut Frame, body: Rect, app: &App) {
    let label = if app.search.query.is_empty() {
        "输入关键字 · Enter 下一条 · Esc 关闭".to_string()
    } else if app.search.hits.is_empty() {
        "无命中".to_string()
    } else {
        format!("{}/{} 条", app.search.cur + 1, app.search.hits.len())
    };
    let text = format!(" / {}  ·  {label} ", app.search.query);
    let inner = body.width.saturating_sub(1) as usize;
    let span = grid::clip(&text, inner);
    let w = (UnicodeWidthStr::width(span.as_str()) as u16 + 1)
        .min(BAR_COLS)
        .min(body.width);
    let Some(bar) =
        overlay::at_bottom_center(GeoRect::new(body.x, body.y, body.width, body.height), w, 1)
    else {
        return;
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" / ", Style::default().fg(style::primary())),
            Span::styled(
                grid::clip(&app.search.query, inner),
                Style::default().fg(style::text()),
            ),
            Span::styled(
                format!("  ·  {label} "),
                Style::default().fg(if app.search.hits.is_empty() {
                    style::warning()
                } else {
                    style::faint()
                }),
            ),
        ]))
        .style(
            Style::default()
                .bg(style::surface_2())
                .remove_modifier(Modifier::DIM),
        ),
        Rect::new(bar.x, bar.y, bar.w, bar.h),
    );
}

/// 滚动位置浮标（右下角单行）。
fn render_hint(f: &mut Frame, body: Rect, text: &str) {
    let w = (UnicodeWidthStr::width(text) as u16 + 2).min(body.width);
    let hint = Rect {
        x: body.right().saturating_sub(w),
        y: body.bottom().saturating_sub(1),
        width: w,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Right)
            .style(Style::default().fg(style::faint()).bg(style::surface())),
        hint,
    );
}

/// 按「会话版本 + 渲染宽度」重建检索索引；命中行号取自整段会话物化行。
///
/// 用独立视图实例物化：索引是旁路查询，不得挤掉正文视图的行高缓存，
/// 否则每次搜索都会让下一帧重新测量整屏。
fn sync_index(app: &mut App, width: u16) {
    let msgs = app.messages.len();
    let last = app.messages.back().map(|m| m.content.len()).unwrap_or(0);
    if !app.search.stale(msgs, width, last) {
        return;
    }
    let mut probe = ChatView::new();
    let full = probe.layout_from(app, width as usize, usize::MAX, true, 0, true);
    let lines = full.lines.iter().map(plain).collect();
    app.search.reindex((msgs, width, last), lines);
}

/// 行转纯文本（拼接各 span 内容，保留原始顺序）。
fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// 命中高亮样式层：当前命中用主色底，其余命中用强调色底，两者都在
/// 原 span 样式上叠加，保留角色色与修饰符。
fn mark(line: Line<'static>, query: &str, active: bool) -> Line<'static> {
    let text = plain(&line);
    let hits = find_all(&text, query);
    if hits.is_empty() {
        return line;
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut pos = 0usize;
    for span in line.spans {
        let s = span.content.to_string();
        let start = pos;
        let end = pos + s.len();
        let mut local = 0usize;
        for &(h0, h1) in hits.iter().filter(|(a, b)| *b > start && *a < end) {
            let a = h0.max(start) - start;
            let b = h1.min(end) - start;
            if a > local {
                out.push(Span::styled(s[local..a].to_string(), span.style));
            }
            out.push(Span::styled(
                s[a..b].to_string(),
                span.style
                    .fg(style::on_color())
                    .bg(if active {
                        style::primary()
                    } else {
                        style::accent()
                    })
                    .add_modifier(Modifier::BOLD),
            ));
            local = b;
        }
        if local < s.len() {
            out.push(Span::styled(s[local..].to_string(), span.style));
        }
        pos = end;
    }
    let mut marked = Line::from(out);
    marked.style = line.style;
    marked.alignment = line.alignment;
    marked
}
