// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::*;
use super::super::*;
use crate::memory::MirrorMemory;
use std::time::Duration;

/// 接入真实事件管道的会话角色子进程（复用 client::event 的测试夹具）。
///
/// 运行轮只走事件管道，用例需要一条真实通道才能观察「已下发」；这里起的是
/// 真实子进程与真实帧，不伪造投影结果。
fn attach_pipe(app: &mut App) {
    let exe = std::env::current_exe().expect("测试二进制路径");
    let spec = crate::engine::event::pipe::Spec::new(
        &exe.to_string_lossy(),
        &[
            "--exact",
            "client::event::tests::child",
            "--nocapture",
            "--quiet",
            "--test-threads=1",
        ],
    )
    .env("AIRY_EV_CLIENT_CHILD", "session");
    app.ev = Some(crate::client::event::EventClient::start(&spec).expect("事件管道可起"));
}

/// 多会话 tab：新建保留当前内容、主会话与 tab 间切换往返一致。
///
/// submit_input 会 spawn 后台请求（需要 tokio 运行时）；测试环境无
/// 事件循环消费结果，提交后用 abort_task 清空在途请求再操作 tab。
#[tokio::test]
async fn session_tabs_new_and_switch_roundtrip() {
    let _h = crate::test_env::Home::new("session-tabs");
    let mut app = App::new("agents/main.agent.yaml");

    // 初始：仅主会话（槽 0）
    assert_eq!(app.session_view().tab_count(), 1);
    assert_eq!(app.current_tab_index(), 0);

    // 主会话发一条消息 → 标题派生
    app.submit_input("帮我写一个冒泡排序").expect("submit");
    assert_eq!(app.session_view().tab_title(0), "帮我写一个冒泡排序");
    app.abort_task();

    // Ctrl+T 新建：主会话内容保留，新 tab 为空（仅含系统提示）
    app.new_session_tab();
    assert_eq!(app.session_view().tab_count(), 2);
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
    assert_eq!(app.session_view().tab_title(1), "继续聊另一个话题");
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
    let mut app = App::new("agents/main.agent.yaml");

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

/// U14 §6.7 #4：完成通知由 App 生产、主循环消费，取走即清零（不重复弹）。
#[test]
fn notice_is_drained_exactly_once() {
    let _h = crate::test_env::Home::new("u14-notice");
    let mut app = App::new("agents/main.agent.yaml");
    app.notice = Some(crate::app::Notice {
        title: "Airymax".to_string(),
        body: "回合完成".to_string(),
    });
    assert!(app.take_notice().is_some(), "首次取走应得到通知");
    assert!(app.take_notice().is_none(), "取走后不得重复投递");
}

/// B2（0.1.18）§5A.3 W4：本地打字机已移除——流式增量到达当拍即整块上屏
/// （`poll_pending` 单拍内直接追加），上屏与落定之间不存在动画门控；同时
/// 落地按来源记入 Stream 档位，供 L4 调度器成帧。
#[tokio::test]
async fn stream_delta_lands_on_arrival_and_settles() {
    let _h = crate::test_env::Home::new("b2-reveal");
    let mut app = App::new("agents/main.agent.yaml");
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
        status: None,
        resume_token: None,
        rounds_used: None,
        rounds_limit: None,
        ..Default::default()
    })));
    assert!(sent.is_ok(), "结果通道应可发送");
    assert!(!app.poll_pending(true), "落定后不应再有待办请求");
    assert!(!app.loading, "结果到达即落定（不受动画门控）");
    assert_eq!(app.tokens, 7, "权威 token 消耗随落定入账");
}

/// U2（0.1.19 L7）：Ctrl+C 双击退出守卫——首按只提示，窗口内二次按下
/// 才确认；窗口边界按严格小于裁决，超时复位；busy 态共用同一入口。
#[test]
fn ctrl_c_exit_guard_window() {
    let _h = crate::test_env::Home::new("exit-guard");
    let mut app = App::new("agents/main.agent.yaml");
    let t0 = Instant::now();

    assert!(!app.confirm_exit(t0), "首次误触不得退出");
    assert!(
        app.messages.iter().any(|m| m.content.contains("Ctrl+C")),
        "首次误触须给出提示"
    );

    // busy 态同规：窗口内二次按下即确认
    app.loading = true;
    assert!(app.confirm_exit(t0 + Duration::from_millis(499)));

    // 确认后复位：紧邻的下一次按下重新武装
    assert!(!app.confirm_exit(t0 + Duration::from_millis(500)));
    // 恰好 500ms 不算窗口内：重新计时
    assert!(!app.confirm_exit(t0 + Duration::from_millis(1_000)));
    // 新窗口内再次按下：确认退出
    assert!(app.confirm_exit(t0 + Duration::from_millis(1_100)));
}

/// U2（0.1.19 L7）：键位分派接线——空闲态连续两次 Ctrl+C 才请求退出。
#[tokio::test]
async fn ctrl_c_key_needs_two_presses() {
    let _h = crate::test_env::Home::new("exit-guard-key");
    let mut app = App::new("agents/main.agent.yaml");
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('c'),
        crossterm::event::KeyModifiers::CONTROL,
    );

    let first = crate::keys::handle(&mut app, key).await.expect("handle");
    assert!(
        matches!(first, crate::keys::Flow::Continue),
        "首按 Ctrl+C 不得退出"
    );
    let second = crate::keys::handle(&mut app, key).await.expect("handle");
    assert!(
        matches!(second, crate::keys::Flow::SigInt),
        "窗口内二次按下应退出"
    );
}

/// U5（0.1.19 L7）：启动期连接探测晚到结果热修正——成功落连接态与版本
/// 并排 Poll 帧；失败落不可达且不残留上一次的版本信息。
#[test]
fn boot_probe_applies_late_result() {
    let _h = crate::test_env::Home::new("boot-probe");
    let mut app = App::new("agents/main.agent.yaml");
    app.take_landed();

    app.apply_probe(Ok(crate::client::HealthResponse {
        status: "healthy".to_string(),
        version: Some("9.9.9".to_string()),
        uptime_seconds: None,
    }));
    assert!(app.connected, "成功探测应置连接态");
    assert_eq!(app.gateway_version.as_deref(), Some("9.9.9"));
    assert!(
        app.status_message.contains("9.9.9"),
        "状态条应携带网关版本: {}",
        app.status_message
    );
    assert_eq!(
        app.take_landed(),
        Some(crate::engine::sched::Lane::Poll),
        "探测结果落地应排帧刷新"
    );

    app.apply_probe(Err(anyhow!("connection refused")));
    assert!(!app.connected, "失败探测应置不可达");
    assert!(app.gateway_version.is_none(), "失败探测不得残留旧版本");
    assert!(
        app.status_message.contains("unreachable"),
        "状态条应提示不可达: {}",
        app.status_message
    );
    assert_eq!(app.take_landed(), Some(crate::engine::sched::Lane::Poll));
}

/// U8（0.1.19 L7）：leader 取最近完整围栏代码块，未闭合尾块与空块不计。
#[test]
fn last_code_picks_latest_complete_block() {
    let _h = crate::test_env::Home::new("leader-code");
    let mut app = App::new("agents/main.agent.yaml");
    app.add_message(
        MessageRole::Agent,
        "先看这段：\n```rust\nlet a = 1;\n```\n".into(),
    );
    assert_eq!(app.last_code().as_deref(), Some("let a = 1;"));
    app.add_message(
        MessageRole::Agent,
        "```python\nprint(1)\n```\n尾块未闭合：\n```rust\nlet b = 2;\n".into(),
    );
    assert_eq!(
        app.last_code().as_deref(),
        Some("print(1)"),
        "未闭合尾块不得遮蔽更早的完整块"
    );
    app.add_message(MessageRole::Agent, "```sh\necho done\n```".into());
    assert_eq!(app.last_code().as_deref(), Some("echo done"));
}

/// U8（0.1.19 L7）：前缀键静置满 500ms 才浮出提示，取消即清空序列。
#[test]
fn leader_hint_waits_delay_and_cancels() {
    let _h = crate::test_env::Home::new("leader-hint");
    let mut app = App::new("agents/main.agent.yaml");
    let delay = Duration::from_millis(crate::budget::LEADER_HINT_DELAY_MS);
    let t0 = std::time::Instant::now();
    app.leader_press();
    assert!(app.leader_pending());
    assert!(!app.leader_hint());
    assert!(app.leader_wait(t0).is_some(), "待续期须给出收窄等待");
    assert!(!app.leader_hint_due(t0 + delay - Duration::from_millis(1)));
    assert!(!app.leader_hint(), "未满 500ms 不得浮出");
    assert!(app.leader_hint_due(t0 + delay + Duration::from_millis(50)));
    assert!(app.leader_hint());
    assert!(app.leader_wait(std::time::Instant::now()).is_none());
    app.leader_cancel();
    assert!(!app.leader_pending());
    assert!(!app.leader_hint());
}

/// §6.5 #4：审批回送失败 fail-closed——写端关闭时不得假装已放行：队列移除、
/// 明确标注工具将在 CLI 侧超时拒绝；空队列再入只提示，不产生决议。
#[test]
fn approval_write_failure_fails_closed() {
    let _h = crate::test_env::Home::new("approval-write-fail");
    let mut app = App::new("agents/main.agent.yaml");
    attach_pipe(&mut app);
    let ev = app.ev.clone().expect("事件管道已接入");
    ev.stop(Duration::from_millis(50)).expect("停止可完成");
    app.approvals.push(approval_req("r1", "shell_exec"));

    app.approve_request("allow");

    assert!(app.approvals.is_empty(), "回送失败须清空待审队列");
    let text: String = app
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("审批回送失败"), "text={}", text);
    assert!(text.contains("fail-closed"), "text={}", text);
    assert!(
        !text.contains("已允许本次执行"),
        "回送失败不得回显为已放行: text={}",
        text
    );

    let before = app.messages.len();
    app.approve_request("allow");
    assert_eq!(app.messages.len(), before + 1, "空队列再入只提示");
    assert!(app.messages.back().unwrap().content.contains("没有待决议"));
}

/// §6.12 #5：降级完成的回复带「· via fallback」角标且标记只消费一次；
/// 角标是瞬时呈现态，不落任何持久面（TUI 侧无记忆写入口，X1-28a）。
#[test]
fn fallback_badge_is_transient_and_once() {
    let _h = crate::test_env::Home::new("fallback-badge");
    let mut app = App::new("agents/main.agent.yaml");
    app.memory = Box::new(MirrorMemory::volatile());
    app.fallback = true;

    let run = |body: &str| RunResponse {
        session_id: "s".to_string(),
        response: body.to_string(),
        tokens_used: None,
        cost_usd: None,
        thinking: None,
        tool_trace: None,
        gccp_need_interaction: false,
        gccp_questions: Vec::new(),
        status: None,
        resume_token: None,
        rounds_used: None,
        rounds_limit: None,
        ..Default::default()
    };

    app.apply_chat_result("问题".to_string(), Ok(run("降级回复正文")));
    let last = app.messages.back().expect("回复已入库");
    assert!(last.content.contains("降级回复正文"), "c={}", last.content);
    assert!(
        last.content.contains("· via fallback"),
        "降级回复须带角标: c={}",
        last.content
    );

    let recent = app.memory.recent(4);
    assert_eq!(recent.len(), 0, "TUI 不写记忆: {:?}", recent);

    app.apply_chat_result("追问".to_string(), Ok(run("正常回复")));
    let last = app.messages.back().expect("第二条回复已入库");
    assert!(
        !last.content.contains("via fallback"),
        "标记不得跨轮残留: c={}",
        last.content
    );
}

/// 缺口④（0.1.20）：Plugins 面板整表滚动——翻页偏移只增不减到负、封顶不回绕。
#[test]
fn plugins_scroll_clamps() {
    let _h = crate::test_env::Home::new("plugins-scroll");
    let mut app = App::new("agents/main.agent.yaml");
    app.plugins_pgup();
    assert_eq!(app.plugins_scroll, 0, "顶部 PgUp 不得回绕");
    app.plugins_pgdn();
    assert!(app.plugins_scroll > 0, "PgDn 应下翻");
    app.plugins_pgup();
    assert_eq!(app.plugins_scroll, 0, "翻回顶部归零");
}

/// 缺口③（0.1.20）：工具结果正文必须入对话历史——此前成功结果整体丢弃、
/// 失败只留 120 字符首行，用户回看对话无法知道工具实际返回了什么。
#[test]
fn tool_result_body_lands_in_history() {
    let _h = crate::test_env::Home::new("hist-trace");
    let mut app = App::new("agents/main.agent.yaml");
    app.memory = Box::new(MirrorMemory::volatile());

    let trace = crate::client::ToolTrace {
        tool: "fs_read".to_string(),
        arguments: "{}".to_string(),
        result: "第一行正文\n第二行正文".to_string(),
        ok: Some(1),
        switch_reason: None,
    };
    let run = RunResponse {
        session_id: "s".to_string(),
        response: "done".to_string(),
        tokens_used: None,
        cost_usd: None,
        thinking: None,
        tool_trace: Some(vec![trace]),
        gccp_need_interaction: false,
        gccp_questions: Vec::new(),
        status: None,
        resume_token: None,
        rounds_used: None,
        rounds_limit: None,
        ..Default::default()
    };
    app.apply_chat_result("读文件".to_string(), Ok(run));
    let hit = app
        .messages
        .iter()
        .find(|m| m.role == MessageRole::ToolResult)
        .expect("成功结果应入史");
    assert!(
        hit.content.contains("第二行正文"),
        "正文须完整入史（非仅首行）: c={}",
        hit.content
    );
}

// ─────────── U15（0.1.19 L7）：实时统计节流（§6.7 #5）───────────

/// §6.7 #5：busy 耗时统计按 DEFAULT_LIVE_PUBLISH_INTERVAL_MS 节拍发布——
/// 节拍内多帧推进（16ms 帧节拍）快照冻结不随帧重算，跨节拍才取真实
/// 耗时；busy 重启即归零，不残留上一轮耗时（状态行不逐 delta 刷屏）。
#[test]
fn busy_secs_snapshot_publishes_on_interval() {
    let _h = crate::test_env::Home::new("busy-secs-throttle");
    let mut app = App::new("agents/main.agent.yaml");

    app.begin_busy();
    assert_eq!(app.busy_shown_secs, 0.0, "busy 启动即归零");

    // 首帧（<250ms）：保持受理初值，不逐帧重算
    app.set_clock(0);
    assert_eq!(app.busy_shown_secs, 0.0);
    // 同一节拍内连续多帧：快照冻结，不随帧跳变
    app.set_clock(16);
    app.set_clock(200);
    app.set_clock(249);
    assert_eq!(app.busy_shown_secs, 0.0, "节拍内快照不得跳变");
    // 跨节拍：发布真实耗时（时钟单调，快照必已离开零值）
    app.set_clock(250);
    assert!(app.busy_shown_secs > 0.0, "跨节拍须刷新: {}", app.busy_shown_secs);
    // 新节拍内再推进：继续冻结
    let shown = app.busy_shown_secs;
    app.set_clock(400);
    assert_eq!(app.busy_shown_secs, shown, "新节拍内不得刷新");
    // 再跨节拍：继续按节拍发布
    app.set_clock(500);
    assert!(app.busy_shown_secs > shown, "再跨节拍须继续刷新");
    // busy 重启：归零，上一轮耗时不得残留
    app.begin_busy();
    assert_eq!(app.busy_shown_secs, 0.0);
}

// ─────────── C-14-07：任务集 Failed/Paused 状态机与三选 ───────────

fn paused_run() -> RunResponse {
    RunResponse {
        session_id: "s".to_string(),
        response: String::new(),
        tokens_used: Some(3),
        cost_usd: None,
        thinking: None,
        tool_trace: None,
        gccp_need_interaction: false,
        gccp_questions: Vec::new(),
        status: Some("paused".to_string()),
        resume_token: Some("tok-1".to_string()),
        rounds_used: Some(8),
        rounds_limit: Some(8),
        ..Default::default()
    }
}

/// 预算暂停：状态迁移只认 run_end.status，保存令牌并给出三选，且不把
/// 引擎预算提示当任务回复入史。
#[test]
fn paused_run_enters_three_choice_state() {
    use crate::gccp::{FlowPhase, TaskState};
    let _h = crate::test_env::Home::new("c14-paused");
    let mut app = App::new("agents/main.agent.yaml");
    app.memory = Box::new(MirrorMemory::volatile());
    app.task_mode = true;
    app.set_flow_phase(FlowPhase::Executing);

    app.apply_chat_result("执行任务".to_string(), Ok(paused_run()));

    assert_eq!(app.task_state, TaskState::Paused, "预算耗尽须进入暂停态");
    assert_eq!(app.resume_token.as_deref(), Some("tok-1"), "续跑令牌须保存");
    let note = app.messages.back().expect("暂停说明已入史").content.clone();
    assert!(note.contains("工具轮次 8/8"), "须说明预算用量: {note}");
    for key in ["继续", "放弃", "查看失败步骤"] {
        assert!(note.contains(key), "三选缺少 {key}: {note}");
    }
    assert!(
        !app.messages
            .iter()
            .any(|m| m.role == MessageRole::Agent && m.content.contains("budget")),
        "预算提示不得作为回复入史"
    );
}

/// C-15 / IS-12：暂停只由 task_state 事件推进，本地不持有暂停态。
#[test]
fn pause_state_follows_task_events() {
    use crate::gccp::{FlowPhase, TaskState};
    let _h = crate::test_env::Home::new("c15-pause-events");
    let mut app = App::new("agents/main.agent.yaml");
    app.task_mode = true;
    app.set_flow_phase(FlowPhase::Executing);

    app.observe_task_state("paused");
    assert_eq!(app.task_state, TaskState::Paused);
    app.observe_task_state("executing");
    assert_eq!(app.task_state, TaskState::Executing);
    app.observe_task_state("unknown-state");
    assert_eq!(
        app.task_state,
        TaskState::Executing,
        "未知状态不得改写任务态"
    );
}

/// 非预算中断：任务上下文中的错误落到 Failed，禁止静默停留在执行中。
#[test]
fn failed_run_enters_failed_state() {
    use crate::gccp::TaskState;
    let _h = crate::test_env::Home::new("c14-failed");
    let mut app = App::new("agents/main.agent.yaml");
    app.memory = Box::new(MirrorMemory::volatile());
    app.task_mode = true;

    app.apply_chat_result(
        "执行任务".to_string(),
        Err(anyhow::anyhow!("tool service unreachable")),
    );

    assert_eq!(app.task_state, TaskState::Failed);
    let note = app.messages.back().expect("失败说明已入史").content.clone();
    assert!(note.contains("查看失败步骤"), "须给出失败出口: {note}");
}

/// D-12：暂停态无本地令牌时「继续」改从服务端取回断点，不整轮重放。
/// 重启后本地令牌随进程消失，但 agent_d 仍保存着 paused 回合。
#[tokio::test]
async fn resume_without_token_queries_server_turns() {
    use crate::gccp::TaskState;
    let _h = crate::test_env::Home::new("c14-no-token");
    let mut app = App::new("agents/main.agent.yaml");
    app.task_state = TaskState::Paused;
    app.resume_token = None;

    app.submit_input("继续").expect("continue");
    assert!(app.is_busy(), "应发起 agent.turns 断点查询");
    let note = app.messages.back().expect("说明已入史").content.clone();
    assert!(
        note.contains("从服务端取回断点"),
        "须说明正在取回断点: {note}"
    );
    app.abort_task();
}

/// D-12：agent.turns 返回 paused 回合时，取令牌原地续跑。
#[tokio::test]
async fn apply_turn_state_resumes_with_server_token() {
    use crate::gccp::TaskState;
    let _h = crate::test_env::Home::new("c14-turns");
    let mut app = App::new("agents/main.agent.yaml");
    app.task_state = TaskState::Paused;
    app.resume_token = None;
    attach_pipe(&mut app);

    let turns = serde_json::json!({"count": 2, "turns": [
        {"turn_id": "a", "state": "running", "token": "skip"},
        {"turn_id": "b", "state": "paused", "token": "tok-9"},
    ]});
    app.apply_turn_state(Ok(turns));
    assert!(app.is_busy(), "取得令牌后应发起续跑");
    assert_eq!(app.task_state, TaskState::Executing);
    app.abort_task();
}

/// 有令牌时「继续」以 resume_token 续跑，且不把「继续」当用户消息入史。
#[tokio::test]
async fn resume_with_token_dispatches_resume() {
    use crate::gccp::TaskState;
    let _h = crate::test_env::Home::new("c14-resume");
    let mut app = App::new("agents/main.agent.yaml");
    app.task_state = TaskState::Paused;
    app.resume_token = Some("tok-2".to_string());
    attach_pipe(&mut app);

    app.submit_input("继续").expect("continue");
    assert!(app.is_busy(), "有令牌应发起续跑请求");
    assert!(
        !app.messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "继续"),
        "三选输入不应作为用户消息入史"
    );
    app.abort_task();
}

/// 查看失败步骤：只读轨迹字段，列出失败调用与换路原因。
#[test]
fn view_failed_steps_lists_switch_reason() {
    use crate::gccp::TaskState;
    let _h = crate::test_env::Home::new("c14-steps");
    let mut app = App::new("agents/main.agent.yaml");
    app.task_state = TaskState::Failed;
    app.last_tool_trace = vec![crate::client::ToolTrace {
        tool: "shell_run".to_string(),
        arguments: "{}".to_string(),
        result: "Error: command not found".to_string(),
        ok: Some(0),
        switch_reason: Some("identical call blocked after 2 failures".to_string()),
    }];

    app.submit_input("查看失败步骤").expect("steps");
    let note = app.messages.back().expect("步骤说明已入史").content.clone();
    assert!(note.contains("shell_run"), "须列出失败工具: {note}");
    assert!(
        note.contains("identical call blocked"),
        "须列出换路原因: {note}"
    );
    assert!(!app.is_busy(), "查看失败步骤不应发起请求");
}
