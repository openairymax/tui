// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话记忆模块。
//
// 设计原则（50 工程标准 E-3 资源确定性 / A-1 极简主义；T-09 铁律，
// 0.1.15 裁决 2026-09-10，方案 §4.7）：
//   - 唯一后端 JsonlMemory：TUI 本地会话记忆缓存，以 JSONL 追加写方式
//     持久化对话记忆，跨会话"记得住"。每条记录含角色、内容、时间戳与
//     标签，支持按关键词与时效召回相关记忆，注入后续请求上下文。仅用
//     标准库文件 I/O，不触达任何 daemon / 运行时库。
//   - 平台级长期记忆（分层、遗忘衰减、语义检索等）一律经 gateway
//     mem.* RPC 由服务端提供（通道在位：见 app/task.rs 的 mem.count /
//     mem.search 调用）。历史上的 memoryrovol C FFI 直连已于 T-09 收回
//     ——客户端进程内链接运行时库复制平台记忆，违反"一切客户端功能
//     走 gateway"铁律。

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 单条记忆记录（JSONL 行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub role: String,       // user / assistant / system
    pub content: String,
    pub timestamp: String,  // ISO8601
    pub tags: String,       // 逗号分隔，如 "task,chat,preference"
    /// 2.1.1.6：思考链（reasoning_content）随助手回复持久化保留。
    /// 旧记录无此字段（serde default 容忍），升级后历史记忆不丢。
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// 召回结果（内容 + 相关度得分）
#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub content: String,
    pub role: String,
    pub score: f32,
}

/// 对话记忆后端 trait（E-4 跨平台一致性：Linux/macOS/Windows 路径统一）
pub trait ConversationMemory: Send + Sync {
    /// 写入一条记忆
    fn push(&mut self, role: &str, content: &str, tags: &str) -> std::io::Result<()>;
    /// 2.1.1.6：写入一条带思考链（reasoning_content）的助手记忆。
    /// 默认实现忽略 reasoning；JSONL 后端重写为持久化 reasoning。
    fn push_with_reasoning(
        &mut self,
        role: &str,
        content: &str,
        reasoning: Option<&str>,
        tags: &str,
    ) -> std::io::Result<()> {
        let _ = reasoning;
        self.push(role, content, tags)
    }
    /// 召回与 query 相关、且 time_before 之前的记忆
    fn recall(&self, query: &str, limit: usize) -> Vec<MemoryHit>;
    /// 最近 N 条对话（按时间倒序）
    fn recent(&self, n: usize) -> Vec<MemoryRecord>;
    /// 记忆条数
    fn len(&self) -> usize;
    /// 2.2.2.1：当前记忆后端名（TUI 展示用，默认 "Jsonl"）。
    /// 持久化失败降级 volatile 内存时覆盖为 "volatile"。
    fn backend_name(&self) -> &'static str {
        "Jsonl"
    }
}

/// JSONL 持久化记忆后端（默认）。
///
/// 存储路径：`$AIRY_HOME/tui/memory.jsonl`（默认 `~/.airymaxrt/tui/`）。
/// 追加写保证崩溃安全（单条记录完整落盘）；召回用词频 + 时效加权，
/// 无需外部依赖，满足"非任务集对话记得住"的基线能力。
pub struct JsonlMemory {
    path: PathBuf,
    records: Vec<MemoryRecord>,
    max_records: usize,
}

impl JsonlMemory {
    /// 创建记忆后端。dir 为记忆目录（未指定时用 $AIRY_HOME/tui 或 ~/.airymaxrt/tui）。
    pub fn new(dir: Option<&Path>) -> std::io::Result<Self> {
        let dir = match dir {
            Some(d) => d.to_path_buf(),
            None => memory_dir(),
        };
        fs::create_dir_all(&dir)?;
        let path = dir.join("memory.jsonl");
        let mut mem = Self {
            path,
            records: Vec::new(),
            max_records: 2000,
        };
        mem.load()?;
        Ok(mem)
    }

    fn load(&mut self) -> std::io::Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let f = fs::File::open(&self.path)?;
        let reader = BufReader::new(f);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(rec) = serde_json::from_str::<MemoryRecord>(&line) {
                self.records.push(rec);
            }
        }
        Ok(())
    }
}

impl ConversationMemory for JsonlMemory {
    fn push(&mut self, role: &str, content: &str, tags: &str) -> std::io::Result<()> {
        self.push_impl(role, content, None, tags)
    }

    fn push_with_reasoning(
        &mut self,
        role: &str,
        content: &str,
        reasoning: Option<&str>,
        tags: &str,
    ) -> std::io::Result<()> {
        self.push_impl(role, content, reasoning, tags)
    }

    fn recall(&self, query: &str, limit: usize) -> Vec<MemoryHit> {
        let tokens: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .filter(|t| t.len() >= 2)
            .collect();
        // 防自我回灌（2026-08-26，与 CLI cli_chat.c 同语义）：当前输入在
        // 发起对话前刚被 push 为 user 记录，query 又是这条输入本身，直接
        // 命中率最高。把该记录注入上下文会形成"模型读到自己刚收到的输入
        // 的记忆"的回声，污染当前问题的理解。跳过与 query 相同/带「用户: 」
        // 前缀的记录。
        let q = query.trim();
        let now = chrono::Utc::now().timestamp();
        let mut hits: Vec<MemoryHit> = self
            .records
            .iter()
            .filter(|r| r.role != "system") // 系统提示不召回
            .filter(|r| {
                let c = r.content.trim();
                if c.eq_ignore_ascii_case(q) {
                    return false;
                }
                // 「用户: <input>」前缀形式（旧格式记忆）
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
                Some(MemoryHit {
                    content: r.content.clone(),
                    role: r.role.clone(),
                    score: score * decay,
                })
            })
            .collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        hits
    }

    fn recent(&self, n: usize) -> Vec<MemoryRecord> {
        self.records
            .iter()
            .rev()
            .take(n)
            .cloned()
            .collect()
    }

    fn len(&self) -> usize {
        self.records.len()
    }
}

impl JsonlMemory {
    /// 2.1.1.6：带思考链的追加写实现（push / push_with_reasoning 共用）。
    fn push_impl(
        &mut self,
        role: &str,
        content: &str,
        reasoning: Option<&str>,
        tags: &str,
    ) -> std::io::Result<()> {
        let rec = MemoryRecord {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            tags: tags.to_string(),
            reasoning: reasoning.map(|s| s.to_string()),
        };
        // 追加写：单条记录一次 write + flush，崩溃时最多丢当前记录
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{}", serde_json::to_string(&rec)?)?;
        f.flush()?;
        self.records.push(rec);
        if self.records.len() > self.max_records {
            // 裁剪最旧记录并重写文件（保持文件与内存一致）
            let drain = self.records.len() - self.max_records;
            self.records.drain(..drain);
            self.rewrite()?;
        }
        Ok(())
    }

    fn rewrite(&self) -> std::io::Result<()> {
        let mut f = OpenOptions::new().create(true).truncate(true).write(true).open(&self.path)?;
        for rec in &self.records {
            writeln!(f, "{}", serde_json::to_string(rec)?)?;
        }
        f.flush()?;
        Ok(())
    }
}

/// 解析 JSONL 时间戳（YYYY-MM-DDTHH:MM:SS）为 epoch 秒；失败返回 0
fn parse_ts(ts: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S")
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}

/// 记忆目录：$AIRY_HOME/data/agentrt/tui（AIRY_HOME 路径体系收敛，2026-08-19）
fn memory_dir() -> PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui"])
}

/// 构造记忆模块：T-09 后唯一后端为 JsonlMemory（TUI 本地会话缓存，
/// 纯标准库文件 I/O）；平台级长期记忆一律走 gateway mem.* RPC。
pub fn build_memory(dir: Option<&Path>) -> Box<dyn ConversationMemory> {
    match JsonlMemory::new(dir) {
        Ok(m) => Box::new(m),
        Err(e) => {
            log::warn!("memory: JsonlMemory init failed ({}), using volatile memory", e);
            Box::new(JsonlMemory {
                path: PathBuf::from("/dev/null"),
                records: Vec::new(),
                max_records: 2000,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_memory() -> (tempfile::TempDir, JsonlMemory) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mem = JsonlMemory::new(Some(dir.path())).expect("jsonl memory");
        (dir, mem)
    }

    #[test]
    fn push_persists_across_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut m = JsonlMemory::new(Some(dir.path())).expect("new");
            m.push("user", "我的名字是小明", "chat").expect("push");
            m.push("assistant", "你好，小明！", "chat").expect("push");
        }
        // 重新加载：跨会话"记得住"
        let m = JsonlMemory::new(Some(dir.path())).expect("reload");
        assert_eq!(m.len(), 2);
        let recent = m.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].role, "assistant");
    }

    #[test]
    fn push_with_reasoning_persists_reasoning() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut m = JsonlMemory::new(Some(dir.path())).expect("new");
            m.push_with_reasoning("assistant", "最终回答", Some("思考过程"), "chat")
                .expect("push");
        }
        // 重新加载：思考链随记录落盘保留（2.1.1.6）
        let m = JsonlMemory::new(Some(dir.path())).expect("reload");
        let rec = m.recent(1).into_iter().next().expect("record");
        assert_eq!(rec.reasoning.as_deref(), Some("思考过程"));
        // 旧格式记录（无 reasoning 字段）反序列化兼容
        let raw = "{\"role\":\"assistant\",\"content\":\"旧记录\",\"timestamp\":\"2026-01-01T00:00:00\",\"tags\":\"chat\"}";
        let old: MemoryRecord = serde_json::from_str(raw).expect("old record parse");
        assert_eq!(old.reasoning, None);
    }

    #[test]
    fn recall_finds_related_by_keyword() {
        let (_d, mut m) = temp_memory();
        m.push("user", "我在做性能监控优化", "chat").expect("push");
        m.push("user", "今天天气不错", "chat").expect("push");
        let hits = m.recall("性能监控", 3);
        assert!(!hits.is_empty());
        assert!(hits[0].content.contains("性能监控"));
    }

    #[test]
    fn recall_ignores_system_role() {
        let (_d, mut m) = temp_memory();
        m.push("system", "请判断模式", "meta").expect("push");
        m.push("user", "帮我写代码", "chat").expect("push");
        let hits = m.recall("判断模式", 3);
        assert!(hits.iter().all(|h| h.role != "system"));
    }

    #[test]
    fn recall_excludes_self_feed() {
        // 防自我回灌（2026-08-26）：当前输入刚 push 后 recall 同 query，
        // 不得把该输入自身作为"相关记忆"回灌（与 CLI cli_chat.c 同语义）。
        let (_d, mut m) = temp_memory();
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
    fn parse_mode_detail_parses_markers() {
        assert_eq!(
            crate::app::parse_mode_detail("[MODE:TASK] 执行任务"),
            (crate::app::ModeMarker::Task, "执行任务".into())
        );
        assert_eq!(
            crate::app::parse_mode_detail("[MODE:CHAT]\n闲聊"),
            (crate::app::ModeMarker::Chat, "闲聊".into())
        );
        assert_eq!(
            crate::app::parse_mode_detail("普通回复"),
            (crate::app::ModeMarker::Chat, "普通回复".into())
        );
    }

    #[test]
    fn parse_mode_detail_tolerates_leading_text() {
        // 容错（2026-08-26）：LLM 输出带简短前导（「好的，」等）时仍能识别
        // 模式标记；正文提及 [MODE:CHAT]（超 64 字符窗口）不误判。
        assert_eq!(
            crate::app::parse_mode_detail("好的，[MODE:TASK]\n开始执行任务"),
            (crate::app::ModeMarker::Task, "开始执行任务".into())
        );
        assert_eq!(
            crate::app::parse_mode_detail("好的 [MODE:CHAT] 我们聊聊"),
            (crate::app::ModeMarker::Chat, "我们聊聊".into())
        );
        // 正文中（64 字符后）出现的标记样式文字不应触发模式切换
        let body = format!("这是一段很长的普通对话内容，{}", "x".repeat(80));
        assert_eq!(
            crate::app::parse_mode_detail(&body),
            (crate::app::ModeMarker::Chat, body)
        );
    }
}
