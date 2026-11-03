// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 斜杠命令：/model /key /status /skills /chain /daemons /ops /rpc 的解析与执行。

use super::*;

impl App {
    /// /model 命令：查看（无参数）或设置（/model <模型名>）当前模型。
    ///
    /// 模型名写回统一配置权威源 $AIRY_HOME/config/model.yaml 的 default_model，
    /// 后续 agent.run 请求携带 model 字段；为空时由 gateway/llm_d 依次回落默认。
    pub(super) fn cmd_model(&mut self, input: &str) {
        let arg = input[6..].trim();
        if arg.is_empty() {
            let cur = if self.model.is_empty() {
                "（默认，由网关 / llm_d 自动回落）".to_string()
            } else {
                self.model.clone()
            };
            self.add_message(MessageRole::System, format!("当前模型：{}", cur));
            self.add_message(
                MessageRole::System,
                "设置模型：/model <模型名>（写回 $AIRY_HOME/config/model.yaml 的 default_model）"
                    .to_string(),
            );
            self.add_message(
                MessageRole::System,
                format!("默认配置：{}", self.config_file),
            );
            return;
        }
        self.model = arg.to_string();
        persist_model(&self.model);
        self.add_log("INFO", format!("模型切换为 {}", self.model));
        self.add_message(
            MessageRole::System,
            format!("模型已设置为：{}（已写回 model.yaml）", self.model),
        );
        self.toast(format!("模型已切换：{}", self.model));
    }

    /// /set-key 命令：便捷写入 $AIRY_HOME/config/secrets.env 中的模型 API Key
    /// （F2 配置面板 API Key 配置节的编辑入口；llm_d 热加载，无需重启）。
    ///
    ///   /set-key                         → 列出已知 Key 与用法
    ///   /set-key DEEPSEEK_API_KEY sk-…  → 写入（原位替换 / 追加，chmod 600）
    pub(super) fn cmd_set_key(&mut self, input: &str) {
        let rest = input.trim_start_matches("/set-key").trim();
        // 无参数：提示用法与已知 Key 清单
        if rest.is_empty() {
            let known: Vec<&str> = crate::secrets::KNOWN_KEYS.iter().map(|(k, _)| *k).collect();
            self.add_message(
                MessageRole::System,
                format!("用法：/set-key <KEY> <VALUE>（写入 $AIRY_HOME/config/secrets.env，chmod 600）\n已知 Key：{}", known.join(" / ")),
            );
            return;
        }
        let (key, value) = match rest.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (rest.trim(), ""),
        };
        if !crate::secrets::valid_key_name(key) {
            self.add_message(
                MessageRole::System,
                format!("Key 名非法：{}（应为大写下划线，如 DEEPSEEK_API_KEY）", key),
            );
            return;
        }
        if value.is_empty() {
            self.add_message(
                MessageRole::System,
                format!("用法：/set-key {} <VALUE>（值不能为空）", key),
            );
            return;
        }
        match crate::secrets::set_key(key, value) {
            Ok(()) => {
                self.add_log("INFO", format!("API Key {} 已写入 secrets.env", key));
                self.add_message(
                    MessageRole::System,
                    format!(
                        "API Key {} 已写入 {}（{}）",
                        key,
                        crate::secrets::secrets_path().display(),
                        crate::secrets::mask(value)
                    ),
                );
                self.toast(format!("API Key 已写入：{}", key));
            }
            Err(e) => {
                self.add_log("ERROR", format!("写入 secrets.env 失败：{}", e));
                self.add_message(MessageRole::System, format!("写入 secrets.env 失败：{}", e));
            }
        }
    }

    /// /status 命令：展示运行时状态总览（连接/版本/模型/用量/记忆/技能）。
    pub(super) fn cmd_status(&mut self) {
        let conn = if self.connected { "ONLINE" } else { "OFFLINE" };
        let model = if self.model.is_empty() {
            "默认（网关 / llm_d 自动回落）".to_string()
        } else {
            self.model.clone()
        };
        let phase = self.flow_phase.label();
        let text = format!(
            "运行时状态\n  \
             连接: {}  v{}\n  \
             模型: {}\n  \
             阶段: {} · 回合: {} · Token: {} · 成本: ${:.4} · 耗时: {}\n  \
             记忆: {} 条 · 技能: {} 条\n  \
             配置: {}",
            conn,
            self.gateway_version.as_deref().unwrap_or("unknown"),
            model,
            phase,
            self.turn,
            self.tokens,
            self.cost,
            self.elapsed_time(),
            self.memory.len(),
            self.skills.len(),
            self.config_file
        );
        self.add_message(MessageRole::System, text);
        self.add_log("INFO", "状态查询（/status）".to_string());
    }

    /// /skills 命令：列出共享技能库（任务成功后自动沉淀的可复用技能）。
    pub(super) fn cmd_skills(&mut self) {
        let list = self.skills.list();
        if list.is_empty() {
            self.add_message(
                MessageRole::System,
                "共享技能库为空：任务完成后经验会自动沉淀为可复用技能\
                 （经网关 mem.* 与 CLI 共享同一存储）。"
                    .to_string(),
            );
            return;
        }
        let mut text = format!("共享技能库（{} 条，经网关 mem.* 与 CLI 共享）", list.len());
        for s in list.iter().take(12) {
            text.push_str(&format!(
                "\n  ✓ {}（{} · 复用 {} 次）：{}",
                s.name, s.category, s.success_count, s.summary
            ));
        }
        if list.len() > 12 {
            text.push_str(&format!("\n  … 另有 {} 条", list.len() - 12));
        }
        self.add_message(MessageRole::System, text);
    }

    /// /chain 命令：无参数列出 hall_store 任务（最新在前）；带 task_id 回放该任务
    /// 全部类别事件（按 gseq 因果序 = 决策链）。数据经 gateway hall.tasks/hall.replay。
    ///
    /// 结果异步返回：先给"读取中"提示，poll_chain 消费后渲染进对话区。
    pub(super) fn cmd_chain(&mut self, input: &str) {
        let arg = input[6..].trim().to_string();
        if self.chain_pending.is_some() {
            self.add_message(MessageRole::System, "决策链查询进行中，请稍候…".to_string());
            return;
        }
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.chain_task = arg.clone();
        if arg.is_empty() {
            self.add_message(MessageRole::System, "正在读取任务列表…".to_string());
            tokio::spawn(async move {
                let _ = tx.send(ChainOutcome::Tasks(gw.hall_tasks().await));
            });
        } else {
            self.add_message(
                MessageRole::System,
                format!("正在回放决策链（task_id={}）…", arg),
            );
            tokio::spawn(async move {
                let _ = tx.send(ChainOutcome::Events(gw.hall_replay(&arg, None).await));
            });
        }
        self.chain_pending = Some(rx);
    }

    /// /daemons：14 个 daemon 命名空间经 gateway health_check 聚合在线
    /// 状态（0.1.9 M4 整编口径，清单见 OPS_DAEMON_NS；gateway_d 自身以
    /// 连接状态呈现）。
    ///
    /// 结果异步返回：先给"检查中"提示，poll_ops 消费后渲染进对话区。
    pub(super) fn cmd_daemons(&mut self) {
        if self.ops_pending.is_some() {
            self.add_message(MessageRole::System, "运维命令执行中，请稍候…".to_string());
            return;
        }
        self.ops_label = "daemons".to_string();
        self.add_message(MessageRole::System, "正在检查 daemon 在线状态…".to_string());
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut results = Vec::new();
            for ns in OPS_DAEMON_NS {
                let r = gw
                    .rpc_call(&format!("{}.health_check", ns), serde_json::json!({}))
                    .await;
                results.push((ns.to_string(), r));
            }
            let _ = tx.send(OpsOutcome::Daemons(results));
        });
        self.ops_pending = Some(rx);
    }

    /// 通用运维方法调用（/agents /tools /models /mem /rpc 共用）。
    pub(super) fn cmd_ops_call(&mut self, method: &str, params: serde_json::Value) {
        if self.ops_pending.is_some() {
            self.add_message(MessageRole::System, "运维命令执行中，请稍候…".to_string());
            return;
        }
        self.ops_label = method.to_string();
        self.add_message(MessageRole::System, format!("正在调用 {} …", method));
        let method_owned = method.to_string();
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(OpsOutcome::Call(gw.rpc_call(&method_owned, params).await));
        });
        self.ops_pending = Some(rx);
    }

    /// /rpc <ns>.<method> [json]：通用 JSON-RPC 直调（对齐 C CLI /rpc）。
    pub(super) fn cmd_rpc(&mut self, input: &str) {
        let rest = input[5..].trim().to_string();
        if rest.is_empty() {
            self.add_message(
                MessageRole::System,
                "用法：/rpc <ns>.<method> [json]（如 /rpc tool.list_tools）".to_string(),
            );
            return;
        }
        let (method, params) = match rest.split_once(char::is_whitespace) {
            Some((m, p)) => (m.trim().to_string(), p.trim().to_string()),
            None => (rest.clone(), String::new()),
        };
        if method.is_empty() || !method.contains('.') {
            self.add_message(
                MessageRole::System,
                format!("方法格式应为 <ns>.<method>，收到：{}", method),
            );
            return;
        }
        let params_val = if params.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&params).unwrap_or_else(|_| serde_json::json!({ "raw": params }))
        };
        self.cmd_ops_call(&method, params_val);
    }

    /// /trace：打开工具轨迹面板（0.1.20 批次五）。对话区克制展示策略
    /// 不变，全量明细（动作名 / 成败 / 参数 / 结果首行）只在面板按需查看。
    pub(super) fn open_trace(&mut self) {
        self.active_panel = ActivePanel::Trace;
        self.trace_scroll = 0;
        self.stop_hall_watch();
    }

    /// 工具轨迹面板滚动（↑ ↓ / PgUp PgDn，Esc 返回对话区）。
    pub fn trace_up(&mut self) {
        self.trace_scroll = self.trace_scroll.saturating_sub(1);
    }

    pub fn trace_down(&mut self) {
        self.trace_scroll = self.trace_scroll.saturating_add(1);
    }

    pub fn trace_pgup(&mut self) {
        self.trace_scroll = self.trace_scroll.saturating_sub(TRACE_PAGE);
    }

    pub fn trace_pgdn(&mut self) {
        self.trace_scroll = self.trace_scroll.saturating_add(TRACE_PAGE);
    }

    /// /mem <关键词>：检索长期记忆并跟随 mem.get 补正文（0.1.20 批次五）。
    ///
    /// mem.search 只回 record_id + score（正文不随行），与 C CLI 同法逐条
    /// mem.get 补齐后在对话区呈现可读命中列表；失败原因原样回显，不臆造。
    pub(super) fn cmd_mem_search(&mut self, query: &str) {
        if self.mem_rx.is_some() {
            self.toast("记忆检索进行中，请稍候");
            return;
        }
        let gw = self.gateway.clone();
        let q = query.to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(mem_fetch(gw, &q).await);
        });
        self.mem_rx = Some(rx);
    }

    /// 消费 /mem 检索回包（主循环每帧调用）。
    pub fn poll_mem(&mut self) {
        let Some(mut rx) = self.mem_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(MemOutcome::Hits(hits)) => {
                self.add_message(MessageRole::System, mem_hits_text(&hits));
            }
            Ok(MemOutcome::Failed(reason)) => {
                self.add_message(MessageRole::System, format!("记忆检索失败：{reason}"));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => self.mem_rx = Some(rx),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }
}

// ── /mem 检索跟随（0.1.20 批次五）──────────────────────────────────────

/// /mem 检索超时（检索 + 至多 MEM_HITS_MAX 次 mem.get 的整体预算）。
const MEM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// 工具轨迹面板翻页步长（行）。
const TRACE_PAGE: usize = 10;

/// /mem 检索回包：命中列表或失败原因（原样保留）。
pub(super) enum MemOutcome {
    Hits(MemHits),
    Failed(String),
}

/// /mem 检索结果：query 原文 + 命中列表 + 检索声明的总数。
pub(super) struct MemHits {
    pub query: String,
    pub hits: Vec<MemHit>,
    pub total: u64,
}

/// /mem 检索单条命中（正文经 mem.get 跟随补齐）。
pub(super) struct MemHit {
    pub score: f64,
    pub record_id: String,
    pub text: String,
}

/// 一次完整检索：mem.search → 命中逐条 mem.get 补正文；正文取不到留空。
async fn mem_fetch(gw: GatewayClient, query: &str) -> MemOutcome {
    let search = tokio::time::timeout(
        MEM_TIMEOUT,
        gw.rpc_call(
            "mem.search",
            serde_json::json!({ "query": query, "limit": MEM_HITS_MAX }),
        ),
    )
    .await;
    let v = match search {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return MemOutcome::Failed(e.to_string()),
        Err(_) => return MemOutcome::Failed(format!("检索超时（{}s）", MEM_TIMEOUT.as_secs())),
    };
    let raw = v
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let total = v
        .get("total")
        .and_then(|t| t.as_u64())
        .unwrap_or(raw.len() as u64);
    let mut hits = Vec::new();
    for h in raw.iter().take(MEM_HITS_MAX) {
        let Some(rid) = h.get("record_id").and_then(|r| r.as_str()) else {
            continue;
        };
        let score = h.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
        let text = gw
            .rpc_call("mem.get", serde_json::json!({ "record_id": rid }))
            .await
            .ok()
            .and_then(|v| v.get("data").and_then(|d| d.as_str()).map(str::to_string))
            .unwrap_or_default();
        hits.push(MemHit {
            score,
            record_id: rid.to_string(),
            text,
        });
    }
    MemOutcome::Hits(MemHits {
        query: query.to_string(),
        hits,
        total,
    })
}

/// 命中列表文本（对话区呈现）：总览 + 每命中「分数 · id · 正文首行」。
fn mem_hits_text(hits: &MemHits) -> String {
    if hits.hits.is_empty() {
        return format!(
            "记忆检索 “{}”：无命中（库内共 {} 条可查）。",
            hits.query, hits.total
        );
    }
    let mut out = format!("记忆检索 “{}”：{} 条命中", hits.query, hits.hits.len());
    if hits.total > hits.hits.len() as u64 {
        out.push_str(&format!(
            "（库内共 {} 条，展示前 {}）",
            hits.total,
            hits.hits.len()
        ));
    }
    out.push('\n');
    for (i, h) in hits.hits.iter().enumerate() {
        let id8: String = h.record_id.chars().take(8).collect();
        let head = one_line_clip(&h.text, MEM_HIT_CLIP_CHARS);
        if head.is_empty() {
            out.push_str(&format!(
                "  {}. {:.3}  {}  （正文未取到）\n",
                i + 1,
                h.score,
                id8
            ));
        } else {
            out.push_str(&format!("  {}. {:.3}  {}  {}\n", i + 1, h.score, id8, head));
        }
    }
    out.trim_end().to_string()
}

/// UTF-8 安全单行截断：折叠空白，超长加省略号。
fn one_line_clip(s: &str, max_chars: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let head: String = flat.chars().take(max_chars).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无命中的诚实空态：给 query 与库内总量，不合成条目。
    #[test]
    fn mem_hits_text_empty_reports_total() {
        let hits = MemHits {
            query: "天气".to_string(),
            hits: Vec::new(),
            total: 42,
        };
        let t = mem_hits_text(&hits);
        assert!(t.contains("无命中"));
        assert!(t.contains("42"));
    }

    /// 命中行格式：序号 + 分数三位小数 + id 前 8 位 + 正文首行。
    #[test]
    fn mem_hits_text_lists_score_id_head() {
        let hits = MemHits {
            query: "天气".to_string(),
            hits: vec![MemHit {
                score: 0.8234,
                record_id: "ab12cd34ef56".to_string(),
                text: "上海今天多云转晴\n26°C".to_string(),
            }],
            total: 1,
        };
        let t = mem_hits_text(&hits);
        assert!(t.contains("1 条命中"));
        assert!(t.contains("0.823"));
        assert!(t.contains("ab12cd34"));
        assert!(t.contains("上海今天多云转晴 26°C"));
    }

    /// 正文缺失的命中仍给分数与 id，标注「正文未取到」（诚实语义）。
    #[test]
    fn mem_hits_text_marks_missing_body() {
        let hits = MemHits {
            query: "x".to_string(),
            hits: vec![MemHit {
                score: 0.5,
                record_id: "rec9".to_string(),
                text: String::new(),
            }],
            total: 1,
        };
        assert!(mem_hits_text(&hits).contains("正文未取到"));
    }

    /// 单行截断：折叠换行、UTF-8 安全、超长加省略号。
    #[test]
    fn one_line_clip_folds_and_clips() {
        assert_eq!(one_line_clip("a\nb  c", 10), "a b c");
        let long = "字".repeat(12);
        let out = one_line_clip(&long, 8);
        assert!(out.chars().count() <= 9);
        assert!(out.ends_with('…'));
    }
}
