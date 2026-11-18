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
        // 客户端预分配会话 ID：Ctrl+X 中止时凭此调用 gateway task.cancel
        let session_id = self.new_session_id();
        let sid_for_task = session_id.clone();
        let prompt_len = prompt.len();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let r = gateway
                .send_message(
                    &prompt,
                    &agent_file,
                    model.as_deref(),
                    Some(&sid_for_task),
                    agent,
                    history,
                    gccp_answers.as_deref(),
                )
                .await;
            let _ = tx.send(PendingOutcome::Run(r));
        });
        log::info!(
            "start_pending: 后台请求已发起（session={}，prompt_len={}，kind={:?}）",
            session_id,
            prompt_len,
            kind
        );
        self.begin_busy();
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: None,
            tool_rx: None,
        });
    }

    /// 发起流式对话轮请求（统一任务编排入口，C-15）：所有普通对话轮走
    /// task.submit_stream（服务端裁决 chat/task），与 CLI 同源。对话不再由
    /// 客户端预判链路；长任务在门禁处得到 needs_intent，由 apply 侧进入
    /// 目标确认后带确认重新提交。
    ///
    /// `messages` 为完整对话历史（OpenAI messages 数组，末条 user 为
    /// 增强后 prompt）；prompt 取末条 user content，随 params.goal 透传。
    ///
    /// `resume`（C-14-07）：预算暂停后的断点续跑令牌。Some 时不携带历史
    /// （messages 为 None），引擎按令牌内的步骤摘要续跑，不整轮重放。
    pub(super) fn start_stream_pending(
        &mut self,
        kind: PendingKind,
        messages: Option<serde_json::Value>,
        resume: Option<String>,
    ) {
        let gateway = self.gateway.clone();
        let agent_file = self.agent_file.clone();
        let model = if self.model.is_empty() {
            None
        } else {
            Some(self.model.clone())
        };
        let session_id = self.new_session_id();
        // prompt：末条 user 消息文本（chat_round 构造的增强 prompt）；
        // 续跑时不带历史，prompt 为空，上下文全部来自令牌
        let prompt = messages
            .as_ref()
            .and_then(|m| m.as_array())
            .and_then(|a| {
                a.iter()
                    .rev()
                    .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            })
            .and_then(|m| m.get("content").and_then(|c| c.as_str()))
            .unwrap_or("")
            .to_string();
        // 续跑是已确认任务的继续执行；新对话轮不带 gccp_confirmed
        let confirmed = resume.is_some();
        // 流式通道：tokio 任务把事件帧转译后的文本增量送进 mpsc，主循环
        // poll_pending 消费；工具/思考/错误事件走独立通道渲染
        let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tool_tx, tool_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sid_for_task = session_id.clone();
        let task = tokio::spawn(async move {
            let result = gateway
                .run_task_stream_turn(
                    &prompt,
                    &agent_file,
                    model.as_deref(),
                    &sid_for_task,
                    None,
                    messages,
                    None,
                    resume.as_deref(),
                    resume.as_deref().map(|_| "task"),
                    confirmed,
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
            "start_stream_pending: task.submit_stream 已发起（session={}）",
            session_id
        );
        self.begin_busy();
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: Some(stream_rx),
            tool_rx: Some(tool_rx),
        });
        // 占位消息：流式输出目标（chat.rs 按 streaming_text 增量渲染）
        self.streaming_text.clear();
        self.stream_sanitizer.reset();
        self.stream_reasoning.clear();
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        self.last_reasoning = None;
        self.stream_tool_events.clear();
    }

    /// D-12：向网关查询暂停回合，取回重启后仍可续跑的令牌。
    /// 走后台任务，避免 UI 线程在服务重启窗口被阻塞。
    pub(super) fn start_turns_pending(&mut self) {
        let gateway = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let r = gateway.rpc_call("agent.turns", serde_json::json!({})).await;
            let _ = tx.send(PendingOutcome::Turns(r));
        });
        log::info!("start_turns_pending: 查询 agent.turns 断点");
        self.begin_busy();
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind: PendingKind::TurnState,
            task: Some(task),
            session_id: String::new(),
            stream_rx: None,
            tool_rx: None,
        });
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
        log::debug!(
            "start_connect_then: 未连接，先做健康检查后继续（kind={:?}）",
            kind
        );
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
        self.begin_busy();
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
        });
    }

    /// 发起任务执行轮事件流请求（C-15 统一入口：task.submit_stream）。
    ///
    /// 用户在 GRAD 阶段确认后进入执行：携带 `mode="task"` 与
    /// `gccp_confirmed=true` 通过服务端目标门禁，计划/子目标/状态迁移经
    /// plan/step/task_state 事件渲染（不再依赖正文 [MODE:TASK] 标记）。
    pub(super) fn start_run_stream_pending(
        &mut self,
        kind: PendingKind,
        prompt: &str,
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
                .run_task_stream_turn(
                    &prompt,
                    &agent_file,
                    model.as_deref(),
                    &sid_for_task,
                    None,
                    history,
                    gccp_answers.as_deref(),
                    None,
                    Some("task"),
                    true,
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
            "start_run_stream_pending: task.submit_stream 已发起（session={}，kind={:?}）",
            session_id,
            kind
        );
        self.begin_busy();
        self.set_task_control(TaskControl::Running);
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: Some(task),
            session_id,
            stream_rx: Some(stream_rx),
            tool_rx: Some(tool_rx),
        });
        // 占位清场：思考链/工具行本轮从零开始（B3：净化器状态同步复位）
        self.streaming_text.clear();
        self.stream_sanitizer.reset();
        self.stream_reasoning.clear();
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        self.last_reasoning = None;
        self.stream_tool_events.clear();
        self.stream_error = None;
    }

    /// 将 SSE 工具事件（__airy_evt JSON）渲染为对话内的工具状态行。
    /// D-11（0.1.21）：可见性由 tool.list 真实目录驱动，客户端不再维护
    /// 白名单；未登记工具照常上屏，回退原始 tool_id 与参数摘要——隐藏未知
    /// 工具会让新工具在旧客户端上完全不可见，且「跑了却空白」更难诊断。
    /// tool_call  → `<tool>（<参数摘要>）…`
    /// tool_result→ `<tool> 完成` / `<tool>（失败）[: 短错误]`
    pub(super) fn render_tool_event(evt_json: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(evt_json).ok()?;
        let kind = v.get("__airy_evt")?.as_str()?;
        // C-15：计划/子目标/规划降级事件不带工具名，先于工具动作表渲染
        match kind {
            "plan" => {
                let n = v.get("step_count").and_then(|x| x.as_i64()).unwrap_or(0);
                return Some(format!("已生成 {n} 步执行计划"));
            }
            "plan_step" => {
                let idx = v.get("index").and_then(|x| x.as_i64()).unwrap_or(0) + 1;
                let cnt = v.get("count").and_then(|x| x.as_i64()).unwrap_or(0);
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
                return Some(if text.is_empty() {
                    format!("计划 {idx}/{cnt}")
                } else {
                    format!("计划 {idx}/{cnt}：{text}")
                });
            }
            "plan_unavailable" => {
                let reason = v
                    .get("reason")
                    .and_then(|x| x.as_str())
                    .unwrap_or("unknown");
                return Some(format!(
                    "未生成执行计划（{reason}），按工具循环直接执行"
                ));
            }
            _ => {}
        }
        let raw = v.get("tool").and_then(|t| t.as_str()).unwrap_or("tool");
        // 目录已加载而工具未登记时追加标记（照常上屏，不隐藏）。
        let tool = crate::app::toolcat::label(raw);
        let args = v.get("args").and_then(|a| a.as_str()).unwrap_or("");
        match kind {
            "tool_call" => Some(crate::app::toolcat::event_line(raw, args, true)),
            "tool_result" => {
                let ok = v.get("ok").and_then(|o| o.as_i64()).unwrap_or(0) != 0;
                let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
                if ok {
                    Some(format!("{tool} 完成"))
                } else {
                    // 失败附首行短错误（≤80 字符），便于诊断；成功不回传内容
                    let err: String = summary
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                    Some(format!(
                        "{tool}（失败）{}",
                        if err.is_empty() {
                            String::new()
                        } else {
                            format!(" · {err}")
                        }
                    ))
                }
            }
            _ => None,
        }
    }

    /* ==================== 多会话 tab（2026-08-21） ==================== */
}
