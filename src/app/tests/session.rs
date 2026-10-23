// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::super::config::{load_saved_model, persist_model};
use super::super::context::HistoryPolicy;
use super::super::text::derive_session_title;
use super::super::*;
use crate::memory::{ConversationMemory, GatewayMemory};

/// 模型名持久化往返：persist_model（写 model.yaml default_model）→ load_saved_model 一致。
#[test]
fn model_persist_roundtrip() {
    let _h = crate::test_env::Home::new("model-persist");
    persist_model("deepseek-flash");
    assert_eq!(load_saved_model().as_deref(), Some("deepseek-flash"));
    // 再次切换覆盖
    persist_model("gpt-4-turbo");
    assert_eq!(load_saved_model().as_deref(), Some("gpt-4-turbo"));
    // 其余内容保留（重复写不产生多行 default_model）
    let raw = std::fs::read_to_string(crate::models_cfg::model_yaml_path()).expect("model.yaml");
    assert_eq!(raw.matches("default_model:").count(), 1);
}

/// model.yaml 缺失或损坏时 load_saved_model 返回 None（回落默认模型）。
#[test]
fn model_load_missing_or_corrupt() {
    let _h = crate::test_env::Home::new("model-load");
    assert_eq!(load_saved_model(), None);
    let path = crate::models_cfg::model_yaml_path();
    std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
    // 损坏内容：行级容错解析退回空结构 → 无 default_model
    std::fs::write(&path, "\0\x01 not-valid-yaml{{{").expect("write");
    assert_eq!(load_saved_model(), None);
}

/// /model 命令：设置模型并写回 model.yaml；空参显示（不修改）。
#[test]
fn cmd_model_set_and_query() {
    let _h = crate::test_env::Home::new("cmd-model");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.model.is_empty());
    app.cmd_model("/model deepseek-flash");
    assert_eq!(app.model, "deepseek-flash");
    assert_eq!(load_saved_model().as_deref(), Some("deepseek-flash"));
    app.cmd_model("/model");
    // 查询不改变当前模型
    assert_eq!(app.model, "deepseek-flash");
}

/// --resume 会话恢复：记忆后端 user/assistant 记录还原到消息列表。
#[test]
fn resume_session_restores_history() {
    let _home = crate::test_env::Home::new("resume");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    // 显式注入纯内存镜像后端（volatile：不触网、不落盘），隔离验证恢复
    // 逻辑本身——TUI 记忆统一走网关 mem.*，此处不依赖任何本地文件。
    let mut mem = GatewayMemory::volatile();
    mem.push("user", "上次的问题", "chat").expect("push");
    mem.push("assistant", "上次的回答", "chat").expect("push");
    mem.push("system", "不应恢复的系统消息", "chat")
        .expect("push");
    app.memory = Box::new(mem);
    let n = app.resume_session();
    // user + assistant 共 2 条恢复；system 跳过
    assert_eq!(n, 2);
    // 0.1.19 §6.1 #2：恢复经回放队列节流逐条上屏，泵空后才落进消息列表；
    // 空队列时补一条「已补账恢复 N 条」记账（§6.6 #5 原样复用历史时间戳）。
    let mut now = 0;
    while app.replay_left() > 0 {
        now += crate::budget::REPLAY_INTERVAL_MS;
        app.pump_replay(now);
    }
    let contents: Vec<String> = app.messages.iter().map(|m| m.content.clone()).collect();
    assert!(contents.iter().any(|c| c.contains("上次的问题")));
    assert!(contents.iter().any(|c| c.contains("上次的回答")));
    assert!(contents.iter().any(|c| c.contains("已补账恢复 2 条历史")));
    assert!(!contents.iter().any(|c| c.contains("不应恢复的系统消息")));
}

/// 项目上下文：AGENTS.md 等价物向上查找并注入。
#[test]
fn load_project_context_finds_agents_md() {
    let _g = crate::test_env::lock_env();
    let dir = tempfile::tempdir().expect("tempdir");
    // 模拟项目根：.git 目录 + AGENTS.md
    std::fs::create_dir_all(dir.path().join(".git")).expect("create .git");
    std::fs::write(dir.path().join("AGENTS.md"), "项目约定：优先使用相对路径")
        .expect("write AGENTS.md");
    // 嵌套子目录：从子目录向上查找
    let sub = dir.path().join("src/sub");
    std::fs::create_dir_all(&sub).expect("create sub");

    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.load_project_context(Some(&sub)));
    assert!(app.project_context.contains("项目约定"));
    assert!(app.project_context.contains("AGENTS.md"));
}

/// 会话标题派生：首行按 24 列封顶（存储护栏，宽度由 L2 裁决），空输入回退占位。
#[test]
fn derive_session_title_truncates_and_falls_back() {
    assert_eq!(derive_session_title("你好"), "你好");
    assert_eq!(derive_session_title("  带空格的输入  "), "带空格的输入");
    let long = "这是一个超过二十四字符长度的超长会话标题用来测试截断逻辑是否生效";
    let t = derive_session_title(long);
    assert!(
        crate::engine::grid::width(&t) <= 24,
        "标题应封顶 24 列: {}",
        t
    );
    assert!(t.ends_with('…'), "超长标题应有省略号: {}", t);
    assert_eq!(derive_session_title("   "), "（空会话）");
}

/// B1（0.1.18）V1.1/V1.3：普通对话轮 Gated 策略——无指代信号的新主题输入
/// **不注入任何历史**：旧 assistant 全文条数恒为 0（上下文串轮根因消除），
/// msgs 退化为单条（本轮增强 prompt）。
#[test]
fn b1_gated_drops_history_for_new_topic() {
    let _h = crate::test_env::Home::new("b1-gated-new");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.add_message(MessageRole::User, "什么是阶跃函数？".to_string());
    app.add_message(MessageRole::Agent, "阶跃函数是一类不连续函数……".to_string());
    app.add_message(MessageRole::User, "介绍你自己".to_string());

    let prompt = app.build_context_prompt("介绍你自己");
    let history = app.build_history_messages(&prompt, "介绍你自己", HistoryPolicy::Gated);
    assert!(history.is_none(), "新主题输入不得携带历史: {:?}", history);
}

/// B1（0.1.18）V1.2：指代消解输入（短输入/指代词）保留**最近一轮**
/// User+Assistant 对，且首尾包裹边界标记——修复不牺牲多轮连贯性。
#[test]
fn b1_gated_keeps_last_round_with_boundary_marks() {
    let _h = crate::test_env::Home::new("b1-gated-anaphora");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.add_message(MessageRole::User, "什么是阶跃函数？".to_string());
    app.add_message(MessageRole::Agent, "阶跃函数是一类不连续函数……".to_string());
    app.add_message(MessageRole::User, "那它的反函数呢".to_string());

    let prompt = app.build_context_prompt("那它的反函数呢");
    let history = app
        .build_history_messages(&prompt, "那它的反函数呢", HistoryPolicy::Gated)
        .expect("指代输入应注入历史");
    let arr = history.as_array().expect("messages array");
    // 最近一轮 2 条 + 本轮 1 条；更早轮次不注入
    assert_eq!(arr.len(), 3, "只注入最近一轮: {:?}", arr);
    let first = arr[0]["content"].as_str().unwrap();
    let second = arr[1]["content"].as_str().unwrap();
    let last = arr[2]["content"].as_str().unwrap();
    assert!(
        first.contains("以下为历史对话"),
        "首条 user 应带前缀标记: {}",
        first
    );
    assert!(
        second.contains("历史对话到此结束"),
        "末条 assistant 应带后缀标记: {}",
        second
    );
    assert!(last.contains(&prompt), "末条 user 应为增强 prompt 本体");
    assert_eq!(arr[0]["role"], "user");
    assert_eq!(arr[1]["role"], "assistant");
    assert_eq!(arr[2]["role"], "user");
}

/// B1（0.1.18）：Full 策略（GCCP 任务确认链）保留全部轮次，同样带边界标记。
#[test]
fn b1_full_keeps_all_rounds_with_boundary_marks() {
    let _h = crate::test_env::Home::new("b1-full");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.add_message(MessageRole::User, "第一轮问题".to_string());
    app.add_message(MessageRole::Agent, "第一轮回答".to_string());
    app.add_message(MessageRole::User, "第二轮问题".to_string());
    app.add_message(MessageRole::Agent, "第二轮回答".to_string());
    app.add_message(MessageRole::User, "当前输入".to_string());

    let prompt = app.build_context_prompt("当前输入");
    let history = app
        .build_history_messages(&prompt, "当前输入", HistoryPolicy::Full)
        .expect("Full 策略应注入历史");
    let arr = history.as_array().unwrap();
    assert_eq!(arr.len(), 5, "全部历史轮次 + 本轮: {:?}", arr);
    assert!(arr[0]["content"]
        .as_str()
        .unwrap()
        .contains("以下为历史对话"));
    assert!(arr[3]["content"]
        .as_str()
        .unwrap()
        .contains("历史对话到此结束"));
}

/// B1（0.1.18）V1.3：增强 prompt 恒含轮次边界声明；记忆命中以
/// 【历史记忆参考】降权段落注入并标注归属轮次。
#[test]
fn b1_prompt_declares_turn_boundary_and_marks_memory_turn() {
    let _h = crate::test_env::Home::new("b1-prompt");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    let mut mem = GatewayMemory::volatile();
    mem.push("assistant", "阶跃函数是不连续的函数", "chat,turn:1")
        .expect("push");
    app.memory = Box::new(mem);

    // 带分隔符的查询可命中关键词（recall 分词规则）
    let prompt = app.build_context_prompt("阶跃函数，再介绍一下");
    assert!(
        prompt.contains("【轮次边界】"),
        "prompt 应声明轮次边界: {}",
        prompt
    );
    assert!(
        prompt.contains("【历史记忆参考】"),
        "记忆应降权注入: {}",
        prompt
    );
    assert!(
        prompt.contains("assistant·第1轮"),
        "记忆应标注归属轮次: {}",
        prompt
    );
    // 用户输入置于末行（回答锚点）
    assert!(prompt.rfind("用户: ").is_some(), "应含用户输入行");
}

/// B1（0.1.18）：召回结果透传轮次标注（turn:N）；旧格式记录无标注为 None。
#[test]
fn b1_memory_hit_carries_turn_annotation() {
    let _h = crate::test_env::Home::new("b1-hit-turn");
    let mut mem = GatewayMemory::volatile();
    // 词对无子串包含关系：recall 为子串匹配，"quicksort"/"sort" 这类
    // 包含词对会双命中，无法验证"仅一条命中"。
    mem.push("assistant", "the sky is blue", "chat,turn:3")
        .expect("push");
    mem.push("assistant", "grass is green", "task")
        .expect("push");
    let hits = mem.recall("sky blue", 5);
    assert_eq!(hits.len(), 1, "仅命中带关键词记录: {:?}", hits);
    assert_eq!(hits[0].turn, Some(3), "turn:N 应被解析");
    let hits2 = mem.recall("grass", 5);
    assert_eq!(hits2.len(), 1);
    assert_eq!(hits2[0].turn, None, "旧格式无 turn 标注应为 None");
}

