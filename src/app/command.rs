// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 斜杠命令：/model /key /status /skills /chain /daemons /ops /rpc 的解析与执行。

use super::*;

impl App {
    /// /model 命令：查看（无参数）或设置（/model <模型名>）当前模型。
    ///
    /// 模型名持久化到 $AIRY_HOME/tui/config.toml，后续 agent.run 请求
    /// 携带 model 字段；为空时由 gateway/llm_d 依次回落默认模型。
    pub(super) fn cmd_model(&mut self, input: &str) {
        let arg = input[6..].trim();
        if arg.is_empty() {
            let cur = if self.model.is_empty() {
                "（默认，由网关 / llm_d 自动回落）".to_string()
            } else {
                format!("{}", self.model)
            };
            self.add_message(MessageRole::System, format!("当前模型：{}", cur));
            self.add_message(
                MessageRole::System,
                format!("设置模型：/model <模型名>（持久化到 $AIRY_HOME/tui/config.toml）"),
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
            format!("模型已设置为：{}（已持久化）", self.model),
        );
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
            }
            Err(e) => {
                self.add_log("ERROR", format!("写入 secrets.env 失败：{}", e));
                self.add_message(
                    MessageRole::System,
                    format!("写入 secrets.env 失败：{}", e),
                );
            }
        }
    }

    /// /status 命令：展示运行时状态总览（连接/版本/模型/用量/记忆/技能）。
    pub(super) fn cmd_status(&mut self) {
        let conn = if self.connected {
            "ONLINE"
        } else {
            "OFFLINE"
        };
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

    /// /skills 命令：列出本地技能库（任务成功后自动沉淀的可复用技能）。
    pub(super) fn cmd_skills(&mut self) {
        let list = self.skills.list();
        if list.is_empty() {
            self.add_message(
                MessageRole::System,
                "本地技能库为空：任务完成后经验会自动沉淀为可复用技能。".to_string(),
            );
            return;
        }
        let mut text = format!("本地技能库（{} 条）", list.len());
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
            self.add_message(
                MessageRole::System,
                "决策链查询进行中，请稍候…".to_string(),
            );
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

    /// /daemons：16 个 daemon 命名空间经 gateway health_check 聚合在线状态。
    ///
    /// 结果异步返回：先给"检查中"提示，poll_ops 消费后渲染进对话区。
    pub(super) fn cmd_daemons(&mut self) {
        if self.ops_pending.is_some() {
            self.add_message(
                MessageRole::System,
                "运维命令执行中，请稍候…".to_string(),
            );
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
            self.add_message(
                MessageRole::System,
                "运维命令执行中，请稍候…".to_string(),
            );
            return;
        }
        self.ops_label = method.to_string();
        self.add_message(
            MessageRole::System,
            format!("正在调用 {} …", method),
        );
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
            serde_json::from_str(&params)
                .unwrap_or_else(|_| serde_json::json!({ "raw": params }))
        };
        self.cmd_ops_call(&method, params_val);
    }
}
