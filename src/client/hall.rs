// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 任务看板/事件流通道（0.1.18 拆分自 client.rs）：hall.* RPC + hall.watch SSE。
//
// 读侧（board/tasks/replay/stream）走统一 rpc_call，形态解析失败一律降级为
// 空集合——看板是观察面，不因数据缺字段阻断 TUI。写侧唯一长连接是
// hall.watch：独立无超时 client + 断连 2s 自愈重连，接收端 drop 即退出
// （事件驱动，不做轮询）。

use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use std::time::Duration;
use tokio_stream::StreamExt;

use super::{GatewayClient, HallBoard, HallEvent, HallTask};

/// 单条 SSE 帧缓冲上限（1 MiB）。hall 事件为紧凑 JSON，正常远小于此值；
/// 若网关推送异常（长流无 "\n\n" 分隔），无界累积会导致内存耗尽。
const MAX_SSE_BUFFER: usize = 1 << 20;

/// 将 chunk 追加进 SSE 帧缓冲；追加后总量超限时丢弃半帧并从零累积
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

impl GatewayClient {
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
    pub async fn hall_replay(
        &self,
        task_id: &str,
        category: Option<&str>,
    ) -> Result<Vec<HallEvent>> {
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

#[cfg(test)]
mod tests {
    use super::{sse_buf_extend, sse_frame_end, MAX_SSE_BUFFER};

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
}
