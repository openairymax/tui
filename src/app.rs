// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Application state for the AgentRT TUI.

use anyhow::Result;
use chrono::Local;
use std::collections::VecDeque;
use std::time::Instant;

use crate::client::GatewayClient;
use crate::gccp::{self, FlowPhase, GccpState};
use crate::memory::{self, ConversationMemory};
use crate::skills::{self, SkillStore};

/// Maximum number of chat messages to keep in memory.
const MAX_CHAT_MESSAGES: usize = 500;

/// Maximum number of log entries to keep.
const MAX_LOG_ENTRIES: usize = 200;

/// Active panel for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Chat,
    Help,
    Config,
    Logs,
    Memory,
    Plugins,
}

/// Represents a chat message in the conversation.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MessageRole {
    User,
    Agent,
    System,
    ToolCall,
    ToolResult,
}

/// Represents a log entry.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub daemon: Option<String>,
}

/// Application state.
pub struct App {
    /// Agent file being used
    pub agent_file: String,
    /// Chat messages
    pub messages: VecDeque<ChatMessage>,
    /// User input buffer
    pub input: String,
    /// Currently active panel
    pub active_panel: ActivePanel,
    /// Scroll position in chat
    pub scroll_offset: u16,
    /// Gateway client
    pub gateway: GatewayClient,
    /// Connected status
    pub connected: bool,
    /// Gateway version
    pub gateway_version: Option<String>,
    /// Current turn number
    pub turn: u64,
    /// Total tokens used
    pub tokens: u64,
    /// Total cost in USD
    pub cost: f64,
    /// Elapsed time since session start
    pub session_start: Instant,
    /// Whether MCP is enabled
    pub mcp_enabled: bool,
    /// Whether A2A is enabled
    pub a2a_enabled: bool,
    /// Log entries
    pub logs: VecDeque<LogEntry>,
    /// Help text cached
    pub help_text: Vec<String>,
    /// Config content
    pub config_content: String,
    /// Memory stats
    pub memory_stats: String,
    /// Plugin list
    pub plugin_list: String,
    /// Whether we are currently loading (waiting for response)
    pub loading: bool,
    /// Status message
    pub status_message: String,
    /// LLM 判定的当前模式：true = 任务集（多步任务编排），false = 普通对话
    pub task_mode: bool,
    /// 对话记忆后端（跨会话"记得住"）
    pub memory: Box<dyn ConversationMemory>,
    /// 任务流阶段（对话 / GCCP 任务事实确认 / GRAD 任务流程图确认 / 执行）
    pub flow_phase: FlowPhase,
    /// GCCP 五问状态（任务事实确认）
    pub gccp: GccpState,
    /// Skills 本地技能库（任务成功后自动沉淀经验）
    pub skills: Box<dyn SkillStore>,
}

impl App {
    pub fn new(agent_file: &str, gateway: GatewayClient) -> Self {
        Self {
            agent_file: agent_file.to_string(),
            messages: VecDeque::with_capacity(MAX_CHAT_MESSAGES),
            input: String::new(),
            active_panel: ActivePanel::Chat,
            scroll_offset: 0,
            gateway,
            connected: false,
            gateway_version: None,
            turn: 0,
            tokens: 0,
            cost: 0.0,
            session_start: Instant::now(),
            mcp_enabled: false,
            a2a_enabled: false,
            logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
            help_text: build_help_text(),
            config_content: String::new(),
            memory_stats: String::new(),
            plugin_list: String::new(),
            loading: false,
            status_message: "Press Enter to start".to_string(),
            task_mode: false,
            memory: {
                let m = memory::build_memory(None);
                log::info!("memory: {} records loaded", m.len());
                m
            },
            flow_phase: FlowPhase::Chat,
            gccp: GccpState::default(),
            skills: {
                let s = skills::build_skill_store(None);
                log::info!("skills: {} skills loaded", s.len());
                s
            },
        }
    }

    /// Toggle a panel. If already active, go back to Chat.
    pub fn toggle_panel(&mut self, panel: ActivePanel) {
        if self.active_panel == panel {
            self.active_panel = ActivePanel::Chat;
        } else {
            self.active_panel = panel;
        }
    }

    /// Submit the current input as a message.
    ///
    /// 任务流分派：
    ///   - Chat / Executing：普通对话；任务集执行中检测"完成"信号 → 沉淀技能
    ///   - GccpRound1/2/3：任务事实确认（GCCP）各轮用户回答
    ///   - GradConfirm：任务流程图（GRAD）确认
    pub async fn submit_input(&mut self) -> Result<()> {
        let input = self.input.trim().to_string();
        self.input.clear();

        if input.is_empty() {
            return Ok(());
        }

        // Add user message
        self.add_message(MessageRole::User, input.clone());

        // 记忆：持久化用户输入（普通对话与任务均记录）
        if let Err(e) = self.memory.push("user", &input, "chat") {
            log::warn!("memory push(user) failed: {}", e);
        }

        // Check connection
        if !self.connected {
            self.check_connection().await?;
        }
        if !self.connected {
            self.add_message(
                MessageRole::System,
                "Not connected to gateway. Run 'agentrt' to start the server.".to_string(),
            );
            return Ok(());
        }
        if input.eq_ignore_ascii_case("exit") {
            self.add_message(MessageRole::System, "Type Ctrl+C to quit.".to_string());
            return Ok(());
        }

        match self.flow_phase {
            FlowPhase::Chat | FlowPhase::Executing => {
                // 任务集执行中：用户宣告完成 → 自动提炼经验并沉淀为技能
                if self.flow_phase == FlowPhase::Executing && gccp::is_task_done_input(&input) {
                    self.complete_task().await;
                    return Ok(());
                }
                self.chat_round(&input).await?;
            }
            FlowPhase::GccpRound1 => self.gccp_round1(&input).await?,
            FlowPhase::GccpRound2 => self.gccp_round2(&input).await?,
            FlowPhase::GccpRound3 => self.gccp_round3(&input).await?,
            FlowPhase::GradConfirm => self.grad_confirm(&input).await?,
        }

        Ok(())
    }

    /// 普通对话轮次：发送增强 prompt，并按 LLM 判定的模式切换任务流。
    async fn chat_round(&mut self, input: &str) -> Result<()> {
        self.loading = true;
        self.turn += 1;

        // 构造增强 prompt：系统指令（LLM 判定任务集）+ 技能 + 记忆 + 历史
        let prompt = self.build_context_prompt(input);

        match self.gateway.send_message(&prompt, &self.agent_file).await {
            Ok(response) => {
                if let Some(t) = response.tokens_used {
                    self.tokens += t;
                }
                if let Some(c) = response.cost_usd {
                    self.cost += c;
                }

                let (mode, cleaned) = parse_mode_detail(&response.response);
                self.add_message(MessageRole::Agent, cleaned.clone());
                // 记忆：持久化助手响应
                if let Err(e) = self.memory.push("assistant", &cleaned, "chat") {
                    log::warn!("memory push(assistant) failed: {}", e);
                }

                // 执行阶段 LLM 自报 [TASK:DONE] → 自动沉淀技能
                let llm_done = gccp::has_task_done_marker(&response.response);

                match mode {
                    ModeMarker::TaskGccp => {
                        // 大任务集：进入任务事实确认（GCCP）
                        self.start_gccp(input).await?;
                    }
                    ModeMarker::Task => {
                        self.task_mode = true;
                        self.flow_phase = FlowPhase::Executing;
                    }
                    ModeMarker::Chat => {
                        self.task_mode = false;
                        self.flow_phase = FlowPhase::Chat;
                    }
                }

                if llm_done && self.flow_phase == FlowPhase::Executing {
                    self.complete_task().await;
                }
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("Error: {}", e));
            }
        }

        self.loading = false;
        Ok(())
    }

    /// 大任务集启动：进入任务事实确认（GCCP），提出第 1-2 问。
    async fn start_gccp(&mut self, goal: &str) -> Result<()> {
        self.task_mode = true;
        self.gccp.reset();
        self.gccp.goal = goal.to_string();
        self.flow_phase = FlowPhase::GccpRound1;
        self.add_message(
            MessageRole::System,
            "检测到大任务集，进入「任务事实确认」（GCCP）阶段（共 5 问，分三轮提问）。".to_string(),
        );
        self.ask_gccp_round(1).await
    }

    /// 向 LLM 请求生成指定轮次的问题并展示（round=1: Q1-Q2, 2: Q3-Q4, 3: Q5）。
    async fn ask_gccp_round(&mut self, round: u8) -> Result<()> {
        let prompt = match round {
            1 => gccp::build_q12_prompt(&self.gccp.goal),
            2 => gccp::build_q34_prompt(&self.gccp),
            3 => gccp::build_q5_prompt(&self.gccp),
            _ => return Ok(()),
        };

        // 本轮问题生成后，进入对应作答阶段
        self.flow_phase = match round {
            1 => FlowPhase::GccpRound1,
            2 => FlowPhase::GccpRound2,
            3 => FlowPhase::GccpRound3,
            _ => return Ok(()),
        };

        self.loading = true;
        let result = self.gateway.send_message(&prompt, &self.agent_file).await;
        self.loading = false;

        match result {
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
                let hint = match round {
                    1 => "请输入第 1、2 问的回答（每行一个，对应 Q1、Q2）：",
                    2 => "请输入第 3、4 问的回答（每行一个，对应 Q3、Q4）：",
                    _ => "请输入第 5 问的回答：",
                };
                self.add_message(MessageRole::System, hint.to_string());
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("GCCP 提问失败：{}", e));
            }
        }
        Ok(())
    }

    /// GCCP 第 1 轮：用户回答 Q1-Q2 → LLM 思考 → 提出 Q3-Q4。
    async fn gccp_round1(&mut self, input: &str) -> Result<()> {
        let ans = gccp::parse_answers(input);
        if ans.is_empty() {
            self.add_message(MessageRole::System, "回答不能为空，请重新输入。".to_string());
            return Ok(());
        }
        self.gccp.a1 = ans.first().cloned().unwrap_or_default();
        self.gccp.a2 = ans.get(1).cloned().unwrap_or_default();
        // LLM 基于回答思考后提出第 3-4 问
        self.ask_gccp_round(2).await
    }

    /// GCCP 第 2 轮：用户回答 Q3-Q4 → LLM 思考 → 提出第 5 问。
    async fn gccp_round2(&mut self, input: &str) -> Result<()> {
        let ans = gccp::parse_answers(input);
        if ans.is_empty() {
            self.add_message(MessageRole::System, "回答不能为空，请重新输入。".to_string());
            return Ok(());
        }
        self.gccp.a3 = ans.first().cloned().unwrap_or_default();
        self.gccp.a4 = ans.get(1).cloned().unwrap_or_default();
        // LLM 基于回答思考后提出第 5 问
        self.ask_gccp_round(3).await
    }

    /// GCCP 第 3 轮：用户回答第 5 问 → 五问齐备 → 生成 GRAD 任务流程图。
    async fn gccp_round3(&mut self, input: &str) -> Result<()> {
        self.gccp.a5 = input.trim().to_string();
        if self.gccp.a5.is_empty() {
            self.add_message(MessageRole::System, "回答不能为空，请重新输入。".to_string());
            return Ok(());
        }
        self.flow_phase = FlowPhase::GradConfirm;

        self.loading = true;
        let prompt = gccp::build_grad_prompt(&self.gccp);
        let result = self.gateway.send_message(&prompt, &self.agent_file).await;
        self.loading = false;

        match result {
            Ok(r) => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
                self.gccp.grad_plan = r.response.trim().to_string();
                self.add_message(MessageRole::Agent, r.response.clone());
                if let Err(e) = self.memory.push("assistant", &r.response, "task") {
                    log::warn!("memory push(assistant) failed: {}", e);
                }
                self.add_message(
                    MessageRole::System,
                    "请确认「任务流程图」（GRAD）：输入「确认」开始执行，或输入修改意见。".to_string(),
                );
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("GRAD 生成失败：{}", e));
            }
        }
        Ok(())
    }

    /// GRAD：确认流程图后开始执行；否则按反馈修订流程图。
    async fn grad_confirm(&mut self, input: &str) -> Result<()> {
        if gccp::is_confirm(input) {
            self.flow_phase = FlowPhase::Executing;
            self.add_message(MessageRole::System, "任务流程图已确认，开始执行任务集。".to_string());

            // 注入目标 + 已确认事实 + 流程图，LLM 开始执行
            let prompt = gccp::build_execute_prompt(&self.gccp);
            self.loading = true;
            let result = self.gateway.send_message(&prompt, &self.agent_file).await;
            self.loading = false;

            match result {
                Ok(r) => {
                    if let Some(t) = r.tokens_used {
                        self.tokens += t;
                    }
                    if let Some(c) = r.cost_usd {
                        self.cost += c;
                    }
                    let cleaned = gccp::strip_task_done(&r.response);
                    self.add_message(MessageRole::Agent, cleaned.clone());
                    if let Err(e) = self.memory.push("assistant", &cleaned, "task") {
                        log::warn!("memory push(assistant) failed: {}", e);
                    }
                    if gccp::has_task_done_marker(&r.response) {
                        self.complete_task().await;
                    }
                }
                Err(e) => {
                    self.add_message(MessageRole::System, format!("Error: {}", e));
                }
            }
        } else {
            // 用户反馈 → LLM 修订流程图
            self.loading = true;
            let prompt = format!(
                "用户对任务流程图（GRAD）的反馈：{}\n请基于反馈修订流程图，以 [GRAD] 开头，\
                 包含任务目标、执行步骤与验收标准。",
                input
            );
            let result = self.gateway.send_message(&prompt, &self.agent_file).await;
            self.loading = false;

            match result {
                Ok(r) => {
                    if let Some(t) = r.tokens_used {
                        self.tokens += t;
                    }
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
                Err(e) => {
                    self.add_message(MessageRole::System, format!("Error: {}", e));
                }
            }
        }
        Ok(())
    }

    /// 任务成功收尾：自动提炼经验 → 沉淀为本地技能 → 回到对话模式。
    ///
    /// 这是 Skills 本地技能库的核心闭环：任务成功后不"用过即忘"，
    /// 而是将本次执行过程交给 LLM 提炼为可复用技能存入本地库，
    /// 后续任务在 build_context_prompt 中召回匹配技能，Agent 越用越强。
    async fn complete_task(&mut self) {
        let recent = self.memory.recent(40);
        let conv: Vec<String> = recent
            .iter()
            .rev()
            .map(|r| format!("{}: {}", r.role, r.content))
            .collect();
        let conv_text = conv.join("\n");

        let distilled = if conv_text.trim().is_empty() {
            false
        } else {
            let prompt = skills::build_distill_prompt(&conv_text);
            self.loading = true;
            let result = self.gateway.send_message(&prompt, &self.agent_file).await;
            self.loading = false;
            match result {
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
            }
        };

        self.task_mode = false;
        self.flow_phase = FlowPhase::Chat;
        if !distilled {
            self.add_message(
                MessageRole::System,
                "任务已完成（技能沉淀跳过或失败，详见日志）。".to_string(),
            );
        }
    }

    /// 构造发送给 LLM 的增强 prompt。
    ///
    /// 结构：系统判定指令 → 召回的可复用技能 → 相关记忆 → 最近对话历史 → 用户输入。
    /// "是否进入任务集"由 LLM 判断：回复以 [MODE:TASK]/[MODE:CHAT]/[MODE:TASK:GCCP] 开头。
    fn build_context_prompt(&self, input: &str) -> String {
        let mut ctx = String::from(
            "你是 AirymaxRT 智能体运行底座（AgentRT Runtime）的助手。\n\
             请先判断本次请求意图，然后正常回答：\n\
             - 若属于普通对话（闲聊、问答、寒暄），回复以 [MODE:CHAT] 开头；\n\
             - 若属于需要多步执行、工具调用或复杂编排的任务集，回复以 [MODE:TASK] 开头；\n\
             - 若属于大型/高复杂度任务集（需先确认任务事实再执行），回复以 [MODE:TASK:GCCP] 开头；\n\
             - 任务集执行完成时，可在回复末尾追加 [TASK:DONE]。\n\n",
        );

        // 技能上下文：召回本地技能库中沉淀的相关技能（越用越聪明）
        let skill_hits = self.skills.find(input, 3);
        if !skill_hits.is_empty() {
            ctx.push_str("【可复用技能】\n");
            for s in skill_hits {
                ctx.push_str(&format!(
                    "- {}（{}，复用 {} 次）：{}\n  步骤：{}\n",
                    s.name, s.category, s.success_count, s.summary, s.procedure
                ));
            }
            ctx.push('\n');
        }

        // 记忆上下文：召回与本次输入相关的历史记忆
        let hits = self.memory.recall(input, 5);
        if !hits.is_empty() {
            ctx.push_str("【相关记忆】\n");
            for h in hits {
                ctx.push_str(&format!("- ({}): {}\n", h.role, h.content));
            }
            ctx.push('\n');
        }

        // 对话历史：最近 6 条（不重复注入当前输入）
        let recent = self.memory.recent(6);
        if !recent.is_empty() {
            ctx.push_str("【对话历史】\n");
            for rec in recent.iter().rev() {
                let speaker = if rec.role == "user" { "用户" } else { "助手" };
                ctx.push_str(&format!("{}: {}\n", speaker, rec.content));
            }
            ctx.push('\n');
        }

        ctx.push_str(&format!("用户: {}\n", input));
        ctx
    }

    /// Check gateway connection.
    pub async fn check_connection(&mut self) -> Result<()> {
        match self.gateway.health_check().await {
            Ok(health) => {
                self.connected = true;
                self.gateway_version = health.version.clone();
                self.status_message = format!("Connected to AgentRT v{}", health.version.as_deref().unwrap_or("unknown"));
            }
            Err(e) => {
                self.connected = false;
                self.status_message = format!("Gateway unreachable: {}", e);
            }
        }
        Ok(())
    }

    /// Add a chat message.
    pub fn add_message(&mut self, role: MessageRole, content: String) {
        let msg = ChatMessage {
            role,
            content,
            timestamp: Local::now().format("%H:%M:%S").to_string(),
        };

        if self.messages.len() >= MAX_CHAT_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
    }

    /// Scroll up in chat.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll down in chat.
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll up one page.
    pub fn scroll_page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(10);
    }

    /// Scroll down one page.
    pub fn scroll_page_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(10);
    }

    /// Shutdown gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        if self.connected {
            self.add_message(MessageRole::System, "Shutting down...".to_string());
        }
        Ok(())
    }

    /// Get elapsed time as formatted string.
    pub fn elapsed_time(&self) -> String {
        let elapsed = self.session_start.elapsed();
        let seconds = elapsed.as_secs();
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let secs = seconds % 60;

        if hours > 0 {
            format!("{}h{:02}m{:02}s", hours, minutes, secs)
        } else {
            format!("{:02}m{:02}s", minutes, secs)
        }
    }
}

fn build_help_text() -> Vec<String> {
    vec![
        "AirymaxRT 智能体运行底座 - 帮助".to_string(),
        String::new(),
        "快捷键:".to_string(),
        "  F1          - 显示帮助面板".to_string(),
        "  F2          - 显示配置".to_string(),
        "  F3          - 显示运行时日志".to_string(),
        "  F4          - 显示记忆统计".to_string(),
        "  F5          - 显示插件列表".to_string(),
        "  Enter       - 发送消息".to_string(),
        "  Alt+Enter   - 换行（多行输入）".to_string(),
        "  Ctrl+C      - 退出 TUI".to_string(),
        "  Esc         - 返回对话".to_string(),
        "  Up/Down     - 滚动对话".to_string(),
        "  PgUp/PgDn   - 滚动对话（翻页）".to_string(),
        String::new(),
        "任务流:".to_string(),
        "  是否进入任务集由 LLM 判断，状态栏显示当前阶段徽章。".to_string(),
        "  GCCP（任务事实确认）：大任务集启动时共 5 问，分三轮提问，".to_string(),
        "    前 2 问作答后 LLM 思考再问 3-4 问，再思考后问第 5 问。".to_string(),
        "  GRAD（任务流程图确认）：五问齐备后生成流程图，确认后开始执行。".to_string(),
        "  任务集执行中输入「完成」或 LLM 回复 [TASK:DONE] 即完成。".to_string(),
        String::new(),
        "记忆:".to_string(),
        "  对话历史自动持久化（$AIRY_HOME/tui/memory.jsonl），跨会话可召回。".to_string(),
        String::new(),
        "Skills 本地技能库:".to_string(),
        "  任务成功后自动提炼经验并沉淀为可复用技能（$AIRY_HOME/tui/skills.jsonl）。".to_string(),
        "  区别于社区官方技能库：本地技能是 Agent 在任务中自我总结的，".to_string(),
        "  用得多、沉淀多、可用工具就多，不用重复造技能的轮子。".to_string(),
        String::new(),
        "状态栏:".to_string(),
        "  显示阶段、技能数、回合数、Token、成本与耗时。".to_string(),
        "  MCP/A2A 指示器显示协议可用性。".to_string(),
    ]
}

/// LLM 判定的模式（任务集判定结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeMarker {
    /// 普通对话
    Chat,
    /// 任务集（简单任务，无需 GCCP）
    Task,
    /// 大任务集（需先任务事实确认 GCCP）
    TaskGccp,
}

/// 解析 LLM 返回的模式标记（由 LLM 判定"是否进入任务集"）。
///
/// 支持格式：`[MODE:TASK]` / `[MODE:CHAT]` / `[MODE:TASK:GCCP]`（标记后可有换行/空白）。
/// 返回 (是否任务集, 剥离标记后的内容)。未匹配标记时视为普通对话原样返回。
/// 保持兼容旧接口（memory.rs 测试使用）；新代码请用 parse_mode_detail。
#[allow(dead_code)]
pub fn parse_mode_marker(resp: &str) -> (bool, String) {
    let (mode, rest) = parse_mode_detail(resp);
    (mode != ModeMarker::Chat, rest)
}

/// 解析 LLM 返回的模式标记详情（区分普通任务集与大任务集 GCCP）。
pub fn parse_mode_detail(resp: &str) -> (ModeMarker, String) {
    let t = resp.trim_start();
    if let Some(idx) = t.find(']') {
        let head = &t[..=idx];
        let marker = match head {
            "[MODE:TASK:GCCP]" => ModeMarker::TaskGccp,
            "[MODE:TASK]" => ModeMarker::Task,
            "[MODE:CHAT]" => ModeMarker::Chat,
            _ => return (ModeMarker::Chat, resp.to_string()),
        };
        let rest = t[idx + 1..].trim_start().to_string();
        return (marker, rest);
    }
    (ModeMarker::Chat, resp.to_string())
}