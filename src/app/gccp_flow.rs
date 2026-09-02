// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP 任务事实确认与 GRAD 双思考：澄清、计划、确认、收尾与技能沉淀。

use super::*;

impl App {
    /// 大任务集启动：进入任务事实确认（GCCP），提出第 1 问。
    pub(super) fn start_gccp(&mut self, goal: &str) {
        self.task_mode = true;
        self.gccp.reset();
        self.gccp.goal = goal.to_string();
        self.set_flow_phase(FlowPhase::GccpRound(1));
        log::info!("start_gccp: 任务事实确认启动（goal={}）", goal);
        self.add_message(
            MessageRole::System,
            "检测到大任务集，进入「任务事实确认」（GCCP）阶段（共 5 问，逐一询问，每问之间我会先思考再提问）。".to_string(),
        );
        self.ask_gccp_round(1);
    }

    /// GCCP 两段式交互第二段（P-A）：处理目标澄清轮的用户作答，携带
    /// gccp_answers 重发同一 prompt 完成澄清闭环。
    ///
    /// 输入语义：
    ///   - 「跳过」/「skip」：放弃本轮问答，以空答案（{}）重发——引擎
    ///     走降级确认继续（不与第一段无答案重发混淆，避免无限交互循环）；
    ///   - 普通输入：逐行对应未回答的问题（支持 "1: xxx" 序号前缀），
    ///     收集为 {"<question_id>":"<answer>",...} 答案 JSON。
    pub(super) fn gccp_clarify_answer(&mut self, input: &str) -> Result<()> {
        let Some(pending) = self.gccp_pending.take() else {
            self.set_flow_phase(FlowPhase::Chat);
            return Ok(());
        };
        let t = input.trim();
        let answers_json = if t.eq_ignore_ascii_case("跳过") || t.eq_ignore_ascii_case("skip") {
            log::info!("gccp_clarify_answer: 用户放弃问答，以空答案重发（引擎降级确认）");
            "{}".to_string()
        } else {
            let mut answers: std::collections::BTreeMap<String, serde_json::Value> =
                pending.answers.clone();
            let lines: Vec<String> = gccp::parse_answers(t)
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .collect();
            let unanswered: Vec<&GccpQuestion> = pending
                .questions
                .iter()
                .filter(|q| !answers.contains_key(&q.id))
                .collect();
            for (i, q) in unanswered.iter().enumerate() {
                if let Some(a) = lines.get(i) {
                    answers.insert(q.id.clone(), serde_json::Value::String(a.clone()));
                }
            }
            serde_json::to_string(&answers).unwrap_or_else(|_| "{}".to_string())
        };
        self.add_message(
            MessageRole::System,
            "已收到目标澄清答案，继续处理…".to_string(),
        );
        // 重发同一请求（第二段）：原始用户输入作为 ChatRound.input（保持
        // 记忆/模式判定语义），增强 prompt + 历史保持一致，答案透传 gateway。
        self.dispatch_with_agent(
            PendingKind::ChatRound { input: pending.raw_input },
            &pending.prompt,
            None,
            pending.history,
            Some(answers_json),
        );
        Ok(())
    }

    /// 向 LLM 请求生成指定轮次的问题并展示（round = 1..=5，每轮只问 1 个问题）。
    pub(super) fn ask_gccp_round(&mut self, round: u8) {
        if !(1..=5).contains(&round) {
            return;
        }
        let prompt = gccp::build_qn_prompt(&self.gccp, round);

        // 本轮问题生成后，进入对应作答阶段
        self.set_flow_phase(FlowPhase::GccpRound(round));

        self.dispatch(PendingKind::AskGccp { round }, &prompt);
    }

    /// 应用 GCCP 提问轮的 LLM 结果：解析问题并提示用户作答。
    pub(super) fn apply_ask_gccp(&mut self, round: u8, res: Result<RunResponse>) {
        match res {
            Ok(r) => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
                // 解析本轮问题并存入 GccpState
                for (n, q) in gccp::parse_questions(&r.response) {
                    match n {
                        1 => self.gccp.q1 = q.clone(),
                        2 => self.gccp.q2 = q.clone(),
                        3 => self.gccp.q3 = q.clone(),
                        4 => self.gccp.q4 = q.clone(),
                        5 => self.gccp.q5 = q.clone(),
                        _ => {}
                    }
                }
                self.add_message(MessageRole::Agent, r.response.clone());
                if let Err(e) = self.memory.push("assistant", &r.response, "task") {
                    log::warn!("memory push(assistant) failed: {}", e);
                }
                self.add_message(
                    MessageRole::System,
                    format!("请输入第 {} 问的回答：", round),
                );
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("GCCP 提问失败：{}", e));
            }
        }
    }

    /// GCCP 第 N 轮：用户回答第 N 问 → LLM 思考 → 提出下一问；第 5 问回答后生成 GRAD。
    pub(super) fn gccp_round_n(&mut self, round: u8, input: &str) -> Result<()> {
        let ans = gccp::parse_answers(input);
        if ans.is_empty() {
            self.add_message(MessageRole::System, "回答不能为空，请重新输入。".to_string());
            return Ok(());
        }
        let answer = ans.first().cloned().unwrap_or_default();
        match round {
            1 => self.gccp.a1 = answer,
            2 => self.gccp.a2 = answer,
            3 => self.gccp.a3 = answer,
            4 => self.gccp.a4 = answer,
            _ => self.gccp.a5 = answer,
        }

        if round >= 5 {
            // 五问齐备 → 生成 GRAD 任务流程图
            self.set_flow_phase(FlowPhase::GradConfirm);
            log::info!("gccp_round_n: 五问齐备，生成 GRAD 任务流程图");
            let prompt = gccp::build_grad_prompt(&self.gccp);
            self.dispatch(PendingKind::GradPlan, &prompt);
        } else {
            // LLM 基于回答思考后提出下一个问题（逐一询问）
            self.ask_gccp_round(round + 1);
        }
        Ok(())
    }

    /// 应用 GRAD 流程图生成结果（五问齐备后）。
    pub(super) fn apply_grad_plan(&mut self, res: Result<RunResponse>) {
        match res {
            Ok(r) => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
                self.gccp.grad_plan = r.response.trim().to_string();
                // 结构化 DAG 依赖图：解析 [DAG] 块（失败降级为纯文本流程图，不影响流程）
                if let Some(dag) = gccp::parse_dag(&r.response) {
                    self.gccp.dag = Some(dag);
                } else {
                    self.gccp.dag = None;
                }
                // 节点状态数组随 DAG 就绪（mark_all_running/done 才有载体，
                // 2.3.9 层级可视化：Executing 阶段逐节点着色）
                self.gccp.init_node_states();
                self.add_message(MessageRole::Agent, r.response.clone());
                if let Err(e) = self.memory.push("assistant", &r.response, "task") {
                    log::warn!("memory push(assistant) failed: {}", e);
                }
                self.add_message(
                    MessageRole::System,
                    "请确认「任务流程图」（GRAD）：输入「确认」开始执行，或输入修改意见。".to_string(),
                );
                log::info!(
                    "apply_grad_plan: GRAD 已生成（grad_plan_len={}，dag={}）",
                    self.gccp.grad_plan.len(),
                    self.gccp.dag.as_ref().map(|d| d.node_count()).unwrap_or(0)
                );
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("GRAD 生成失败：{}", e));
            }
        }
    }

    /// GRAD：确认流程图后开始执行；否则按反馈修订流程图。
    pub(super) fn grad_confirm(&mut self, input: &str) -> Result<()> {
        if gccp::is_confirm(input) {
            self.set_flow_phase(FlowPhase::Executing);
            log::info!("grad_confirm: 流程图已确认，开始执行任务集");
            self.add_message(MessageRole::System, "任务流程图已确认，开始执行任务集。".to_string());
            // 节点进入执行中（P2-C：Executing 阶段 DAG 持续渲染）
            self.gccp.mark_all_running();

            // 注入目标 + 已确认事实 + 流程图，LLM 开始执行。
            // 任务执行必须携带 agent 编排 spec：gateway 依据 params.agent 走
            // spawn+invoke（agent_d）真实调度，否则只会进入纯 LLM 工具循环。
            let prompt = gccp::build_execute_prompt(&self.gccp);
            let agent_spec = serde_json::json!({ "role": "coding" });
            // 执行轮同样携带对话历史（GCCP 确认过程），编排分支可引用上下文
            let history = self.build_history_messages(&prompt);
            // 0.1.9 M5 W1：执行轮切 agent.run_stream v1 事件流（协议先行）——
            // token 增量/工具进度/思考链/结构化错误实时渲染，替代一次性
            // agent.run 的"静默等待"。引擎事件经 gateway 纯翻译后逐帧消费。
            self.start_run_stream_pending(
                PendingKind::GradConfirm { confirmed: true },
                &prompt,
                Some(agent_spec),
                history,
                None,
            );
        } else {
            // 用户反馈 → LLM 修订流程图（修订轮不走 agent 编排，保持对话式修订）
            let prompt = format!(
                "用户对任务流程图（GRAD）的反馈：{}\n请基于反馈修订流程图，以 [GRAD] 开头，\
                 包含任务目标、执行步骤与验收标准。",
                input
            );
            self.dispatch(PendingKind::GradConfirm { confirmed: false }, &prompt);
        }
        Ok(())
    }

    /// 应用 GRAD 确认/修订轮的 LLM 结果。
    pub(super) fn apply_grad_confirm(&mut self, confirmed: bool, res: Result<RunResponse>) {
        // 0.1.9 M5 W1：执行轮（confirmed=true）已切 agent.run_stream 事件流，
        // 先落定流式尾段（工具行/思考链/打字机占位）；error 事件 → Err 呈现
        let stream_err = if confirmed {
            self.settle_stream_tail()
        } else {
            None
        };
        let res = match (stream_err, res) {
            (Some(msg), Ok(_)) => Err(anyhow::anyhow!("{}", msg)),
            (_, other) => other,
        };
        match res {
            Ok(r) => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
                if confirmed {
                    if let Some(c) = r.cost_usd {
                        self.cost += c;
                    }
                    let cleaned = gccp::strip_task_done(&r.response);
                    self.add_message(MessageRole::Agent, cleaned.clone());
                    // 思考链副本随执行轮回复持久化（与 apply_chat_result 对齐，
                    // 2.1.1.6：思考 token 不丢失）
                    let reasoning = self.pending_reasoning.take();
                    if let Err(e) = self.memory.push_with_reasoning(
                        "assistant",
                        &cleaned,
                        reasoning.as_deref(),
                        "task",
                    ) {
                        log::warn!("memory push(assistant) failed: {}", e);
                    }
                    if gccp::has_task_done_marker(&r.response) {
                        self.complete_task();
                    }
                } else {
                    self.gccp.grad_plan = r.response.trim().to_string();
                    self.add_message(MessageRole::Agent, r.response.clone());
                    if let Err(e) = self.memory.push("assistant", &r.response, "task") {
                        log::warn!("memory push(assistant) failed: {}", e);
                    }
                    self.add_message(
                        MessageRole::System,
                        "已修订流程图，请再次确认：输入「确认」开始执行。".to_string(),
                    );
                }
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("Error: {}", e));
            }
        }
    }

    /// 任务成功收尾：自动提炼经验 → 沉淀为本地技能 → 回到对话模式。
    ///
    /// 这是 Skills 本地技能库的核心闭环：任务成功后不"用过即忘"，
    /// 而是将本次执行过程交给 LLM 提炼为可复用技能存入本地库，
    /// 后续任务在 build_context_prompt 中召回匹配技能，Agent 越用越强。
    pub(super) fn complete_task(&mut self) {
        log::info!("complete_task: 任务完成信号触发，开始经验蒸馏");
        // DAG 节点全部完成（P2-C 过程可视化收尾）
        self.gccp.mark_all_done();
        let recent = self.memory.recent(40);
        let conv: Vec<String> = recent
            .iter()
            .rev()
            .map(|r| format!("{}: {}", r.role, r.content))
            .collect();
        let conv_text = conv.join("\n");

        if conv_text.trim().is_empty() {
            self.finish_task(false);
        } else {
            let prompt = skills::build_distill_prompt(&conv_text);
            self.dispatch(PendingKind::Distill, &prompt);
        }
    }

    /// 任务收尾（技能蒸馏结果应用后）。
    pub(super) fn finish_task(&mut self, distilled: bool) {
        self.task_mode = false;
        self.set_flow_phase(FlowPhase::Chat);
        self.set_task_control(TaskControl::Running);
        self.gccp.dag = None;
        log::info!("finish_task: 任务收尾完成（distilled={}）", distilled);
        if !distilled {
            self.add_message(
                MessageRole::System,
                "任务已完成（技能沉淀跳过或失败，详见日志）。".to_string(),
            );
        }
    }

    /// 应用技能蒸馏轮的 LLM 结果。
    pub(super) fn apply_distill_result(&mut self, res: Result<RunResponse>) {
        let distilled = match res {
            Ok(r) => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
                match skills::parse_distilled_skill(&r.response) {
                    Some(skill) => match self.skills.save(skill.clone()) {
                        Ok(()) => {
                            self.add_message(
                                MessageRole::System,
                                format!(
                                    "任务完成。经验已自动沉淀为可复用技能「{}」，\
                                     本地技能库现有 {} 条（Agent 可用能力 +1）。",
                                    skill.name,
                                    self.skills.len()
                                ),
                            );
                            true
                        }
                        Err(e) => {
                            log::warn!("skills: save failed: {}", e);
                            false
                        }
                    },
                    None => {
                        log::warn!("skills: distill output not parseable");
                        false
                    }
                }
            }
            Err(e) => {
                log::warn!("skills: distill call failed: {}", e);
                false
            }
        };
        self.finish_task(distilled);
    }

    /* ==================== 后台 LLM 请求机制 ==================== */
}
