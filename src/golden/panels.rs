// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 八面板 golden（0.1.20 TUI 缺口⑥）：Config/Logs/Memory/Plugins/Board/
// Events/Trace/DaemonLogs 逐面板走真实渲染，断言标题与用户可见的关键行
// （含空态诚实措辞与新增的翻页/键位提示）。此前这些面板零 golden 覆盖，
// "面板存在但渲染错位"只能靠人工肉眼回归。

use super::*;
use crate::app::ActivePanel;
use crate::client::ToolTrace;

#[test]
fn golden_config_panel_shows_model_and_actions() {
    let mut fx = Fixture::new("g-config");
    let row = crate::models_cfg::ModelRow {
        name: "gpt-golden".to_string(),
        ..Default::default()
    };
    crate::models_cfg::patch_model_yaml(0, &row, &Default::default()).expect("seed");
    crate::models_cfg::set_default_model("gpt-golden").expect("default");
    fx.app.active_panel = ActivePanel::Config;
    let s = fx.draw();
    assert_has(&s, "配置");
    assert_has(&s, "gpt-golden");
    assert_has(&s, "可执行动作");
    assert_has(&s, "↑↓ 选择 · Enter 执行 · PgUp/PgDn 翻阅");
}

#[test]
fn golden_logs_panel_shows_entries() {
    let mut fx = Fixture::new("g-logs");
    fx.app.add_log("INFO", "golden 日志行".to_string());
    fx.app.active_panel = ActivePanel::Logs;
    let s = fx.draw();
    assert_has(&s, "运行时日志");
    assert_has(&s, "golden 日志行");
}

#[test]
fn golden_logs_panel_empty_is_honest() {
    let mut fx = Fixture::new("g-logs-empty");
    fx.app.active_panel = ActivePanel::Logs;
    let s = fx.draw();
    assert_has(&s, "运行时日志");
    assert_has(&s, "暂无日志");
}

#[test]
fn golden_memory_panel_empty_is_honest() {
    let mut fx = Fixture::new("g-mem");
    fx.app.active_panel = ActivePanel::Memory;
    let s = fx.draw();
    assert_has(&s, "记忆库");
    assert_has(&s, "暂无记忆记录");
}

/// 固定条数的内存技能库：golden 专用，验证整表渲染不受任何常量截断。
struct CountedStore(Vec<crate::skills::SkillRecord>);
impl crate::skills::SkillStore for CountedStore {
    fn save(&mut self, s: crate::skills::SkillRecord) -> std::io::Result<()> {
        self.0.push(s);
        Ok(())
    }
    fn find(&self, _: &str, limit: usize) -> Vec<crate::skills::SkillRecord> {
        self.0.iter().take(limit).cloned().collect()
    }
    fn list(&self) -> Vec<crate::skills::SkillRecord> {
        self.0.clone()
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[test]
fn golden_plugins_panel_renders_beyond_old_truncation() {
    let mut fx = Fixture::new("g-plug");
    // 12 条技能：旧实现 take(10) 下第 11/12 条永不可见，golden 直接钉死回归
    fx.app.skills = Box::new(CountedStore(
        (1..=12)
            .map(|i| crate::skills::SkillRecord::new(
                &format!("技能{i:02}"),
                "test",
                "",
                "整表渲染验证",
                "",
                "",
                "",
            ))
            .collect(),
    ));
    fx.app.active_panel = ActivePanel::Plugins;
    // 40 行视口：12 条技能（24 行）+ 计数头 + 页脚全部可见，
    // 断言的是"不截断数据"，不借视口高度做文章
    let s = fx.draw_at(W, 40);
    assert_has(&s, "插件 / 技能库");
    assert_has(&s, "技能01");
    assert_has(&s, "技能12");
    assert_has(&s, "共 12 条 · PgUp/PgDn 翻阅");
}

#[test]
fn golden_board_panel_shows_counts_and_filter() {
    let mut fx = Fixture::new("g-board");
    fx.app.active_panel = ActivePanel::Board;
    let s = fx.draw();
    assert_has(&s, "任务看板");
    assert_has(&s, "执行实例 0 · 在线 Agent 0");
    assert_has(&s, "全部");
}

#[test]
fn golden_events_panel_empty_is_honest() {
    let mut fx = Fixture::new("g-events");
    fx.app.active_panel = ActivePanel::Events;
    let s = fx.draw();
    assert_has(&s, "事件流");
    assert_has(&s, "暂无事件");
}

#[test]
fn golden_trace_panel_renders_tool_detail() {
    let mut fx = Fixture::new("g-trace");
    fx.app.last_tool_trace = vec![ToolTrace {
        tool: "fs_read".to_string(),
        arguments: "{\"path\":\"a.txt\"}".to_string(),
        result: "文件内容样例".to_string(),
        ok: Some(1),
    }];
    fx.app.active_panel = ActivePanel::Trace;
    let s = fx.draw();
    assert_has(&s, "工具轨迹");
    assert_has(&s, "fs_read");
    assert_has(&s, "成功");
    assert_has(&s, "文件内容样例");
    // 缺口⑤：观测面板页脚互达提示
    assert_has(&s, "F10 健康 · F11 演化 · F12 轨迹");
}

#[test]
fn golden_daemonlogs_panel_empty_is_honest() {
    let mut fx = Fixture::new("g-dlogs");
    fx.app.active_panel = ActivePanel::BackendLog;
    let s = fx.draw();
    assert_has(&s, "后端日志");
    // 临时 HOME 无日志目录：如实给出路径说明，不伪造内容
    assert_has(&s, "日志目录为空或暂无日志文件");
}
