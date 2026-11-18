// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 记忆模块单测：镜像顺序、隐私（思考链不入召回）、CLI 整轮还原、
// 会话归属（D-14）与模式标记解析。

use super::*;

#[test]
fn push_recent_returns_newest_first() {
    let mut m = GatewayMemory::volatile();
    m.push("user", "我的名字是小明", "chat").expect("push");
    m.push("assistant", "你好，小明！", "chat").expect("push");
    assert_eq!(m.len(), 2);
    let recent = m.recent(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].role, "assistant");
    assert_eq!(recent[1].role, "user");
}

#[test]
fn push_never_records_reasoning() {
    // 0.1.18 B4（隐私）：思考链不得落长期会话记忆——写入路径不提供
    // reasoning 入参，落库记录的 reasoning 恒为 None（防止后续轮次召回
    // 把模型内部推理回灌进上下文）。
    let mut m = GatewayMemory::volatile();
    m.push("assistant", "最终回答", "chat").expect("push");
    m.push("user", "无思考链", "chat").expect("push");
    assert!(
        m.recent(2).iter().all(|r| r.reasoning.is_none()),
        "TUI 写入的记忆不得携带思考链"
    );
}

#[test]
fn push_records_conversation() {
    // D-14：写入记录携带会话归属，供 sessions.json 删档后按 conv 还原。
    let mut m = GatewayMemory::volatile();
    m.push("user", "无归属", "chat").expect("push");
    assert_eq!(m.recent(1)[0].conv, None, "未绑定会话时不伪造归属");
    m.set_conversation("conv_test");
    m.push("user", "有归属", "chat").expect("push");
    assert_eq!(m.recent(1)[0].conv.as_deref(), Some("conv_test"));
}

#[test]
fn volatile_backend_is_ready() {
    // 纯内存后端无异步水合，历史还原闸门恒开。
    let m = GatewayMemory::volatile();
    assert!(m.is_ready());
}

#[test]
fn recall_finds_related_by_keyword() {
    let mut m = GatewayMemory::volatile();
    m.push("user", "我在做性能监控优化", "chat").expect("push");
    m.push("user", "今天天气不错", "chat").expect("push");
    let hits = m.recall("性能监控", 3);
    assert!(!hits.is_empty());
    assert!(hits[0].content.contains("性能监控"));
}

#[test]
fn recall_ignores_system_role() {
    let mut m = GatewayMemory::volatile();
    m.push("system", "请判断模式", "meta").expect("push");
    m.push("user", "帮我写代码", "chat").expect("push");
    let hits = m.recall("判断模式", 3);
    assert!(hits.iter().all(|h| h.role != "system"));
}

#[test]
fn recall_excludes_self_feed() {
    // 防自我回灌（2026-08-26）：当前输入刚 push 后 recall 同 query，
    // 不得把该输入自身作为"相关记忆"回灌（与 CLI cli_chat_memory.c 同语义）。
    let mut m = GatewayMemory::volatile();
    m.push("user", "性能监控优化方案", "chat").expect("push"); // 旧记录
    m.push("user", "性能监控", "chat").expect("push"); // 当前输入（应被排除）
    let hits = m.recall("性能监控", 5);
    assert!(
        !hits.iter().any(|h| h.content == "性能监控"),
        "self-feed echo must be excluded"
    );
    assert!(
        hits.iter().any(|h| h.content == "性能监控优化方案"),
        "related older memory should still be recalled"
    );
}

#[test]
fn volatile_backend_name() {
    let m = GatewayMemory::volatile();
    assert_eq!(m.backend_name(), "volatile");
}

#[test]
fn metadata_parses_string_and_object() {
    // mem.recent 以字符串回传 stored JSON
    let md = parse_metadata(Some(&serde_json::json!(
        "{\"source\":\"tui\",\"role\":\"assistant\",\"tags\":\"task\"}"
    )));
    assert_eq!(md_str(&md, "role").as_deref(), Some("assistant"));
    assert_eq!(md_str(&md, "tags").as_deref(), Some("task"));
    // 缺失/非法 → Null，取值安全返回 None
    let md = parse_metadata(Some(&serde_json::json!("not-json")));
    assert_eq!(md_str(&md, "role"), None);
    assert_eq!(parse_metadata(None), serde_json::Value::Null);
}

#[test]
fn epoch_to_iso_roundtrip() {
    let ts = epoch_to_iso(1_700_000_000);
    assert_eq!(parse_ts(&ts), 1_700_000_000);
}

#[test]
fn split_cli_turn_restores_user_and_assistant() {
    // CLI 整轮合写一条（无 metadata.role），TUI 须还原为两条。
    let recs = split_cli_turn(
        1_700_000_000,
        "用户: 帮我看下内存\nAgentRT: 建议先看 mem_d 指标",
        "chat",
    );
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].role, "user");
    assert_eq!(recs[0].content, "帮我看下内存");
    assert_eq!(recs[1].role, "assistant");
    assert_eq!(recs[1].content, "建议先看 mem_d 指标");
    assert_eq!(recs[1].reasoning, None);
    assert_eq!(recs[0].tags, "chat");
    // 两条共享同一时间戳（同轮），时序由稳定排序保持 user→assistant
    assert_eq!(recs[0].timestamp, recs[1].timestamp);
    // CLI 记录无会话归属，不参与 TUI 会话还原
    assert!(recs.iter().all(|r| r.conv.is_none()));
}

#[test]
fn split_cli_turn_extracts_reasoning() {
    let recs = split_cli_turn(
        1_700_000_000,
        "用户: 为什么慢\nAgentRT: 因为锁竞争\n[reasoning] 先看火焰图",
        "chat",
    );
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[1].content, "因为锁竞争");
    assert_eq!(recs[1].reasoning.as_deref(), Some("先看火焰图"));
}

/// B4（0.1.18）V4.2：兼容反序列化的旧记录即便带思考链，召回命中
/// （`MemoryHit`）只承载 content/role/score/turn——推理原文没有任何注入
/// 通道进入上下文。构造一条"英文 CoT 与关键词同现"的旧记录：命中分虽被
/// reasoning 提权（+0.3），注入内容仍须纯净。
#[test]
fn legacy_reasoning_never_reaches_recall() {
    let m = GatewayMemory::volatile();
    {
        let mut g = m.mirror.lock().unwrap_or_else(|e| e.into_inner());
        g.push(MemoryRecord {
            role: "assistant".to_string(),
            content: "答案是先看火焰图".to_string(),
            timestamp: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            tags: "chat".to_string(),
            reasoning: Some("SECRET_COT_MARKER the lock contention is hot".to_string()),
            conv: None,
        });
    }
    let hits = m.recall("火焰图", 5);
    assert!(!hits.is_empty(), "应命中该旧记录: {:?}", hits);
    for h in &hits {
        assert!(
            !h.content.contains("SECRET_COT_MARKER"),
            "思考链原文不得进入召回注入内容: {h:?}"
        );
    }
}

#[test]
fn split_cli_turn_passes_through_other_formats() {
    // TUI 自身记录格式（无 "用户: " 前缀）→ 单条，内容原样
    let recs = split_cli_turn(1_700_000_000, "直接的一段话", "task");
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].role, "user");
    assert_eq!(recs[0].content, "直接的一段话");
    // 有 "用户: " 前缀但无回复 → 单条 user
    let recs = split_cli_turn(1_700_000_000, "用户: 只有提问", "chat");
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].role, "user");
    assert_eq!(recs[0].content, "只有提问");
}

#[test]
fn sanitize_reply_parses_markers() {
    // B3：模式判定能力不回归（V3.2），标记同时从正文剥离入协议段
    let r = crate::app::sanitize_reply("[MODE:TASK] 执行任务");
    assert_eq!(r.mode, crate::app::ModeMarker::Task);
    assert_eq!(r.body, "执行任务");
    assert!(r.protocol.contains("[MODE:TASK]"));

    let r = crate::app::sanitize_reply("[MODE:CHAT]\n闲聊");
    assert_eq!(r.mode, crate::app::ModeMarker::Chat);
    assert_eq!(r.body, "闲聊");

    let r = crate::app::sanitize_reply("普通回复");
    assert_eq!(r.mode, crate::app::ModeMarker::Chat);
    assert_eq!(r.body, "普通回复");
    assert!(r.protocol.is_empty());
}

#[test]
fn sanitize_reply_tolerates_leading_text() {
    // 容错（2026-08-26）：LLM 输出带简短前导（「好的，」等）时仍能识别
    // 模式标记；B3 后前导与标记一并剥离，正文只留纯净内容（V3.3）。
    let r = crate::app::sanitize_reply("好的，[MODE:TASK]\n开始执行任务");
    assert_eq!(r.mode, crate::app::ModeMarker::Task);
    assert_eq!(r.body, "开始执行任务");

    let r = crate::app::sanitize_reply("好的 [MODE:CHAT] 我们聊聊");
    assert_eq!(r.mode, crate::app::ModeMarker::Chat);
    assert_eq!(r.body, "我们聊聊");
    assert!(r.protocol.contains("[MODE:CHAT]"));

    // 正文中（256 字节窗口后）出现的标记样式文字不触发模式切换，
    // 且字面一律从用户面剥离（V3.1）
    let body = format!(
        "这是一段很长的普通对话内容，{}[MODE:TASK] 后续",
        "x".repeat(300)
    );
    let r = crate::app::sanitize_reply(&body);
    assert_eq!(r.mode, crate::app::ModeMarker::Chat);
    assert!(!r.body.contains("[MODE:"));
}
