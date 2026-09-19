// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话记忆模块。
//
// 设计原则（50 工程标准 E-3 资源确定性 / A-1 极简主义；0.1.16 架构改造，
// 用户决策 2026-09-13「CLI 是核心，TUI 是可选的增强」）：
//   - TUI 不持有任何独立记忆存储。唯一权威后端是网关记忆服务 mem_d，
//     经 gateway `mem.*` RPC 访问——与 CLI（cli_chat_memory.c）同一后端、
//     同一存储、同一 schema，二者共享同一记忆库。TUI 只是渲染层。
//   - 进程内 GatewayMemory.mirror 仅作渲染/召回的**同步读缓存**：启动时经
//     `mem.recent` 水合，写入经 `mem.write` 异步回投网关，打开记忆面板时
//     再水合一次。网关不可达时降级为纯内存（volatile），不落任何本地文件。
//   - 平台级长期记忆（分层、遗忘衰减、语义检索等）一律由 mem_d 提供；TUI
//     绝不经 FFI 直连 daemon 或运行时库（0.1.15 铁律 / T-09）。
//
// 历史：0.1.15 及以前 TUI 使用独立本地 JSONL 库
// （$AIRY_HOME/data/agentrt/tui/memory.jsonl），与网关记忆库互不相通——
// 这正是"CLI 核心 / TUI 增强"决策下必须消除的存储分叉。本模块随 0.1.16
// 架构改造收敛为网关单后端（依据 0.1.9 §7 W4「记忆面板统一为 gateway」）。

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::client::GatewayClient;

/// 镜像保留上限（渲染/召回窗口；网关侧容量由 mem_d 自行管理）。
const MAX_RECORDS: usize = 2000;

/// 启动/面板打开时的水合条数（mem.recent 上限：mem_handlers clamp 0..1000）。
const HYDRATE_LIMIT: usize = 1000;

/// 单条记忆记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub role: String, // user / assistant / system
    pub content: String,
    pub timestamp: String, // ISO8601
    pub tags: String,      // 逗号分隔，如 "task,chat,preference"
    /// 2.1.1.6：思考链（reasoning_content）随助手回复持久化保留，
    /// 存于网关记录的 metadata.reasoning 字段。
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// 召回结果（内容 + 相关度得分）
#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub content: String,
    pub role: String,
    pub score: f32,
    /// 0.1.18 B1：归属轮次（写入侧 tags 内 "turn:N" 标注；旧记录无标注
    /// 为 None）。上下文注入时标注归属，为"按轮检索"铺路。
    pub turn: Option<u64>,
}

/// 对话记忆后端 trait（E-4 跨平台一致性：Linux/macOS/Windows 路径统一）
pub trait ConversationMemory: Send + Sync {
    /// 写入一条记忆。
    ///
    /// 0.1.18 B4（隐私）：不再提供携带思考链（reasoning_content）的写入
    /// 路径——思考链是模型内部推理碎片，不得落长期会话记忆，否则会被后续
    /// 轮次的召回回灌进上下文。历史记忆中的 reasoning 字段仍可被反序列化
    /// 读取（兼容 CLI 旧记录），但 TUI 侧只写不携带。
    fn push(&mut self, role: &str, content: &str, tags: &str) -> std::io::Result<()>;
    /// 召回与 query 相关、且 time_before 之前的记忆
    fn recall(&self, query: &str, limit: usize) -> Vec<MemoryHit>;
    /// 最近 N 条对话（按时间倒序）
    fn recent(&self, n: usize) -> Vec<MemoryRecord>;
    /// 记忆条数
    fn len(&self) -> usize;
    /// 从权威后端重新水合镜像（网关可用时）。默认后端为纯内存，no-op。
    fn refresh(&self) {}
    /// 2.2.2.1：当前记忆后端名（TUI 展示用）。
    fn backend_name(&self) -> &'static str {
        "gateway"
    }
}

/// 网关记忆后端（TUI 唯一后端）。
///
/// 权威存储为网关记忆服务 mem_d（`mem.*` RPC，与 CLI 同一后端）；
/// 本结构持有的 mirror 是渲染/召回的同步读缓存：启动水合 + 写入回投 +
/// 面板打开时再水合。无 tokio 运行时（单测）时降级为纯内存 volatile。
pub struct GatewayMemory {
    /// 网关客户端（refresh 用水合；None = volatile 降级）
    client: Option<GatewayClient>,
    /// 同步读缓存（旧→新有序，recent 时反向取）
    mirror: Arc<Mutex<Vec<MemoryRecord>>>,
    /// 写投递通道：push 立即入队，后台任务串行投递 `mem.write`
    tx: Option<tokio::sync::mpsc::UnboundedSender<MemoryRecord>>,
    /// 后端名（"gateway" / "volatile"），面板展示用
    backend: &'static str,
}

impl GatewayMemory {
    /// 构造网关记忆后端。
    ///
    /// 需处于 tokio 运行时内（`run_tui` 的 App::new）；同步上下文（单测）
    /// 无运行时，自动降级为纯内存镜像（volatile，不触网、不落盘）。
    pub fn new(client: GatewayClient) -> Self {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                log::warn!("memory: 无 tokio 运行时，记忆降级为纯内存镜像（volatile）");
                return Self::volatile();
            }
        };
        let mirror: Arc<Mutex<Vec<MemoryRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        handle.spawn(writer(client.clone(), rx));
        handle.spawn(hydrate(client.clone(), mirror.clone()));
        Self {
            client: Some(client),
            mirror,
            tx: Some(tx),
            backend: "gateway",
        }
    }

    /// 纯内存镜像后端（离线/单测）：不触网、不落盘。
    pub fn volatile() -> Self {
        Self {
            client: None,
            mirror: Arc::new(Mutex::new(Vec::new())),
            tx: None,
            backend: "volatile",
        }
    }

    fn push_impl(&mut self, role: &str, content: &str, tags: &str) -> std::io::Result<()> {
        let rec = MemoryRecord {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            tags: tags.to_string(),
            // 0.1.18 B4：TUI 写入恒不携带思考链（字段仅为兼容反序列化
            // CLI 旧记录而保留）。
            reasoning: None,
        };
        // 权威存储在网关：入队后台投递 `mem.write`。投递失败（运行时已退出）
        // 仅记日志——镜像已乐观更新，用户交互不因此中断。
        if let Some(tx) = &self.tx {
            if tx.send(rec.clone()).is_err() {
                log::warn!("memory: 网关写投递通道已关闭，记录仅在镜像");
            }
        }
        let mut guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(rec);
        if guard.len() > MAX_RECORDS {
            let drain = guard.len() - MAX_RECORDS;
            guard.drain(..drain);
        }
        Ok(())
    }
}

impl ConversationMemory for GatewayMemory {
    fn push(&mut self, role: &str, content: &str, tags: &str) -> std::io::Result<()> {
        self.push_impl(role, content, tags)
    }

    fn recall(&self, query: &str, limit: usize) -> Vec<MemoryHit> {
        let tokens: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .filter(|t| t.len() >= 2)
            .collect();
        // 防自我回灌（2026-08-26，与 CLI cli_chat_memory.c 同语义）：当前输入
        // 在发起对话前刚被 push 为 user 记录，query 又是这条输入本身，直接
        // 命中率最高。把该记录注入上下文会形成"模型读到自己刚收到的输入的
        // 记忆"的回声，污染当前问题的理解。跳过与 query 相同/带「用户: 」
        // 前缀的记录。
        let q = query.trim();
        let now = chrono::Utc::now().timestamp();
        let guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        let mut hits: Vec<MemoryHit> = guard
            .iter()
            .filter(|r| r.role != "system") // 系统提示不召回
            .filter(|r| {
                let c = r.content.trim();
                if c.eq_ignore_ascii_case(q) {
                    return false;
                }
                // 「用户: <input>」前缀形式（CLI 写入的旧格式记忆）
                if let Some(stripped) = c.strip_prefix("用户: ") {
                    if stripped.trim() == q {
                        return false;
                    }
                }
                true
            })
            .filter_map(|r| {
                let mut score = 0.0f32;
                for t in &tokens {
                    let lt = t.to_lowercase();
                    if r.content.to_lowercase().contains(&lt) {
                        score += 1.0;
                    }
                    if r.tags.to_lowercase().contains(&lt) {
                        score += 0.5;
                    }
                    /* 缺口 #8 修复：思考链（reasoning）参与召回打分——此前
                     * 只匹配 content/tags，思考链"只存档不可用"。权重低于
                     * content（思考链是内部推导，非直接事实表述）。 */
                    if let Some(rz) = r.reasoning.as_ref() {
                        if rz.to_lowercase().contains(&lt) {
                            score += 0.3;
                        }
                    }
                }
                if score <= 0.0 {
                    return None;
                }
                // 时效加权：越近越高（线性衰减 30 天）
                let age = (now - parse_ts(&r.timestamp)) as f32;
                let decay = (1.0 - (age / (30.0 * 86400.0))).clamp(0.2, 1.0);
                // 0.1.18 B1：解析写入侧的轮次标注（"turn:N"），召回侧透传
                let turn = r.tags.split(',').find_map(|t| {
                    t.trim()
                        .strip_prefix("turn:")
                        .and_then(|n| n.parse::<u64>().ok())
                });
                Some(MemoryHit {
                    content: r.content.clone(),
                    role: r.role.clone(),
                    score: score * decay,
                    turn,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        hits
    }

    fn recent(&self, n: usize) -> Vec<MemoryRecord> {
        let guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().rev().take(n).cloned().collect()
    }

    fn len(&self) -> usize {
        self.mirror.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn refresh(&self) {
        let (Some(client), Ok(handle)) =
            (self.client.as_ref(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        handle.spawn(hydrate(client.clone(), self.mirror.clone()));
    }

    fn backend_name(&self) -> &'static str {
        self.backend
    }
}

/// 后台写投递：把镜像新增记录逐条经 `mem.write` 持久化到网关记忆服务。
///
/// 记录 schema（与 CLI 共享同一存储的唯一约定）：
///   data     = 记忆内容
///   metadata = { source:"tui", role, tags, reasoning? }
async fn writer(client: GatewayClient, mut rx: tokio::sync::mpsc::UnboundedReceiver<MemoryRecord>) {
    while let Some(rec) = rx.recv().await {
        let mut meta = serde_json::Map::new();
        meta.insert("source".to_string(), serde_json::json!("tui"));
        meta.insert("role".to_string(), serde_json::json!(rec.role));
        meta.insert("tags".to_string(), serde_json::json!(rec.tags));
        if let Some(rz) = rec.reasoning.as_ref() {
            meta.insert("reasoning".to_string(), serde_json::json!(rz));
        }
        let params = serde_json::json!({
            "data": rec.content,
            "metadata": serde_json::Value::Object(meta),
        });
        if let Err(e) = client.rpc_call("mem.write", params).await {
            log::warn!("memory: mem.write 失败（记录仅在镜像，待下次水合）: {}", e);
        }
    }
}

/// 从网关记忆服务水合镜像：`mem.recent` 取最近 HYDRATE_LIMIT 条，按
/// created_at 升序重建镜像（旧→新）。失败时保持镜像现状（离线可用）。
async fn hydrate(client: GatewayClient, mirror: Arc<Mutex<Vec<MemoryRecord>>>) {
    let params = serde_json::json!({ "limit": HYDRATE_LIMIT });
    let result = match client.rpc_call("mem.recent", params).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("memory: mem.recent 水合失败（离线，镜像保持现状）: {}", e);
            return;
        }
    };
    let Some(items) = result.get("records").and_then(|v| v.as_array()) else {
        return;
    };
    let mut recs: Vec<(i64, MemoryRecord)> = Vec::with_capacity(items.len());
    for it in items {
        let data = it.get("data").and_then(|v| v.as_str()).unwrap_or("");
        if data.trim().is_empty() {
            continue;
        }
        let created = it.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let md = parse_metadata(it.get("metadata"));
        let tags = md_str(&md, "tags")
            .or_else(|| md_str(&md, "kind"))
            .unwrap_or_else(|| "chat".to_string());
        // TUI 记录 metadata.role 为权威角色；CLI 记录无 role（整轮合写一条），
        // 按 CLI 格式还原为 user/assistant 两条——同一 mem_d 的忠实渲染。
        match md_str(&md, "role") {
            Some(role) => recs.push((
                created,
                MemoryRecord {
                    role,
                    content: data.to_string(),
                    timestamp: epoch_to_iso(created),
                    tags,
                    reasoning: md_str(&md, "reasoning"),
                },
            )),
            None => {
                for r in split_cli_turn(created, data, &tags) {
                    recs.push((created, r));
                }
            }
        }
    }
    recs.sort_by_key(|(t, _)| *t);
    let mut list: Vec<MemoryRecord> = recs.into_iter().map(|(_, r)| r).collect();
    if list.len() > MAX_RECORDS {
        let drain = list.len() - MAX_RECORDS;
        list.drain(..drain);
    }
    let n = list.len();
    let mut guard = mirror.lock().unwrap_or_else(|e| e.into_inner());
    *guard = list;
    log::info!("memory: 自网关水合 {} 条记忆（mem.recent）", n);
}

/// 还原 CLI 写入的组合轮记录为 user/assistant 两条。
///
/// CLI（`cli_chat_memory.c:cli_chat_mem_record_gw`）每轮以**单条** `mem.write`
/// 记录整轮：`data = "用户: <input>\nAgentRT: <reply>[\n[reasoning] <chain>]"`
/// 且 metadata 无 `role`。TUI 是共享记忆（同一 mem_d）的渲染层，若整轮按单条
/// `user` 呈现则角色错乱——故按该格式拆分还原；非该格式（TUI 自身记录、或
/// 其它写入方）原样保留为单条。
fn split_cli_turn(created: i64, data: &str, tags: &str) -> Vec<MemoryRecord> {
    const USER_PREFIX: &str = "用户: ";
    const ASSISTANT_SEP: &str = "\nAgentRT: ";
    const REASONING_SEP: &str = "\n[reasoning] ";
    let ts = epoch_to_iso(created);
    let single = |role: &str, content: &str, reasoning: Option<String>| MemoryRecord {
        role: role.to_string(),
        content: content.to_string(),
        timestamp: ts.clone(),
        tags: tags.to_string(),
        reasoning,
    };
    let Some(rest) = data.strip_prefix(USER_PREFIX) else {
        return vec![single("user", data, None)];
    };
    let Some(pos) = rest.find(ASSISTANT_SEP) else {
        // 仅用户侧（无回复）——按 user 单条。
        return vec![single("user", rest, None)];
    };
    let user = &rest[..pos];
    let reply_part = &rest[pos + ASSISTANT_SEP.len()..];
    let (reply, reasoning) = match reply_part.find(REASONING_SEP) {
        Some(rp) => (
            &reply_part[..rp],
            Some(reply_part[rp + REASONING_SEP.len()..].to_string()),
        ),
        None => (reply_part, None),
    };
    vec![
        single("user", user, None),
        single("assistant", reply, reasoning),
    ]
}

/// 解析 gateway 返回的 metadata：mem.recent 以字符串回传（stored JSON），
/// 也容忍对象形式（内部旧路径）。供技能库（skills.rs）同一后端复用。
pub(crate) fn parse_metadata(v: Option<&serde_json::Value>) -> serde_json::Value {
    match v {
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
        }
        Some(other) => other.clone(),
        None => serde_json::Value::Null,
    }
}

/// metadata 取字符串字段。供技能库（skills.rs）同一后端复用。
pub(crate) fn md_str(md: &serde_json::Value, key: &str) -> Option<String> {
    md.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// 解析 ISO 时间戳（YYYY-MM-DDTHH:MM:SS，本地时区墙钟）为 epoch 秒；失败返回 0。
///
/// 与 `push_impl` / `epoch_to_iso` 一致按本地时区解释（面板按本地时间展示）；
/// 返回值为绝对 epoch 秒，可与时区无关地与 `now` 相减算时效衰减。
fn parse_ts(ts: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// epoch 秒 → 本地 ISO 时间戳（与 MemoryRecord.timestamp 同格式）。
/// 供技能库（skills.rs）同一后端复用。
pub(crate) fn epoch_to_iso(secs: i64) -> String {
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_else(|| Local::now().format("%Y-%m-%dT%H:%M:%S").to_string())
}

/// 构造记忆模块：TUI 唯一后端为网关记忆服务（`mem.*` RPC，与 CLI 同一
/// 存储）；无 tokio 运行时时降级为纯内存镜像（不落盘）。
pub fn build_memory(gateway: &GatewayClient) -> Box<dyn ConversationMemory> {
    Box::new(GatewayMemory::new(gateway.clone()))
}

#[cfg(test)]
mod tests {
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
}
