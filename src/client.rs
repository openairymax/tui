// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// HTTP client for AgentRT gateway communication.
// Simplified version for the TUI.

use anyhow::{Context, Result};
use log::{debug, error, info};
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

/// Gateway API client for the TUI application.
///
/// reqwest::Client 内部为 Arc 连接池，Clone 成本低；后台任务需要
/// 独立的 client 副本发起 LLM 请求（与主循环渲染解耦）。
#[derive(Clone)]
pub struct GatewayClient {
    base_url: String,
    http: HttpClient,
}

impl GatewayClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(format!("agentrt-tui/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    pub async fn health_check(&self) -> Result<HealthResponse> {
        // gateway 实际端点：GET /health → {"status":"healthy","service":"gateway"}
        let url = format!("{}/health", self.base_url);
        debug!("GET {}", url);
        // 健康检查必须快速失败（2s 超时），否则离线时阻塞 TUI 启动
        let resp = tokio::time::timeout(
            Duration::from_secs(2),
            self.http.get(&url).send(),
        )
        .await
        .context("Gateway health check timed out (2s)")??;
        let status = resp.status();
        let body = resp.text().await?;
        debug!("← health {} ({} bytes)", status, body.len());
        serde_json::from_str(&body).context("Failed to parse health response")
    }

    /// 发送对话/任务请求到 gateway（agent.run）。
    ///
    /// `model` 为 None 或空串时省略 model 字段：gateway 回落到
    /// env AIRY_AGENT_MODEL → 用户覆盖 $AIRY_CONFIG_DIR/model.yaml →
    /// 内置默认；最终 llm_d 无模型时再回落其 global.default_model。
    ///
    /// `session_id` 为客户端预分配的会话 ID（`sess_` 前缀，用于 Ctrl+X
    /// 调用 agent.cancel 中止运行中请求）；None 时由网关生成。
    ///
    /// `agent` 为可选的 agent 编排 spec（JSON 对象，如 `{"role":"coding"}`）。
    /// 携带时 gateway 走 agent_d 编排分支（spawn+invoke），否则维持纯 LLM
    /// 工具循环——任务执行场景必须携带，否则编排分支永不触发。
    ///
    /// `history` 为可选的完整对话历史（OpenAI messages 数组，user/assistant
    /// 交替，含当前输入作为末条 user 消息）。携带时 gateway 以整个数组作为
    /// 工具循环的初始上下文（M1/M2 修复），否则退化为单条 prompt。
    pub async fn send_message(
        &self,
        prompt: &str,
        agent_file: &str,
        model: Option<&str>,
        session_id: Option<&str>,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
    ) -> Result<RunResponse> {
        // gateway 采用 JSON-RPC（POST /），method=agent.run，params 透传 prompt/agent_file
        let url = format!("{}/", self.base_url);
        let mut params = serde_json::json!({
            "prompt": prompt,
            "agent_file": agent_file,
            "interactive": true,
        });
        if let Some(m) = model {
            if !m.is_empty() {
                params["model"] = serde_json::Value::String(m.to_string());
            }
        }
        if let Some(sid) = session_id {
            if !sid.is_empty() {
                params["session_id"] = serde_json::Value::String(sid.to_string());
            }
        }
        if let Some(a) = agent {
            // gateway 判定 params.agent（JSON 对象）存在才进入编排分支
            params["agent"] = a;
        }
        if let Some(h) = history {
            // 完整对话历史（OpenAI messages 数组）：gateway 透传为工具循环初始上下文
            params["messages"] = h;
        }
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent.run",
            "params": params,
        });

        let start = Instant::now();
        info!(
            "POST {} (prompt_len={}, session={})",
            url,
            prompt.len(),
            session_id.unwrap_or("(网关生成)")
        );
        let resp = self.http.post(&url).json(&request).send().await?;
        let elapsed = start.elapsed();
        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            error!("← agent/run FAILED: HTTP {} ({}ms) → {}", status.as_u16(),
                   elapsed.as_millis(), body);
            // 2.3.4：完整 body 可能含 daemon 内部细节（路径/panic/响应原文），
            // 已写入日志（上方 error!）。界面错误链只保留状态码，body 不上屏。
            anyhow::bail!("Gateway error (HTTP {})", status.as_u16());
        }

        // 解析 JSON-RPC：优先 result，出错时透出 error.message
        let json: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse run response")?;
        if let Some(err) = json.get("error") {
            let msg = err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Gateway error: {}", msg);
        }
        let result = json.get("result")
            .context("Missing result in JSON-RPC response")?;

        let response = result.get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = result.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tokens_used = result.get("tokens_used").and_then(|v| v.as_u64());
        let cost_usd = result.get("cost_usd").and_then(|v| v.as_f64());

        // 双思考轨迹（可选）：gateway 回传 think.process 的
        // {plan: DAG, feedback: GRAD 反馈, stats}，供对话面板展示
        let thinking = result.get("thinking").and_then(|v| v.as_object()).cloned();

        // Agent 工具调用轨迹（可选）
        let tool_trace = result.get("tool_trace").and_then(|v| {
            serde_json::from_value::<Vec<ToolTrace>>(v.clone()).ok()
        });

        info!("← agent/run OK ({}ms, {} tokens)",
              elapsed.as_millis(),
              tokens_used.unwrap_or(0));
        Ok(RunResponse {
            session_id,
            response,
            tokens_used,
            cost_usd,
            thinking,
            tool_trace,
        })
    }

    /// 中止运行中的 agent.run 请求（Ctrl+X 服务端配合）。
    ///
    /// gateway 侧运行中请求注册表置位 cancelled，工具循环轮次间检查后中断，
    /// 原请求返回 -32800 "Request cancelled by user"。
    pub async fn cancel_session(&self, session_id: &str) -> Result<()> {
        let url = format!("{}/", self.base_url);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "agent.cancel",
            "params": { "session_id": session_id },
        });
        let resp = self.http.post(&url).json(&request).send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("agent.cancel HTTP {}: {}", status.as_u16(), body);
        }
        let json: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse cancel response")?;
        if let Some(err) = json.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            // 请求已完成（未找到活动条目）属于正常情况，仅记录
            debug!("agent.cancel: {}", msg);
            return Ok(());
        }
        info!("agent.cancel OK (session={})", session_id);
        Ok(())
    }

    /// 列出 tool_d 当前 pending 审批请求（tool.pending，Claude Code 风格 permission prompt）。
    ///
    /// 返回待人工决议的请求列表（request_id/tool/agent/params）。gateway 转发
    /// tool_d.pending 的 result（内嵌 JSON 字符串或 {"pending":[...]}），此处
    /// 兼容两种形态；查询失败返回空列表（不阻断对话，审批交互尽力而为）。
    pub async fn list_pending_approvals(&self) -> Result<Vec<PendingApproval>> {
        let url = format!("{}/", self.base_url);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tool.pending",
            "params": {},
        });
        let resp = self.http.post(&url).json(&request).send().await?;
        let body = resp.text().await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse pending approvals response")?;
        if json.get("error").is_some() {
            debug!("tool.pending returned error: {}", body);
            return Ok(Vec::new());
        }
        let Some(result) = json.get("result") else {
            return Ok(Vec::new());
        };
        // 形态 1: {"pending": [...]}
        if let Some(arr) = result.get("pending").and_then(|v| v.as_array()) {
            return Ok(serde_json::from_value(serde_json::Value::Array(arr.clone()))
                .unwrap_or_default());
        }
        // 形态 2: result 本身是内嵌 JSON 字符串
        if let Some(s) = result.as_str() {
            if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
                if let Some(arr) = inner.get("pending").and_then(|v| v.as_array()) {
                    return Ok(serde_json::from_value(serde_json::Value::Array(arr.clone()))
                        .unwrap_or_default());
                }
                if let Ok(list) = serde_json::from_value::<Vec<PendingApproval>>(inner) {
                    return Ok(list);
                }
            }
        }
        // 形态 3: result 直接是数组
        if let Some(arr) = result.as_array() {
            return Ok(serde_json::from_value(serde_json::Value::Array(arr.clone()))
                .unwrap_or_default());
        }
        Ok(Vec::new())
    }

    /// 决议一个 pending 审批请求（tool.approve）。
    ///
    /// `decision` ∈ {"allow", "always", "deny"}。成功返回 true；
    /// gateway 返回 error（如 request_id 已决议/不存在）时返回 false。
    pub async fn resolve_approval(&self, request_id: &str, decision: &str) -> Result<bool> {
        let url = format!("{}/", self.base_url);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tool.approve",
            "params": { "request_id": request_id, "decision": decision },
        });
        let resp = self.http.post(&url).json(&request).send().await?;
        let body = resp.text().await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse approve response")?;
        if json.get("error").is_some() {
            debug!("tool.approve error: {}", body);
            return Ok(false);
        }
        info!("tool.approve OK (request_id={}, decision={})", request_id, decision);
        Ok(true)
    }

    /// SSE 流式对话：POST /api/v1/chat/stream，逐块回调 `on_chunk`。
    ///
    /// gateway 端（http_gateway_routes.c）实现完整工具循环：
    /// 响应为 `data: <增量块>\n\n` SSE 事件流，`data: [DONE]` 收尾。
    /// 文本块回调 `on_chunk`；工具事件（__airy_evt: tool_call /
    /// tool_result）回调 `on_event`（JSON 原文）。返回累积的完整回复文本
    /// （不含工具事件 JSON）。
    ///
    /// `messages` 为完整 OpenAI messages 数组（多轮上下文），必须非空。
    /// `model` 为空时省略：gateway 回落 deepseek-v4-flash。
    pub async fn stream_chat<F, E>(
        &self,
        messages: serde_json::Value,
        model: Option<&str>,
        mut on_chunk: F,
        mut on_event: E,
    ) -> Result<String>
    where
        F: FnMut(&str),
        E: FnMut(&str),
    {
        let url = format!("{}/api/v1/chat/stream", self.base_url);
        let mut params = serde_json::json!({ "messages": messages });
        if let Some(m) = model {
            if !m.is_empty() {
                params["model"] = serde_json::Value::String(m.to_string());
            }
        }
        info!("POST {} (stream, msgs_len={})", url, params["messages"].as_array().map(|a| a.len()).unwrap_or(0));
        let mut resp = self.http.post(&url).json(&params).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            anyhow::bail!("Stream endpoint HTTP {}: {}", status.as_u16(), body);
        }

        let mut buf = String::new();
        let mut full = String::new();
        while let Some(chunk) = resp.chunk().await? {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // 按 SSE 事件分隔符 \n\n 切分完整事件
            while let Some(pos) = buf.find("\n\n") {
                let evt = buf[..pos].to_string();
                buf.drain(..pos + 2);
                let Some(data) = evt.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(full);
                }
                // 工具事件：JSON 载荷含 __airy_evt 标记，转发给 on_event，
                // 不污染正文累积
                if data.starts_with('{') {
                    let trimmed = data.trim();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        if v.get("__airy_evt").is_some() {
                            on_event(trimmed);
                            continue;
                        }
                    }
                }
                full.push_str(data);
                on_chunk(data);
            }
        }
        Ok(full)
    }

    #[allow(dead_code)]
    pub async fn get_logs(&self, lines: u32) -> Result<Vec<LogEntry>> {
        let url = format!("{}/api/v1/logs?lines={}", self.base_url, lines);
        let resp = self.http.get(&url).send().await?;
        let body = resp.text().await?;
        serde_json::from_str(&body).context("Failed to parse logs")
    }

    #[allow(dead_code)]
    pub async fn get_memory_stats(&self) -> Result<String> {
        let url = format!("{}/api/v1/memory/stats", self.base_url);
        let resp = self.http.get(&url).send().await?;
        Ok(resp.text().await.unwrap_or_default())
    }

    #[allow(dead_code)]
    pub async fn get_plugins(&self) -> Result<String> {
        let url = format!("{}/api/v1/plugins", self.base_url);
        let resp = self.http.get(&url).send().await?;
        Ok(resp.text().await.unwrap_or_default())
    }

    /// 通用 JSON-RPC 调用（POST /），返回 result 节点（无 result 时返回 Null）。
    ///
    /// 公开给运维命令（/daemons /agents /tools /models /mem /rpc）复用；
    /// 方法须在 gateway 转发白名单内（agent.* / tool.* / llm.* / mem.* / hall.* 等）。
    pub async fn rpc_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}/", self.base_url);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": method,
            "params": params,
        });
        let resp = self.http.post(&url).json(&request).send().await?;
        let body = resp.text().await?;
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse JSON-RPC response")?;
        if let Some(err) = json.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("{}: {}", method, msg);
        }
        Ok(json.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    /// 任务看板（hall.board）：work_hall 持久化执行实例 + agent_d 在线 agent 名单。
    ///
    /// gateway 直接读取 $AIRY_HOME/state/work_hall_state.json 并转发
    /// agent_d.list 实时数据，任何前端都能拿到同一块看板。
    pub async fn hall_board(&self) -> Result<HallBoard> {
        let result = self.rpc_call("hall.board", serde_json::json!({})).await?;
        serde_json::from_value(result).context("Failed to parse hall.board")
    }

    /// 任务列表（hall.tasks）：hall_store 任务文件枚举，最新在前。
    pub async fn hall_tasks(&self) -> Result<Vec<HallTask>> {
        let result = self.rpc_call("hall.tasks", serde_json::json!({})).await?;
        Ok(result
            .get("tasks")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    /// 单任务事件回放（hall.replay）：按 (ts_utc, seq) 全局因果序。
    ///
    /// `category` 为空时合并该任务全部类别（决策链语义）。
    pub async fn hall_replay(&self, task_id: &str, category: Option<&str>) -> Result<Vec<HallEvent>> {
        let params = match category {
            Some(c) if !c.is_empty() => serde_json::json!({ "task_id": task_id, "category": c }),
            _ => serde_json::json!({ "task_id": task_id }),
        };
        let result = self.rpc_call("hall.replay", params).await?;
        Ok(result
            .get("events")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    /// 全局事件流（hall.stream）：跨任务按 (ts_utc, seq) 合并，取最新 `limit` 条。
    pub async fn hall_stream(&self, limit: u64) -> Result<Vec<HallEvent>> {
        let result = self
            .rpc_call("hall.stream", serde_json::json!({ "limit": limit }))
            .await?;
        Ok(result
            .get("events")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    /// 订阅 hall.watch SSE 推送流（实时事件驱动，2026-08-21）。
    ///
    /// gateway 的 GET /api/v1/hall/watch 是长连接 SSE：每次 hall 事件落盘
    /// 即推 `data: <compact event JSON>`（hall.stream 是 poll-based pull，
    /// watch 是 real-time push 侧）。独立无超时 client，断连后 2s 自动重连；
    /// 接收端 drop 时（离开看板/事件流面板）watch 任务自动退出。
    pub fn hall_watch_events(&self) -> tokio::sync::mpsc::UnboundedReceiver<String> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let base = self.base_url.clone();
        tokio::spawn(async move {
            let client = match HttpClient::builder()
                .user_agent(format!("agentrt-tui-watch/{}", env!("CARGO_PKG_VERSION")))
                .build()
            {
                Ok(c) => c,
                Err(_) => return,
            };
            loop {
                let url = format!("{}/api/v1/hall/watch", base);
                if let Ok(resp) = client.get(&url).send().await {
                    let mut stream = resp.bytes_stream();
                    let mut buf: Vec<u8> = Vec::new();
                    while let Some(chunk) = stream.next().await {
                        let chunk = match chunk {
                            Ok(c) => c,
                            Err(_) => break,
                        };
                        buf.extend_from_slice(&chunk);
                        // SSE 帧以空行分隔；每帧含 0..N 个 "data: " 行。
                        while let Some(pos) = sse_frame_end(&buf) {
                            let frame: Vec<u8> = buf.drain(..pos).collect();
                            let text = String::from_utf8_lossy(&frame);
                            for line in text.lines() {
                                if let Some(d) = line.strip_prefix("data: ") {
                                    if tx.send(d.to_string()).is_err() {
                                        return; // 接收端已 drop
                                    }
                                }
                            }
                        }
                    }
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        rx
    }
}

/// 定位 SSE 帧结束位置（首个空行 "\n\n"，含末尾分隔符）。
fn sse_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct HealthResponse {
    pub status: String,
    pub version: Option<String>,
    pub uptime_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RunResponse {
    /// 网关侧会话 ID（客户端预分配的 sid_for_task 已用于 Ctrl+X 取消，
    /// 此处保留字段保证反序列化数据完整性，UI 刻意不展示）
    #[allow(dead_code)]
    pub session_id: String,
    pub response: String,
    pub tokens_used: Option<u64>,
    pub cost_usd: Option<f64>,
    /// 双思考轨迹：{plan: DAG 计划, feedback: GRAD 反馈, stats}
    pub thinking: Option<serde_json::Map<String, serde_json::Value>>,
    /// Agent 工具调用轨迹（LLM→工具→结果），供对话面板展示
    pub tool_trace: Option<Vec<ToolTrace>>,
}

/// 单次工具调用记录
#[derive(Debug, Clone, Deserialize)]
pub struct ToolTrace {
    pub tool: String,
    /// 工具调用参数（UI 克制设计不展示；保留字段保证反序列化数据完整性）
    #[allow(dead_code)]
    pub arguments: String,
    pub result: String,
    pub ok: Option<i64>,
}

/// 待人工决议的工具审批请求（tool.pending / tool.approve）。
#[derive(Debug, Clone, Deserialize)]
pub struct PendingApproval {
    pub request_id: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub params: String,
    #[serde(default)]
    pub created_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub daemon: Option<String>,
}

/// 任务看板条目（work_hall 持久化执行实例快照）。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallBoardEntry {
    pub execution_id: String,
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub workflow_name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub task_id: u64,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub completed_at: u64,
}

/// hall.board 聚合结果：执行实例 + 在线 agent。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallBoard {
    pub entries: Vec<HallBoardEntry>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub agent_total: Option<u64>,
    #[serde(default)]
    pub source: String,
}

/// hall.tasks 条目：hall_store 磁盘任务枚举。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallTask {
    pub tenant_id: String,
    pub task_id: String,
    #[serde(default)]
    pub latest_ts: String,
    #[serde(default)]
    pub event_count: u64,
}

/// hall.replay / hall.stream 单条事件。
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HallEvent {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub ts_utc: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub gseq: u64,
    #[serde(default)]
    pub content: serde_json::Value,
}