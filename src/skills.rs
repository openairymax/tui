// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 可复用技能库（程序性记忆）。
//
// 设计原则（50 工程标准 E-3 资源确定性 / A-1 极简主义；0.1.16 架构改造，
// 用户决策 2026-09-13「CLI 是核心，TUI 是可选的增强」）：
//   - 技能即"程序性记忆"：TUI 不持有任何独立技能存储。唯一权威后端是网关
//     记忆服务 mem_d，经 `mem.*` RPC 写入/读取——与 CLI 同一后端、同一存储、
//     同一 schema，二者共享同一记忆库。TUI 只是渲染层。
//   - 技能记录以 `metadata.kind="skill"` 作为分区标识：与对话记忆（kind 缺省）
//     同库同流但语义隔离——记忆面板按 kind 过滤只渲染对话轮次，技能库面板按
//     kind 过滤只渲染技能（见 memory.rs / 本模块 hydrate）。由此实现"TUI 与
//     CLI 使用一套记忆"，且零新增 gateway 服务面。
//   - 进程内 mirror 仅作渲染/召回的**同步读缓存**：启动时经 `mem.recent` 水合，
//     写入经 `mem.write` 异步回投网关。网关不可达时降级为纯内存（volatile），
//     不落任何本地文件。
//
// 历史：0.1.15 及以前 TUI 使用独立本地 JSONL 库
// （$AIRY_HOME/data/agentrt/tui/skills.jsonl），与网关记忆库互不相通——这正是
// "CLI 核心 / TUI 增强"决策下必须消除的存储分叉。本模块随 0.1.16 架构改造收敛
// 为网关单后端（与 memory.rs 同构，依据 0.1.9 §7 W4「记忆面板统一为 gateway」）。

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::client::GatewayClient;
use crate::memory::{epoch_to_iso, md_str, parse_metadata};

/// 技能镜像保留上限（渲染/召回窗口；网关侧容量由 mem_d 自行管理）。
const MAX_SKILLS: usize = 500;

/// 启动/面板打开时的水合条数（mem.recent 上限：mem_handlers clamp 0..1000）。
const HYDRATE_LIMIT: usize = 1000;

/// 记忆分区标识：技能记录（区别于 kind 缺省的对话记忆）。
pub const KIND_SKILL: &str = "skill";

/// 一条可复用技能（任务经验沉淀的产物）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    /// 技能名（snake_case，如 debug_http_timeout）
    pub name: String,
    /// 类别（development/security/ops/text-processing/...）
    pub category: String,
    /// 触发关键词（逗号分隔，用于检索召回）
    pub trigger: String,
    /// 一句话摘要
    pub summary: String,
    /// 可复用执行步骤（含要点与顺序）
    pub procedure: String,
    /// 经验与教训
    pub lessons: String,
    /// 标签（逗号分隔）
    pub tags: String,
    /// 沉淀时间（ISO8601）
    pub created_at: String,
    /// 复用成功次数
    pub success_count: u32,
}

impl SkillRecord {
    /// 构造一条新技能
    #[allow(dead_code)] // 单测与后续 API 使用
    pub fn new(
        name: &str,
        category: &str,
        trigger: &str,
        summary: &str,
        procedure: &str,
        lessons: &str,
        tags: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            category: category.to_string(),
            trigger: trigger.to_string(),
            summary: summary.to_string(),
            procedure: procedure.to_string(),
            lessons: lessons.to_string(),
            tags: tags.to_string(),
            created_at: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            success_count: 0,
        }
    }

    /// 序列化为 `mem.write` 的 metadata（kind=skill + 全部索引字段）。
    ///
    /// 技能正文（procedure）作为 `data` 单独承载；其余字段进 metadata，使
    /// 记录在 mem_d 中自描述、可被任何 `mem.*` 消费端（CLI 等）无损还原。
    fn to_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "source": "tui",
            "kind": KIND_SKILL,
            "name": self.name.as_str(),
            "category": self.category.as_str(),
            "trigger": self.trigger.as_str(),
            "summary": self.summary.as_str(),
            "lessons": self.lessons.as_str(),
            "tags": self.tags.as_str(),
            "success_count": self.success_count,
            "created_at": self.created_at.as_str(),
        })
    }

    /// 从 mem.recent 记录（data + metadata）还原技能。
    ///
    /// 非技能记录（metadata.kind≠"skill"）或缺 name 时返回 None。
    fn from_mem_record(created_epoch: i64, data: &str, md: &serde_json::Value) -> Option<Self> {
        if md_str(md, "kind").as_deref() != Some(KIND_SKILL) {
            return None;
        }
        let name = md_str(md, "name")?;
        if name.trim().is_empty() {
            return None;
        }
        Some(Self {
            name,
            category: md_str(md, "category").unwrap_or_else(|| "general".to_string()),
            trigger: md_str(md, "trigger").unwrap_or_default(),
            summary: md_str(md, "summary").unwrap_or_default(),
            procedure: data.to_string(),
            lessons: md_str(md, "lessons").unwrap_or_default(),
            tags: md_str(md, "tags").unwrap_or_default(),
            created_at: md_str(md, "created_at").unwrap_or_else(|| epoch_to_iso(created_epoch)),
            success_count: md
                .get("success_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
        })
    }
}

/// 技能库后端 trait（E-4 跨平台一致性：Linux/macOS/Windows 路径统一）
pub trait SkillStore: Send + Sync {
    /// 保存一条技能（同名技能视为再次沉淀，累加复用次数）
    fn save(&mut self, skill: SkillRecord) -> std::io::Result<()>;
    /// 按触发关键词/摘要召回相关技能（按相关度倒序）
    fn find(&self, query: &str, limit: usize) -> Vec<SkillRecord>;
    /// 列出全部技能（按沉淀时间倒序）
    #[allow(dead_code)] // 单测与后续面板展示使用
    fn list(&self) -> Vec<SkillRecord>;
    /// 技能条数
    fn len(&self) -> usize;
    /// 从权威后端重新水合镜像（网关可用时）。默认后端为纯内存，no-op。
    fn refresh(&self) {}
    /// 当前技能后端名（TUI 展示用）。
    fn backend_name(&self) -> &'static str {
        "gateway"
    }
}

/// 网关技能后端（TUI 唯一后端）。
///
/// 权威存储为网关记忆服务 mem_d（`mem.*` RPC，与 CLI 同一后端，技能以
/// `metadata.kind="skill"` 与对话记忆分区共存）；本结构持有的 mirror 是渲染/
/// 召回的同步读缓存：启动水合 + 写入回投 + 面板打开时再水合。无 tokio 运行时
/// （单测）时降级为纯内存 volatile。
pub struct GatewaySkillStore {
    /// 网关客户端（refresh 用水合；None = volatile 降级）
    client: Option<GatewayClient>,
    /// 同步读缓存（旧→新有序，list 时反向取）
    mirror: Arc<Mutex<Vec<SkillRecord>>>,
    /// 写投递通道：save 立即入队，后台任务串行投递 `mem.write`
    tx: Option<tokio::sync::mpsc::UnboundedSender<SkillRecord>>,
    /// 后端名（"gateway" / "volatile"），面板展示用
    backend: &'static str,
}

impl GatewaySkillStore {
    /// 构造网关技能后端。
    ///
    /// 需处于 tokio 运行时内（`run_tui` 的 App::new）；同步上下文（单测）
    /// 无运行时，自动降级为纯内存镜像（volatile，不触网、不落盘）。
    pub fn new(client: GatewayClient) -> Self {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                log::warn!("skills: 无 tokio 运行时，技能库降级为纯内存镜像（volatile）");
                return Self::volatile();
            }
        };
        let mirror: Arc<Mutex<Vec<SkillRecord>>> = Arc::new(Mutex::new(Vec::new()));
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
}

impl SkillStore for GatewaySkillStore {
    fn save(&mut self, skill: SkillRecord) -> std::io::Result<()> {
        // 同名技能视为再次沉淀：在镜像内合并并累加复用次数。mem_d 只提供插入
        // 语义，故采用 append-only：新记录携带**累计后**的 success_count，水合
        // 时按 name 去重取最新记录即为权威计数（见 hydrate）。
        let merged = {
            let mut guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
            match guard.iter().position(|s| s.name == skill.name) {
                Some(pos) => {
                    let mut m = guard[pos].clone();
                    m.success_count = m.success_count.saturating_add(1);
                    if !skill.procedure.trim().is_empty() {
                        m.procedure = skill.procedure.clone();
                    }
                    if !skill.lessons.trim().is_empty() {
                        m.lessons = skill.lessons.clone();
                    }
                    m.trigger = skill.trigger.clone();
                    m.summary = skill.summary.clone();
                    m.tags = skill.tags.clone();
                    guard[pos] = m.clone();
                    m
                }
                None => {
                    guard.push(skill.clone());
                    if guard.len() > MAX_SKILLS {
                        let drain = guard.len() - MAX_SKILLS;
                        guard.drain(..drain);
                    }
                    skill
                }
            }
        };
        // 权威存储在网关：入队后台投递 `mem.write`。投递失败（运行时已退出）
        // 仅记日志——镜像已乐观更新，用户交互不因此中断。
        if let Some(tx) = &self.tx {
            if tx.send(merged).is_err() {
                log::warn!("skills: 网关写投递通道已关闭，技能仅在镜像");
            }
        }
        Ok(())
    }

    fn find(&self, query: &str, limit: usize) -> Vec<SkillRecord> {
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_lowercase())
            .collect();
        let guard = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        let mut scored: Vec<(SkillRecord, f32)> = guard
            .iter()
            .map(|s| {
                let mut score = 0.0f32;
                for t in &tokens {
                    if s.trigger.to_lowercase().contains(t) {
                        score += 3.0;
                    }
                    if s.summary.to_lowercase().contains(t) {
                        score += 2.0;
                    }
                    if s.name.to_lowercase().contains(t) {
                        score += 2.0;
                    }
                    if s.procedure.to_lowercase().contains(t) {
                        score += 1.0;
                    }
                }
                // 复用次数加权：常用技能优先
                let weighted = score * (1.0 + (s.success_count.min(9) as f32) * 0.1);
                (s.clone(), weighted)
            })
            .collect();
        scored.retain(|(_, s)| *s > 0.0);
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored.into_iter().map(|(s, _)| s).collect()
    }

    fn list(&self) -> Vec<SkillRecord> {
        self.mirror
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .cloned()
            .collect()
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

/// 后台写投递：把技能记录逐条经 `mem.write` 持久化到网关记忆服务。
///
/// 记录 schema（与 CLI 共享同一存储的唯一约定）：
///   data     = procedure（可复用执行步骤，技能正文）
///   metadata = { source:"tui", kind:"skill", name, category, trigger,
///                summary, lessons, tags, success_count, created_at }
async fn writer(
    client: GatewayClient,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SkillRecord>,
) {
    while let Some(rec) = rx.recv().await {
        // 技能正文以 procedure 为准；其后备为 summary，保证 data 非空。
        let data = if rec.procedure.trim().is_empty() {
            rec.summary.clone()
        } else {
            rec.procedure.clone()
        };
        let params = serde_json::json!({
            "data": data,
            "metadata": rec.to_metadata(),
        });
        if let Err(e) = client.rpc_call("mem.write", params).await {
            log::warn!("skills: mem.write 失败（技能仅在镜像，待下次水合）: {}", e);
        }
    }
}

/// 从网关记忆服务水合技能镜像：`mem.recent` 取最近 HYDRATE_LIMIT 条，仅保留
/// `metadata.kind=="skill"` 的记录，按 name 去重取最新（append-only 的累计次数
/// 由最新记录承载），再按沉淀时间升序重建镜像（旧→新）。失败时保持镜像现状。
async fn hydrate(client: GatewayClient, mirror: Arc<Mutex<Vec<SkillRecord>>>) {
    let params = serde_json::json!({ "limit": HYDRATE_LIMIT });
    let result = match client.rpc_call("mem.recent", params).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("skills: mem.recent 水合失败（离线，镜像保持现状）: {}", e);
            return;
        }
    };
    let Some(items) = result.get("records").and_then(|v| v.as_array()) else {
        return;
    };
    let mut recs: Vec<(i64, SkillRecord)> = Vec::new();
    for it in items {
        let created = it.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let data = it.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let md = parse_metadata(it.get("metadata"));
        if let Some(rec) = SkillRecord::from_mem_record(created, data, &md) {
            recs.push((created, rec));
        }
    }
    // 升序后按 name 去重：后者覆盖前者 = 最新记录胜出（携带累计计数）。
    recs.sort_by_key(|(t, _)| *t);
    let mut dedup: Vec<(i64, SkillRecord)> = Vec::new();
    for (t, rec) in recs {
        if let Some(slot) = dedup.iter_mut().find(|(_, r)| r.name == rec.name) {
            *slot = (t, rec);
        } else {
            dedup.push((t, rec));
        }
    }
    dedup.sort_by_key(|(t, _)| *t);
    let mut list: Vec<SkillRecord> = dedup.into_iter().map(|(_, r)| r).collect();
    if list.len() > MAX_SKILLS {
        let drain = list.len() - MAX_SKILLS;
        list.drain(..drain);
    }
    let n = list.len();
    let mut guard = mirror.lock().unwrap_or_else(|e| e.into_inner());
    *guard = list;
    log::info!("skills: 自网关水合 {} 条技能（mem.recent, kind=skill）", n);
}

/// 构造技能库后端：TUI 唯一后端为网关记忆服务（`mem.*` RPC，与 CLI 同一
/// 存储）；无 tokio 运行时时降级为纯内存镜像（不落盘）。
pub fn build_skill_store(gateway: &GatewayClient) -> Box<dyn SkillStore> {
    Box::new(GatewaySkillStore::new(gateway.clone()))
}

/// 构建经验提炼提示词。
///
/// 任务成功后，将最近一段对话（目标 + 步骤 + 结果）交给 LLM，
/// 要求其输出一条结构化技能（JSON），格式与 `SkillRecord` 对齐。
pub fn build_distill_prompt(conversation: &str) -> String {
    format!(
        "你是技能提炼器。请根据以下任务执行过程，提炼一条可复用的技能，\
         避免后续任务'用过即忘'。\n\
         只输出一个 JSON 对象，字段严格为：\n\
         {{\"name\":\"snake_case技能名\",\"category\":\"分类\",\
         \"trigger\":\"触发关键词，逗号分隔\",\"summary\":\"一句话摘要\",\
         \"procedure\":\"可复用执行步骤，含要点与顺序\",\"lessons\":\"经验与教训\",\
         \"tags\":\"标签，逗号分隔\"}}\n\
         不要输出 JSON 以外的任何内容。\n\n\
         【任务执行过程】\n{}\n",
        conversation
    )
}

/// 解析 LLM 提炼结果（JSON）为 SkillRecord。
///
/// 容错处理：剥离可能的 Markdown 代码围栏与前后空白，再尝试解析；
/// 解析失败或缺少必填字段（name/trigger/procedure）时返回 None。
pub fn parse_distilled_skill(raw: &str) -> Option<SkillRecord> {
    let t = raw.trim();
    let json = if t.starts_with("```") {
        let start = t.find('\n').unwrap_or(0) + 1;
        let end = t.rfind("```").unwrap_or(t.len());
        t[start..end].trim()
    } else {
        // 可能包含 "JSON:" 前缀等，取第一个 '{' 到最后一个 '}'
        let s = t.find('{')?;
        let e = t.rfind('}')?;
        if e > s {
            &t[s..=e]
        } else {
            t
        }
    };

    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = v.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let trigger = v.get("trigger").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if trigger.is_empty() {
        return None;
    }
    let procedure = v.get("procedure").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if procedure.is_empty() {
        return None;
    }

    Some(SkillRecord {
        name,
        category: v.get("category").and_then(|x| x.as_str()).unwrap_or("general").trim().to_string(),
        trigger,
        summary: v.get("summary").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
        procedure,
        lessons: v.get("lessons").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
        tags: v.get("tags").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
        created_at: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        success_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> GatewaySkillStore {
        GatewaySkillStore::volatile()
    }

    #[test]
    fn save_and_find_roundtrip() {
        let mut store = store();
        let skill = SkillRecord::new(
            "debug_http_timeout",
            "development",
            "http,timeout,curl,connection",
            "调试 HTTP 连接超时",
            "1. curl -v 定位阶段; 2. 检查 DNS 与代理; 3. 加长超时重试",
            "超时多为代理而非目标服务器",
            "debug,http",
        );
        store.save(skill).unwrap();
        assert_eq!(store.len(), 1);

        let hits = store.find("http timeout 超时", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "debug_http_timeout");
    }

    #[test]
    fn same_name_merges_and_counts() {
        let mut store = store();
        let s1 = SkillRecord::new("s", "dev", "a,b", "sum", "step1", "lesson", "t");
        let s2 = SkillRecord::new("s", "dev", "a,b", "sum2", "step2", "lesson2", "t");
        store.save(s1).unwrap();
        store.save(s2).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.list()[0].success_count, 1);
        // 最新 procedure 生效（非空覆盖）
        assert_eq!(store.list()[0].procedure, "step2");
    }

    #[test]
    fn find_empty_query_returns_nothing() {
        let mut store = store();
        store
            .save(SkillRecord::new("x", "dev", "k1", "d", "p", "l", "t"))
            .unwrap();
        assert!(store.find("", 5).is_empty());
    }

    #[test]
    fn volatile_backend_name() {
        assert_eq!(store().backend_name(), "volatile");
    }

    #[test]
    fn metadata_roundtrip_preserves_fields() {
        // 技能经 mem.write metadata + data(procedure) 往返后可无损还原
        // （这是与 CLI 共享同一 mem_d 存储的 schema 契约）。
        let s = SkillRecord::new(
            "fix_dns",
            "ops",
            "dns,timeout",
            "DNS 超时处理",
            "1. 检查 resolv.conf",
            "优先系统 DNS",
            "dns",
        );
        let md = s.to_metadata();
        assert_eq!(md_str(&md, "kind").as_deref(), Some(KIND_SKILL));
        let back = SkillRecord::from_mem_record(1_700_000_000, &s.procedure, &md).expect("restore");
        assert_eq!(back.name, "fix_dns");
        assert_eq!(back.category, "ops");
        assert_eq!(back.trigger, "dns,timeout");
        assert_eq!(back.summary, "DNS 超时处理");
        assert_eq!(back.procedure, "1. 检查 resolv.conf");
        assert_eq!(back.lessons, "优先系统 DNS");
        assert_eq!(back.tags, "dns");
        assert_eq!(back.success_count, 0);
        assert_eq!(back.created_at, s.created_at);
    }

    #[test]
    fn from_mem_record_rejects_non_skill() {
        // 对话记忆（无 kind / kind=chat）不得被技能库消费
        let chat = serde_json::json!({ "source": "tui", "role": "user", "tags": "chat" });
        assert!(SkillRecord::from_mem_record(1, "hello", &chat).is_none());
        let chat_kind = serde_json::json!({ "kind": "chat", "name": "x" });
        assert!(SkillRecord::from_mem_record(1, "hello", &chat_kind).is_none());
        // kind=skill 但缺 name → 丢弃
        let no_name = serde_json::json!({ "kind": KIND_SKILL });
        assert!(SkillRecord::from_mem_record(1, "hello", &no_name).is_none());
    }

    #[test]
    fn parse_distilled_skill_plain_json() {
        let raw = r#"{"name":"fix_dns_timeout","category":"ops","trigger":"dns,timeout","summary":"DNS 解析超时处理","procedure":"1. 检查 resolv.conf; 2. 改用系统 DNS","lessons":"优先系统 DNS","tags":"dns"}"#;
        let s = parse_distilled_skill(raw).expect("parse");
        assert_eq!(s.name, "fix_dns_timeout");
        assert_eq!(s.category, "ops");
        assert_eq!(s.success_count, 0);
    }

    #[test]
    fn parse_distilled_skill_with_fence() {
        let raw = "```json\n{\"name\":\"a_b\",\"trigger\":\"t1,t2\",\"procedure\":\"p\"}\n```";
        let s = parse_distilled_skill(raw).expect("parse");
        assert_eq!(s.name, "a_b");
        assert_eq!(s.procedure, "p");
    }

    #[test]
    fn parse_distilled_skill_invalid_returns_none() {
        assert!(parse_distilled_skill("不认识的回复").is_none());
        assert!(parse_distilled_skill(r#"{"name":"x"}"#).is_none());
    }

    #[test]
    fn build_distill_prompt_contains_json_fields() {
        let p = build_distill_prompt("用户: 帮我修超时\n助手: 已修复");
        assert!(p.contains("name"));
        assert!(p.contains("procedure"));
        assert!(p.contains("任务执行过程"));
        assert!(p.contains("用户: 帮我修超时"));
    }
}
