// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话记忆模块。
//
// 设计原则（50 工程标准 E-3 资源确定性 / A-1 极简主义；0.1.16 架构改造，
// 用户决策 2026-09-13「CLI 是核心，TUI 是可选的增强」；0.1.21 X1-28a）：
//   - TUI 不持有任何独立记忆存储。唯一权威后端是网关记忆服务 mem_d，
//     经 gateway `mem.*` RPC 访问——与 CLI（cli_chat_memory.c）同一后端、
//     同一存储、同一 schema，二者共享同一记忆库。TUI 只是渲染层。
//   - 记忆写入归运行侧机制（CLI / daemon），TUI 侧**只读**：本模块不提供
//     任何写入口（无 `mem.write`，无 push/save 接口）。进程内 mirror 仅作
//     渲染/召回的同步读缓存，启动与面板打开时经 `mem.recent` 水合；网关
//     不可达时降级为纯内存（volatile），不落任何本地文件。
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

use std::sync::atomic::{AtomicBool, Ordering};

/// 镜像保留上限（渲染/召回窗口；网关侧容量由 mem_d 自行管理）。
///
/// 0.1.21 P1-3：水合改为按会话分段后，窗口不再是「全局 1000 条」，最坏
/// 情形是 TABS_MAX(9) × HYDRATE_LIMIT(1000) + 全局窗口。上限按此留余量，
/// 否则旧会话刚水合就被新会话挤出，等于没修。
const MAX_RECORDS: usize = 10_000;

/// 启动/面板打开时的水合条数（mem.recent 上限：mem_handlers clamp 0..1000）。
/// 会话历史还原（D-14）按同一窗口读取。
pub(crate) const HYDRATE_LIMIT: usize = 1000;

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
    /// D-14：所属会话标识（网关记录 metadata.conv）。CLI 旧记录无此字段。
    #[serde(default)]
    pub conv: Option<String>,
}

/// 召回结果（内容 + 相关度得分）
#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub content: String,
    pub role: String,
    pub score: f32,
    /// 0.1.18 B1：归属轮次（记录 tags 内 "turn:N" 标注，由运行侧写入；
    /// 旧记录无标注为 None）。上下文注入时标注归属，为"按轮检索"铺路。
    pub turn: Option<u64>,
}

/// 对话记忆后端 trait（E-4 跨平台一致性：Linux/macOS/Windows 路径统一）。
///
/// 只读契约（0.1.21 X1-28a）：TUI 不写记忆，trait 不设任何写方法；记忆
/// 由运行侧机制（CLI / daemon）写入 mem_d，TUI 只做水合、召回与渲染。
pub trait ConversationMemory: Send + Sync {
    /// 召回与 query 相关的记忆（按相关度截断）。
    ///
    /// `conv` 非空时只在同一会话内召回（D-14 会话隔离）；为空表示跨会话
    /// 召回（调用方无稳定会话身份时的兼容路径）。
    fn recall(&self, query: &str, limit: usize, conv: Option<&str>) -> Vec<MemoryHit>;
    /// 最近 N 条对话（按时间倒序）
    fn recent(&self, n: usize) -> Vec<MemoryRecord>;
    /// 记忆条数
    fn len(&self) -> usize;
    /// 水合接口：把 `mem.recent` 的查询结果落地到镜像（App 经事件管道
    /// 下发 query 帧后调用）。解析失败或离线时保持现状并置就绪——历史
    /// 还原闸门不因后端不可用而永挂。
    fn ingest_recent(&self, _result: &serde_json::Value) {}
    /// 按会话范围水合（P1-3）：`conv` 为 Some 时只替换该会话的记忆段，
    /// None 表示全局窗口（补齐无会话归属的记录）。默认实现退化为整表水合。
    fn ingest_recent_scope(&self, _conv: Option<&str>, result: &serde_json::Value) {
        self.ingest_recent(result);
    }
    /// 水合就绪闸门（D-14）：镜像已从权威后端加载完成。纯内存后端恒就绪。
    fn is_ready(&self) -> bool {
        true
    }
    /// 2.2.2.1：当前记忆后端名（TUI 展示用）。
    fn backend_name(&self) -> &'static str {
        "gateway"
    }
    /// 单测夹具：向镜像注入记录（生产无写入口，读取行为仍需可测）。
    #[cfg(test)]
    fn seed(&mut self, role: &str, content: &str, tags: &str, conv: Option<&str>);
}

/// 记忆镜像后端（TUI 唯一后端）。
///
/// 权威存储为网关记忆服务 mem_d（`mem.*`，与 CLI 同一后端）；本结构持有的
/// mirror 是渲染/召回的同步读缓存：启动与面板打开时由 App 下发查询水合。
/// 单测/离线可用 `volatile`（纯内存、不落盘）。写入不在本结构职责内
/// （0.1.21 X1-28a：TUI 只读）。
pub struct MirrorMemory {
    /// 同步读缓存（旧→新有序，recent 时反向取）
    mirror: Arc<Mutex<Vec<MemoryRecord>>>,
    /// 水合完成标志（会话历史还原的就绪闸门，D-14）
    ready: Arc<AtomicBool>,
    /// 后端名（"gateway" / "volatile"），面板展示用
    backend: &'static str,
}

impl MirrorMemory {
    /// 构造记忆镜像后端。
    ///
    /// 传输不在此处：水合查询由 App 经事件管道（query 帧）下发，结果经
    /// `ingest_recent` 落地。单测等同步上下文使用 volatile。
    pub fn new() -> Self {
        Self {
            mirror: Arc::new(Mutex::new(Vec::new())),
            ready: Arc::new(AtomicBool::new(false)),
            backend: "gateway",
        }
    }

    /// 纯内存镜像后端（离线/单测）：不触网、不落盘。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn volatile() -> Self {
        Self {
            mirror: Arc::new(Mutex::new(Vec::new())),
            ready: Arc::new(AtomicBool::new(true)),
            backend: "volatile",
        }
    }
}

/// 是否为 CJK 表意/假名区段（中文整句按二元组切分的判据）。
fn is_han(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF
    )
}

/// 查询分词：ASCII 词按分隔符切（≥2 字符），CJK 连续段取相邻二元组
/// （单字段保留单字）。与 mem_d `engine/vector.c` 的 `vec_han_step` 同口径；
/// 旧实现把中文整句当一个 token，跨记录的局部词永远无法命中。
fn query_tokens(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut han: Vec<char> = Vec::new();

    fn flush_word(word: &mut String, out: &mut Vec<String>) {
        if word.chars().count() >= 2 {
            out.push(word.to_lowercase());
        }
        word.clear();
    }

    fn flush_han(han: &mut Vec<char>, out: &mut Vec<String>) {
        if han.len() == 1 {
            out.push(han[0].to_string());
        } else {
            for w in han.windows(2) {
                out.push(w.iter().collect());
            }
        }
        han.clear();
    }

    for c in query.chars() {
        if is_han(c) {
            flush_word(&mut word, &mut out);
            han.push(c);
        } else if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            flush_han(&mut han, &mut out);
            word.push(c);
        } else {
            flush_word(&mut word, &mut out);
            flush_han(&mut han, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_han(&mut han, &mut out);
    out
}

impl ConversationMemory for MirrorMemory {
    fn recall(&self, query: &str, limit: usize, conv: Option<&str>) -> Vec<MemoryHit> {
        let tokens = query_tokens(query);
        // 防自我回灌（2026-08-26，与 CLI cli_chat_memory.c 同语义）：运行侧
        // 把当前输入记为 user 记录，query 又是这条输入本身，命中率最高。把该
        // 记录注入上下文会形成"模型读到自己刚收到的输入的记忆"的回声，污染
        // 当前问题的理解。跳过与 query 相同/带「用户: 」前缀的记录。
        let q = query.trim();
        let now = chrono::Utc::now().timestamp();
        let guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        let mut hits: Vec<MemoryHit> = guard
            .iter()
            .filter(|r| r.role != "system") // 系统提示不召回
            // D-14 会话隔离：给定 conv 时只召回同一会话的记录，避免历史
            // 会话的记忆被当作本轮背景注入（跨会话串台）。
            .filter(|r| match conv {
                Some(c) => r.conv.as_deref() == Some(c),
                None => true,
            })
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
                // 逐记录只做一次归一化，避免每个 token 重复分配小写副本。
                let content = r.content.to_lowercase();
                let tags = r.tags.to_lowercase();
                let reasoning = r.reasoning.as_ref().map(|s| s.to_lowercase());
                let mut score = 0.0f32;
                for t in &tokens {
                    if content.contains(t.as_str()) {
                        score += 1.0;
                    }
                    if tags.contains(t.as_str()) {
                        score += 0.5;
                    }
                    /* 缺口 #8 修复：思考链（reasoning）参与召回打分——此前
                     * 只匹配 content/tags，思考链"只存档不可用"。权重低于
                     * content（思考链是内部推导，非直接事实表述）。 */
                    if reasoning.as_ref().is_some_and(|rz| rz.contains(t.as_str())) {
                        score += 0.3;
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

    fn ingest_recent(&self, result: &serde_json::Value) {
        if self.backend == "volatile" {
            return;
        }
        apply_recent(&self.mirror, &self.ready, result);
    }

    /// P1-3：按会话段水合。刻意不置就绪闸门——一轮水合由多条 query 串成，
    /// App 在队列排空后再以 `ingest_recent(Null)` 收口置就绪，避免历史还原
    /// 在只水合了部分会话时提前触发、把其余 tab 判成「无历史」。
    fn ingest_recent_scope(&self, conv: Option<&str>, result: &serde_json::Value) {
        if self.backend == "volatile" {
            return;
        }
        apply_recent_scope(&self.mirror, conv, result);
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    fn backend_name(&self) -> &'static str {
        self.backend
    }

    #[cfg(test)]
    fn seed(&mut self, role: &str, content: &str, tags: &str, conv: Option<&str>) {
        let rec = MemoryRecord {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            tags: tags.to_string(),
            reasoning: None,
            conv: conv.map(str::to_string),
        };
        let mut guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(rec);
        if guard.len() > MAX_RECORDS {
            let drain = guard.len() - MAX_RECORDS;
            guard.drain(..drain);
        }
    }
}

/// 把 `mem.recent` 的查询结果落地到镜像（按 created_at 升序重建，旧→新）。
///
/// 同步、无 IO：查询由 App 经事件管道下发，本函数只解析。失败（无
/// records / 离线空结果）时保持镜像现状，并一律置就绪——历史还原闸门
/// 不能因网关不可用而永挂。
fn apply_recent(mirror: &Mutex<Vec<MemoryRecord>>, ready: &AtomicBool, result: &serde_json::Value) {
    ready.store(true, Ordering::Relaxed);
    let Some(list) = parse_recent(result) else {
        return;
    };
    let mut guard = mirror.lock().unwrap_or_else(|e| e.into_inner());
    let n = list.len();
    *guard = list;
    log::info!("memory: 自网关水合 {} 条记忆（mem.recent）", n);
}

/// P1-3：分会话水合——`conv` 为 Some 时只替换该会话的记忆段，其余会话
/// 保持原样；`conv` 为 None 是全局窗口，只按记录身份去重追加（补齐无会话
/// 归属的记录），不冲掉已按会话水合的段。
fn apply_recent_scope(
    mirror: &Mutex<Vec<MemoryRecord>>,
    conv: Option<&str>,
    result: &serde_json::Value,
) {
    let Some(list) = parse_recent(result) else {
        return;
    };
    let n = list.len();
    let mut guard = mirror.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = conv {
        guard.retain(|r| r.conv.as_deref() != Some(c));
    }
    for r in list {
        if guard.iter().any(|e| same_record(e, &r)) {
            continue;
        }
        guard.push(r);
    }
    guard.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    if guard.len() > MAX_RECORDS {
        let drain = guard.len() - MAX_RECORDS;
        guard.drain(..drain);
    }
    log::info!("memory: 按会话水合 {} 条记忆（conv={:?}）", n, conv);
}

/// 记录身份：同一后端记录会出现在多条水合窗口里，据此去重。
fn same_record(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    a.timestamp == b.timestamp && a.role == b.role && a.content == b.content && a.conv == b.conv
}

/// 解析 `mem.recent` 结果（按 created 升序，超窗截断）。无 records 字段
/// 返回 None（保持镜像现状）。
fn parse_recent(result: &serde_json::Value) -> Option<Vec<MemoryRecord>> {
    let items = result.get("records").and_then(|v| v.as_array())?;
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
                    conv: md_str(&md, "conv"),
                },
            )),
            None => {
                let conv = md_str(&md, "conv");
                for r in split_cli_turn(created, data, &tags, conv.as_deref()) {
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
    Some(list)
}

/// 还原 CLI 写入的组合轮记录为 user/assistant 两条。
///
/// CLI（`cli_chat_memory.c:cli_chat_mem_record_gw`）每轮以**单条** `mem.write`
/// 记录整轮：`data = "用户: <input>\nAgentRT: <reply>[\n[reasoning] <chain>]"`
/// 且 metadata 无 `role`。TUI 是共享记忆（同一 mem_d）的渲染层，若整轮按单条
/// `user` 呈现则角色错乱——故按该格式拆分还原；非该格式（旧 TUI 记录、
/// 或其它写入方）原样保留为单条。
fn split_cli_turn(created: i64, data: &str, tags: &str, conv: Option<&str>) -> Vec<MemoryRecord> {
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
        // 会话归属由写入侧 metadata.conv 提供（0.1.21 S-P1-16）；旧 CLI
        // 记录无此字段，保持 None 不参与按会话还原。
        conv: conv.map(str::to_string),
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

/// 构造记忆模块：TUI 唯一后端为网关记忆服务（`mem.*`，与 CLI 同一存储）。
/// 读取由 App 经事件管道水合（query 帧），本构造函数不做 IO。
pub fn build_memory() -> Box<dyn ConversationMemory> {
    Box::new(MirrorMemory::new())
}

#[cfg(test)]
mod tests;
