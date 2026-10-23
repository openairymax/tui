// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::super::*;

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

/// U3（0.1.19 L7）：阈值边界——恰好 16KiB 直插，超 1 字节折叠；
/// 折叠后提交取回原文。
#[test]
fn paste_fold_threshold_boundary() {
    let _h = crate::test_env::Home::new("paste-fold-boundary");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    let limit = crate::budget::MAX_PASTE_FOLD_BYTES;

    let exact = "a".repeat(limit);
    app.input_paste(&exact);
    assert_eq!(app.input, exact, "阈值内直插");
    assert!(app.paste_folds.is_empty(), "阈值内不建折叠条目");
    assert_eq!(app.take_input(), exact);

    let over = "b".repeat(limit + 1);
    app.input_paste(&over);
    assert!(
        app.input.starts_with("[已粘贴 17 KB · 1 行]"),
        "超阈值须折叠为占位: input={}",
        app.input
    );
    assert_eq!(app.take_input(), over, "提交须取回原文");
    assert!(app.paste_folds.is_empty(), "提交后折叠台账清空");
}

/// U3（0.1.19 L7）：多字节文本按字节判阈、按字符推导（不切片），
/// 折叠与展开全程 UTF-8 安全。
#[test]
fn paste_fold_utf8_safe() {
    let _h = crate::test_env::Home::new("paste-fold-utf8");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    let body = "中文".repeat(crate::budget::MAX_PASTE_FOLD_BYTES / 6 + 1);

    app.input_paste(&body);
    assert!(app.input.contains("已粘贴"), "input={}", app.input);
    assert_eq!(app.take_input(), body, "多字节原文完整取回");
}

/// U3（0.1.19 L7）：多次折叠按出现顺序展开；占位被删改的条目丢弃，
/// 原文中恰好包含占位字样也不会被二次展开。
#[test]
fn paste_fold_multi_and_stale() {
    let _h = crate::test_env::Home::new("paste-fold-multi");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    let limit = crate::budget::MAX_PASTE_FOLD_BYTES;

    let first = "x\n".repeat(limit + 1);
    let second = "y\n".repeat(limit + 1);
    app.input_paste(&first);
    app.input_insert_text(" 中间 ");
    app.input_paste(&second);
    assert_eq!(
        app.take_input(),
        format!("{} 中间 {}", first, second),
        "同形占位按出现顺序各展开一次"
    );

    let mut inner = String::from("[已粘贴 17 KB · 1 行]");
    inner.push_str(&"z".repeat(limit));
    app.input_paste(&inner);
    assert_eq!(app.take_input(), inner, "原文内的占位字样不得二次展开");

    app.input_paste(&first);
    app.input.clear();
    assert_eq!(app.take_input(), "", "占位被删则原文随之丢弃");
    assert!(app.paste_folds.is_empty());
}

/// U6（0.1.19 L7，§6.10 #6）：大 cell 入库即裁剪——头尾保留、标记含
/// 原始字节数、originalBytes 照实记账，渲染树不随单条内容无界增长。
#[test]
fn add_message_caps_oversized_cell() {
    let _h = crate::test_env::Home::new("cell-cap");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    let body = format!("HEAD{}TAIL", "x".repeat(crate::budget::MAX_TOOL_TEXT_BYTES));

    app.add_message(MessageRole::Agent, body.clone());
    let msg = app.messages.back().expect("消息已入库");
    assert_eq!(msg.original_bytes, body.len(), "原始字节数照实记账");
    assert!(msg.content.len() < body.len(), "超预算须折叠");
    assert!(msg.content.starts_with("HEAD"), "头段保留");
    assert!(msg.content.ends_with("TAIL"), "尾段保留");
    assert!(
        msg.content
            .contains(&format!("[truncated: {} bytes total]", body.len())),
        "标记须携带原始字节数"
    );
}

/// U7（0.1.19 L7）：提示同屏不超过三条，到期由注入时刻裁决清退。
#[test]
fn toasts_stay_bounded_and_expire() {
    let _h = crate::test_env::Home::new("toast-cap");
    let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
    let mut app = App::new("agents/main.agent.yaml", gw);
    for i in 0..5 {
        app.toast(format!("提示 {i}"));
    }
    assert_eq!(
        app.toasts().len(),
        crate::budget::MAX_ACTIVE_TOASTS,
        "溢出淘汰最旧，同屏守住上限"
    );
    assert!(app.toast_wait(std::time::Instant::now()).is_some());
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(crate::budget::TOAST_TTL_MS);
    assert!(app.toast_expire(deadline), "到期须报告变化以补帧");
    assert_eq!(app.toasts().len(), 0);
}

