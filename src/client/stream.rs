// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 流式执行轮通道（0.1.18 拆分自 client.rs）：agent.run_stream 事件消费。
//
// 传输与帧解码由 agentrt-rs 协议客户端统一承担（端点与 v1 信封不在此处
// 重写），本模块只做 UI 语义转译与 SSE 缓冲边界控制。所有权衡：文本增量
// 即时回调（打字机）与最终权威文本分离——message 帧存在时以帧内容为准，
// 否则回退增量累积，避免引擎不发 message 时丢内容。

use anyhow::Result;
use log::{debug, info};

use agentrt_rs::run_stream::{gen, run_stream_events};

use super::GatewayClient;
use super::RunResponse;

impl GatewayClient {
    /// agent.run_stream 事件流（0.1.9 M5 W1，方案 §2.4 v1 信封协议）。
    ///
    /// 传输与帧解码由 agentrt-rs 协议客户端统一承担（端点与信封不在此处
    /// 重写），本方法只做 UI 语义转译，投递两个回调通道：
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
        info!(
            "POST {}/api/v1/agent/run/stream (agent.run_stream, prompt_len={}, session={})",
            self.base_url,
            prompt.len(),
            session_id
        );

        // v1 事件消费状态（本方法内局部；渲染状态经回调推给 UI）
        let mut final_content: Option<String> = None;
        let mut text_acc = String::new();
        let mut err_msg: Option<String> = None;
        let mut tokens: Option<u64> = None;
        // tool_end 只带 tool_id：以本地映射回填工具名（渲染需要动作短语）
        let mut tool_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        run_stream_events(&self.rs, params, |ev| {
            match ev.event_type.as_str() {
                // token_delta：增量文本 → 打字机实时渲染 + 本地累积
                gen::AIRY_RS_TYPE_TOKEN_DELTA => {
                    if let Some(d) = ev.data_str(gen::AIRY_RS_K_DELTA) {
                        if !d.is_empty() {
                            text_acc.push_str(d);
                            on_text(d);
                        }
                    }
                }
                // tool_start / tool_end：工具进度行（克制：只显示动作名与
                // 成败，不暴露参数与结果内容）
                gen::AIRY_RS_TYPE_TOOL_START => {
                    let tool = ev
                        .data_str(gen::AIRY_RS_K_TOOL)
                        .unwrap_or("tool")
                        .to_string();
                    if let Some(tid) = ev.data_str(gen::AIRY_RS_K_TOOL_ID) {
                        tool_names.insert(tid.to_string(), tool.clone());
                    }
                    let line = serde_json::json!({
                        "__airy_evt": "tool_call",
                        "tool": tool,
                    });
                    on_tool(&line.to_string());
                }
                gen::AIRY_RS_TYPE_TOOL_END => {
                    let tid = ev
                        .data_str(gen::AIRY_RS_K_TOOL_ID)
                        .unwrap_or("")
                        .to_string();
                    let tool = tool_names.get(&tid).cloned().unwrap_or_else(|| {
                        if tid.is_empty() {
                            "tool".to_string()
                        } else {
                            tid.clone()
                        }
                    });
                    let ok = ev.data_str(gen::AIRY_RS_K_STATUS) == Some("ok");
                    // 失败附引擎结果预览首行（≤80 字符）便于诊断
                    let mut line = serde_json::json!({
                        "__airy_evt": "tool_result",
                        "tool": tool,
                        "ok": if ok { 1 } else { 0 },
                    });
                    if !ok {
                        if let Some(summary) = ev.data_str(gen::AIRY_RS_K_RESULT_HASH) {
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
                gen::AIRY_RS_TYPE_MESSAGE => {
                    if let Some(c) = ev.data_str(gen::AIRY_RS_K_CONTENT) {
                        if !c.is_empty() {
                            final_content = Some(c.to_string());
                        }
                    }
                    if let Some(r) = ev.data_str(gen::AIRY_RS_K_REASONING) {
                        if !r.is_empty() {
                            let line = serde_json::json!({
                                "__airy_evt": "reasoning",
                                "content": r,
                            });
                            on_tool(&line.to_string());
                        }
                    }
                }
                // plan：引擎目标计划（DAG 已由 GRAD 确认阶段展示，此处克制
                // 不进对话区，仅入日志）
                gen::AIRY_RS_TYPE_PLAN => {
                    debug!("agent.run_stream: plan event (ignored, GRAD DAG shown)");
                }
                // error：结构化错误帧 → Err（UI 只呈现可读消息）
                gen::AIRY_RS_TYPE_ERROR => {
                    if let Some(m) = ev.data_str(gen::AIRY_RS_K_MSG) {
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
                gen::AIRY_RS_TYPE_RUN_END => {
                    tokens = ev.data_i64(gen::AIRY_RS_K_USE_TICKS).map(|t| t as u64);
                    let status = ev.data_str(gen::AIRY_RS_K_STATUS).unwrap_or("completed");
                    debug!("agent.run_stream: run_end (status={})", status);
                    // B2（0.1.18）V2.1：引擎三段耗时（think/llm/tool，毫秒）
                    // 随 run_end 透传。首字延迟不达标时，这三项即归因证据
                    // （区分模型长思考 / 网关串行 / 工具阻塞）；旧引擎无此
                    // 三键时不记录，保持日志安静。
                    let seg = |k: &str| ev.data_i64(k);
                    if let (Some(think), Some(llm), Some(tool)) = (
                        seg(gen::AIRY_RS_K_THINK_MS),
                        seg(gen::AIRY_RS_K_LLM_MS),
                        seg(gen::AIRY_RS_K_TOOL_MS),
                    ) {
                        info!(
                            "agent.run_stream: 分段耗时 think_ms={think} llm_ms={llm} \
                             tool_ms={tool} total_ms={}",
                            ev.data_i64(gen::AIRY_RS_K_DURATION).unwrap_or(-1)
                        );
                    }
                }
                // 其余（run_start / 未知）：宽容忽略（§2.4.4）
                _ => {}
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

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
}

/// T-20：单条 SSE 帧缓冲上限（1 MiB）。hall 事件为紧凑 JSON，正常远小于此值；
/// 若网关推送异常（长流无 "\n\n" 分隔），无界累积会导致内存耗尽。
pub(super) const MAX_SSE_BUFFER: usize = 1 << 20;

/// T-20：将 chunk 追加进 SSE 帧缓冲；追加后总量超限时丢弃半帧并从零累积
/// （不完整帧本就无法解析，保持内存有界即协议自愈）。
pub(super) fn sse_buf_extend(buf: &mut Vec<u8>, chunk: &[u8]) {
    if buf.len() + chunk.len() > MAX_SSE_BUFFER {
        buf.clear();
    }
    buf.extend_from_slice(chunk);
}

/// 定位 SSE 帧结束位置（首个空行 "\n\n"，含末尾分隔符）。
pub(super) fn sse_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2)
}
