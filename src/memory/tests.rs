// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 记忆模块单测：镜像顺序、召回语义、CLI 整轮还原、会话归属（D-14）与
// 模式标记解析。TUI 侧只读（X1-28a：写入口 = 0），夹具经 seed 注入。

use super::*;

#[test]
fn seed_recent_returns_newest_first() {
    let mut m = MirrorMemory::volatile();
    m.seed("user", "我的名字是小明", "chat", None);
    m.seed("assistant", "你好，小明！", "chat", None);
    assert_eq!(m.len(), 2);
    let recent = m.recent(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].role, "assistant");
    assert_eq!(recent[1].role, "user");
}

#[test]
fn seed_keeps_conversation() {
    // D-14：记录携带会话归属，供 sessions.json 删档后按 conv 还原。
    let mut m = MirrorMemory::volatile();
    m.seed("user", "无归属", "chat", None);
    assert_eq!(m.recent(1)[0].conv, None, "无归属记录不得伪造会话");
    m.seed("user", "有归属", "chat", Some("conv_test"));
    assert_eq!(m.recent(1)[0].conv.as_deref(), Some("conv_test"));
}

#[test]
fn volatile_backend_is_ready() {
    // 纯内存后端无异步水合，历史还原闸门恒开。
    let m = MirrorMemory::volatile();
    assert!(m.is_ready());
}

#[test]
fn recall_finds_related_by_keyword() {
    let mut m = MirrorMemory::volatile();
    m.seed("user", "我在做性能监控优化", "chat", None);
    m.seed("user", "今天天气不错", "chat", None);
    let hits = m.recall("性能监控", 3, None);
    assert!(!hits.is_empty());
    assert!(hits[0].content.contains("性能监控"));
}

#[test]
fn recall_ignores_system_role() {
    let mut m = MirrorMemory::volatile();
    m.seed("system", "请判断模式", "meta", None);
    m.seed("user", "帮我写代码", "chat", None);
    let hits = m.recall("判断模式", 3, None);
    assert!(hits.iter().all(|h| h.role != "system"));
}

#[test]
fn recall_excludes_self_feed() {
    // 防自我回灌（2026-08-26）：运行侧记下当前输入后 recall 同 query，
    // 不得把该输入自身作为"相关记忆"回灌（与 CLI cli_chat_memory.c 同语义）。
    let mut m = MirrorMemory::volatile();
    m.seed("user", "性能监控优化方案", "chat", None); // 旧记录
    m.seed("user", "性能监控", "chat", None); // 当前输入（应被排除）
    let hits = m.recall("性能监控", 5, None);
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
    let m = MirrorMemory::volatile();
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
        None,
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
    // 未提供 metadata.conv 的旧记录保持无归属，不参与按会话还原
    assert!(recs.iter().all(|r| r.conv.is_none()));
}

/// S-P1-16：写入侧 metadata.conv 透传到还原记录，--resume 后同一 CLI
/// 会话的多轮才能归入同一 tab（旧实现硬编码 None 导致全部串台）。
#[test]
fn split_cli_turn_carries_conv() {
    let recs = split_cli_turn(
        1_700_000_000,
        "用户: 继续上次\nAgentRT: 好的",
        "chat,turn:7",
        Some("sess_abc123"),
    );
    assert_eq!(recs.len(), 2);
    assert!(recs.iter().all(|r| r.conv.as_deref() == Some("sess_abc123")));
}

/// S-P1-16：给定 conv 时召回只返回该会话的记录；None 保持全局口径。
#[test]
fn recall_filters_by_conv() {
    let mut m = MirrorMemory::volatile();
    m.seed("user", "甲会话的性能监控结论", "chat", Some("sess_a"));
    m.seed("user", "乙会话的性能监控结论", "chat", Some("sess_b"));
    let scoped = m.recall("性能监控", 5, Some("sess_a"));
    assert_eq!(scoped.len(), 1);
    assert!(scoped[0].content.contains('甲'));
    let all = m.recall("性能监控", 5, None);
    assert_eq!(all.len(), 2);
}

/// S-P1-16：中文按二元组切分——查询"监控性能"在"性能监控优化"中没有
/// 整串子串，旧分词（整句单 token）零命中；二元组可跨词序命中。
#[test]
fn recall_cjk_bigram_crosses_order() {
    let mut m = MirrorMemory::volatile();
    m.seed("user", "性能监控优化的三条建议", "chat", None);
    let hits = m.recall("监控性能", 3, None);
    assert!(!hits.is_empty(), "二元组应命中: {:?}", hits);
}

/// S-P1-16：中英混合查询同时保留 ASCII 词与 CJK 二元组。
#[test]
fn query_tokens_mixes_ascii_and_cjk() {
    let t = super::query_tokens("看 mem_d 的延迟");
    assert!(t.iter().any(|s| s == "mem_d"));
    assert!(t.iter().any(|s| s == "延迟"));
    assert!(t.iter().any(|s| s == "的延"));
    // 单字段保留原字
    assert!(super::query_tokens("锁").iter().any(|s| s == "锁"));
}

#[test]
fn split_cli_turn_extracts_reasoning() {
    let recs = split_cli_turn(
        1_700_000_000,
        "用户: 为什么慢\nAgentRT: 因为锁竞争\n[reasoning] 先看火焰图",
        "chat",
        None,
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
    let m = MirrorMemory::volatile();
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
    let hits = m.recall("火焰图", 5, None);
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
    let recs = split_cli_turn(1_700_000_000, "直接的一段话", "task", None);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].role, "user");
    assert_eq!(recs[0].content, "直接的一段话");
    // 有 "用户: " 前缀但无回复 → 单条 user
    let recs = split_cli_turn(1_700_000_000, "用户: 只有提问", "chat", None);
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

/// 构造一页 mem.recent 应答（records 内 data/created_at/metadata）。
fn page(conv: &str, items: &[(i64, &str, &str)]) -> serde_json::Value {
    let records: Vec<serde_json::Value> = items
        .iter()
        .map(|(created, role, content)| {
            serde_json::json!({
                "data": content,
                "created_at": created,
                "metadata": serde_json::json!({ "role": role, "conv": conv }),
            })
        })
        .collect();
    serde_json::json!({ "records": records, "next": 0, "has_more": false })
}

fn conv_log(conv: &str) -> HydrateLog<'_> {
    HydrateLog {
        channel: "test",
        id: "mem.hydrate",
        conv: Some(conv),
        elapsed_ms: 0,
    }
}

/// X8-01a：首页之后的更早页必须追加而不是替换——替换会把刚取到的新页
/// 挤掉（旧实现整段 retain）。
#[test]
fn page_ingest_appends_older_page() {
    let m = MirrorMemory::new();
    let at = conv_log("sess_a");
    m.ingest_recent_page(
        &page(
            "sess_a",
            &[(300, "user", "新一"), (301, "assistant", "新二")],
        ),
        &at,
        true,
    );
    m.ingest_recent_page(&page("sess_a", &[(100, "user", "旧一")]), &at, false);
    assert_eq!(m.len(), 3, "更早页应追加: {:?}", m.recent(10));
    let recs = m.records_of("sess_a");
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].content, "旧一", "records_of 必须旧→新");
    assert_eq!(recs[2].content, "新二");
}

/// X8-02b：删除导致游标前移时会重复返回一段，按记录身份去重后无重复。
#[test]
fn page_ingest_dedupes_overlap() {
    let m = MirrorMemory::new();
    let at = conv_log("sess_b");
    m.ingest_recent_page(
        &page("sess_b", &[(200, "user", "甲"), (201, "assistant", "乙")]),
        &at,
        true,
    );
    // 第二页与第一页重叠一条（甲），另含一条更早的丙
    m.ingest_recent_page(
        &page("sess_b", &[(100, "user", "丙"), (200, "user", "甲")]),
        &at,
        false,
    );
    let recs = m.records_of("sess_b");
    assert_eq!(recs.len(), 3, "重叠记录只保留一份: {recs:?}");
    let contents: Vec<&str> = recs.iter().map(|r| r.content.as_str()).collect();
    assert_eq!(contents, vec!["丙", "甲", "乙"]);
}

/// 首页（新一轮水合）替换该会话段，且不影响其他会话。
#[test]
fn page_ingest_first_page_replaces_only_its_conv() {
    let m = MirrorMemory::new();
    m.ingest_recent_page(
        &page("sess_a", &[(100, "user", "a-旧"), (200, "user", "a-新")]),
        &conv_log("sess_a"),
        true,
    );
    m.ingest_recent_page(
        &page("sess_c", &[(150, "user", "c-独有")]),
        &conv_log("sess_c"),
        true,
    );
    // 会话 A 重新水合，只有一条新窗口记录
    m.ingest_recent_page(
        &page("sess_a", &[(200, "user", "a-新")]),
        &conv_log("sess_a"),
        true,
    );
    let a = m.records_of("sess_a");
    assert_eq!(a.len(), 1, "首页替换该会话段: {a:?}");
    assert_eq!(a[0].content, "a-新");
    assert_eq!(m.records_of("sess_c").len(), 1, "其他会话不受影响");
}

/// 全局窗口（conv=None）只追加不替换：不得冲掉已按会话水合的段。
#[test]
fn page_ingest_global_window_keeps_scoped_segments() {
    let m = MirrorMemory::new();
    m.ingest_recent_page(
        &page("sess_d", &[(100, "user", "会话内")]),
        &conv_log("sess_d"),
        true,
    );
    let global = HydrateLog {
        channel: "test",
        id: "mem.hydrate",
        conv: None,
        elapsed_ms: 0,
    };
    let mut v = page("", &[(90, "user", "无归属")]);
    v["records"][0]["metadata"] = serde_json::json!({ "role": "user" });
    m.ingest_recent_page(&v, &global, true);
    assert_eq!(m.len(), 2, "全局窗口不得清空会话段: {:?}", m.recent(10));
}
