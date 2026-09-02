// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Skills 本地技能库。
//
// 设计原则（50 工程标准 E-3 资源确定性 / A-1 极简主义）：
//   - 任务成功后自动提炼经验并沉淀为可复用技能，避免"用过即忘"：
//     任务集完成时调用 LLM 将本次执行过程提炼为 SkillRecord，追加写入本地库，
//     后续任务在 build_context_prompt 中召回匹配技能注入上下文。
//   - 默认后端 JsonlSkillStore：JSONL 追加写持久化（崩溃安全），检索按触发
//     关键词与摘要匹配，无外部依赖。
//   - 与 memory.rs 同构：先默认 JSONL 后端，后续可平滑替换为商业记忆库 FFI。

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

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
}

/// 本地技能库后端 trait（E-4 跨平台一致性：路径统一，不暴露本地绝对路径）
pub trait SkillStore: Send + Sync {
    /// 保存一条技能（追加写；同名技能视为更新成功次数）
    fn save(&mut self, skill: SkillRecord) -> std::io::Result<()>;
    /// 按触发关键词/摘要召回相关技能（按相关度倒序）
    fn find(&self, query: &str, limit: usize) -> Vec<SkillRecord>;
    /// 列出全部技能（按沉淀时间倒序）
    #[allow(dead_code)] // 单测与后续面板展示使用
    fn list(&self) -> Vec<SkillRecord>;
    /// 技能条数
    fn len(&self) -> usize;
}

/// JSONL 持久化技能库后端（默认）。
///
/// 存储路径：`$AIRY_HOME/tui/skills.jsonl`（默认 `~/.airymaxrt/tui/`）。
/// 追加写保证崩溃安全；同名技能合并时更新成功次数并重写文件。
pub struct JsonlSkillStore {
    path: PathBuf,
    skills: Vec<SkillRecord>,
    max_skills: usize,
}

impl JsonlSkillStore {
    /// 创建技能库。dir 未指定时用 $AIRY_HOME/tui 或 ~/.airymaxrt/tui。
    pub fn new(dir: Option<&Path>) -> std::io::Result<Self> {
        let dir = match dir {
            Some(d) => d.to_path_buf(),
            None => skill_dir(),
        };
        fs::create_dir_all(&dir)?;
        let path = dir.join("skills.jsonl");
        let mut store = Self {
            path,
            skills: Vec::new(),
            max_skills: 500,
        };
        store.load()?;
        Ok(store)
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
            if let Ok(rec) = serde_json::from_str::<SkillRecord>(&line) {
                self.skills.push(rec);
            }
        }
        Ok(())
    }
}

impl SkillStore for JsonlSkillStore {
    fn save(&mut self, skill: SkillRecord) -> std::io::Result<()> {
        // 同名技能视为再次沉淀：合并并累加成功次数，避免重复堆积
        if let Some(existing) = self
            .skills
            .iter_mut()
            .find(|s| s.name == skill.name)
        {
            existing.success_count += 1;
            existing.procedure = if skill.procedure.trim().is_empty() {
                existing.procedure.clone()
            } else {
                skill.procedure.clone()
            };
            existing.lessons = if skill.lessons.trim().is_empty() {
                existing.lessons.clone()
            } else {
                skill.lessons.clone()
            };
            existing.trigger = skill.trigger;
            existing.summary = skill.summary;
            existing.tags = skill.tags;
            return self.rewrite();
        }

        // 追加写：单条记录一次 write + flush
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{}", serde_json::to_string(&skill)?)?;
        f.flush()?;
        self.skills.push(skill);
        if self.skills.len() > self.max_skills {
            let drain = self.skills.len() - self.max_skills;
            self.skills.drain(..drain);
            self.rewrite()?;
        }
        Ok(())
    }

    fn find(&self, query: &str, limit: usize) -> Vec<SkillRecord> {
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_lowercase())
            .collect();
        let mut scored: Vec<(SkillRecord, f32)> = self
            .skills
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
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        scored.into_iter().map(|(s, _)| s).collect()
    }

    fn list(&self) -> Vec<SkillRecord> {
        self.skills.iter().rev().cloned().collect()
    }

    fn len(&self) -> usize {
        self.skills.len()
    }
}

impl JsonlSkillStore {
    fn rewrite(&self) -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)?;
        for rec in &self.skills {
            writeln!(f, "{}", serde_json::to_string(rec)?)?;
        }
        f.flush()?;
        Ok(())
    }
}

/// 技能库目录：$AIRY_HOME/data/agentrt/tui（AIRY_HOME 路径体系收敛，2026-08-19）
fn skill_dir() -> PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui"])
}

/// 构造技能库后端（当前仅 JSONL，后续可扩展 FFI）。
pub fn build_skill_store(dir: Option<&Path>) -> Box<dyn SkillStore> {
    match JsonlSkillStore::new(dir) {
        Ok(s) => Box::new(s),
        Err(e) => {
            log::warn!("skills: JsonlSkillStore init failed ({}), using volatile store", e);
            Box::new(JsonlSkillStore {
                path: PathBuf::from("/dev/null"),
                skills: Vec::new(),
                max_skills: 500,
            })
        }
    }
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

    fn temp_store() -> (tempfile::TempDir, JsonlSkillStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = JsonlSkillStore::new(Some(dir.path())).expect("store");
        (dir, store)
    }

    #[test]
    fn save_and_find_roundtrip() {
        let (_dir, mut store) = temp_store();
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
        let (_dir, mut store) = temp_store();
        let s1 = SkillRecord::new("s", "dev", "a,b", "sum", "step1", "lesson", "t");
        let s2 = SkillRecord::new("s", "dev", "a,b", "sum2", "step2", "lesson2", "t");
        store.save(s1).unwrap();
        store.save(s2).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.list()[0].success_count, 1);
        // 最新 procedure 生效
        assert_eq!(store.list()[0].procedure, "step2");
    }

    #[test]
    fn find_empty_query_returns_nothing() {
        let (_dir, mut store) = temp_store();
        store
            .save(SkillRecord::new("x", "dev", "k1", "d", "p", "l", "t"))
            .unwrap();
        assert!(store.find("", 5).is_empty());
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
