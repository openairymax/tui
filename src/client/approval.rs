// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 工具审批通道（0.1.18 拆分自 client.rs）：tool.pending / tool.approve。
//
// 审批交互为尽力而为语义——查询失败返回空列表、决议失败返回 false，
// 都不阻断对话主链路（工具循环自身有超时与拒绝兜底）。因此本模块只做
// 形态兼容与错误降噪，不向上传播可恢复错误。

use anyhow::{Context, Result};
use log::{debug, info};

use super::GatewayClient;
use super::PendingApproval;

impl GatewayClient {
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
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse pending approvals response")?;
        if json.get("error").is_some() {
            debug!("tool.pending returned error: {}", body);
            return Ok(Vec::new());
        }
        let Some(result) = json.get("result") else {
            return Ok(Vec::new());
        };
        // 形态 1: {"pending": [...]}
        if let Some(arr) = result.get("pending").and_then(|v| v.as_array()) {
            return Ok(
                serde_json::from_value(serde_json::Value::Array(arr.clone())).unwrap_or_default(),
            );
        }
        // 形态 2: result 本身是内嵌 JSON 字符串
        if let Some(s) = result.as_str() {
            if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
                if let Some(arr) = inner.get("pending").and_then(|v| v.as_array()) {
                    return Ok(
                        serde_json::from_value(serde_json::Value::Array(arr.clone()))
                            .unwrap_or_default(),
                    );
                }
                if let Ok(list) = serde_json::from_value::<Vec<PendingApproval>>(inner) {
                    return Ok(list);
                }
            }
        }
        // 形态 3: result 直接是数组
        if let Some(arr) = result.as_array() {
            return Ok(
                serde_json::from_value(serde_json::Value::Array(arr.clone())).unwrap_or_default(),
            );
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
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse approve response")?;
        if json.get("error").is_some() {
            debug!("tool.approve error: {}", body);
            return Ok(false);
        }
        info!(
            "tool.approve OK (request_id={}, decision={})",
            request_id, decision
        );
        Ok(true)
    }
}
