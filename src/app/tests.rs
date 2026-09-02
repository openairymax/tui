// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 应用状态单元测试：与 app 各职责域子模块共享同一模块视图。

use super::*;
use crate::memory::{JsonlMemory, MemoryRecord};

/// SSE 工具事件渲染：tool_call / tool_result JSON → 过程化状态行。
/// 只展示动作名与成败，不暴露参数与返回内容（2026-08-17）。
#[test]
fn render_tool_event_parses_sse_json() {
    let call = r#"{"__airy_evt":"tool_call","tool":"web_search","args":{"query":"hello"}}"#;
    let line = App::render_tool_event(call).expect("tool_call renders");
    assert!(line.contains("web_search"), "line={}", line);
    assert!(line.contains("搜索网络"), "line={}", line);
    assert!(!line.contains("hello"), "参数不得暴露: line={}", line);
    assert!(!line.contains("调用工具"), "过程化后无旧文案: line={}", line);

    let result =
        r#"{"__airy_evt":"tool_result","tool":"web_search","call_id":"c1","ok":1,"summary":"3 results"}"#;
    let line = App::render_tool_event(result).expect("tool_result renders");
    assert!(line.contains("web_search"), "line={}", line);
    assert!(line.contains("完成"), "line={}", line);
    assert!(!line.contains("3 results"), "成功结果内容不得暴露: line={}", line);

    let fail =
        r#"{"__airy_evt":"tool_result","tool":"shell_run","call_id":"c2","ok":0,"summary":"boom"}"#;
    let line = App::render_tool_event(fail).expect("failed tool_result renders");
    assert!(line.contains("失败"), "line={}", line);
    assert!(line.contains("boom"), "失败应附短错误: line={}", line);

    // 非工具事件 / 非法 JSON → None（不污染对话）
    assert!(App::render_tool_event(r#"{"type":"ping"}"#).is_none());
    assert!(App::render_tool_event("not json").is_none());
}

/// 模型名持久化往返：persist_model → load_saved_model 一致。
#[test]
fn model_persist_roundtrip() {
    let _h = crate::test_env::Home::new("model-persist");
    persist_model("deepseek-v4-flash");
    assert_eq!(load_saved_model().as_deref(), Some("deepseek-v4-flash"));
    // 再次切换覆盖
    persist_model("gpt-4-turbo");
    assert_eq!(load_saved_model().as_deref(), Some("gpt-4-turbo"));
}

/// config.toml 缺失或损坏时 load_saved_model 返回 None（回落默认模型）。
#[test]
fn model_load_missing_or_corrupt() {
    let _h = crate::test_env::Home::new("model-load");
    assert_eq!(load_saved_model(), None);
    let cfg_dir = tui_config_dir();
    std::fs::create_dir_all(&cfg_dir).expect("create dir");
    std::fs::write(cfg_dir.join("config.toml"), "not-valid-toml{{").expect("write");
    assert_eq!(load_saved_model(), None);
}

/// /model 命令：设置模型并持久化；空参显示（不修改）。
#[test]
fn cmd_model_set_and_query() {
    let _h = crate::test_env::Home::new("cmd-model");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
        .expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.model.is_empty());
    app.cmd_model("/model deepseek-v4-flash");
    assert_eq!(app.model, "deepseek-v4-flash");
    assert_eq!(load_saved_model().as_deref(), Some("deepseek-v4-flash"));
    app.cmd_model("/model");
    // 查询不改变当前模型
    assert_eq!(app.model, "deepseek-v4-flash");
}

/// --resume 会话恢复：记忆库 user/assistant 记录还原到消息列表。
#[test]
fn resume_session_restores_history() {
    let home = crate::test_env::Home::new("resume");
    let mem_dir = home.path().join("tui");
    std::fs::create_dir_all(&mem_dir).expect("create mem dir");
    let recs = [
        MemoryRecord {
            role: "user".into(),
            content: "上次的问题".into(),
            timestamp: "2026-08-08T10:00:00".into(),
            tags: "chat".into(),
            reasoning: None,
        },
        MemoryRecord {
            role: "assistant".into(),
            content: "上次的回答".into(),
            timestamp: "2026-08-08T10:00:01".into(),
            tags: "chat".into(),
            reasoning: None,
        },
        MemoryRecord {
            role: "system".into(),
            content: "不应恢复的系统消息".into(),
            timestamp: "2026-08-08T10:00:02".into(),
            tags: "chat".into(),
            reasoning: None,
        },
    ];
    let path = mem_dir.join("memory.jsonl");
    let mut lines = String::new();
    for r in &recs {
        lines.push_str(&serde_json::to_string(r).expect("serialize"));
        lines.push('\n');
    }
    std::fs::write(&path, lines).expect("write memory");

    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
        .expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    // build_memory 在 memoryrovol feature 下优先 MemoryRovol 后端（不读 JSONL），
    // 此处显式注入 JsonlMemory 以验证 resume_session 的恢复逻辑本身。
    app.memory = Box::new(JsonlMemory::new(Some(&mem_dir)).expect("jsonl memory"));
    let n = app.resume_session();
    // user + assistant 共 2 条恢复；system 跳过
    assert_eq!(n, 2);
    let contents: Vec<String> =
        app.messages.iter().map(|m| m.content.clone()).collect();
    assert!(contents.iter().any(|c| c.contains("上次的问题")));
    assert!(contents.iter().any(|c| c.contains("上次的回答")));
    assert!(contents.iter().any(|c| c.contains("已恢复上次会话")));
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

    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
        .expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.load_project_context(Some(&sub)));
    assert!(app.project_context.contains("项目约定"));
    assert!(app.project_context.contains("AGENTS.md"));
}

/// 多会话 tab：新建保留当前内容、主会话与 tab 间切换往返一致。
///
/// submit_input 会 spawn 后台请求（需要 tokio 运行时）；测试环境无
/// 事件循环消费结果，提交后用 abort_task 清空在途请求再操作 tab。
#[tokio::test]
async fn session_tabs_new_and_switch_roundtrip() {
    let _h = crate::test_env::Home::new("session-tabs");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
        .expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);

    // 初始：仅主会话（槽 0）
    assert_eq!(app.tab_count(), 1);
    assert_eq!(app.current_tab_index(), 0);

    // 主会话发一条消息 → 标题派生
    app.submit_input("帮我写一个冒泡排序").expect("submit");
    assert_eq!(app.tab_title(0), "帮我写一个冒泡排序");
    app.abort_task();

    // Ctrl+T 新建：主会话内容保留，新 tab 为空（仅含系统提示）
    app.new_session_tab();
    assert_eq!(app.tab_count(), 2);
    assert_eq!(app.current_tab_index(), 1);
    assert!(
        app.messages.iter().all(|m| m.role == MessageRole::System),
        "新会话应仅含系统提示"
    );
    assert!(
        !app.messages.iter().any(|m| m.content.contains("冒泡排序")),
        "新会话不应携带旧内容"
    );
    // 新会话发消息 → 标题派生到 tab 2
    app.submit_input("继续聊另一个话题").expect("submit");
    assert_eq!(app.tab_title(1), "继续聊另一个话题");
    app.abort_task();

    // Alt+1 切回主会话：内容还原
    app.switch_tab(1);
    assert_eq!(app.current_tab_index(), 0);
    assert!(
        app.messages.iter().any(|m| m.content.contains("冒泡排序")),
        "主会话内容应还原"
    );

    // Alt+2 切到新会话：内容还原
    app.switch_tab(2);
    assert_eq!(app.current_tab_index(), 1);
    assert!(
        app.messages.iter().any(|m| m.content.contains("另一个话题")),
        "tab 2 内容应还原"
    );

    // 越界/0：无操作
    app.switch_tab(0);
    assert_eq!(app.current_tab_index(), 1);
    app.switch_tab(9);
    assert_eq!(app.current_tab_index(), 1);
}

/// 会话标题派生：首行截断 ≤24 字符，空输入回退占位。
#[test]
fn derive_session_title_truncates_and_falls_back() {
    assert_eq!(derive_session_title("你好"), "你好");
    assert_eq!(derive_session_title("  带空格的输入  "), "带空格的输入");
    let long = "这是一个超过二十四字符长度的超长会话标题用来测试截断逻辑是否生效";
    let t = derive_session_title(long);
    assert!(t.chars().count() <= 25, "标题应截断: {}", t);
    assert!(t.ends_with('…'), "超长标题应有省略号: {}", t);
    assert_eq!(derive_session_title("   "), "（空会话）");
}

// ─────────── W7：IME 组合期（preedit）行为回归 ───────────
// 仅当 C 词典库可链接（ime_linked）且 agentrt 源码树词典存在时运行，
// 与 ime.rs FFI 测试同门控。覆盖 CJK 组合期关键路径：
// F10 激活 / 字母追加 / 空格·数字选字 / 退格 / Esc 取消 / Enter 提交。

#[cfg(all(feature = "ime", ime_linked))]
fn app_with_ime() -> (crate::test_env::Home, App) {
    let home = crate::test_env::Home::new("ime");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(app.ime_engine.is_some(), "ime_linked 下 App 应加载 IME 引擎");
    app.ime_toggle();
    assert!(app.ime_active, "F10 应进入拼音态");
    (home, app)
}

#[cfg(all(feature = "ime", ime_linked))]
fn ime_type(app: &mut App, s: &str) {
    for ch in s.chars() {
        assert!(app.ime_input_char(ch), "拼音态下字母应被消费: {}", ch);
    }
}

/// F10 切回英文时拼音原文上屏、缓冲清空（与 CLI 语义一致）。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_toggle_off_commits_raw_pinyin() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert_eq!(app.ime_buf, "zhongguo");
    app.ime_toggle();
    assert!(!app.ime_active, "切回英文应退出拼音态");
    assert!(app.ime_buf.is_empty());
    assert!(app.input.contains("zhongguo"), "拼音原文应上屏: {}", app.input);
}

/// 字母追加实时刷新候选；非 [a-z] 可见字符先上屏拼音原文再走正常路径。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_pinyin_composition_refreshes_candidates() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert_eq!(app.ime_buf, "zhongguo");
    assert!(!app.ime_cands.is_empty(), "zhongguo 应有候选");
    assert_eq!(app.ime_cands[0], "中国", "词频最高者应为「中国」");
    // 任意可见非拼音字符：原文上屏并退出拼音态，按键放行
    let consumed = app.ime_input_char('你');
    assert!(!consumed, "非拼音字符不应被拼音态消费");
    assert!(!app.ime_active);
    assert!(app.input.contains("zhongguo"));
    assert!(app.ime_buf.is_empty());
}

/// 空格上屏高亮（默认首）候选，拼音态保持（连续词组输入不中断）。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_space_commits_first_candidate_keeps_active() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert!(app.ime_input_char(' '), "空格应被消费");
    assert!(app.input.contains("中国"), "空格应上屏首候选: {}", app.input);
    assert!(app.ime_buf.is_empty(), "上屏后拼音缓冲应清空");
    assert!(app.ime_active, "选字后应保持拼音态以连续输入");
}

/// 数字键按页内下标选字（微信式分页）。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_digit_selects_candidate() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert!(app.ime_input_char('1'), "数字应被消费");
    assert!(app.input.contains("中国"));
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_active);
}

/// 退格删拼音（候选随之刷新）；拼音删空后再次退格退出拼音态。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_backspace_pops_then_exits() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongg");
    assert_eq!(app.ime_buf, "zhongg");
    assert!(app.ime_backspace());
    assert_eq!(app.ime_buf, "zhong");
    assert!(app.ime_active);
    for _ in 0..5 {
        assert!(app.ime_backspace());
    }
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_active, "缓冲空时拼音态仍在（首退格仅退态）");
    app.ime_backspace();
    assert!(!app.ime_active, "拼音缓冲为空时退格应退出拼音态");
}

/// Esc（ime_cancel）：放弃组合，不插入任何文本，退出拼音态。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_cancel_discards_without_insert() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    app.ime_cancel();
    assert!(!app.ime_active);
    assert!(app.ime_buf.is_empty());
    assert!(app.ime_cands.is_empty());
    assert!(!app.input.contains("zhongguo"), "Esc 不应上屏拼音原文");
    assert_eq!(app.input, "");
}

/// Enter：有候选上屏高亮候选并退出拼音态；无候选提交拼音原文退出。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_enter_commits_candidate_or_raw() {
    let (_d, mut app) = app_with_ime();
    ime_type(&mut app, "zhongguo");
    assert!(app.ime_commit_enter(), "拼音态 Enter 应由调用方先行提交");
    assert!(app.input.contains("中国"));
    assert!(!app.ime_active, "Enter 提交后退出拼音态");

    let (_d2, mut app2) = app_with_ime();
    ime_type(&mut app2, "zzzzz"); // 无候选拼音
    assert!(app2.ime_cands.is_empty(), "zzzzz 应无候选");
    assert!(app2.ime_commit_enter());
    assert!(app2.input.contains("zzzzz"), "无候选时 Enter 提交拼音原文");
    assert!(!app2.ime_active);
}
