// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导端到端回归（H6 防复发）：以按键路径驱动完整状态机——CJK 编辑、
// Esc 快照往返、粘贴清洗、预设联动、直通完成、数字直达。每个用例独占
// 一个 AIRY_HOME 临时目录（lock_env 串行化），覆盖 0.1.7/0.1.8 全部
// 回归场景。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::models_cfg;

use super::lang::Lang;
use super::persist;
use super::state::WizardState;

/// 隔离环境并构造首启向导；返回的守卫自带 test_env 锁与临时目录，
/// 须存活到用例结束
fn fresh(tag: &str) -> (crate::test_env::Home, WizardState) {
    let home = crate::test_env::Home::new(tag);
    std::fs::create_dir_all(home.path().join("config")).expect("config 目录");
    std::fs::create_dir_all(home.path().join("data")).expect("data 目录");
    std::env::remove_var("AIRY_LLM_PROVIDER");
    let w = WizardState::new();
    assert!(w.active, "首次运行应自动启动向导");
    assert_eq!(w.step, 1);
    (home, w)
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(w: &mut WizardState, code: KeyCode) -> bool {
    w.handle_key(&k(code))
}

fn down_n(w: &mut WizardState, n: usize) {
    for _ in 0..n {
        press(w, KeyCode::Down);
    }
}

fn type_str(w: &mut WizardState, s: &str) {
    for c in s.chars() {
        press(w, KeyCode::Char(c));
    }
}

/// 步骤 1 → 2 → 3（默认语言 + 快速配置）
fn to_step3(w: &mut WizardState) {
    assert!(!press(w, KeyCode::Enter));
    assert_eq!(w.step, 2);
    assert!(!press(w, KeyCode::Enter));
    assert_eq!(w.step, 3);
    assert_eq!(w.form.len(), 7, "步骤 3 应播种 7 个字段");
}

#[test]
fn cjk_name_edit_roundtrip() {
    let (_h, mut w) = fresh("cjk");
    to_step3(&mut w);
    down_n(&mut w, 1); // 光标到 Name
    assert!(!press(&mut w, KeyCode::Enter), "Enter 进入编辑");
    type_str(&mut w, "智谱");
    assert_eq!(w.form[1].value, "智谱");
    assert!(!press(&mut w, KeyCode::Backspace), "退格删除整个汉字不 panic");
    assert_eq!(w.form[1].value, "智");
    assert!(!press(&mut w, KeyCode::Enter));
    assert!(!w.editing);
}

#[test]
fn cjk_edit_left_right_home_end() {
    let (_h, mut w) = fresh("cjkmv");
    to_step3(&mut w);
    down_n(&mut w, 1);
    press(&mut w, KeyCode::Enter);
    type_str(&mut w, "中文ab");
    press(&mut w, KeyCode::Home);
    assert_eq!(w.edit_pos, 0);
    press(&mut w, KeyCode::End);
    assert_eq!(w.edit_pos, w.form[1].value.len());
    press(&mut w, KeyCode::Left);
    assert_eq!(w.edit_pos, w.form[1].value.len() - 1, "b 为单字节");
    press(&mut w, KeyCode::Left);
    assert!(w.form[1].value.is_char_boundary(w.edit_pos), "← 落在字符边界");
    press(&mut w, KeyCode::Right);
    press(&mut w, KeyCode::Right);
    assert!(w.form[1].value.is_char_boundary(w.edit_pos), "→ 落在字符边界");
}

#[test]
fn paste_key_strips_newlines() {
    let (_h, mut w) = fresh("paste");
    to_step3(&mut w);
    down_n(&mut w, 6); // ApiKey
    press(&mut w, KeyCode::Enter);
    w.handle_paste("sk-a\nb\r");
    assert_eq!(w.form[6].value, "sk-ab", "粘贴剥离换行/回车");
    assert!(w.form[6].touched);
}

#[test]
fn untouched_finish_fills_preset() {
    let (_h, mut w) = fresh("fill");
    to_step3(&mut w);
    down_n(&mut w, 7); // 动作位
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 4);
    down_n(&mut w, 5);
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 5);
    down_n(&mut w, 4);
    assert!(press(&mut w, KeyCode::Enter), "最后一步动作 = 完成");
    let m = models_cfg::read_model_yaml();
    let row = m.rows.first().expect("应写入模型行");
    assert_eq!(row.base_url, "https://api.deepseek.com", "空白字段预设补全");
    assert_eq!(row.model_id, "deepseek-chat");
    assert_eq!(row.name, "DeepSeek");
}

#[test]
fn full_wizard_flow_reaches_finish() {
    let (_h, mut w) = fresh("flow");
    to_step3(&mut w);
    // 提供商：清空默认值改输 qwen → Enter 应用预设
    press(&mut w, KeyCode::Enter);
    for _ in 0.."deepseek".len() {
        press(&mut w, KeyCode::Backspace);
    }
    type_str(&mut w, "qwen");
    press(&mut w, KeyCode::Enter);
    assert!(w.form[4].value.contains("dashscope"), "base_url 自动填充");
    assert_eq!(w.form[5].value, "qwen-max", "model_id 自动填充");
    // 编辑结束光标已到 1，Down×5 → ApiKey(6)
    down_n(&mut w, 5);
    assert_eq!(w.field_cursor, 6);
    press(&mut w, KeyCode::Enter);
    type_str(&mut w, "sk-abc123");
    assert_eq!(w.form[6].value, "sk-abc123", "API Key 数字可输入");
    press(&mut w, KeyCode::Enter);
    assert_eq!(w.field_cursor, 7, "封顶动作位");
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 4);
    down_n(&mut w, 5);
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 5);
    down_n(&mut w, 4);
    assert!(press(&mut w, KeyCode::Enter));
    assert!(!w.active);
    let r = w.result.expect("完成应有结果");
    assert!(r.configured);
    assert_eq!(r.model, "qwen-max");
    assert!(r.api_key_set, "secrets.env 应写入成功");
    let secrets = std::fs::read_to_string(std::env::var("AIRY_HOME").unwrap() + "/config/secrets.env")
        .expect("secrets.env 存在");
    assert!(secrets.contains("MODEL_1_API_KEY=sk-abc123"));
}

#[test]
fn esc_roundtrip_keeps_all_three_forms() {
    let (_h, mut w) = fresh("escre");
    to_step3(&mut w);
    press(&mut w, KeyCode::Tab); // deepseek → openai
    assert_eq!(w.form[0].value, "openai");
    down_n(&mut w, 4); // BaseUrl(4)
    press(&mut w, KeyCode::Enter);
    for _ in 0..w.form[4].value.len() {
        press(&mut w, KeyCode::Backspace);
    }
    type_str(&mut w, "https://custom.example/v1");
    press(&mut w, KeyCode::Enter); // 光标 5
    down_n(&mut w, 2); // 动作位 7
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 4);
    down_n(&mut w, 2); // ToolRounds(2)
    press(&mut w, KeyCode::Enter);
    for _ in 0.."1000".len() {
        press(&mut w, KeyCode::Backspace);
    }
    type_str(&mut w, "50");
    press(&mut w, KeyCode::Enter); // 光标 3
    assert!(!press(&mut w, KeyCode::Esc), "步骤 4 → 3");
    assert_eq!(w.step, 3);
    assert_eq!(w.form.len(), 7);
    assert_eq!(w.form[0].value, "openai", "提供商保留");
    assert_eq!(w.form[4].value, "https://custom.example/v1", "手改地址保留");
    assert!(!press(&mut w, KeyCode::Esc), "步骤 3 → 2");
    assert_eq!(w.step, 2);
    assert_eq!(w.choice_cursor, 0);
    press(&mut w, KeyCode::Enter); // 快速配置回 3
    assert_eq!(w.step, 3);
    assert_eq!(w.form[4].value, "https://custom.example/v1", "再次进入仍恢复快照");
    assert_eq!(w.field_cursor, 0, "重进后光标归零");
}

#[test]
fn digit_shortcut_steps_1_and_2() {
    let (_h, mut w) = fresh("digit");
    assert!(!press(&mut w, KeyCode::Char('2')));
    assert_eq!(w.step, 2);
    assert!(matches!(w.effective_lang, Lang::English), "数字 2 选 English");
    assert!(!press(&mut w, KeyCode::Char('1')));
    assert_eq!(w.step, 3, "数字 1 选快速配置");
    assert_eq!(w.form.len(), 7);
    // 越界数字被忽略
    let before = w.step;
    assert!(!press(&mut w, KeyCode::Char('9')));
    assert_eq!(w.step, before);
}

#[test]
fn esc_on_step1_skips_without_persist() {
    let (_h, mut w) = fresh("escskip");
    assert!(press(&mut w, KeyCode::Esc), "选项步 Esc 直接关闭向导");
    assert!(!w.active);
    assert!(w.result.is_none(), "跳过不产生结果");
    assert!(persist::is_first_run(), "不写 wizard.toml，下次仍首启");
}

#[test]
fn back_to_step3_restores_snapshot() {
    let (_h, mut w) = fresh("back3");
    to_step3(&mut w);
    press(&mut w, KeyCode::Tab); // openai
    down_n(&mut w, 7);
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 4);
    assert_eq!(w.form.len(), 5, "步骤 4 字段 5 项");
    assert!(!press(&mut w, KeyCode::Esc));
    assert_eq!(w.step, 3);
    assert_eq!(w.form.len(), 7, "返回步骤 3 恢复 7 字段");
    assert_eq!(w.form[0].value, "openai");
    assert!(w.form[4].value.contains("api.openai.com"), "base_url 保留");
    assert_eq!(w.form[5].value, "gpt-4o", "model_id 保留");
}

#[test]
fn step4_adv_values_survive_finish() {
    let (_h, mut w) = fresh("adv");
    to_step3(&mut w);
    down_n(&mut w, 7);
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 4);
    w.form[0].value = "32k".into();
    w.form[1].value = "4k".into();
    w.form[2].value = "50".into();
    down_n(&mut w, 5);
    assert!(!press(&mut w, KeyCode::Enter));
    assert_eq!(w.step, 5);
    assert_eq!(w.form.len(), 4, "步骤 5 双思考字段 4 项");
    down_n(&mut w, 4);
    assert!(press(&mut w, KeyCode::Enter));
    let m = models_cfg::read_model_yaml();
    let row = m.rows.first().expect("应写入模型行");
    assert_eq!(row.context_window, "32k", "上下文窗口编辑保留");
    assert_eq!(row.max_output, "4k", "最大输出编辑保留");
    assert_eq!(row.tool_rounds, "50", "工具轮数编辑保留");
}
