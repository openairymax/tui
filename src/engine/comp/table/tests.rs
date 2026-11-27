// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 表格组件用例（X3-01b/04b/07b/08b/09/10/11/12）：测量边界、约束求解、
// 零溢出与省略号裁剪、三档对齐 golden、稳定排序、选中底色与导航夹取。
// 单文件体积守则：表实现与用例分文件，两侧各自保持在 500 行以内。

use super::*;
use crate::engine::grid;

/// 文本列：显示行文本本身，宽度约束由用例给出。
fn text_col(width: Width, align: Alignment) -> Column<&'static str> {
    Column::new(
        width,
        align,
        |r| (r.to_string(), Style::default()),
        |r| r.to_string(),
    )
}

fn id_col() -> Column<&'static str> {
    Column::new(
        Width::Max(8),
        Alignment::Left,
        |r| (r.to_string(), Style::default()),
        |r| r.to_string(),
    )
}

/// 三列各取一个字段的行类型（看板形态：名称/状态/进度）。
type Row3 = (&'static str, &'static str, &'static str);

fn tri_cols() -> Vec<Column<Row3>> {
    vec![
        Column::new(
            Width::Min(8),
            Alignment::Left,
            |r: &Row3| (r.0.to_string(), Style::default()),
            |r: &Row3| r.0.to_string(),
        ),
        Column::new(
            Width::Max(10),
            Alignment::Left,
            |r: &Row3| (r.1.to_string(), Style::default()),
            |r: &Row3| r.1.to_string(),
        ),
        Column::new(
            Width::Flex(1),
            Alignment::Right,
            |r: &Row3| (r.2.to_string(), Style::default()),
            |r: &Row3| r.2.to_string(),
        ),
    ]
}

fn snapshot<T>(tbl: &mut Table<T>, w: u16, h: u16) -> Vec<String> {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    tbl.layout(area);
    tbl.render(&mut buf);
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// X3-01b：0 宽/0 高与常规空间的测量边界。
#[test]
fn measure_handles_degenerate_space() {
    let mut tbl = Table::new(vec![id_col()]);
    tbl.set_rows(vec!["alpha", "beta"]);
    assert_eq!(tbl.measure(Size { w: 0, h: 0 }), Size { w: 0, h: 0 });
    let s = tbl.measure(Size { w: 40, h: 10 });
    assert!(s.w <= 40 && s.h == 2, "{s:?}");
    let s = tbl.measure(Size { w: 40, h: 1 });
    assert_eq!(s.h, 1);
}

/// X3-07b：四档宽度下列宽总占用永不超出可用宽度。
#[test]
fn four_widths_never_overflow() {
    let mut tbl = Table::new(vec![
        text_col(Width::Min(8), Alignment::Left),
        text_col(Width::Max(12), Alignment::Left),
        text_col(Width::Flex(1), Alignment::Right),
        id_col(),
    ]);
    tbl.set_rows(vec!["任务名称很长很长", "running"]);
    for avail in [40usize, 60, 80, 120] {
        let widths = tbl.widths(avail - tbl.gaps());
        let used: usize = widths.iter().sum::<usize>() + tbl.gaps();
        assert!(used <= avail, "avail={avail} used={used} widths={widths:?}");
    }
}

/// X3-07b：约束语义——Min 不低于下限、Max 不超过上限、Flex 吸收余量。
#[test]
fn min_max_flex_respected() {
    let widths = [Width::Min(10), Width::Max(12), Width::Flex(2)];
    let out = solve(&widths, &[3, 30, 3], 100);
    assert!(out[0] >= 10, "{out:?}");
    assert!(out[1] <= 12, "{out:?}");
    assert!(out[2] > 3, "{out:?}");
    assert!(out.iter().sum::<usize>() <= 100, "{out:?}");
}

/// X3-08b + X3-09a：40 列视口下内容裁剪不越界，且以省略号收尾。
#[test]
fn narrow_view_clips_with_ellipsis() {
    let mut tbl = Table::new(vec![text_col(Width::Max(40), Alignment::Left)]);
    tbl.set_rows(vec!["超长内容会被裁剪"]);
    let lines = snapshot(&mut tbl, 12, 1);
    assert_eq!(grid::width(&lines[0]), 12, "{lines:?}");
    assert!(lines[0].contains('…'), "{lines:?}");
}

/// X3-08b（IS-34）：40 列多列布局右边界零溢出。
#[test]
fn forty_columns_never_overflow() {
    let mut tbl = Table::new(tri_cols());
    tbl.set_rows(vec![("全角名称测试", "执行中状态", "▰▰▰▰▱▱▱▱ 50%")]);
    tbl.set_marker(true);
    for line in snapshot(&mut tbl, 40, 1) {
        let w: usize = line.chars().map(grid::char_w).sum();
        assert!(w <= 40, "行宽 {w} 超出 40：{line:?}");
    }
}

/// X3-09b：左/右/中三档对齐的逐格 golden。
#[test]
fn alignment_golden() {
    for (align, want) in [
        (Alignment::Left, "ab    "),
        (Alignment::Right, "    ab"),
        (Alignment::Center, "  ab  "),
    ] {
        let mut tbl = Table::new(vec![text_col(Width::Max(6), align)]);
        tbl.set_rows(vec!["ab"]);
        assert_eq!(snapshot(&mut tbl, 6, 1)[0], want, "{align:?}");
    }
}

/// X3-10b：等值键在稳定排序后保持输入序。
#[test]
fn stable_sort_keeps_input_order() {
    let mut tbl = Table::new(vec![Column::new(
        Width::Max(10),
        Alignment::Left,
        |r: &(&str, &str)| (r.1.to_string(), Style::default()),
        |r: &(&str, &str)| r.0.to_string(),
    )]);
    tbl.set_rows(vec![("k", "1"), ("k", "2"), ("a", "3")]);
    tbl.sort_by(0, true);
    let order: Vec<&str> = tbl.rows.iter().map(|r| r.1).collect();
    assert_eq!(order, vec!["3", "1", "2"]);
}

/// X3-10a：升降序按列键生效。
#[test]
fn sort_direction_switches() {
    let mut tbl = Table::new(vec![id_col()]);
    tbl.set_rows(vec!["b", "c", "a"]);
    tbl.sort_by(0, true);
    assert_eq!(tbl.rows, vec!["a", "b", "c"]);
    tbl.sort_by(0, false);
    assert_eq!(tbl.rows, vec!["c", "b", "a"]);
}

/// X3-11b：选中行整行底色可被 Buffer 断言。
#[test]
fn selection_paints_row_background() {
    let mut tbl = Table::new(vec![id_col()]);
    tbl.set_rows(vec!["a", "b"]);
    tbl.set_selected_style(Style::default().bg(ratatui::style::Color::Blue));
    let area = Rect::new(0, 0, 8, 2);
    let mut buf = Buffer::empty(area);
    tbl.layout(area);
    tbl.select(1);
    tbl.render(&mut buf);
    assert_eq!(buf[(0, 0)].bg, ratatui::style::Color::Reset);
    assert_eq!(buf[(0, 1)].bg, ratatui::style::Color::Blue);
}

/// X3-12b：上下/Home/End 导航夹取在 [0, len-1]，空表无动作。
#[test]
fn nav_clamps_and_handles_empty() {
    let mut tbl = Table::new(vec![id_col()]);
    tbl.set_rows(vec!["a", "b", "c"]);
    assert_eq!(tbl.event(Input::Key(KeyEvent::from(KeyCode::Up))), Act::Up);
    assert_eq!(tbl.sel, 0, "顶部不再上移");
    tbl.event(Input::Key(KeyEvent::from(KeyCode::End)));
    assert_eq!(tbl.sel, 2);
    tbl.event(Input::Key(KeyEvent::from(KeyCode::Down)));
    assert_eq!(tbl.sel, 2, "底部不再下移");
    tbl.event(Input::Key(KeyEvent::from(KeyCode::Home)));
    assert_eq!(tbl.sel, 0);

    let mut empty = Table::new(vec![id_col()]);
    empty.set_rows(Vec::new());
    assert_eq!(empty.event(Input::Key(KeyEvent::from(KeyCode::Down))), Act::None);
    assert_eq!(empty.measure(Size { w: 20, h: 5 }), Size { w: 1, h: 0 });
}

/// X3-04b + X3-12a：鼠标点击按落格行反查并回传选中动作。
#[test]
fn mouse_click_selects_row() {
    let mut tbl = Table::new(vec![id_col()]);
    tbl.set_rows(vec!["a", "b", "c"]);
    let area = Rect::new(2, 5, 10, 3);
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
    tbl.layout(area);
    tbl.render(&mut buf);
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 6,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    assert_eq!(
        tbl.event(Input::Mouse { event: down, row: 6 }),
        Act::Select(1)
    );
    assert_eq!(tbl.event(Input::Mouse { event: down, row: 99 }), Act::None);
}
