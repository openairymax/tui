// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT gateway 通信客户端（TUI 侧，退役中）。
//
// 运行轮已全部改由事件管道（event 子模块 + airy_cli）承载，本模块只剩观察面
// 的读侧 RPC：健康探针、tool.pending/tool.approve、hall.*、types 契约。
// 分层如下：
//   - 本模块：连接构造、健康探针、通用 rpc_call（传输骨架）
//   - approval：tool.pending / tool.approve（尽力而为语义）
//   - hall：hall.* 读侧 RPC + hall.watch 长连接
//   - event：事件/投影 API（D-15 子进程），运行轮的唯一入口
//   - types：wire → Rust 反序列化契约
//
// 传输层分工：事件帧由 airy_cli 产出；reqwest 仅承担读侧 JSON-RPC 与
// hall.watch 长连接。

use anyhow::{Context, Result};
use log::debug;
use reqwest::Client as HttpClient;
use std::time::Duration;

mod approval;
pub(crate) mod event;
mod hall;
mod types;

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
