// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 浮层：依赖降级横幅、前缀键提示、瞬态提示、IME 候选区、工具审批条。

use crate::app::App;
use crate::engine::grid;
use crate::engine::health::dur_cn;
use crate::engine::overlay;
use crate::style;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

/// 浮层合成入口（X2-18a）：把 `w × h` 浮层贴到视口右下角，先裁剪再写入。
///
/// 裁剪发生在写入之前，内容渲染器只拿到完整矩形，因此一次合成内不会出现
/// 「部分行列已写、部分被后端截断」的半截浮层。空视口或空请求不成画，
/// 返回 `false` 且不调用渲染器。
pub(super) fn compose_bottom_right<D>(
    buf: &mut Buffer,
    view: Rect,
    w: u16,
    h: u16,
    draw: D,
) -> bool
where
    D: FnOnce(&mut Buffer, Rect),
{
    let Some(area) = overlay::at_bottom_right(eng_rect(view), w, h) else {
        return false;
    };
    draw(buf, term_rect(area));
    true
}

fn eng_rect(r: Rect) -> overlay::Rect {
    overlay::Rect::new(r.x, r.y, r.width, r.height)
}

fn term_rect(r: overlay::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// 近似半透明（X2-19b）：把色号降一档，配合 DIM 让内容退向背景。
///
/// 终端没有 alpha 通道，无法真的与背景混合，只能沿色阶近似：真彩色按
/// 一半亮度收敛；16 色按「亮 → 常 → 灰 → 黑」下移；256 色的亮色档
/// （8~15）同样下移，16 档以上保留色号——色立方本身已暗，再降会丢失
/// 前景与错误色的可区分性。`Reset` 跟随终端默认色，保持不动，避免把
/// 主题底色写死成黑。
fn shade(c: Color) -> Color {
    match c {
        Color::Reset => Color::Reset,
        Color::Rgb(r, g, b) => Color::Rgb(r / 2, g / 2, b / 2),
        Color::Indexed(i @ 8..=15) => Color::Indexed(i - 8),
        Color::Indexed(i) => Color::Indexed(i),
        Color::LightRed => Color::Red,
        Color::LightGreen => Color::Green,
        Color::LightYellow => Color::Yellow,
        Color::LightBlue => Color::Blue,
        Color::LightMagenta => Color::Magenta,
        Color::LightCyan => Color::Cyan,
        Color::White => Color::Gray,
        Color::Gray => Color::DarkGray,
        Color::Black | Color::DarkGray => Color::Black,
        c @ (Color::Red
        | Color::Green
        | Color::Yellow
        | Color::Blue
        | Color::Magenta
        | Color::Cyan) => c,
    }
}

/// 遮罩（X2-19a/b）：把区域内已有单元格整体压暗一档，区域外一格不动。
///
/// 终端没有 alpha 通道，跨平台一致的降亮手段是 `Modifier::DIM`（终端按
/// 自身调色板降亮前景）叠加色号降档：不换符号，宽字符与颜色语义保持。
/// 区域先与缓冲区求交——布局给的是屏幕矩形，缓冲区可能只覆盖局部，
/// 直接按下标写会越界 panic。
pub(super) fn scrim(buf: &mut Buffer, area: Rect) {
    let Some(area) = eng_rect(area).clip(eng_rect(buf.area)) else {
        return;
    };
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = shade(cell.fg);
            cell.bg = shade(cell.bg);
            cell.modifier |= Modifier::DIM;
        }
    }
}

/// 依赖降级横幅（0.1.19 U12，TUI 方案 §6.12 #1/#6）：单行常显最重依赖，
/// 附真实存在的修复命令；恢复后由状态机撤销并进入冷却期，抖动计数进 /health。
pub(super) fn render_degraded(f: &mut Frame, area: Rect, app: &App) {
    // 持续时长按注入的单调时刻计算：刚降级时不加噪声，超过 10s 才显示。
    let age = app.clock_ms.saturating_sub(app.banner.since_ms());
    let since = if age >= 10_000 {
        format!(" · 已持续 {} ", dur_cn(age))
    } else {
        String::new()
    };
    let line = Line::from(vec![
        Span::styled(
            " ▲ ",
            Style::default()
                .fg(style::on_color())
                .bg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} {} ", app.banner.text(), since),
            Style::default()
                .fg(style::on_color())
                .bg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  修复：{} ", app.banner.remedy()),
            Style::default().fg(style::warning()),
        ),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(style::surface())),
        area,
    );
}

/// leader 提示浮层（0.1.19 L7 U8，TUI 方案 §6.4 #7）：单行说明可用续键，
/// 前缀序列的取消与超时由状态机负责，此处只读展示。
pub(super) fn render_leader_hint(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" Ctrl+G ", Style::default().fg(style::primary())),
        Span::styled(
            "前缀 · y 复制最近代码块 · Esc 取消",
            Style::default().fg(style::dim()),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// 瞬态提示条（0.1.19 L7 U7，TUI 方案 §6.3 #4）：输入栏上方逐行展示，
/// 先进先出、同屏不超过三条。文本按显示宽度裁剪，长提示不撑破版面。
pub(super) fn render_toasts(f: &mut Frame, area: Rect, app: &App) {
    let limit = area.width.saturating_sub(4) as usize;
    let lines: Vec<Line> = app
        .toasts()
        .active()
        .map(|text| {
            Line::from(vec![
                Span::styled(
                    " ✓ ",
                    Style::default()
                        .fg(style::success())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(grid::clip(text, limit), Style::default().fg(style::text())),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(style::surface_2())),
        area,
    );
}

/// IME 候选区（输入框第二行）：`[中] 拼音` + 当前页候选。
///
/// 2.2.3 重新设计（2026-08-23）：Ctrl+1 激活后输入框恒为两行（第一行
/// 输入 + 第二行候选区），本函数渲染第二行。拼音缓冲为空时显示
/// `[中] 拼音输入中…` 占位，让激活瞬间即有明确视觉反馈；缓冲非空
/// 时显示拼音高亮 + 微信式分页候选（当前页 9 个，页内高亮蓝底，
/// 多页时尾部页码指示 ‹1/2›；空格/Enter 上屏高亮候选，数字选字，
/// ,/. 或 PgUp/PgDn 翻页，←/→ 移动高亮）。与 C 侧 CLI 的
/// tui_ime_draw_cands 语义一致。
pub(super) fn render_ime_cands(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(
            "[中]",
            Style::default()
                .fg(style::on_color())
                .bg(style::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if app.ime_buf.is_empty() {
        spans.push(Span::styled(
            "拼音输入中…（a-z 拼音 · 1-9 选字 · 空格/Enter 上屏 · ,/. 翻页 · Esc 取消）",
            Style::default().fg(style::faint()),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {} ", app.ime_buf),
            Style::default()
                .fg(style::on_color())
                .bg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        // 当前页切片（微信式分页：每页 9 个，页内高亮）
        let start = app.ime_page * 9;
        let end = (start + 9).min(app.ime_cands.len());
        for (off, cand) in app.ime_cands[start..end].iter().enumerate() {
            let tag = format!("{}.{} ", off + 1, cand);
            if off == app.ime_sel {
                spans.push(Span::styled(
                    tag,
                    Style::default()
                        .fg(style::on_color())
                        .bg(style::primary())
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(tag, Style::default().fg(style::dim())));
            }
        }
        // 页码指示（多页时显示 ‹cur/total›）
        if app.ime_pages > 1 {
            spans.push(Span::styled(
                format!("‹{}/{}›", app.ime_page + 1, app.ime_pages),
                Style::default().fg(style::faint()),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(style::surface())),
        area,
    );
}

/// 工具审批提示条：pending 审批时在输入栏上方高亮显示（Claude Code 风格
/// permission prompt），给出工具、主体与决议快捷提示。
///
/// 审批决议标签：渲染与命中测试共用的唯一文案表（X5-11a）。
const ACT_LABELS: [&str; 3] = ["[y] 允许本次", "[a] 允许本次（总是待支持）", "[n] 拒绝（Esc）"];
/// 决议段之间的间隔列数。
const ACT_GAP: u16 = 2;

/// 决议段总宽（显示列）。
fn act_width() -> u16 {
    let text: u16 = ACT_LABELS
        .iter()
        .map(|l| grid::width(l) as u16)
        .sum::<u16>();
    text + ACT_GAP * (ACT_LABELS.len() as u16 - 1)
}

/// 决议段列区间：横幅第二行右对齐，点击与文字用同一算式对齐。
pub(crate) fn approval_actions(area: Rect) -> [(Rect, u8); 3] {
    let mut x = area.x + area.width.saturating_sub(act_width());
    let mut out = [(Rect::new(0, 0, 0, 0), 0); 3];
    for (i, label) in ACT_LABELS.iter().enumerate() {
        let w = grid::width(label) as u16;
        out[i] = (Rect::new(x, area.y + 1, w, 1), i as u8);
        x = x.saturating_add(w + ACT_GAP);
    }
    out
}

/// 命中决议段：行须在决议行，列须落在某段标签内（间隙不命中）。
pub(crate) fn approval_pick(area: Rect, col: u16, row: u16) -> Option<u8> {
    approval_actions(area)
        .into_iter()
        .find(|(r, _)| {
            row >= r.y && row < r.y.saturating_add(r.height) && col >= r.x && col < r.x + r.width
        })
        .map(|(_, i)| i)
}

pub(super) fn render_approval_banner(f: &mut Frame, area: Rect, app: &App) {
    let Some(a) = app.approvals.first() else {
        return;
    };
    // 参数预览单行截断：决议段固定在行尾，参数预算先扣掉段宽与前后间隔，
    // 长 JSON 只挤自己，不再把决议提示挤出横幅。
    let act_w = act_width();
    let budget = area.width.saturating_sub(act_w + 5) as usize;
    let params_preview = grid::clip(&a.params, budget);
    let line1 = Line::from(vec![
        Span::styled(
            " ⚠ ",
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "工具审批请求",
            Style::default()
                .fg(style::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.approvals.len() > 1 {
                format!("  ·  1 / {}", app.approvals.len())
            } else {
                String::new()
            },
            Style::default().fg(style::faint()),
        ),
        Span::styled(
            format!(
                "  ·  主体: {}",
                if a.agent_id.is_empty() {
                    "unknown"
                } else {
                    &a.agent_id
                }
            ),
            Style::default().fg(style::dim()),
        ),
        Span::styled(
            format!("  ·  工具: {}", a.tool),
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  ·  时间: {}",
                a.created_at
                    .map(|ts| chrono::DateTime::from_timestamp(ts as i64, 0)
                        .map(|dt| dt.format("%H:%M:%S").to_string())
                        .unwrap_or_else(|| "—".to_string()))
                    .unwrap_or_else(|| "—".to_string())
            ),
            Style::default().fg(style::faint()),
        ),
    ]);
    // 决议段右对齐：段起点与 approval_actions 的算式一致，点击落点即段。
    let used = (3 + grid::width(&params_preview)) as u16;
    let pad = area.width.saturating_sub(used + act_w) as usize;
    let line2 = Line::from(vec![
        Span::raw(format!("   {params_preview}{}", " ".repeat(pad))),
        Span::styled(
            ACT_LABELS[0],
            Style::default()
                .fg(style::success())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            ACT_LABELS[1],
            Style::default()
                .fg(style::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            ACT_LABELS[2],
            Style::default()
                .fg(style::danger())
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(Text::from(vec![line1, line2])).style(Style::default().bg(style::surface())),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::{compose_bottom_right, scrim};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Text},
        widgets::{Block, Paragraph, Widget},
    };

    fn dotted(w: u16, h: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        for y in 0..h {
            for x in 0..w {
                buf[(x, y)].set_symbol("·");
            }
        }
        buf
    }

    /// X2-18b：一次合成内整块浮层落格，行行完整，不留半截。
    #[test]
    fn overlay_is_written_whole_in_one_pass() {
        let view = Rect::new(0, 0, 20, 5);
        let mut buf = dotted(20, 5);
        let drawn = compose_bottom_right(&mut buf, view, 6, 3, |b, r| {
            assert_eq!(r, Rect::new(14, 2, 6, 3), "贴右下角且尺寸不缩水");
            let text = Text::from(vec![
                Line::from("AAAAAA"),
                Line::from("BBBBBB"),
                Line::from("CCCCCC"),
            ]);
            Paragraph::new(text).render(r, b);
        });
        assert!(drawn);
        for y in 2..5 {
            for x in 14..20 {
                assert_ne!(buf[(x, y)].symbol(), "·", "浮层格 ({x},{y}) 未写入");
            }
        }
        assert_eq!(buf[(0, 0)].symbol(), "·", "浮层外不得被改写");
        assert_eq!(buf[(13, 4)].symbol(), "·", "浮层左邻格不得被改写");
    }

    /// X2-18b：请求大于视口时先裁到视口，写入不越界也不 panic。
    #[test]
    fn oversized_request_is_clipped_before_write() {
        let view = Rect::new(0, 0, 10, 4);
        let mut buf = dotted(10, 4);
        let drawn = compose_bottom_right(&mut buf, view, 40, 9, |b, r| {
            assert_eq!(r, view, "越界请求收敛为整视口");
            Block::default()
                .style(Style::default().bg(Color::Blue))
                .render(r, b);
        });
        assert!(drawn);
        for y in 0..4 {
            for x in 0..10 {
                assert_eq!(buf[(x, y)].bg, Color::Blue, "裁剪矩形应覆盖整视口");
            }
        }
    }

    /// X2-18b：空视口不成画，渲染器不被调用，缓冲不留残迹。
    #[test]
    fn empty_view_skips_draw() {
        let mut buf = dotted(4, 2);
        let mut called = false;
        let drawn = compose_bottom_right(&mut buf, Rect::new(0, 0, 0, 0), 3, 1, |_, _| {
            called = true;
        });
        assert!(!drawn);
        assert!(!called);
        assert_eq!(buf[(0, 0)].symbol(), "·");
    }

    /// X2-19a：遮罩只压暗区内单元格，区外一格不碰。
    #[test]
    fn scrim_dims_inside_only() {
        let mut buf = dotted(6, 3);
        scrim(&mut buf, Rect::new(1, 1, 3, 1));
        for x in 1..4 {
            assert!(
                buf[(x, 1)].modifier.contains(Modifier::DIM),
                "区内 ({x},1) 应被压暗"
            );
        }
        for (x, y) in [(0, 1), (4, 1), (1, 0), (1, 2)] {
            assert!(
                !buf[(x, y)].modifier.contains(Modifier::DIM),
                "区外 ({x},{y}) 不得被压暗"
            );
        }
    }

    /// X2-19a：遮罩区域先与缓冲区求交，越界部分不写、不 panic。
    #[test]
    fn scrim_clips_to_buffer() {
        let mut buf = dotted(4, 2);
        scrim(&mut buf, Rect::new(2, 1, 10, 10));
        assert!(buf[(3, 1)].modifier.contains(Modifier::DIM));
        assert!(!buf[(1, 1)].modifier.contains(Modifier::DIM));
    }

    /// X2-19b 视觉 golden：遮罩结果落在固定色阶表上，逐格可离线复算。
    #[test]
    fn scrim_shading_follows_fixed_tone_ladder() {
        let cases = [
            (Color::LightBlue, Color::White, Color::Blue, Color::Gray),
            (
                Color::Rgb(200, 100, 50),
                Color::Indexed(12),
                Color::Rgb(100, 50, 25),
                Color::Indexed(4),
            ),
            (Color::Reset, Color::Reset, Color::Reset, Color::Reset),
            (Color::Gray, Color::Black, Color::DarkGray, Color::Black),
        ];
        let mut buf = dotted(cases.len() as u16, 1);
        for (x, (fg, bg, _, _)) in cases.iter().enumerate() {
            buf[(x as u16, 0)].set_fg(*fg);
            buf[(x as u16, 0)].set_bg(*bg);
        }
        scrim(&mut buf, Rect::new(0, 0, cases.len() as u16, 1));
        for (x, (_, _, want_fg, want_bg)) in cases.iter().enumerate() {
            let cell = &buf[(x as u16, 0)];
            assert_eq!((cell.fg, cell.bg), (*want_fg, *want_bg), "第 {x} 档色阶");
            assert!(cell.modifier.contains(Modifier::DIM), "第 {x} 档须带 DIM");
        }
    }
}

#[cfg(test)]
mod act_tests {
    use super::*;

    #[test]
    fn action_segments_are_right_aligned_and_disjoint() {
        let area = Rect::new(0, 10, 100, 2);
        let acts = approval_actions(area);
        assert_eq!(acts[2].0.x + acts[2].0.width, 100);
        for pair in acts.windows(2) {
            let (a, _) = pair[0];
            let (b, _) = pair[1];
            assert!(a.x + a.width < b.x, "决议段不得重叠");
        }
    }

    #[test]
    fn pick_maps_columns_to_same_segment() {
        let area = Rect::new(0, 10, 100, 2);
        for (idx, (r, want)) in approval_actions(area).into_iter().enumerate() {
            assert_eq!(approval_pick(area, r.x, 11), Some(want), "第 {idx} 段起点");
            assert_eq!(
                approval_pick(area, r.x + r.width - 1, 11),
                Some(want),
                "第 {idx} 段末列"
            );
        }
    }

    #[test]
    fn title_row_and_gaps_do_not_decide() {
        let area = Rect::new(0, 10, 100, 2);
        assert_eq!(approval_pick(area, 99, 10), None);
        let (first, _) = approval_actions(area)[0];
        assert_eq!(approval_pick(area, first.x - 1, 11), None);
        assert_eq!(approval_pick(area, 99, 12), None);
    }
}

