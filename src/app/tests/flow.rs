// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::*;
use super::super::*;
use crate::memory::GatewayMemory;
use std::time::Duration;

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

/// U14 §6.7 #4：完成通知由 App 生产、主循环消费，取走即清零（不重复弹）。
#[test]
fn notice_is_drained_exactly_once() {
    let _h = crate::test_env::Home::new("u14-notice");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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

/// U2（0.1.19 L7）：Ctrl+C 双击退出守卫——首按只提示，窗口内二次按下
/// 才确认；窗口边界按严格小于裁决，超时复位；busy 态共用同一入口。
#[test]
fn ctrl_c_exit_guard_window() {
    let _h = crate::test_env::Home::new("exit-guard");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
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

/// §6.5 #4：审批渠道故障 fail-closed——队列逐条按「unavailable」拒绝呈递，
/// 与用户主动拒绝（rejected）分属不同标注，绝不放行；空队列再入静默 no-op。
#[tokio::test]
async fn approval_unavailable_fails_closed() {
    let _h = crate::test_env::Home::new("approval-unavailable");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.approvals.push(approval_req("r1", "shell_exec"));
    app.approvals.push(approval_req("r2", "file_write"));

    app.reject_unavailable("channel closed");

    assert!(app.approvals.is_empty(), "渠道故障须清空待审队列");
    let text: String = app
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("审批不可用，已拒绝工具「shell_exec」"),
        "text={}",
        text
    );
    assert!(
        text.contains("（unavailable：channel closed）"),
        "text={}",
        text
    );
    assert!(
        !text.contains("（rejected"),
        "渠道故障不得与用户拒绝混同: text={}",
        text
    );

    let before = app.messages.len();
    app.reject_unavailable("again");
    assert_eq!(app.messages.len(), before, "空队列再入为 no-op");
}

/// §6.12 #5：降级完成的回复带「· via fallback」角标且标记只消费一次；
/// 角标是瞬时呈现态——长期记忆只存正文，不随记录落盘。
#[test]
fn fallback_badge_is_transient_and_once() {
    let _h = crate::test_env::Home::new("fallback-badge");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    app.memory = Box::new(GatewayMemory::volatile());
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
    assert_eq!(recent.len(), 1, "仅一条助手记忆: {:?}", recent);
    assert_eq!(recent[0].content, "降级回复正文", "记忆只存正文");

    app.apply_chat_result("追问".to_string(), Ok(run("正常回复")));
    let last = app.messages.back().expect("第二条回复已入库");
    assert!(
        !last.content.contains("via fallback"),
        "标记不得跨轮残留: c={}",
        last.content
    );
}

// ─────────── U15（0.1.19 L7）：实时统计节流（§6.7 #5）───────────

/// §6.7 #5：busy 耗时统计按 DEFAULT_LIVE_PUBLISH_INTERVAL_MS 节拍发布——
/// 节拍内多帧推进（16ms 帧节拍）快照冻结不随帧重算，跨节拍才取真实
/// 耗时；busy 重启即归零，不残留上一轮耗时（状态行不逐 delta 刷屏）。
#[test]
fn busy_secs_snapshot_publishes_on_interval() {
    let _h = crate::test_env::Home::new("busy-secs-throttle");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);

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

