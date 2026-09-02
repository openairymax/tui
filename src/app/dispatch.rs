// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后台请求派发：pending 任务启动、连接后回调、工具事件渲染。

use super::*;

impl App {
    /// 分派一个后台 LLM 请求：已连接直接发起；未连接先检查连接，通过后继续。
    pub(super) fn dispatch(&mut self, kind: PendingKind, prompt: &str) {
        self.dispatch_with_agent(kind, prompt, None, None, None);
    }

    /// 分派一个后台请求，可携带 agent 编排 spec（任务执行场景：gateway
    /// 依据 params.agent 走 spawn+invoke 编排分支，而非纯 LLM 工具循环）。
    ///
    /// `history`：完整对话历史（OpenAI messages 数组，含当前输入作为末条
    /// user 消息）；携带时 gateway 以整个数组作为工具循环初始上下文。
    /// `gccp_answers`：GCCP 两段式交互第二段答案 JSON（可 None）。
    pub(super) fn dispatch_with_agent(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
        gccp_answers: Option<String>,
    ) {
        log::trace!(
            "dispatch: {:?}（prompt_len={}，connected={}，agent={}，history={}，gccp_answers={}）",
            kind,
            prompt.len(),
            self.connected,
            agent.is_some(),
            history.is_some(),
            gccp_answers.is_some()
        );
        if self.connected {
            self.start_pending(kind, prompt, agent, history, gccp_answers);
        } else {
            self.start_connect_then(kind, prompt, agent, history, gccp_answers);
        }
    }

    /// 发起后台 LLM 请求（网关调用在 tokio 任务中执行，不阻塞事件循环渲染）。
    ///
    /// `agent`：携带时请求经 agent_d 编排执行（spawn+invoke），否则纯 LLM 对话。
    /// `history`：完整对话历史（OpenAI messages 数组），随请求透传 gateway。
    /// `gccp_answers`：GCCP 两段式交互第二段答案 JSON（可 None，透传 think.process）。
    pub(super) fn start_pending(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
        gccp_answers: Option<String>,
    ) {
        let gateway = self.gateway.clone();
        let agent_file = self.agent_file.clone();
        let prompt = prompt.to_string();
        // 当前模型（/model 设置）随请求下发；空则不携带，由网关/llm_d 回落默认
        let model = if self.model.is_empty() {
            None
        } else {
            Some(self.model.clone())
        };
        // 客户端预分配会话 ID：Ctrl+X 中止时凭此调用 gateway agent.cancel
        let session_id = self.new_session_id();
        let sid_for_task = session_id.clone();
        let prompt_len = prompt.len();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let r = gateway
                .send_message(&prompt, &agent_file, model.as_deref(), Some(&sid_for_task), agent, history, gccp_answers.as_deref())
                .await;
            let _ = tx.send(PendingOutcome::Run(r));
        });
        log::info!(
            "start_pending: 后台请求已发起（session={}，prompt_len={}，kind={:?}）",
            session_id,
            prompt_len,
            kind
        );
        self.loading = true;
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: None,
            tool_rx: None,
            streamed: false,
            finish: None,
        });
    }

    /// 发起流式对话请求（普通对话路径）：SSE 增量块经 channel 逐块渲染，
    /// 完整结果经 oneshot 回传后按 ChatRound 相同逻辑应用。
    pub(super) fn start_stream_pending(&mut self, kind: PendingKind, messages: serde_json::Value) {
        let gateway = self.gateway.clone();
        let model = if self.model.is_empty() {
            None
        } else {
            Some(self.model.clone())
        };
        let session_id = self.new_session_id();
        // 流式通道：tokio 任务把 SSE 块逐块送进 mpsc，主循环 poll_pending 消费；
        // 工具事件（tool_call/tool_result）走独立通道渲染工具状态行
        let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tool_tx, tool_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = gateway
                .stream_chat(
                    messages,
                    model.as_deref(),
                    |chunk| {
                        let _ = stream_tx.send(chunk.to_string());
                    },
                    |evt| {
                        let _ = tool_tx.send(evt.to_string());
                    },
                )
                .await;
            let _ = tx.send(PendingOutcome::Run(result.map(|full| RunResponse {
                session_id: String::new(),
                response: full,
                tokens_used: None,
                cost_usd: None,
                thinking: None,
                tool_trace: None,
                gccp_need_interaction: false,
                gccp_questions: Vec::new(),
            })));
        });
        log::info!("start_stream_pending: 流式请求已发起（session={}）", session_id);
        self.loading = true;
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: Some(stream_rx),
            tool_rx: Some(tool_rx),
            streamed: true,
            finish: None,
        });
        // 占位消息：流式输出目标（chat.rs 按 streaming_text 增量渲染）
        self.streaming_text.clear();
        self.streaming_reveal = 0;
        self.stream_reasoning.clear();
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        self.pending_reasoning = None;
        self.last_reveal_tick = Instant::now();
        self.stream_tool_events.clear();
    }

    /// 未连接时的连接检查：后台执行健康检查，通过后继续真实请求。
    pub(super) fn start_connect_then(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
        gccp_answers: Option<String>,
    ) {
        log::debug!("start_connect_then: 未连接，先做健康检查后继续（kind={:?}）", kind);
        let gateway = self.gateway.clone();
        let prompt = prompt.to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let ok = match gateway.health_check().await {
                Ok(h) => h.status == "healthy" || h.status == "ok",
                Err(_) => false,
            };
            let _ = tx.send(PendingOutcome::Connect(ok));
        });
        self.loading = true;
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind: PendingKind::CheckConnect {
                kind: Box::new(kind),
                prompt,
                agent,
                history,
                gccp_answers,
            },
            task: Some(task),
            session_id: String::new(),
            stream_rx: None,
            tool_rx: None,
            streamed: false,
            finish: None,
        });
    }

    /// 发起 agent.run_stream 事件流请求（0.1.9 M5 W1，任务执行轮）。
    ///
    /// 经 gateway POST /api/v1/agent/run/stream 消费 §2.4 v1 事件帧：
    /// token_delta → 打字机实时渲染（stream_rx）；工具进度/思考链/错误 →
    /// 工具事件通道（tool_rx）；完整结果经 oneshot 回传后按 GradConfirm
    /// 语义 apply（执行轮最终以 message 帧 content 为准）。
    pub(super) fn start_run_stream_pending(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
        gccp_answers: Option<String>,
    ) {
        let gateway = self.gateway.clone();
        let agent_file = self.agent_file.clone();
        let prompt = prompt.to_string();
        let model = if self.model.is_empty() {
            None
        } else {
            Some(self.model.clone())
        };
        let session_id = self.new_session_id();
        let sid_for_task = session_id.clone();
        let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tool_tx, tool_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = gateway
                .run_stream_turn(
                    &prompt,
                    &agent_file,
                    model.as_deref(),
                    &sid_for_task,
                    agent,
                    history,
                    gccp_answers.as_deref(),
                    |chunk| {
                        let _ = stream_tx.send(chunk.to_string());
                    },
                    |evt| {
                        let _ = tool_tx.send(evt.to_string());
                    },
                )
                .await;
            let _ = tx.send(PendingOutcome::Run(result));
        });
        log::info!(
            "start_run_stream_pending: agent.run_stream 已发起（session={}，kind={:?}）",
            session_id,
            kind
        );
        self.loading = true;
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: Some(stream_rx),
            tool_rx: Some(tool_rx),
            streamed: true,
            finish: None,
        });
        // 占位清场：打字机/思考链/工具行本轮从零开始
        self.streaming_text.clear();
        self.streaming_reveal = 0;
        self.stream_reasoning.clear();
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        self.pending_reasoning = None;
        self.stream_tool_events.clear();
        self.stream_error = None;
    }

    /// 动作短语映射（与 C 版 airy_cli cli_tool_action 对齐）：对话只展示
    /// "正在做什么"，不暴露工具参数与返回内容；未知工具保留原名。
    pub(super) fn tool_action(tool: &str) -> String {
        let action = match tool {
            "web_search" => "搜索网络",
            "web_fetch" => "抓取网页",
            "fs_read" => "读取文件",
            "fs_write" => "写入文件",
            "fs_list" | "fs_ls" => "列出目录",
            "fs_info" => "查看文件信息",
            "fs_mkdir" => "创建目录",
            "fs_rm" => "删除文件",
            "agent.spawn" => "派生智能体",
            "agent.invoke" => "调用智能体",
            "think.depth" => "深度思考",
            "memory.get" => "读取记忆",
            "memory.put" => "写入记忆",
            _ => return tool.to_string(),
        };
        action.to_string()
    }

    /// 将 SSE 工具事件（__airy_evt JSON）渲染为对话内的工具状态行。
    /// 过程化（2026-08-17）：只展示"正在做什么"（动作名），不暴露工具
    /// 参数与返回内容（代码/URL/文件内容等操作细节保留在日志与模型上下文）。
    /// tool_call  → `[Sub <tool> Agent] <动作>…`
    /// tool_result→ `[Sub <tool> Agent] <动作> 完成` / `<动作>（失败）[: 短错误]`
    pub(super) fn render_tool_event(evt_json: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(evt_json).ok()?;
        let kind = v.get("__airy_evt")?.as_str()?;
        let tool = v.get("tool").and_then(|t| t.as_str()).unwrap_or("tool");
        let action = Self::tool_action(tool);
        match kind {
            "tool_call" => Some(format!("{} {}…", tool, action)),
            "tool_result" => {
                let ok = v.get("ok").and_then(|o| o.as_i64()).unwrap_or(0) != 0;
                let summary = v
                    .get("summary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if ok {
                    Some(format!("{} {} 完成", tool, action))
                } else {
                    // 失败附首行短错误（≤80 字符），便于诊断；成功不回传内容
                    let err: String = summary
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                    Some(format!("{} {}（失败）{}", tool, action,
                                 if err.is_empty() { String::new() } else { format!(" · {err}") }))
                }
            }
            _ => None,
        }
    }

    /* ==================== 多会话 tab（2026-08-21） ==================== */
}
