// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 运行时间线呈现矩阵（0.1.21 X6-02a/02b）：以真实 App 与真实渲染路径断言
// 用户屏上读到的依赖序、类别分支符与八态字形。用例不注入测试旁路，时间线
// 只在执行相位出现；空时间线不得占屏，也不留孤立标题。

use super::*;
use crate::engine::project::timeline::{Entry, Kind};
use crate::gccp::FlowPhase;

fn entry(kind: Kind, seq: u64, label: &str, state: &str) -> Entry {
    Entry {
        kind,
        seq,
        id: label.to_string(),
        label: label.to_string(),
        state: state.to_string(),
    }
}

fn rows() -> Vec<Entry> {
    vec![
        entry(Kind::Task, 1, "t1", "executing"),
        entry(Kind::Step, 2, "读取依赖", "planned"),
        entry(Kind::Tool, 3, "read", "ok"),
    ]
}

/// X6-02a：执行相位把三表依赖序摊到屏上，类别与状态同时可读。
#[test]
fn executing_phase_shows_timeline() {
    let mut fx = Fixture::new("timeline-rows");
    fx.app.flow_phase = FlowPhase::Executing;
    fx.app.run_tl = rows();
    let s = fx.draw();
    assert_has(&s, "时间线");
    assert_has(&s, "读取依赖");
    assert_has(&s, "◈");
    assert_has(&s, "├─");
}

/// X6-02b：状态折叠在屏上生效——完成态与计划态字形不同色不同符。
#[test]
fn marks_are_distinguishable_on_screen() {
    let mut fx = Fixture::new("timeline-marks");
    fx.app.flow_phase = FlowPhase::Executing;
    fx.app.run_tl = rows();
    let s = fx.draw();
    assert_has(&s, "◌");
    assert_has(&s, "●");
}

/// 空时间线不占屏：执行相位仍只有阶段头与 DAG 区，不出现孤立标题。
#[test]
fn empty_timeline_leaves_no_trace() {
    let mut fx = Fixture::new("timeline-empty");
    fx.app.flow_phase = FlowPhase::Executing;
    let s = fx.draw();
    assert_lacks(&s, "时间线");
}

/// 窄屏不右溢：最窄断点下时间线仍成行，长标签被裁剪而非撑破视口。
#[test]
fn narrow_viewport_keeps_rows_inside() {
    let mut fx = Fixture::new("timeline-narrow");
    fx.app.flow_phase = FlowPhase::Executing;
    fx.app.run_tl = vec![entry(Kind::Step, 1, &"宽".repeat(80), "running")];
    let s = fx.draw_at(46, H);
    let widest = s
        .rows()
        .iter()
        .map(|row| row.chars().filter(|c| *c == '宽').count())
        .max()
        .unwrap_or(0);
    assert!(
        widest > 0 && widest < 40,
        "长标签未被按宽度裁剪（单行 宽×{widest}）"
    );
    for row in s.rows() {
        assert!(crate::engine::grid::width(row) <= 46, "屏幕行超出视口: {row:?}");
    }
}
