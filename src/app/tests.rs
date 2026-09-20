// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 应用状态单元测试：与 app 各职责域子模块共享同一模块视图。

use super::context::HistoryPolicy;
use super::*;
use crate::memory::GatewayMemory;

/// SSE 工具事件渲染：tool_call / tool_result JSON → 过程化状态行。
/// 只展示动作名与成败，不暴露参数与返回内容（2026-08-17）。
/// 0.1.18 B3（V3.4）：状态行不携带工具原始标识符；未登记工具显示
/// "未知工具（已隐藏）"。
#[test]
fn render_tool_event_parses_sse_json() {
    let call = r#"{"__airy_evt":"tool_call","tool":"web_search","args":{"query":"hello"}}"#;
    let line = App::render_tool_event(call).expect("tool_call renders");
    assert!(line.contains("搜索网络"), "line={}", line);
    assert!(
        !line.contains("web_search"),
        "标识符不得暴露: line={}",
        line
    );
    assert!(!line.contains("hello"), "参数不得暴露: line={}", line);

    let result = r#"{"__airy_evt":"tool_result","tool":"web_search","call_id":"c1","ok":1,"summary":"3 results"}"#;
    let line = App::render_tool_event(result).expect("tool_result renders");
    assert!(line.contains("完成"), "line={}", line);
    assert!(
        !line.contains("web_search"),
        "标识符不得暴露: line={}",
        line
    );
    assert!(
        !line.contains("3 results"),
        "成功结果内容不得暴露: line={}",
        line
    );

    let fail =
        r#"{"__airy_evt":"tool_result","tool":"shell_run","call_id":"c2","ok":0,"summary":"boom"}"#;
    let line = App::render_tool_event(fail).expect("failed tool_result renders");
    assert!(line.contains("失败"), "line={}", line);
    assert!(line.contains("boom"), "失败应附短错误: line={}", line);
    assert!(
        !line.contains("shell_run") && line.contains("未知工具"),
        "未登记标识符须隐藏: line={}",
        line
    );

    // 非工具事件 / 非法 JSON → None（不污染对话）
    assert!(App::render_tool_event(r#"{"type":"ping"}"#).is_none());
    assert!(App::render_tool_event("not json").is_none());
}

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
    let contents: Vec<String> = app.messages.iter().map(|m| m.content.clone()).collect();
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

    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
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
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
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
        app.messages
            .iter()
            .any(|m| m.content.contains("另一个话题")),
        "tab 2 内容应还原"
    );

    // 越界/0：无操作
    app.switch_tab(0);
    assert_eq!(app.current_tab_index(), 1);
    app.switch_tab(9);
    assert_eq!(app.current_tab_index(), 1);
}

/// B2（0.1.18）V2.3：`begin_busy` 是请求发出与计时起点的唯一入口——
/// 置 busy 并重置本回合开始时刻；重复发起（新一轮）必须刷新起点。
#[test]
fn begin_busy_marks_request_start() {
    let _h = crate::test_env::Home::new("b2-busy");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);

    app.loading = false;
    app.begin_busy();
    assert!(app.loading, "begin_busy 应进入 busy 态");
    let first = app.busy_started;
    app.begin_busy();
    assert!(
        app.busy_started >= first,
        "重复发起应刷新计时起点（新一轮请求重新计时）"
    );
}

/// B2（0.1.18）§5A.3 W4：本地打字机已移除——流式增量到达当拍即整块上屏
/// （`poll_pending` 单拍内直接追加），上屏与落定之间不存在动画门控；同时
/// 落地按来源记入 Stream 档位，供 L4 调度器成帧。
#[tokio::test]
async fn stream_delta_lands_on_arrival_and_settles() {
    let _h = crate::test_env::Home::new("b2-reveal");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.loading = true;

    let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (out_tx, out_rx) = tokio::sync::oneshot::channel::<PendingOutcome>();
    app.pending = Some(PendingTurn {
        rx: out_rx,
        kind: PendingKind::StreamRound {
            input: "问一句".to_string(),
        },
        task: None,
        session_id: "sess_b2".to_string(),
        stream_rx: Some(stream_rx),
        tool_rx: None,
    });

    let text = "服务端增量到达即整块上屏，无附加延迟。";
    stream_tx.send(text.to_string()).expect("send delta");
    // 结果尚未到达：本拍消费增量，文本立即整块上屏并记 Stream 落地档位
    assert!(app.poll_pending(true), "结果未到达时应保持待办");
    assert_eq!(app.streaming_text, text);
    assert_eq!(
        app.take_landed(),
        Some(crate::engine::sched::Lane::Stream),
        "流式增量落地应记 Stream 档位"
    );

    // 结果到达：同一拍落定，不等任何上屏动画
    let sent = out_tx.send(PendingOutcome::Run(Ok(RunResponse {
        session_id: "sess_b2".to_string(),
        response: text.to_string(),
        tokens_used: Some(7),
        cost_usd: None,
        thinking: None,
        tool_trace: None,
        gccp_need_interaction: false,
        gccp_questions: Vec::new(),
    })));
    assert!(sent.is_ok(), "结果通道应可发送");
    assert!(!app.poll_pending(true), "落定后不应再有待办请求");
    assert!(!app.loading, "结果到达即落定（不受动画门控）");
    assert_eq!(app.tokens, 7, "权威 token 消耗随落定入账");
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

// ─────────── W7：IME 组合期（preedit）行为回归 ───────────
// 仅当 C 词典库可链接（ime_linked）且 agentrt 源码树词典存在时运行，
// 与 ime.rs FFI 测试同门控。覆盖 CJK 组合期关键路径：
// F10 激活 / 字母追加 / 空格·数字选字 / 退格 / Esc 取消 / Enter 提交。

#[cfg(all(feature = "ime", ime_linked))]
fn app_with_ime() -> (crate::test_env::Home, App) {
    let home = crate::test_env::Home::new("ime");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(
        app.ime_engine.is_some(),
        "ime_linked 下 App 应加载 IME 引擎"
    );
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
    assert!(
        app.input.contains("zhongguo"),
        "拼音原文应上屏: {}",
        app.input
    );
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
    assert!(
        app.input.contains("中国"),
        "空格应上屏首候选: {}",
        app.input
    );
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
///
/// 两个用例分属独立作用域：Home 持进程级 ENV_LOCK 直至作用域结束，
/// 同作用域内再建第二个 Home 会自死锁（首次 ime_linked 运行暴露）。
#[cfg(all(feature = "ime", ime_linked))]
#[test]
fn ime_enter_commits_candidate_or_raw() {
    {
        let (_d, mut app) = app_with_ime();
        ime_type(&mut app, "zhongguo");
        assert!(app.ime_commit_enter(), "拼音态 Enter 应由调用方先行提交");
        assert!(app.input.contains("中国"));
        assert!(!app.ime_active, "Enter 提交后退出拼音态");
    }

    {
        let (_d2, mut app2) = app_with_ime();
        ime_type(&mut app2, "zzzzz"); // 无候选拼音
        assert!(app2.ime_cands.is_empty(), "zzzzz 应无候选");
        assert!(app2.ime_commit_enter());
        assert!(app2.input.contains("zzzzz"), "无候选时 Enter 提交拼音原文");
        assert!(!app2.ime_active);
    }
}

/// B11（0.1.18）V11.3 滚动契约钳位：内容未超出视口（chat_scroll_max=0）
/// 时所有上滚为 no-op，不积累脏偏移；超出时偏移钳位到可滚总量，
/// 翻页步长消费 page_step（= 视口高度，渲染每帧回写）。
#[test]
fn b11_scroll_clamps_to_chat_scroll_max() {
    let _h = crate::test_env::Home::new("b11-scroll-clamp");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);

    app.chat_scroll_max = 0;
    app.scroll_up();
    app.scroll_page_up();
    app.wheel_up(3);
    app.scroll_top();
    assert_eq!(app.scroll_offset, 0, "无滚动量时上滚/到顶必须 no-op");

    app.page_step = 5;
    app.chat_scroll_max = 20;
    app.scroll_up();
    assert_eq!(app.scroll_offset, 1);
    app.scroll_page_up();
    assert_eq!(app.scroll_offset, 6, "翻页步长 = page_step");
    app.scroll_top();
    assert_eq!(app.scroll_offset, 20, "到顶 = 可滚总量");
    app.scroll_page_up();
    assert_eq!(app.scroll_offset, 20, "顶部之上钳位");
    app.scroll_page_down();
    assert_eq!(app.scroll_offset, 15);
    app.scroll_bottom();
    assert_eq!(app.scroll_offset, 0, "到底 = 最新消息");
    app.scroll_down();
    assert_eq!(app.scroll_offset, 0, "底部之下钳位");
}

/// B11（0.1.18/W14）滚轮行数修饰键语义：Ctrl 细粒度 1 行；Shift 加速 =
/// 视口高度（翻页）；默认 3 行。
#[test]
fn b11_wheel_lines_follows_modifiers() {
    let _h = crate::test_env::Home::new("b11-wheel-lines");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.page_step = 9;
    assert_eq!(app.wheel_lines(false, false), 3);
    assert_eq!(app.wheel_lines(true, false), 9, "Shift = 翻页步长");
    assert_eq!(app.wheel_lines(false, true), 1, "Ctrl = 单行");
    assert_eq!(app.wheel_lines(true, true), 1, "Ctrl 优先细粒度");
}

/// B11（0.1.18）V11.1：鼠标捕获默认关（保终端原生文本选择）；Ctrl+M
/// 翻转状态，run_app 循环头据差分发送终端序列。
#[test]
fn b11_toggle_mouse_capture_flips_state() {
    let _h = crate::test_env::Home::new("b11-mouse-toggle");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    assert!(!app.mouse_capture, "默认关");
    assert!(app.toggle_mouse_capture());
    assert!(app.mouse_capture);
    assert!(!app.toggle_mouse_capture());
}

/// B11（0.1.18）Alt+F 焦点视图：快照最近一条回复进入全屏只读视图；
/// 无回复时提示且不切面板；快照独立于对话区（打开后新消息不影响）。
#[test]
fn b11_focus_open_snapshots_last_reply() {
    let _h = crate::test_env::Home::new("b11-focus-open");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);

    app.focus_open();
    assert_ne!(app.active_panel, ActivePanel::Focus, "无回复不切面板");
    assert!(app
        .messages
        .iter()
        .any(|m| m.role == MessageRole::System && m.content.contains("暂无可全屏查看的回复")));

    app.add_message(MessageRole::User, "问题".to_string());
    app.add_message(MessageRole::Agent, "第一条回复".to_string());
    app.add_message(MessageRole::User, "追问".to_string());
    app.add_message(MessageRole::Agent, "第二条回复".to_string());
    app.focus_open();
    assert_eq!(app.active_panel, ActivePanel::Focus);
    let msg = app.focus_msg.as_ref().expect("快照应存在");
    assert_eq!(msg.content, "第二条回复", "快照取最近一条回复");
    assert_eq!(app.focus_scroll, 0);

    app.add_message(MessageRole::Agent, "打开后新回复".to_string());
    assert_eq!(
        app.focus_msg.as_ref().expect("快照仍在").content,
        "第二条回复",
        "快照独立于后续消息"
    );
}

/// B11（0.1.18）焦点视图滚动钳位：focus_scroll_max=0 时 no-op；超出视口
/// 时钳位到可滚总量；翻页步长与对话区同源（page_step）。
#[test]
fn b11_focus_scroll_clamps() {
    let _h = crate::test_env::Home::new("b11-focus-scroll");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.page_step = 4;

    app.focus_scroll_max = 0;
    app.focus_scroll_up();
    app.focus_page_up();
    assert_eq!(app.focus_scroll, 0, "无滚动量时 no-op");

    app.focus_scroll_max = 10;
    app.focus_scroll_up();
    assert_eq!(app.focus_scroll, 1);
    app.focus_page_up();
    assert_eq!(app.focus_scroll, 5, "翻页步长 = page_step");
    for _ in 0..10 {
        app.focus_scroll_up();
    }
    assert_eq!(app.focus_scroll, 10, "钳位到可滚总量");
    app.focus_scroll_down();
    assert_eq!(app.focus_scroll, 9);
    app.focus_page_down();
    assert_eq!(app.focus_scroll, 5);
    app.focus_scroll_down();
    assert_eq!(app.focus_scroll, 4);
    app.focus_page_down();
    app.focus_page_down();
    assert_eq!(app.focus_scroll, 0, "下封 0（顶部）");
}
