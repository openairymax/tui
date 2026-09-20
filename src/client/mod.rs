// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT gateway 通信客户端（TUI 侧）。
//
// 0.1.18 按职责域分文件（文件行数纪律：单文件 ≤800 行）：同一个
// `impl GatewayClient` 分散在子模块，子模块以 `use super::*` 继承本模块
// 的类型与私有字段，外部路径仍为 crate::client。分层如下：
//   - 本模块：连接构造、健康探针、同步 agent.run、取消、通用 rpc_call（传输骨架）
//   - approval：tool.pending / tool.approve（尽力而为语义）
//   - stream：agent.run_stream 事件消费 + SSE 缓冲边界
//   - hall：hall.* 读侧 RPC + hall.watch 长连接
//   - types：wire → Rust 反序列化契约
//
// 传输层分工：agentrt-rs 协议客户端是流式执行轮的唯一通道（端点与 v1
// 信封不在此重写）；reqwest 仅承担本文件的 JSON-RPC 直发与 SSE 长连接。

use anyhow::{Context, Result};
use log::{debug, error, info};
use reqwest::Client as HttpClient;
use std::time::{Duration, Instant};

mod approval;
mod hall;
mod stream;
mod types;

#[cfg(test)]
mod tests;

pub use types::*;

/// Gateway API client for the TUI application.
///
/// reqwest::Client 内部为 Arc 连接池，Clone 成本低；后台任务需要
/// 独立的 client 副本发起 LLM 请求（与主循环渲染解耦）。
#[derive(Clone)]
pub struct GatewayClient {
    base_url: String,
    http: HttpClient,
    /// 协议客户端（agentrt-rs）：流式执行轮的传输与事件解码唯一通道，
    /// TUI 不自带 wire 常量与 SSE 解码实现。
    rs: agentrt_rs::Client,
}

impl GatewayClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let ua = format!("agentrt-tui/{}", env!("AIRY_RT_VERSION"));
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(ua.clone())
            .build()
            .context("Failed to create HTTP client")?;
        let rs = agentrt_rs::client::ClientBuilder::new(&base_url)
            .user_agent(&ua)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create protocol client: {e}"))?;

        Ok(Self { base_url, http, rs })
    }

    pub async fn health_check(&self) -> Result<HealthResponse> {
        // gateway 实际端点：GET /health → {"status":"healthy","service":"gateway"}
        let url = format!("{}/health", self.base_url);
        debug!("GET {}", url);
        // 健康检查必须快速失败（2s 超时），否则离线时阻塞 TUI 启动
        let resp = tokio::time::timeout(Duration::from_secs(2), self.http.get(&url).send())
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
            error!(
                "← agent/run FAILED: HTTP {} ({}ms) → {}",
                status.as_u16(),
                elapsed.as_millis(),
                body
            );
            // 2.3.4：完整 body 可能含 daemon 内部细节（路径/panic/响应原文），
            // 已写入日志（上方 error!）。界面错误链只保留状态码，body 不上屏。
            anyhow::bail!("Gateway error (HTTP {})", status.as_u16());
        }

        // 解析 JSON-RPC：优先 result，出错时透出 error.message
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse run response")?;
        if let Some(err) = json.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Gateway error: {}", msg);
        }
        let result = json
            .get("result")
            .context("Missing result in JSON-RPC response")?;

        let response = result
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = result
            .get("session_id")
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
        let tool_trace = result
            .get("tool_trace")
            .and_then(|v| serde_json::from_value::<Vec<ToolTrace>>(v.clone()).ok());

        info!(
            "← agent/run OK ({}ms, {} tokens)",
            elapsed.as_millis(),
            tokens_used.unwrap_or(0)
        );
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
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse cancel response")?;
        if let Some(err) = json.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            // 请求已完成（未找到活动条目）属于正常情况，仅记录
            debug!("agent.cancel: {}", msg);
            return Ok(());
        }
        info!("agent.cancel OK (session={})", session_id);
        Ok(())
    }

    /// 通用 JSON-RPC 调用（POST /），返回 result 节点（无 result 时返回 Null）。
    ///
    /// 公开给运维命令（/daemons /agents /tools /models /mem /rpc）复用；
    /// 方法须在 gateway 转发白名单内（agent.* / tool.* / llm.* / mem.* / hall.* 等）。
    pub async fn rpc_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
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
        Ok(json
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}
