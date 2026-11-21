// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后台请求派发：运行轮参数装配、工具事件渲染。

use super::lane::TurnArgs;
use super::*;

impl App {
    /// 分派一个后台 LLM 请求（统一走事件管道）。
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
        self.start_turn(
            kind,
            prompt,
            TurnArgs {
                messages: history,
                gccp_answers,
                agent,
                ..TurnArgs::default()
            },
        );
    }

    /// 发起流式对话轮请求（统一任务编排入口，C-15）：所有普通对话轮经
    /// 事件管道下发（服务端裁决 chat/task），与 CLI 同源。对话不再由
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
        // goal 取用户原文（与 CLI 同一语义）：增强 prompt 仅作 messages 末条
        // 进入 opts，避免把客户端自带的系统提示当成目标文本交给服务端裁决。
        // 续跑轮不带历史，goal 保持为空，上下文全部来自令牌。
        let goal = match (&kind, &resume) {
            (PendingKind::StreamRound { input }, None) if !input.trim().is_empty() => {
                input.clone()
            }
            _ => prompt,
        };
        self.start_turn(
            kind,
            &goal,
            TurnArgs {
                messages,
                resume,
                ..TurnArgs::default()
            },
        );
    }

    /// 流式渲染态归零：新轮开始前清空上一轮的增量、工具行与思考链残留。
    pub(super) fn clear_stream(&mut self) {
        self.streaming_text.clear();
        self.stream_sanitizer.reset();
        self.stream_reasoning.clear();
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        self.last_reasoning = None;
        self.stream_tool_events.clear();
    }

    /// D-12：向网关查询暂停回合，取回重启后仍可续跑的令牌。
    ///
    /// 观察面读走查询帧（D-15）：`agent.turns` 经事件管道下发，应答由
    /// poll_pending 摘取后按 TurnState 落定；本地不再持有网关客户端调用。
    pub(super) fn start_turns_pending(&mut self) {
        log::info!("start_turns_pending: 查询 agent.turns 断点");
        match self.send_query("turns.get", "agent.turns", &serde_json::json!({})) {
            Ok(()) => {
                // 占位句柄：TurnState 的结果来自查询应答而非 oneshot，rx
                // 只为满足 PendingTurn 形状；该通道不参与落定判定。
                let (_tx, rx) = tokio::sync::oneshot::channel();
                self.pending = Some(PendingTurn {
                    rx,
                    kind: PendingKind::TurnState,
                    task: None,
                    session_id: String::new(),
                    stream_rx: None,
                    tool_rx: None,
                });
                self.begin_busy();
                self.set_task_control(TaskControl::Running);
            }
            Err(e) => self.apply_turn_state(Err(anyhow!(e))),
        }
    }

    /// 发起任务执行轮（C-15 统一入口）：GRAD 确认后带 `mode="task"` 与
    /// `gccp_confirmed=true` 通过服务端目标门禁；计划/子目标/状态迁移由
    /// 事件投影上屏，不再依赖正文 [MODE:TASK] 标记。
    pub(super) fn start_run_stream_pending(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        history: Option<serde_json::Value>,
        gccp_answers: Option<String>,
    ) {
        self.start_turn(
            kind,
            prompt,
            TurnArgs {
                messages: history,
                gccp_answers,
                confirmed: true,
                mode: Some("task"),
                ..TurnArgs::default()
            },
        );
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
