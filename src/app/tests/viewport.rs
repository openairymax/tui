// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

use super::super::*;

/// SSE 工具事件渲染（D-11）：以真实 tool_id + 参数摘要上屏，未登记工具
/// 照常显示（目录未加载也不隐藏）；成功结果正文仍不外泄。
#[test]
fn render_tool_event_parses_sse_json() {
    let call = r#"{"__airy_evt":"tool_call","tool":"web_search","args":"{\"query\":\"hello\"}"}"#;
    let line = App::render_tool_event(call).expect("tool_call renders");
    assert!(line.contains("web_search"), "tool_id 须可见: line={}", line);
    assert!(line.contains("hello"), "参数摘要须可见: line={}", line);

    let result = r#"{"__airy_evt":"tool_result","tool":"web_search","call_id":"c1","ok":1,"summary":"3 results"}"#;
    let line = App::render_tool_event(result).expect("tool_result renders");
    assert!(line.contains("完成"), "line={}", line);
    assert!(line.contains("web_search"), "line={}", line);
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
    assert!(line.contains("shell_run"), "tool_id 须可见: line={}", line);

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
    let mut app = App::new("agents/main.agent.yaml");

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
    let mut app = App::new("agents/main.agent.yaml");
    app.page_step = 9;
    assert_eq!(app.wheel_lines(false, false), 3);
    assert_eq!(app.wheel_lines(true, false), 9, "Shift = 翻页步长");
    assert_eq!(app.wheel_lines(false, true), 1, "Ctrl = 单行");
    assert_eq!(app.wheel_lines(true, true), 1, "Ctrl 优先细粒度");
}

/// X5-09b 锚点联动：上滚阅读时新增行只落在底部，视口内文字不漂移；
/// 宽度变化属重排，不得计入新增行。
#[test]
fn x5_anchor_holds_viewport_on_new_lines() {
    let _h = crate::test_env::Home::new("x5-anchor");
    let mut app = App::new("agents/main.agent.yaml");
    app.follow_observe(10, 80);

    app.scroll_offset = 4;
    app.follow_observe(13, 80);
    assert_eq!(app.scroll_offset, 7, "新增 3 行 → 距底偏移同步 +3");

    let before = app.scroll_offset;
    app.follow_observe(13, 60);
    assert_eq!(app.scroll_offset, before, "重排不改变锚点");

    app.follow_observe(20, 60);
    assert_eq!(app.scroll_offset, 14, "宽度稳定后继续按新增行增长");

    app.scroll_offset = 0;
    app.follow_observe(24, 60);
    assert_eq!(app.scroll_offset, 0, "钉底时保持跟随最新");
}

/// X5（0.1.21）：鼠标为一等公民，捕获默认开；Ctrl+M 翻转状态，run_app
/// 循环头据差分发送终端序列，关闭后回归终端原生文本选择。
#[test]
fn b11_toggle_mouse_capture_flips_state() {
    let _h = crate::test_env::Home::new("b11-mouse-toggle");
    let mut app = App::new("agents/main.agent.yaml");
    assert!(app.mouse_capture, "默认开");
    assert!(!app.toggle_mouse_capture());
    assert!(!app.mouse_capture);
    assert!(app.toggle_mouse_capture());
}

/// X8-03a/b：顶部前插更早记录时冻结一次跟随增长，视口锚点不向旧内容
/// 漂移；冻结只消耗一次，后续底部新增恢复原语义。
#[test]
fn x8_prepend_freeze_holds_anchor_once() {
    let _h = crate::test_env::Home::new("x8-prepend-freeze");
    let mut app = App::new("agents/main.agent.yaml");
    app.follow_observe(10, 80);
    app.scroll_offset = 4;

    app.follow_freeze_once = true;
    app.follow_observe(14, 80);
    assert_eq!(app.scroll_offset, 4, "前插 4 行不得推动距底偏移");
    assert_eq!(app.follow_new, 0, "旧内容不算新增行");

    app.follow_observe(15, 80);
    assert_eq!(app.scroll_offset, 5, "冻结只一次，后续按新增行增长");
}

/// X8-03a：滚到顶前插更早记录——以当前最早消息为锚，不重复、不越界，
/// 并置位冻结标记。
#[test]
fn x8_prepend_older_page_anchors() {
    let _h = crate::test_env::Home::new("x8-prepend-page");
    let mut app = App::new("agents/main.agent.yaml");
    // 水合路径只在网关后端生效（volatile 无落盘镜像）。
    app.memory = Box::new(crate::memory::MirrorMemory::new());
    let conv = app.current_conv();
    let at = crate::memory::HydrateLog {
        channel: "test",
        id: "mem.hydrate",
        conv: Some(conv.as_str()),
        elapsed_ms: 0,
    };
    let rec = |created: i64, role: &str, text: &str| {
        serde_json::json!({
            "data": text,
            "created_at": created,
            "metadata": serde_json::json!({ "role": role, "conv": conv }),
        })
    };
    let v = serde_json::json!({
        "records": [
            rec(1, "user", "旧一"),
            rec(2, "assistant", "旧二"),
            rec(3, "user", "新一"),
        ],
    });
    app.memory.ingest_recent_page(&v, &at, true);
    app.add_message(MessageRole::User, "新一".to_string());

    assert_eq!(app.prepend_older_page(&conv), 2, "仅前插锚点之前的记录");
    let texts: Vec<&str> = app.messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(texts, vec!["旧一", "旧二", "新一"]);
    assert!(app.follow_freeze_once, "前插后须冻结一次跟随增长");
    assert_eq!(app.prepend_older_page(&conv), 0, "锚点已在队首，不得重复");
}

/// B11（0.1.18）Alt+F 焦点视图：快照最近一条回复进入全屏只读视图；
/// 无回复时提示且不切面板；快照独立于对话区（打开后新消息不影响）。
#[test]
fn b11_focus_open_snapshots_last_reply() {
    let _h = crate::test_env::Home::new("b11-focus-open");
    let mut app = App::new("agents/main.agent.yaml");

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
    let mut app = App::new("agents/main.agent.yaml");
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
    let mut app = App::new("agents/main.agent.yaml");
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
    let mut app = App::new("agents/main.agent.yaml");
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
    let mut app = App::new("agents/main.agent.yaml");
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
    let mut app = App::new("agents/main.agent.yaml");
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
    let mut app = App::new("agents/main.agent.yaml");
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

/// X5-12b（P4 出口判据）：关闭鼠标捕获后，键盘仍覆盖点击动作的全部
/// 语义——半页滚动、无链接提示、无审批时的决议反馈都可达，不存在
/// 「只有鼠标能做」的操作。
#[test]
fn x5_keyboard_paths_work_without_mouse() {
    use crate::engine::action::Action;

    let _h = crate::test_env::Home::new("x5-keyboard-only");
    let mut app = App::new("agents/main.agent.yaml");
    assert!(!app.toggle_mouse_capture(), "捕获已关闭，仅键盘操作");
    app.chat_scroll_max = 40;
    app.page_step = 10;
    app.apply_action(Action::HalfUp);
    assert_eq!(app.scroll_offset, 5, "Alt+U 半页上滚");
    app.apply_action(Action::HalfDown);
    assert_eq!(app.scroll_offset, 0, "Alt+D 半页下滚");

    app.link_rows.clear();
    app.apply_action(Action::OpenLink);
    assert_eq!(app.toasts().len(), 1, "无链接时给出可达反馈而非静默");

    app.apply_action(Action::Allow);
    assert!(
        app.messages
            .back()
            .is_some_and(|m| m.content.contains("没有待决议")),
        "无审批时键盘决议也有明确反馈"
    );
}

