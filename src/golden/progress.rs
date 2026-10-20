// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 进程呈现能力矩阵（TUI 建设方案 §6.2 / §6.5 / §6.6 / §6.9）：品牌头生命
// 周期、阶段徽章、工具事件卡、任务集 DAG 进度、输入引导、通知时序与底部跟随
// 浮标的数值生命期。断言只读真实渲染结果与状态量，不引入测试旁路。

use super::*;
use crate::engine::notify::{frame, Method};
use crate::gccp::{FlowPhase, NodeState, TaskControl};

/// §6.2 #1：品牌头空会话默认形态，以及模型/用量填充后的数值形态。
#[test]
fn hero_lifecycle() {
    let mut fx = Fixture::new("progress-hero");
    fx.app.flow_phase = FlowPhase::Chat;
    let s = fx.draw();
    assert_has(&s, "◈ AirymaxRT");
    assert_has(&s, "connecting");
    assert_has(&s, "默认模型");
    assert_has(&s, "0 tok · $0.0000");
    // 阶段徽章是状态条末段，行尾空白会被渲染裁剪，故以分隔符为锚断言。
    assert_has(&s, "│   对话");

    fx.app.model = "qwen-max".to_string();
    fx.app.tokens = 12_345;
    fx.app.cost = 0.008;
    let s = fx.draw();
    assert_has(&s, "qwen-max");
    assert_has(&s, "12345 tok · $0.0080");
}

/// §6.2 #2：阶段徽章随任务生命周期切换，暂停与恢复提示同步。
#[test]
fn phase_badge_lifecycle() {
    let mut fx = Fixture::new("progress-phase");
    fx.app.flow_phase = FlowPhase::GccpRound(2);
    fx.app.gccp.a1 = "答一".to_string();
    fx.app.gccp.a2 = "答二".to_string();
    let s = fx.draw();
    assert_has(&s, "任务事实确认 2/5");

    fx.app.flow_phase = FlowPhase::Executing;
    let s = fx.draw();
    assert_has(&s, "│   任务集");

    fx.app.task_control = TaskControl::Paused;
    let s = fx.draw();
    assert_has(&s, "⏸ 已暂停");

    fx.app.resume_task();
    let s = fx.draw();
    assert_lacks(&s, "已暂停");
    assert_has(&s, "▶ 已恢复，继续等待回复。");
}

/// §6.5 #1：工具事件卡头部取自首 token 与已知工具表，图标区分成败与回传。
#[test]
fn tool_event_cards() {
    let mut fx = Fixture::new("progress-tool");
    add(&mut fx.app, MessageRole::ToolCall, "web_search rust async");
    add(&mut fx.app, MessageRole::ToolCall, "web_fetch （失败） 超时");
    add(&mut fx.app, MessageRole::ToolResult, "{\"ok\":true}");
    let s = fx.draw();
    assert_has(&s, "[Sub web_search Agent] ✓");
    assert_has(&s, "[Sub web_fetch Agent] ✗");
    assert_has(&s, "[Sub exec Agent] ▸");
}

/// §6.6 #1：任务集 DAG 的执行态标题、步骤计数与节点标记，进度随状态推进。
#[test]
fn dag_progress_lifecycle() {
    let mut fx = Fixture::new("progress-dag");
    let json = concat!(
        "{\"nodes\":[",
        "{\"id\":\"n1\",\"label\":\"收集需求\"},",
        "{\"id\":\"n2\",\"label\":\"设计接口\"},",
        "{\"id\":\"n3\",\"label\":\"实现代码\"},",
        "{\"id\":\"n4\",\"label\":\"运行测试\"},",
        "{\"id\":\"n5\",\"label\":\"交付发布\"}],",
        "\"edges\":[{\"from\":\"n1\",\"to\":\"n2\"},{\"from\":\"n2\",\"to\":\"n3\"},",
        "{\"from\":\"n3\",\"to\":\"n4\"},{\"from\":\"n4\",\"to\":\"n5\"}]}"
    );
    fx.app.gccp.dag = crate::gccp::parse_dag(json);
    fx.app.gccp.init_node_states();
    fx.app.flow_phase = FlowPhase::Executing;
    let s = fx.draw();
    assert_has(&s, "任务集执行中");
    assert_has(&s, "· 5 个步骤");
    assert_has(&s, "进度");
    assert_has(&s, "0/5");
    assert_has(&s, "○");
    assert_has(&s, "├─");
    assert_has(&s, "└─");

    fx.app.gccp.node_states[0] = NodeState::Done;
    let s = fx.draw();
    assert_has(&s, "1/5");
    assert_has(&s, "●");
}

/// §6.2 #3：输入引导随 GCCP 轮次切到「回答第 N 问」。
#[test]
fn input_hint_tracks_round() {
    let mut fx = Fixture::new("progress-hint");
    fx.app.flow_phase = FlowPhase::GccpRound(3);
    let s = fx.draw();
    assert_has(&s, "回答第 3 问：");
}

/// §6.9 #1：通知序列化对空标题/空正文与首尾空白的三态折叠。
#[test]
fn notify_frame_boundaries() {
    assert_eq!(frame(Method::Osc9, "", "编译完成"), "\x1b]9;编译完成\x07");
    assert_eq!(frame(Method::Osc9, "构建", ""), "\x1b]9;构建\x07");
    assert_eq!(
        frame(Method::Osc777, " 标题 ", " 正文 "),
        "\x1b]777;notify;标题;正文\x07"
    );
}

/// §6.9 #2：底部跟随浮标按「出路 + 节流」计数的数值生命期。
#[test]
fn follow_badge_numeric_lifecycle() {
    let mut fx = Fixture::new("progress-follow");
    fx.app.set_clock(100);
    fx.app.follow_observe(10, W);
    fx.app.scroll_offset = 5;
    fx.app.follow_observe(10, W);
    assert_eq!(fx.app.follow_badge(), Some(0));

    fx.app.follow_observe(12, W);
    assert_eq!(fx.app.follow_badge(), Some(0));

    fx.app.set_clock(400);
    fx.app.follow_observe(12, W);
    assert_eq!(fx.app.follow_badge(), Some(2));

    fx.app.scroll_offset = 0;
    fx.app.follow_observe(12, W);
    assert_eq!(fx.app.follow_badge(), None);
}
