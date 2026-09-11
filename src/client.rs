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
            .user_agent(format!("agentrt-tui/{}", env!("AIRY_RT_VERSION")))
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
    ///
    /// `gccp_answers` 为 GCCP 两段式交互第二段的用户答案 JSON（可 None；
    /// 第一段 think.process 返回 gccp_need_interaction 后，客户端展示问题、
    /// 收集答案并以本参数重发同一 prompt 完成澄清闭环）。
    // 公开 API 签名稳定优先（WS-1 冻结）：7 个业务参数为 gateway params
    // 的显式形态，压缩为参数结构体会破坏既有全部调用点。
    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        &self,
        prompt: &str,
        agent_file: &str,
        model: Option<&str>,
        session_id: Option<&str>,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
        gccp_answers: Option<&str>,
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
        if let Some(a) = gccp_answers {
            // GCCP 两段式交互第二段：用户答案 JSON（gateway 透传 think.process）
            if !a.is_empty() {
                params["gccp_answers"] = serde_json::Value::String(a.to_string());
            }
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

        // GCCP 两段式交互（P-A）：think.process 第一段挂起时 gateway 回传
        // {gccp_need_interaction:1, gccp_questions:[...]}——客户端展示问题、
        // 收集答案后以 gccp_answers 重发同一 prompt（第二段完成澄清闭环）。
        let gccp_need_interaction = result
            .get("gccp_need_interaction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let gccp_questions: Vec<GccpQuestion> = result
            .get("gccp_questions")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

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
            gccp_need_interaction,
            gccp_questions,
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

    /// agent.run_stream 事件流（0.1.9 M5 W1，方案 §2.4 v1 信封协议）。
    ///
    /// POST /api/v1/agent/run/stream（gateway SSE 纯翻译端点），消费
    /// agent_d 引擎流式事件帧，并按 UI 语义转译为两个回调通道：
    ///   - `on_text`：token_delta.delta 增量文本（对话打字机实时渲染）；
    ///   - `on_tool`：工具进度 / 思考链 / 错误事件（__airy_evt 语义 JSON，
    ///     与 stream_chat 工具事件通道同格式，poll_pending 复用同一套消费）。
    ///
    /// 返回最终 RunResponse：response = message 帧 content（引擎权威最终
    /// 文本）；引擎无 message 时回退 token_delta 累积。结构化错误帧
    /// （error 事件 code/message）以 Err 呈现，原始 JSON 不上屏。
    ///
    /// `params` 透传语义与 send_message 一致（prompt/agent_file/model/
    /// session_id/agent/messages/gccp_answers），任务执行轮带 agent spec
    /// 走 agent_d 编排分支。
    // 10 参数 = 7 业务参数 + 双回调（流式渲染通道）：签名冻结理由同
    // send_message；回调独立泛型不可并入参数结构体。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_stream_turn<F, E>(
        &self,
        prompt: &str,
        agent_file: &str,
        model: Option<&str>,
        session_id: &str,
        agent: Option<serde_json::Value>,
        messages: Option<serde_json::Value>,
        gccp_answers: Option<&str>,
        mut on_text: F,
        mut on_tool: E,
    ) -> Result<RunResponse>
    where
        F: FnMut(&str),
        E: FnMut(&str),
    {
        use crate::run_stream::decode_sse_line;

        let url = format!("{}/api/v1/agent/run/stream", self.base_url);
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
        if !session_id.is_empty() {
            params["session_id"] = serde_json::Value::String(session_id.to_string());
        }
        if let Some(a) = agent {
            // gateway 判定 params.agent（JSON 对象）存在才进入编排分支
            params["agent"] = a;
        }
        if let Some(h) = messages {
            params["messages"] = h;
        }
        if let Some(a) = gccp_answers {
            if !a.is_empty() {
                params["gccp_answers"] = serde_json::Value::String(a.to_string());
            }
        }
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent.run_stream",
            "params": params,
        });
        info!(
            "POST {} (agent.run_stream, prompt_len={}, session={})",
            url,
            prompt.len(),
            session_id
        );
        // 流式请求使用独立无总超时 client（工具循环时长可能远超 60s）
        let stream_client = HttpClient::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(format!("agentrt-tui-stream/{}", env!("AIRY_RT_VERSION")))
            .build()
            .context("Failed to create stream HTTP client")?;
        let resp = stream_client.post(&url).json(&request).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            anyhow::bail!("run_stream endpoint HTTP {}: {}", status.as_u16(), body);
        }

        // v1 事件消费状态（本方法内局部；渲染状态经回调推给 UI）
        let mut final_content: Option<String> = None;
        let mut text_acc = String::new();
        let mut err_msg: Option<String> = None;
        let mut tokens: Option<u64> = None;
        // tool_end 只带 tool_id：以本地映射回填工具名（渲染需要动作短语）
        let mut tool_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        let mut stream = resp.bytes_stream();
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("run_stream network: {}", e))?;
            pending.extend_from_slice(&chunk);
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches('\r');
                let Some(ev) = decode_sse_line(text) else {
                    continue;
                };
                match ev.event_type.as_str() {
                    // token_delta：增量文本 → 打字机实时渲染 + 本地累积
                    crate::run_stream::gen::AIRY_RS_TYPE_TOKEN_DELTA => {
                        if let Some(d) = ev.data_str(crate::run_stream::gen::AIRY_RS_K_DELTA)
                        {
                            if !d.is_empty() {
                                text_acc.push_str(d);
                                on_text(d);
                            }
                        }
                    }
                    // tool_start / tool_end：工具进度行（克制：只显示
                    // 动作名与成败，不暴露参数与结果内容）
                    crate::run_stream::gen::AIRY_RS_TYPE_TOOL_START => {
                        let tool = ev
                            .data_str(crate::run_stream::gen::AIRY_RS_K_TOOL)
                            .unwrap_or("tool")
                            .to_string();
                        if let Some(tid) =
                            ev.data_str(crate::run_stream::gen::AIRY_RS_K_TOOL_ID)
                        {
                            tool_names.insert(tid.to_string(), tool.clone());
                        }
                        let line = serde_json::json!({
                            "__airy_evt": "tool_call",
                            "tool": tool,
                        });
                        on_tool(&line.to_string());
                    }
                    crate::run_stream::gen::AIRY_RS_TYPE_TOOL_END => {
                        let tid = ev
                            .data_str(crate::run_stream::gen::AIRY_RS_K_TOOL_ID)
                            .unwrap_or("")
                            .to_string();
                        let tool =
                            tool_names.get(&tid).cloned().unwrap_or_else(|| {
                                if tid.is_empty() {
                                    "tool".to_string()
                                } else {
                                    tid.clone()
                                }
                            });
                        let ok = ev
                            .data_str(crate::run_stream::gen::AIRY_RS_K_STATUS)
                            == Some("ok");
                        // 失败附引擎结果预览首行（≤80 字符）便于诊断
                        let mut line = serde_json::json!({
                            "__airy_evt": "tool_result",
                            "tool": tool,
                            "ok": if ok { 1 } else { 0 },
                        });
                        if !ok {
                            if let Some(summary) = ev.data_str(
                                crate::run_stream::gen::AIRY_RS_K_RESULT_HASH,
                            ) {
                                let err: String = summary
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .chars()
                                    .take(80)
                                    .collect();
                                line["summary"] = serde_json::Value::String(err);
                            }
                        }
                        on_tool(&line.to_string());
                    }
                    // message：引擎权威最终文本（role=assistant）；
                    // reasoning 为全流程思考链（一次性投递）
                    crate::run_stream::gen::AIRY_RS_TYPE_MESSAGE => {
                        if let Some(c) = ev
                            .data_str(crate::run_stream::gen::AIRY_RS_K_CONTENT)
                        {
                            if !c.is_empty() {
                                final_content = Some(c.to_string());
                            }
                        }
                        if let Some(r) =
                            ev.data_str(crate::run_stream::gen::AIRY_RS_K_REASONING)
                        {
                            if !r.is_empty() {
                                let line = serde_json::json!({
                                    "__airy_evt": "reasoning",
                                    "content": r,
                                });
                                on_tool(&line.to_string());
                            }
                        }
                    }
                    // plan：引擎目标计划（DAG 已由 GRAD 确认阶段展示，
                    // 此处克制不进对话区，仅入日志）
                    crate::run_stream::gen::AIRY_RS_TYPE_PLAN => {
                        debug!("agent.run_stream: plan event (ignored, GRAD DAG shown)");
                    }
                    // error：结构化错误帧 → Err（UI 只呈现可读消息）
                    crate::run_stream::gen::AIRY_RS_TYPE_ERROR => {
                        if let Some(m) =
                            ev.data_str(crate::run_stream::gen::AIRY_RS_K_MSG)
                        {
                            if err_msg.is_none() {
                                err_msg = Some(m.to_string());
                            }
                            let line = serde_json::json!({
                                "__airy_evt": "error",
                                "message": m,
                            });
                            on_tool(&line.to_string());
                        }
                    }
                    // run_end：流收尾（completed/cancelled/failed）；
                    // use_ticks = 本 run token 消耗
                    crate::run_stream::gen::AIRY_RS_TYPE_RUN_END => {
                        tokens = ev
                            .data_i64(crate::run_stream::gen::AIRY_RS_K_USE_TICKS)
                            .map(|t| t as u64);
                        let status = ev
                            .data_str(crate::run_stream::gen::AIRY_RS_K_STATUS)
                            .unwrap_or("completed");
                        debug!("agent.run_stream: run_end (status={})", status);
                    }
                    // 其余（run_start / 未知）：宽容忽略（§2.4.4）
                    _ => {}
                }
            }
        }
        /* 尾部残行（无 \n 收尾） */
        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            if let Some(ev) = decode_sse_line(text.trim_end_matches('\r')) {
                if ev.event_type == crate::run_stream::gen::AIRY_RS_TYPE_MESSAGE {
                    if let Some(c) = ev.data_str(crate::run_stream::gen::AIRY_RS_K_CONTENT) {
                        if !c.is_empty() {
                            final_content = Some(c.to_string());
                        }
                    }
                }
            }
        }

        // 结构化错误优先：引擎失败仅发 error 帧（无 message）
        if let Some(m) = err_msg {
            return Err(anyhow::anyhow!("{}", m));
        }
        // 引擎成功以 message 帧为权威内容；无 message 时回退增量累积
        let response = final_content.unwrap_or(text_acc);
        if response.trim().is_empty() {
            return Err(anyhow::anyhow!("agent.run_stream: empty result"));
        }
        info!(
            "← agent.run_stream OK ({} chars, tokens={:?})",
            response.len(),
            tokens
        );
        Ok(RunResponse {
            session_id: session_id.to_string(),
            response,
            tokens_used: tokens,
            cost_usd: None,
            thinking: None,
            tool_trace: None,
            gccp_need_interaction: false,
            gccp_questions: Vec::new(),
        })
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
                .user_agent(format!("agentrt-tui-watch/{}", env!("AIRY_RT_VERSION")))
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
                        // T-20：异常流（长流无帧分隔）时丢弃半帧，保持内存有界；
                        // 不完整帧本就无法解析，重连可恢复（协议自愈）。
                        sse_buf_extend(&mut buf, &chunk);
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

/// T-20：单条 SSE 帧缓冲上限（1 MiB）。hall 事件为紧凑 JSON，正常远小于此值；
/// 若网关推送异常（长流无 "\n\n" 分隔），无界累积会导致内存耗尽。
const MAX_SSE_BUFFER: usize = 1 << 20;

/// T-20：将 chunk 追加进 SSE 帧缓冲；追加后总量超限时丢弃半帧并从零累积
/// （不完整帧本就无法解析，保持内存有界即协议自愈）。
fn sse_buf_extend(buf: &mut Vec<u8>, chunk: &[u8]) {
    if buf.len() + chunk.len() > MAX_SSE_BUFFER {
        buf.clear();
    }
    buf.extend_from_slice(chunk);
}

/// 定位 SSE 帧结束位置（首个空行 "\n\n"，含末尾分隔符）。
fn sse_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_frame_end_finds_blank_line_separator() {
        assert_eq!(sse_frame_end(b"data: x\n\n"), Some(9));
        assert_eq!(sse_frame_end(b"data: x\n"), None);
        assert_eq!(sse_frame_end(b""), None);
    }

    #[test]
    fn sse_buf_extend_appends_within_limit() {
        let mut buf = Vec::new();
        sse_buf_extend(&mut buf, b"data: 1\n\n");
        assert_eq!(buf, b"data: 1\n\n");
    }

    #[test]
    fn sse_buf_extend_discards_half_frame_on_overflow() {
        // T-20：异常流无 "\n\n" 分隔时，缓冲超限必须丢弃半帧而不是无界增长。
        let mut buf = vec![b'a'; MAX_SSE_BUFFER];
        sse_buf_extend(&mut buf, b"overflow");
        // 半帧被清零，仅保留新 chunk（内存有界）。
        assert_eq!(buf, b"overflow");
        // 随后可正常重新累积帧。
        sse_buf_extend(&mut buf, b"data: x\n\n");
        assert_eq!(buf, b"overflowdata: x\n\n");
    }

    #[test]
    fn sse_buf_extend_allows_frame_up_to_limit() {
        // 恰好等于上限的帧允许保留（不误伤合法大帧）。
        let mut buf = Vec::new();
        sse_buf_extend(&mut buf, &vec![b'x'; MAX_SSE_BUFFER]);
        assert_eq!(buf.len(), MAX_SSE_BUFFER);
    }

    /* ---- 0.1.15 WS-6 T-04：agent.run_stream 真实对话回放回归 ----
     * 方案 §WS-6 6.2：CI 无法起全栈 gateway，退化为录制回放——以本地
     * SSE mock 服务器驱动 run_stream_turn 真实消费链（解码/累积/回退/
     * 错误上报），锁定社区用户报障「agent.run_stream: empty result」
     * 及中文长回复、错误帧、中断帧三类真实场景。帧字段一律走 gen
     * SSoT 常量，禁止手写 wire 字符串。 */

    use crate::run_stream::gen;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// 起一次性本地 SSE 服务器：accept 一个连接，读完整个请求（头 +
    /// 按 Content-Length 读满请求体，防止带未读数据 close 触发 RST）
    /// 后按给定字节块序列回放响应体（模拟任意 TCP 分片），随后关闭。
    fn spawn_sse_server(chunks: Vec<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut req: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            // 1. 读完请求头（到空行为止）
            let header_end = loop {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
                if let Some(pos) = req
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                {
                    break pos + 4;
                }
            };
            // 2. 按 Content-Length 读满请求体（避免 close 时接收缓冲残留 → RST）
            let content_length = req[..header_end]
                .split(|&b| b == b'\n')
                .find_map(|line| {
                    let line = String::from_utf8_lossy(line);
                    let (k, v) = line.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            while req.len() < header_end + content_length {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            // 3. 回放：HTTP 响应头（无 Content-Length，靠连接关闭结束流）
            let _ = conn.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for c in &chunks {
                if conn.write_all(c).is_err() {
                    break;
                }
                let _ = conn.flush();
            }
            // conn drop → FIN → 客户端 bytes_stream 结束
        });
        format!("http://{}", addr)
    }

    /// 构造 v1 事件信封（字段键走 gen SSoT 常量）。
    fn frame_envelope(event_type: &str, id: i64, data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            gen::AIRY_RS_K_V: gen::AIRY_RS_VERSION,
            gen::AIRY_RS_K_TYPE: event_type,
            gen::AIRY_RS_K_ID: id,
            gen::AIRY_RS_K_RUN_ID: "run_t04",
            gen::AIRY_RS_K_SESSION: "sess_t04",
            gen::AIRY_RS_K_DATA: data,
        })
    }

    fn frame_bytes(v: &serde_json::Value) -> Vec<u8> {
        format!("data: {}\n\n", v).into_bytes()
    }

    async fn drive_turn(
        base_url: &str,
    ) -> (
        Result<RunResponse>,
        Vec<String>,
        Vec<String>,
    ) {
        let text_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let tool_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = GatewayClient::new(base_url).expect("client 构造");
        let t = text_seen.clone();
        let g = tool_seen.clone();
        let result = client
            .run_stream_turn(
                "请分析当前任务",
                "agents/main.agent.yaml",
                None,
                "sess_t04",
                None,
                None,
                None,
                |chunk| t.lock().unwrap().push(chunk.to_string()),
                |evt| g.lock().unwrap().push(evt.to_string()),
            )
            .await;
        let text = text_seen.lock().unwrap().clone();
        let tool = tool_seen.lock().unwrap().clone();
        (result, text, tool)
    }

    #[tokio::test]
    async fn replay_chinese_long_reply_full_flow() {
        // 场景 1：中文长回复全流程——run_start → 多段 token_delta →
        // message（权威全文）→ run_end(completed)。断言权威内容采纳、
        // 打字机增量完整、token 消耗回传。
        let deltas = [
            "好的，我来分析这个任务。",
            "首先需要检查网关连接状态，",
            "然后核对 agent 配置文件，",
            "最后汇总执行计划并给出结论。",
        ];
        let authoritative = deltas.concat();
        let mut body = frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({})));
        for (i, d) in deltas.iter().enumerate() {
            body.extend(frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOKEN_DELTA,
                (i + 1) as i64,
                serde_json::json!({ gen::AIRY_RS_K_DELTA: d }),
            )));
        }
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_MESSAGE,
            6,
            serde_json::json!({
                gen::AIRY_RS_K_ROLE: "assistant",
                gen::AIRY_RS_K_CONTENT: authoritative,
            }),
        )));
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            7,
            serde_json::json!({
                gen::AIRY_RS_K_STATUS: "completed",
                gen::AIRY_RS_K_USE_TICKS: 42,
            }),
        )));
        let url = spawn_sse_server(vec![body]);
        let (result, text, _tool) = drive_turn(&url).await;
        let resp = result.expect("中文长回复应成功");
        assert_eq!(resp.response, authoritative, "message 帧为权威全文");
        assert_eq!(text.concat(), authoritative, "打字机增量完整");
        assert_eq!(resp.tokens_used, Some(42), "use_ticks 回传 token 消耗");
    }

    #[tokio::test]
    async fn replay_structured_error_frame_surfaces_message() {
        // 场景 2：错误帧——引擎失败发 error 帧，客户端转 Err 且消息
        // 可读（UI 只呈现可读消息，不暴露内部栈）。
        let err_text = "模型服务暂时不可用，请稍后重试";
        let body = [
            frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({}))),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_ERROR,
                1,
                serde_json::json!({ gen::AIRY_RS_K_MSG: err_text }),
            )),
        ]
        .concat();
        let url = spawn_sse_server(vec![body]);
        let (result, _text, tool) = drive_turn(&url).await;
        let err = result.expect_err("error 帧必须转 Err");
        assert!(err.to_string().contains(err_text), "错误消息原文可见: {}", err);
        assert!(
            tool.iter().any(|l| l.contains("\"__airy_evt\":\"error\"")),
            "错误事件进入工具通道供 UI 呈现"
        );
    }

    #[tokio::test]
    async fn replay_interrupted_run_keeps_accumulated_text() {
        // 场景 3：中断帧——cancelled 收尾且无 message 时，保留已收到的
        // 增量输出（用户中断不应丢失已生成内容）。
        let body = [
            frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({}))),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOKEN_DELTA,
                1,
                serde_json::json!({ gen::AIRY_RS_K_DELTA: "已完成一半的结论：" }),
            )),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOKEN_DELTA,
                2,
                serde_json::json!({ gen::AIRY_RS_K_DELTA: "网关连接正常。" }),
            )),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_RUN_END,
                3,
                serde_json::json!({ gen::AIRY_RS_K_STATUS: "cancelled" }),
            )),
        ]
        .concat();
        let url = spawn_sse_server(vec![body]);
        let (result, _text, _tool) = drive_turn(&url).await;
        let resp = result.expect("中断但有输出应成功返回");
        assert_eq!(resp.response, "已完成一半的结论：网关连接正常。");
    }

    #[tokio::test]
    async fn replay_empty_stream_reports_empty_result() {
        // 场景 4：空流回归锁定——run_start/run_end 收尾但无任何文本与
        // message，必须报「agent.run_stream: empty result」（社区用户
        // 实测报障原文），不得静默成功。
        let body = [
            frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({}))),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_RUN_END,
                1,
                serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
            )),
        ]
        .concat();
        let url = spawn_sse_server(vec![body]);
        let (result, _text, _tool) = drive_turn(&url).await;
        let err = result.expect_err("空流必须报错");
        assert_eq!(err.to_string(), "agent.run_stream: empty result");
    }

    #[tokio::test]
    async fn replay_tool_progress_events_reach_tool_channel() {
        // 场景 5：工具进度——tool_start/tool_end 经本地 tool_id 映射回填
        // 工具名，成功态 ok=1，进入工具通道渲染。
        let body = [
            frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({}))),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOOL_START,
                1,
                serde_json::json!({
                    gen::AIRY_RS_K_TOOL: "fs.write",
                    gen::AIRY_RS_K_TOOL_ID: "t1",
                }),
            )),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOOL_END,
                2,
                serde_json::json!({
                    gen::AIRY_RS_K_TOOL_ID: "t1",
                    gen::AIRY_RS_K_STATUS: "ok",
                }),
            )),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_MESSAGE,
                3,
                serde_json::json!({ gen::AIRY_RS_K_CONTENT: "done" }),
            )),
            frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_RUN_END,
                4,
                serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
            )),
        ]
        .concat();
        let url = spawn_sse_server(vec![body]);
        let (result, _text, tool) = drive_turn(&url).await;
        let resp = result.expect("工具流程应成功");
        assert_eq!(resp.response, "done");
        assert!(
            tool[0].contains("\"__airy_evt\":\"tool_call\"") && tool[0].contains("fs.write"),
            "tool_call 事件携带工具名: {}",
            tool[0]
        );
        assert!(
            tool[1].contains("\"__airy_evt\":\"tool_result\"") && tool[1].contains("\"ok\":1"),
            "tool_result 成功态 ok=1: {}",
            tool[1]
        );
    }

    #[tokio::test]
    async fn replay_bytes_split_across_tcp_chunks_still_parses() {
        // 场景 6：网络分片——同一 SSE 流按任意字节边界切成三块（帧 JSON
        // 中间截断）逐块到达，解析结果必须与单块一致（TCP 不保证按帧
        // 分段；回归网络层半帧粘包处理）。
        let deltas = ["第一段中文。", "第二段中文。", "第三段中文。"];
        let mut body = frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({})));
        for (i, d) in deltas.iter().enumerate() {
            body.extend(frame_bytes(&frame_envelope(
                gen::AIRY_RS_TYPE_TOKEN_DELTA,
                (i + 1) as i64,
                serde_json::json!({ gen::AIRY_RS_K_DELTA: d }),
            )));
        }
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_MESSAGE,
            4,
            serde_json::json!({ gen::AIRY_RS_K_CONTENT: deltas.concat() }),
        )));
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            5,
            serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
        )));
        let third = body.len() / 3;
        let url = spawn_sse_server(vec![
            body[..third].to_vec(),
            body[third..third * 2].to_vec(),
            body[third * 2..].to_vec(),
        ]);
        let (result, _text, _tool) = drive_turn(&url).await;
        let resp = result.expect("分片流应与整流等价");
        assert_eq!(resp.response, deltas.concat());
    }

    #[tokio::test]
    async fn replay_trailing_frame_without_newline_still_delivers_message() {
        // 场景 7：尾部残行——末帧 message 无 "\n\n" 收尾连接即关闭，
        // 残行仍须被采纳（防上游网关省略流结束符时丢权威内容）。
        let mut body = frame_bytes(&frame_envelope(gen::AIRY_RS_TYPE_RUN_START, 0, serde_json::json!({})));
        let mut last = frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_MESSAGE,
            1,
            serde_json::json!({ gen::AIRY_RS_K_CONTENT: "结尾无换行的权威内容" }),
        ));
        // 去掉末帧的 "\n\n" 收尾
        last.truncate(last.len() - 2);
        body.extend(last);
        let url = spawn_sse_server(vec![body]);
        let (result, _text, _tool) = drive_turn(&url).await;
        let resp = result.expect("残行 message 必须被采纳");
        assert_eq!(resp.response, "结尾无换行的权威内容");
    }
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
    /// GCCP 两段式交互第一段：think.process 挂起，需要用户澄清
    /// （gccp_need_interaction=true 时客户端进入问答轮）
    #[serde(default)]
    pub gccp_need_interaction: bool,
    /// GCCP 交互问题集（id/question/hint/required），展示给用户收集答案
    #[serde(default)]
    pub gccp_questions: Vec<GccpQuestion>,
}

/// GCCP 目标澄清问题（think.process 第一段回传，见 gccp.h airy_gccp_question_t）
#[derive(Debug, Clone, Deserialize)]
pub struct GccpQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub hint: String,
    /// 是否必答（GCCP 协议字段；UI 当前不区分展示，保留保证反序列化完整）
    #[serde(default)]
    #[allow(dead_code)]
    pub required: bool,
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