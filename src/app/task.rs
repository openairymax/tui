// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话与任务提交：输入分发、回合记录、流式与阻塞结果落盘。

use super::*;

impl App {
    /// 切换任务流阶段并记录日志。
    ///
    /// 阶段迁移是排查任务流问题的关键节点：Chat → GCCP → GRAD → Executing，
    /// 每次迁移都留下 info 级埋点（含来源/目标阶段与中文名），便于事后回溯。
    pub(super) fn set_flow_phase(&mut self, to: FlowPhase) {
        let from = self.flow_phase;
        if from != to {
            log::info!(
                "flow_phase: {:?} -> {:?}（{}）",
                from,
                to,
                to.label()
            );
            self.flow_phase = to;
        }
    }

    /// 切换任务控制状态并记录日志。
    ///
    /// 人工中止（Ctrl+X）/ 暂停（Ctrl+Z）/ 恢复 / 自然完成都会经过此处，
    /// 状态迁移是任务控制问题排查的关键节点。
    pub(super) fn set_task_control(&mut self, to: TaskControl) {
        let from = self.task_control;
        if from != to {
            log::info!(
                "task_control: {:?} -> {:?}（{}）",
                from,
                to,
                to.label()
            );
            self.task_control = to;
        }
    }

    /// Submit the current input as a message.
    ///
    /// 任务流分派（同步；网络调用转入后台任务，由主循环 poll_pending 轮询）：
    ///   - Chat / Executing：普通对话；任务集执行中检测"完成"信号 → 沉淀技能
    ///   - GccpRound1/2/3：任务事实确认（GCCP）各轮用户回答
    ///   - GradConfirm：任务流程图（GRAD）确认
    pub fn submit_input(&mut self, input: &str) -> Result<()> {
        let input = input.trim().to_string();

        if input.is_empty() {
            return Ok(());
        }

        // 发送新消息时回到最新位置，确保新对话立即可见
        self.scroll_offset = 0;

        // /hiairy：随时重开首次启动向导（命令不发送给 LLM）
        if input.eq_ignore_ascii_case("/hiairy") {
            self.open_wizard();
            return Ok(());
        }

        // /model：查看或设置当前模型（命令不发送给 LLM）
        let lower = input.to_ascii_lowercase();
        if lower == "/model" || lower.starts_with("/model ") {
            self.cmd_model(&input);
            return Ok(());
        }

        // /set-key：便捷写入 secrets.env 中的模型 API Key（F2 配置面板编辑入口）
        if lower == "/set-key" || lower.starts_with("/set-key ") {
            self.cmd_set_key(&input);
            return Ok(());
        }

        // ── 本地命令（不发送给 LLM，纯前端即时响应）──
        if lower == "/help" {
            self.active_panel = ActivePanel::Help;
            return Ok(());
        }
        if lower == "/clear" {
            self.messages.clear();
            self.streaming_text.clear();
            self.streaming_reveal = 0;
            self.stream_reasoning.clear();
            self.stream_reasoning_model.clear();
            self.stream_reasoning_start = None;
            self.add_message(
                MessageRole::System,
                "对话已清空。输入 /help 查看可用命令。".to_string(),
            );
            self.add_log("INFO", "对话已清空（/clear）".to_string());
            return Ok(());
        }
        if lower == "/status" {
            self.cmd_status();
            return Ok(());
        }
        if lower == "/memory" {
            self.toggle_panel(ActivePanel::Memory);
            return Ok(());
        }
        if lower == "/skills" {
            self.cmd_skills();
            return Ok(());
        }
        if lower == "/board" {
            // 任务看板面板（F6 等价；进入即强制刷新）
            self.active_panel = ActivePanel::Board;
            self.last_hall_poll = Instant::now() - std::time::Duration::from_secs(10);
            return Ok(());
        }
        if lower == "/events" {
            // 事件流面板（F7 等价；进入即强制刷新）
            self.active_panel = ActivePanel::Events;
            self.last_hall_poll = Instant::now() - std::time::Duration::from_secs(10);
            return Ok(());
        }
        if lower == "/chain" || lower.starts_with("/chain ") {
            // 决策链：无参列任务；/chain <task_id> 回放该任务决策链
            self.cmd_chain(&input);
            return Ok(());
        }

        // ── 运维命令（经 gateway RPC，结果异步回填对话区）──
        if lower == "/daemons" {
            self.cmd_daemons();
            return Ok(());
        }
        if lower == "/agents" {
            self.cmd_ops_call("agent.list", serde_json::json!({}));
            return Ok(());
        }
        if lower == "/tools" {
            self.cmd_ops_call("tool.list_tools", serde_json::json!({}));
            return Ok(());
        }
        if lower == "/models" {
            self.cmd_ops_call("llm.list_models", serde_json::json!({}));
            return Ok(());
        }
        if lower == "/mem" || lower.starts_with("/mem ") {
            let arg = input[4..].trim();
            if arg.is_empty() {
                self.cmd_ops_call("mem.count", serde_json::json!({}));
            } else {
                self.cmd_ops_call("mem.search", serde_json::json!({ "query": arg }));
            }
            return Ok(());
        }
        if lower.starts_with("/rpc ") {
            self.cmd_rpc(&input);
            return Ok(());
        }

        // 记录输入历史（Alt+↑/↓ 浏览；去重，最新在后）
        self.push_history(&input);

        // 回合计时开始（结果消费时结算，回合分隔线展示 Worked for Ns）
        self.turn_started = Instant::now();

        // Add user message
        self.add_message(MessageRole::User, input.clone());

        // 多会话：首条用户消息派生会话标题（tab 栏展示）
        let ci = self.current_tab_index();
        if self
            .session_tabs
            .get(ci)
            .map(|t| t.title.is_empty())
            .unwrap_or(false)
        {
            if let Some(tab) = self.session_tabs.get_mut(ci) {
                tab.title = derive_session_title(&input);
            }
        }

        // 记忆：持久化用户输入（普通对话与任务均记录）
        if let Err(e) = self.memory.push("user", &input, "chat") {
            log::warn!("memory push(user) failed: {}", e);
        }

        if input.eq_ignore_ascii_case("exit") {
            self.add_message(MessageRole::System, "Type Ctrl+C to quit.".to_string());
            return Ok(());
        }

        match self.flow_phase {
            FlowPhase::Chat | FlowPhase::Executing => {
                // 任务集执行中：用户宣告完成 → 自动提炼经验并沉淀为技能
                if self.flow_phase == FlowPhase::Executing && gccp::is_task_done_input(&input) {
                    self.complete_task();
                    return Ok(());
                }
                self.chat_round(&input)?;
            }
            FlowPhase::GccpRound(n) => self.gccp_round_n(n, &input)?,
            FlowPhase::GccpClarify => self.gccp_clarify_answer(&input)?,
            FlowPhase::GradConfirm => self.grad_confirm(&input)?,
        }

        Ok(())
    }

    /// 2026-08-17：任务执行期间插入对话（2.3.7）。
    ///
    /// busy 循环中用户输入 Enter 提交 → 先入队（任务不打断），任务完成后
    /// 主循环逐条 pop 并以 submit_input 处理（每条等其完成，单 pending 槽
    /// 不覆盖）。用户消息与回复由 submit_input 统一回显，此处仅记录占位
    /// 提示——体验连续，不割裂。
    pub fn queue_insert_chat(&mut self, input: &str) {
        let input = input.trim().to_string();
        if input.is_empty() {
            return;
        }
        let n = self.insert_queue.len() + 1;
        self.insert_queue.push_back(input);
        self.add_message(
            MessageRole::System,
            format!("（任务执行中，已插入第 {} 条对话，任务完成后自动回复）", n),
        );
    }

    /// 普通对话轮次：发送增强 prompt，并按 LLM 判定的模式切换任务流。
    ///
    /// 普通对话（未连接时先检查；已连接走流式 SSE 增量渲染，Claude 风格）。
    /// 任务执行轮（flow_phase == Executing）走 agent 编排路径（非流式）。
    pub(super) fn chat_round(&mut self, input: &str) -> Result<()> {
        self.turn += 1;

        // 构造增强 prompt：系统指令（LLM 判定任务集）+ 技能 + 记忆 + 项目上下文
        let prompt = self.build_context_prompt(input);
        // 完整对话历史（OpenAI messages 数组，末条为增强 prompt）：
        // 网络层由此获得真实多轮上下文（M1/M2 修复），无需再挤进 prompt 文本。
        let history = self.build_history_messages(&prompt);

        if !self.connected {
            self.dispatch_with_agent(
                PendingKind::ChatRound { input: input.to_string() },
                &prompt,
                None,
                history,
                None,
            );
            return Ok(());
        }

        // 普通对话走流式：SSE 增量渲染（Claude 风格）；任务执行轮保持编排链路
        let messages = history.unwrap_or_else(|| {
            serde_json::json!([{ "role": "user", "content": prompt }])
        });
        self.start_stream_pending(
            PendingKind::StreamRound { input: input.to_string() },
            messages,
        );
        Ok(())
    }

    /// 应用流式对话轮的最终结果（流式结束后按普通对话相同逻辑处理）。
    pub(super) fn apply_stream_result(&mut self, input: String, res: Result<RunResponse>) {
        // 流式工具状态行 → 落为正式消息（先于最终回答；流式路径无 tool_trace，
        // 工具事件仅此一处可见）
        for line in std::mem::take(&mut self.stream_tool_events) {
            self.add_message(MessageRole::ToolCall, line);
        }
        // 思考链（reasoning_content）→ 落为 [Dual Think] 正式消息（折叠展示）。
        // 2.1.1.6：同时保留副本到 pending_reasoning，随 assistant 回复持久化
        // 到记忆库（思考 token 不丢失）。
        if !self.stream_reasoning.is_empty() {
            let reasoning = std::mem::take(&mut self.stream_reasoning);
            self.pending_reasoning = Some(reasoning.clone());
            self.add_message(MessageRole::System, reasoning);
        }
        self.stream_reasoning_model.clear();
        self.stream_reasoning_start = None;
        // 流式结束：把已渲染的 streaming_text 落为正式消息（防止与 result 双写）
        if !self.streaming_text.is_empty() {
            // 内容已实时渲染在占位消息上；此处仅清理占位，避免重复上屏
            self.streaming_text.clear();
        }
        self.streaming_reveal = 0;
        // 0.1.8：本轮收到 gateway 错误帧（llm_d 失败/不可达）→ 把 Ok(空)
        // 转为 Err，复用 apply_chat_result 的失败呈现路径（System 一行摘要
        // + 日志详情），原始 JSON 错误信封永不上屏。
        let res = match (self.stream_error.take(), res) {
            (Some(msg), Ok(_)) => Err(anyhow::anyhow!("{}", msg)),
            (_, other) => other,
        };
        // 复用普通对话的结果应用逻辑（模式判定/技能/记忆/GCCP 入口）
        self.apply_chat_result(input, res);
    }

    /// 应用普通对话轮的 LLM 结果（按 LLM 判定的模式切换任务流）。
    pub(super) fn apply_chat_result(&mut self, input: String, res: Result<RunResponse>) {
        match res {
            Ok(response) => {
                // GCCP 两段式交互第一段（P-A）：think.process 判定输入需要
                // 澄清并挂起，回传问题集（gccp_need_interaction=1）。进入目标
                // 澄清轮：暂存问题集与原始请求，展示问题引导作答；不按普通
                // 回复处理（无 Agent 消息/记忆写入/模式判定——本轮无实质结果）。
                if response.gccp_need_interaction && !response.gccp_questions.is_empty() {
                    let qcount = response.gccp_questions.len();
                    log::info!(
                        "apply_chat_result: GCCP 交互轮（{} 个澄清问题）",
                        qcount
                    );
                    // 重发时保持与第一段一致的 prompt/history（同一任务上下文，
                    // 引擎据此完成目标确认；此处重新构建，两轮间隔内记忆/技能
                    // 状态稳定，产出等价上下文）。
                    let prompt = self.build_context_prompt(&input);
                    let history = self.build_history_messages(&prompt);
                    self.gccp_pending = Some(GccpPending {
                        raw_input: input.clone(),
                        prompt,
                        history,
                        questions: response.gccp_questions,
                        answers: Default::default(),
                    });
                    self.set_flow_phase(FlowPhase::GccpClarify);
                    self.add_message(
                        MessageRole::System,
                        format!(
                            "需要回答 {} 个澄清问题后继续（逐行作答，输入「跳过」放弃本轮问答）",
                            qcount
                        ),
                    );
                    return;
                }

                if let Some(t) = response.tokens_used {
                    self.tokens += t;
                }
                if let Some(c) = response.cost_usd {
                    self.cost += c;
                }

                let (mode, cleaned) = parse_mode_detail(&response.response);

                // 双思考轨迹（GCCP+GRAD）→ 折叠为一行计划摘要，先于工具轨迹展示。
                // 乔布斯式克制：完整 DAG（节点目标/依赖/成本）不下屏，仅给
                // 「N 节点计划 + 首目标」提示，细节见 F3 日志。
                if let Some(th) = &response.thinking {
                    if let Some(summary) = format_thinking_summary(th) {
                        self.add_message(MessageRole::System, summary);
                    }
                }

                // Agent 工具调用轨迹 → 展示为「工具调用/结果」消息（先于最终回答）
                // 乔布斯式克制：大任务集一次可能 10+ 次工具调用，全部上屏会淹没对话，
                // 仅展示前 MAX_VISIBLE_TOOL_TRACES 条，其余折叠为一行摘要（全量见 F3 日志）。
                // 过程化（2026-08-17）：只展示动作名与成败，不暴露参数与结果内容。
                if let Some(trace) = &response.tool_trace {
                    const MAX_VISIBLE_TOOL_TRACES: usize = 4;
                    let total = trace.len();
                    let show = total.min(MAX_VISIBLE_TOOL_TRACES);
                    for t in trace.iter().take(show) {
                        let ok = t.ok.unwrap_or(0) != 0;
                        let action = Self::tool_action(&t.tool);
                        self.add_message(
                            MessageRole::ToolCall,
                            format!("{} {}…{}", t.tool, action, if ok { "" } else { "（失败）" }),
                        );
                        // 成功不回传结果内容（代码/文件全文/URL 等保留在日志）；失败附短错误
                        if !ok {
                            let err: String = t
                                .result
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(120)
                                .collect();
                            if !err.is_empty() {
                                self.add_message(MessageRole::ToolResult, err);
                            }
                        }
                    }
                    if total > show {
                        self.add_message(
                            MessageRole::System,
                            format!(
                                "… 另有 {} 次工具调用已折叠（共 {} 次，全量见 F3 日志）",
                                total - show,
                                total
                            ),
                        );
                    }
                }

                // 空回复占位（2026-08-17）：模型未产生文本回复（thinking 模型
                // 可能只输出 reasoning_content，或 provider 异常）时给出明确提示，
                // 避免对话中出现"空返回"却无任何说明。
                if cleaned.trim().is_empty() {
                    // 丢弃本轮思考链，避免残留给下一轮记录
                    self.pending_reasoning.take();
                    self.add_message(
                        MessageRole::Agent,
                        "（未产生回复：模型可能仅生成了思考内容，请重试）".to_string(),
                    );
                } else {
                    self.add_message(MessageRole::Agent, cleaned.clone());
                    // 记忆：持久化助手响应（2.1.1.6：思考链随记录落盘保留）。
                    // 空回复不写记忆——空记录是记忆污染源（无内容可召回，
                    // 却混入 recent() 蒸馏/上下文，拉低记忆信噪比）。
                    let reasoning = self.pending_reasoning.take();
                    if let Err(e) = self.memory.push_with_reasoning(
                        "assistant",
                        &cleaned,
                        reasoning.as_deref(),
                        "chat",
                    ) {
                        log::warn!("memory push(assistant) failed: {}", e);
                    }
                }

                // 执行阶段 LLM 自报 [TASK:DONE] → 自动沉淀技能
                let llm_done = gccp::has_task_done_marker(&response.response);

                match mode {
                    ModeMarker::TaskGccp => {
                        // 大任务集：进入任务事实确认（GCCP）
                        log::info!("apply_chat_result: LLM 判定大任务集，进入 GCCP");
                        self.start_gccp(&input);
                    }
                    ModeMarker::Task => {
                        log::info!("apply_chat_result: LLM 判定任务集，直接进入执行");
                        self.task_mode = true;
                        self.set_flow_phase(FlowPhase::Executing);
                    }
                    ModeMarker::Chat => {
                        self.task_mode = false;
                        self.set_flow_phase(FlowPhase::Chat);
                    }
                }

                if llm_done && self.flow_phase == FlowPhase::Executing {
                    self.complete_task();
                }
            }
            Err(e) => {
                self.add_log("ERROR", format!("LLM 调用失败：{}", e));
                // 2.3.4：错误详情只进日志（F3 查看）；界面仅显示一句话摘要，
                // 避免错误链（含 HTTP body 原文/内部路径）污染对话区。
                let brief = e.to_string();
                let brief = brief.lines().next().unwrap_or("请求失败");
                let brief: String = brief.chars().take(120).collect();
                self.add_message(MessageRole::System, format!("请求失败：{}", brief));
            }
        }
    }
}
