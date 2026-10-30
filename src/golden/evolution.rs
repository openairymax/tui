// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 演化指标面板 golden（0.1.20 W8）：固定 JSON 夹具走真实解析与真实渲染，
// 断言用户能看到的指标名/值/量纲/出处，以及两种空态的诚实措辞——空指标
// 与 monit 不可达都不得出现替代数值。

use super::*;
use crate::app::ActivePanel;
use crate::engine::evolution::snapshot;
use serde_json::json;

/// 六项指标齐全时逐项上屏，并标注数据源与上报者。
#[test]
fn evolution_panel_renders_live_metrics() {
    let mut fx = Fixture::new("evo-live");
    fx.app.active_panel = ActivePanel::Evolution;
    fx.app.evo = Some(snapshot(&json!({
        "evolution": {
            "available": true,
            "source": "monit",
            "metrics": {
                "patterns": 12.0,
                "detections_total": 3.0,
                "preemptions_total": 1.0,
                "effectiveness": 0.85,
                "records": 6.0,
                "evolve_noop_total": 2.0
            }
        }
    })));
    fx.app.evo_at_ms = 0;
    fx.app.clock_ms = 2_000;
    let s = fx.draw();
    assert_has(&s, "演化指标");
    assert_has(&s, "演化回路在线");
    assert_has(&s, "6 项指标");
    assert_has(&s, "演化模式");
    assert_has(&s, "12");
    assert_has(&s, "检出次数");
    assert_has(&s, "计数器");
    assert_has(&s, "0.85");
    assert_has(&s, "比例");
    assert_has(&s, "数据源：monit · 经 gateway 只读转发");
    assert_has(&s, "上报者：think_d 演化回路");
}

/// monit 已连接但无演化指标：如实说明尚未上报，不出现任何指标行。
#[test]
fn evolution_panel_empty_metrics_is_honest() {
    let mut fx = Fixture::new("evo-empty");
    fx.app.active_panel = ActivePanel::Evolution;
    fx.app.evo = Some(snapshot(&json!({
        "evolution": { "available": true, "source": "monit", "metrics": {} }
    })));
    let s = fx.draw();
    assert_has(&s, "尚未上报指标");
    assert_has(&s, "think_d 未运行");
    assert_lacks(&s, "演化模式");
}

/// monit 不可达：原样展示原因，且不伪造 0 值指标。
#[test]
fn evolution_panel_unavailable_shows_reason() {
    let mut fx = Fixture::new("evo-down");
    fx.app.active_panel = ActivePanel::Evolution;
    fx.app.evo = Some(snapshot(&json!({
        "evolution": { "available": false, "reason": "monit unreachable" }
    })));
    let s = fx.draw();
    assert_has(&s, "演化指标不可用");
    assert_has(&s, "monit unreachable");
    assert_has(&s, "不显示任何替代数值");
    assert_lacks(&s, "演化模式");
}
