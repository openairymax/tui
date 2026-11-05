// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 配置面板动作回归：面板可执行项与写回落点必须同源。
//
// 覆盖两条真实路径：动作清单来自 model.yaml 实读（不是常量表），
// Enter 执行后必须真的改到 $AIRY_HOME/config/model.yaml——断言读回文件，
// 不断言面板状态，避免"面板显示切换成功、文件没动"的假绿。

use super::*;

/// 在临时 AIRY_HOME 写入一行模型与 think 段（配置面板的数据源）。
fn seed_model(name: &str) {
    let row = crate::models_cfg::ModelRow {
        name: name.to_string(),
        mode: "api".to_string(),
        api_format: "openai".to_string(),
        base_url: "http://127.0.0.1:8080/v1".to_string(),
        model_id: "gpt-test".to_string(),
        api_key_env: "MODEL_1_API_KEY".to_string(),
        ..Default::default()
    };
    let think = crate::models_cfg::ThinkCfg {
        enabled: Some(true),
        ..Default::default()
    };
    crate::models_cfg::patch_model_yaml(0, &row, &think).expect("写入 model.yaml");
}

fn app() -> App {
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    App::new("agents/main.agent.yaml", gw)
}

#[test]
fn cfg_actions_follow_model_yaml() {
    let _home = crate::test_env::Home::new("cfg-actions");
    assert!(
        crate::app::cfg_actions()
            .iter()
            .any(|a| matches!(a, crate::app::CfgAction::Wizard)),
        "无模型时也应保留主题 / Key / 向导三项"
    );
    seed_model("m1");
    let acts = crate::app::cfg_actions();
    assert_eq!(
        acts.first(),
        Some(&crate::app::CfgAction::SetModel("m1".to_string())),
        "模型行应排在最前"
    );
    assert!(
        acts.iter().any(|a| matches!(a, crate::app::CfgAction::Think(true))),
        "think.enabled=true 时应给出「关闭」动作"
    );
}

#[test]
fn cfg_enter_writes_default_model() {
    let _home = crate::test_env::Home::new("cfg-model");
    seed_model("m1");
    let mut app = app();
    app.cfg_sel = 0;
    app.cfg_activate();
    assert_eq!(app.model, "m1", "面板执行应与 /model 同一落点");
    assert_eq!(
        crate::models_cfg::read_model_yaml().default_model,
        "m1",
        "default_model 必须真的写入 model.yaml"
    );
}

#[test]
fn cfg_enter_toggles_think() {
    let _home = crate::test_env::Home::new("cfg-think");
    seed_model("m1");
    let mut app = app();
    let idx = crate::app::cfg_actions()
        .iter()
        .position(|a| matches!(a, crate::app::CfgAction::Think(_)))
        .expect("存在双思考动作");
    app.cfg_sel = idx;
    app.cfg_activate();
    assert_eq!(
        crate::models_cfg::read_model_yaml()
            .think
            .and_then(|t| t.enabled),
        Some(false),
        "Enter 应把双思考由开翻到关并写回"
    );
    app.cfg_activate();
    assert_eq!(
        crate::models_cfg::read_model_yaml()
            .think
            .and_then(|t| t.enabled),
        Some(true),
        "再按一次应翻回开启"
    );
}

#[test]
fn cfg_sel_wraps_within_actions() {
    let _home = crate::test_env::Home::new("cfg-wrap");
    seed_model("m1");
    let mut app = app();
    let n = crate::app::cfg_actions().len();
    app.cfg_sel_up();
    assert_eq!(app.cfg_sel, n - 1, "↑ 从首项回到底部");
    app.cfg_sel_down();
    assert_eq!(app.cfg_sel, 0, "↓ 从末项回到顶部");
}
