// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Application state for the AgentRT TUI.

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::time::Instant;

use crate::client::{GatewayClient, RunResponse};
use crate::gccp::{self, FlowPhase, GccpState, TaskControl};
use crate::memory::{self, ConversationMemory};
use crate::skills::{self, SkillStore};
use crate::wizard;

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
    /// 消息时间戳（HH:MM:SS），消息气泡头部展示
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
    /// 当前对话/任务模型（/model <name> 设置并持久化；空 = 由网关/llm_d 回落默认）
    pub model: String,
    /// 用户模型配置文件路径（展示用）
    pub config_file: String,
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
    /// 首次启动向导（首次运行自动弹出；/hiairy 随时重开）
    pub wizard: wizard::WizardState,
    /// 进行中的后台 LLM 请求（主循环每 100ms 轮询 + 渲染，驱动 thinking 动效）
    pending: Option<PendingTurn>,
    /// 任务集执行控制状态（人工暂停/中止，默认运行中）
    pub task_control: TaskControl,
    /// 输入历史（Alt+↑/↓ 浏览，与对话滚动解耦；去重，最多 50 条）
    input_history: Vec<String>,
    /// 输入历史浏览位置（None = 未在浏览，回到手输状态）
    history_pos: Option<usize>,
}

/// 后台 LLM 请求的类型（决定结果如何应用）。
#[derive(Debug)]
enum PendingKind {
    /// 普通对话 / 任务执行轮（原 chat_round）
    ChatRound { input: String },
    /// GCCP 提问轮（原 ask_gccp_round）
    AskGccp { round: u8 },
    /// 五问齐备 → 生成 GRAD 任务流程图（原 gccp_round3 网络部分）
    GradPlan,
    /// GRAD 确认（confirmed=true，开始执行）或修订（confirmed=false）
    GradConfirm { confirmed: bool },
    /// 任务完成经验蒸馏（原 complete_task 网络部分）
    Distill,
    /// 未连接时的连接检查：通过后继续执行 kind/prompt 的真实请求
    CheckConnect {
        kind: Box<PendingKind>,
        prompt: String,
        /// 待继续请求携带的 agent 编排 spec（连接通过后透传）
        agent: Option<serde_json::Value>,
        /// 待继续请求携带的完整对话历史（连接通过后透传）
        history: Option<serde_json::Value>,
    },
}

/// 后台 LLM 请求句柄：网关调用在 tokio 任务中执行，结果经 oneshot 回传。
/// `task` 持有 JoinHandle，供人工中止（Ctrl+X）时取消后台请求。
struct PendingTurn {
    rx: tokio::sync::oneshot::Receiver<PendingOutcome>,
    kind: PendingKind,
    task: Option<tokio::task::JoinHandle<()>>,
    /// 客户端预分配会话 ID（Ctrl+X 时调用 gateway agent.cancel 中止运行中请求）
    session_id: String,
}

/// 后台请求的结果载荷（LLM 调用结果 / 连接检查结果）。
enum PendingOutcome {
    /// LLM 调用结果
    Run(Result<RunResponse>),
    /// 连接检查结果（成功与否）
    Connect(bool),
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
            model: load_saved_model().unwrap_or_default(),
            config_file: format!("{}/config/model.yaml", airy_home()),
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
            wizard: wizard::WizardState::new(),
            pending: None,
            task_control: TaskControl::Running,
            input_history: Vec::with_capacity(16),
            history_pos: None,
        }
    }

    /// 重新打开首次启动向导（/hiairy 命令触发）。
    pub fn open_wizard(&mut self) {
        self.wizard.reopen();
    }

    /// 记录一条运行时日志（F3 面板展示，不依赖网关 HTTP 端点）。
    pub fn add_log(&mut self, level: &str, message: String) {
        if self.logs.len() >= MAX_LOG_ENTRIES {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            level: level.to_string(),
            message,
            daemon: None,
        });
    }

    /// 切换任务流阶段并记录日志。
    ///
    /// 阶段迁移是排查任务流问题的关键节点：Chat → GCCP → GRAD → Executing，
    /// 每次迁移都留下 info 级埋点（含来源/目标阶段与中文名），便于事后回溯。
    fn set_flow_phase(&mut self, to: FlowPhase) {
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
    fn set_task_control(&mut self, to: TaskControl) {
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

    /// 记录一条输入历史（去重：与最近一条相同则跳过；容量 50）。
    fn push_history(&mut self, input: &str) {
        let t = input.trim();
        if t.is_empty() {
            return;
        }
        if self.input_history.last().map(|s| s.as_str()) == Some(t) {
            return;
        }
        self.input_history.push(t.to_string());
        if self.input_history.len() > 50 {
            self.input_history.remove(0);
        }
        // 新提交后回到手输状态
        self.history_pos = None;
    }

    /// 浏览上一条输入历史（Alt+↑；历史空时无操作）。
    pub fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            Some(p) => p.saturating_sub(1),
            None => self.input_history.len() - 1,
        };
        self.history_pos = Some(next);
        self.input = self.input_history[next].clone();
    }

    /// 浏览下一条输入历史（Alt+↓；越界回到手输状态并清空输入）。
    pub fn history_next(&mut self) {
        let Some(p) = self.history_pos else {
            return;
        };
        if p + 1 < self.input_history.len() {
            self.history_pos = Some(p + 1);
            self.input = self.input_history[p + 1].clone();
        } else {
            // 已到最后一条 → 回到手输状态
            self.history_pos = None;
            self.input.clear();
        }
    }

    /// /model 命令：查看（无参数）或设置（/model <模型名>）当前模型。
    ///
    /// 模型名持久化到 $AIRY_HOME/tui/config.toml，后续 agent.run 请求
    /// 携带 model 字段；为空时由 gateway/llm_d 依次回落默认模型。
    fn cmd_model(&mut self, input: &str) {
        let arg = input[6..].trim();
        if arg.is_empty() {
            let cur = if self.model.is_empty() {
                "（默认，由网关 / llm_d 自动回落）".to_string()
            } else {
                format!("{}", self.model)
            };
            self.add_message(MessageRole::System, format!("当前模型：{}", cur));
            self.add_message(
                MessageRole::System,
                format!("设置模型：/model <模型名>（持久化到 $AIRY_HOME/tui/config.toml）"),
            );
            self.add_message(
                MessageRole::System,
                format!("默认配置：{}", self.config_file),
            );
            return;
        }
        self.model = arg.to_string();
        persist_model(&self.model);
        self.add_log("INFO", format!("模型切换为 {}", self.model));
        self.add_message(
            MessageRole::System,
            format!("模型已设置为：{}（已持久化）", self.model),
        );
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

        // 记录输入历史（Alt+↑/↓ 浏览；去重，最新在后）
        self.push_history(&input);

        // Add user message
        self.add_message(MessageRole::User, input.clone());

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
            FlowPhase::GradConfirm => self.grad_confirm(&input)?,
        }

        Ok(())
    }

    /// 普通对话轮次：发送增强 prompt，并按 LLM 判定的模式切换任务流。
    fn chat_round(&mut self, input: &str) -> Result<()> {
        self.turn += 1;

        // 构造增强 prompt：系统指令（LLM 判定任务集）+ 技能 + 记忆
        let prompt = self.build_context_prompt(input);
        // 完整对话历史（OpenAI messages 数组，末条为增强 prompt）：
        // 网络层由此获得真实多轮上下文（M1/M2 修复），无需再挤进 prompt 文本。
        let history = self.build_history_messages(&prompt);
        self.dispatch_with_agent(
            PendingKind::ChatRound { input: input.to_string() },
            &prompt,
            None,
            history,
        );
        Ok(())
    }

    /// 应用普通对话轮的 LLM 结果（按 LLM 判定的模式切换任务流）。
    fn apply_chat_result(&mut self, input: String, res: Result<RunResponse>) {
        match res {
            Ok(response) => {
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
                if let Some(trace) = &response.tool_trace {
                    const MAX_VISIBLE_TOOL_TRACES: usize = 4;
                    let total = trace.len();
                    let show = total.min(MAX_VISIBLE_TOOL_TRACES);
                    for t in trace.iter().take(show) {
                        let ok = t.ok.unwrap_or(0) != 0;
                        let args_short = format_tool_args(&t.arguments, 200);
                        self.add_message(
                            MessageRole::ToolCall,
                            format!(
                                "{} {}{}",
                                t.tool,
                                args_short,
                                if ok { "" } else { "（失败）" }
                            ),
                        );
                        self.add_message(
                            MessageRole::ToolResult,
                            truncate_for_display(&t.result, 2000),
                        );
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
                self.add_message(MessageRole::System, format!("Error: {}", e));
            }
        }
    }

    /// 大任务集启动：进入任务事实确认（GCCP），提出第 1 问。
    fn start_gccp(&mut self, goal: &str) {
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

    /// 向 LLM 请求生成指定轮次的问题并展示（round = 1..=5，每轮只问 1 个问题）。
    fn ask_gccp_round(&mut self, round: u8) {
        if !(1..=5).contains(&round) {
            return;
        }
        let prompt = gccp::build_qn_prompt(&self.gccp, round);

        // 本轮问题生成后，进入对应作答阶段
        self.set_flow_phase(FlowPhase::GccpRound(round));

        self.dispatch(PendingKind::AskGccp { round }, &prompt);
    }

    /// 应用 GCCP 提问轮的 LLM 结果：解析问题并提示用户作答。
    fn apply_ask_gccp(&mut self, round: u8, res: Result<RunResponse>) {
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
    fn gccp_round_n(&mut self, round: u8, input: &str) -> Result<()> {
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
    fn apply_grad_plan(&mut self, res: Result<RunResponse>) {
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
    fn grad_confirm(&mut self, input: &str) -> Result<()> {
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
            self.dispatch_with_agent(
                PendingKind::GradConfirm { confirmed: true },
                &prompt,
                Some(agent_spec),
                history,
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
    fn apply_grad_confirm(&mut self, confirmed: bool, res: Result<RunResponse>) {
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
                    if let Err(e) = self.memory.push("assistant", &cleaned, "task") {
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
    fn complete_task(&mut self) {
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
    fn finish_task(&mut self, distilled: bool) {
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
    fn apply_distill_result(&mut self, res: Result<RunResponse>) {
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

    /// 分派一个后台 LLM 请求：已连接直接发起；未连接先检查连接，通过后继续。
    fn dispatch(&mut self, kind: PendingKind, prompt: &str) {
        self.dispatch_with_agent(kind, prompt, None, None);
    }

    /// 分派一个后台请求，可携带 agent 编排 spec（任务执行场景：gateway
    /// 依据 params.agent 走 spawn+invoke 编排分支，而非纯 LLM 工具循环）。
    ///
    /// `history`：完整对话历史（OpenAI messages 数组，含当前输入作为末条
    /// user 消息）；携带时 gateway 以整个数组作为工具循环初始上下文。
    fn dispatch_with_agent(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
    ) {
        log::trace!(
            "dispatch: {:?}（prompt_len={}，connected={}，agent={}，history={}）",
            kind,
            prompt.len(),
            self.connected,
            agent.is_some(),
            history.is_some()
        );
        if self.connected {
            self.start_pending(kind, prompt, agent, history);
        } else {
            self.start_connect_then(kind, prompt, agent, history);
        }
    }

    /// 发起后台 LLM 请求（网关调用在 tokio 任务中执行，不阻塞事件循环渲染）。
    ///
    /// `agent`：携带时请求经 agent_d 编排执行（spawn+invoke），否则纯 LLM 对话。
    /// `history`：完整对话历史（OpenAI messages 数组），随请求透传 gateway。
    fn start_pending(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
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
                .send_message(&prompt, &agent_file, model.as_deref(), Some(&sid_for_task), agent, history)
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
        });
    }

    /// 未连接时的连接检查：后台执行健康检查，通过后继续真实请求。
    fn start_connect_then(
        &mut self,
        kind: PendingKind,
        prompt: &str,
        agent: Option<serde_json::Value>,
        history: Option<serde_json::Value>,
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
            },
            task: Some(task),
            session_id: String::new(),
        });
    }

    /// 生成客户端预分配会话 ID（sess_ 前缀，gateway 校验后采用）。
    fn new_session_id(&self) -> String {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        // 时间 + 会话序号 + 伪随机位（非密码学用途，仅保证唯一性）
        let seq = self.turn as u128;
        let mix = (now_ms ^ (seq << 32)) * 6364136223846793005;
        format!("sess_{:016x}_{:04x}", mix & 0xFFFFFFFFFFFFFFFF, (seq & 0xFFFF) as u16)
    }

    /// 主循环轮询：后台请求完成则应用结果（返回是否有待办请求，供渲染循环判断）。
    ///
    /// 用户暂停（Ctrl+Z）期间不消费结果——请求继续在网关执行，恢复后结果照常应用。
    pub fn poll_pending(&mut self) -> bool {
        if self.task_control == TaskControl::Paused {
            return true;
        }
        let Some(p) = &mut self.pending else {
            return false;
        };
        let outcome = match p.rx.try_recv() {
            Ok(o) => o,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return true,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                PendingOutcome::Run(Err(anyhow!("LLM 后台任务异常终止")))
            }
        };
        // 取走 kind（避免 move 出借用），先清 pending/loading，再应用结果
        let kind = std::mem::replace(&mut p.kind, PendingKind::ChatRound { input: String::new() });
        self.pending = None;
        self.loading = false;
        self.set_task_control(TaskControl::Running);
        log::info!("poll_pending: 消费后台请求结果（kind={:?}）", kind);
        self.apply_result(kind, outcome);
        self.pending.is_some()
    }

    /// 人工中止当前后台请求（Ctrl+X）：取消 tokio 任务并清理待办状态。
    ///
    /// 客户端侧取消 tokio 任务并恢复 UI（阶段按请求类型回退，交互不挂死）；
    /// 服务端通过 gateway agent.cancel 中止运行中的 agent.run（资源释放）。
    pub fn abort_task(&mut self) {
        let mut session_id = String::new();
        if let Some(p) = &mut self.pending {
            if let Some(t) = p.task.take() {
                t.abort();
            }
            session_id = p.session_id.clone();
        }
        let kind = self
            .pending
            .take()
            .map(|p| p.kind)
            .unwrap_or(PendingKind::ChatRound { input: String::new() });
        self.loading = false;
        self.set_task_control(TaskControl::Running);
        log::info!("abort_task: 人工中止（session={}）", session_id);

        // 服务端取消：凭预分配 session_id 中止 gateway 运行中请求（尽力而为）
        if !session_id.is_empty() {
            let gw = self.gateway.clone();
            tokio::spawn(async move {
                if let Err(e) = gw.cancel_session(&session_id).await {
                    log::debug!("agent.cancel request failed (session={}): {}", session_id, e);
                }
            });
        }

        // 按请求类型回退阶段，避免停留在等待状态
        match kind {
            PendingKind::GradPlan => {
                // GRAD 生成被中止 → 回到五问齐备后的等待（用户可重发）
                self.set_flow_phase(FlowPhase::GradConfirm);
                self.add_message(
                    MessageRole::System,
                    "流程图生成已中止。输入任意内容可重新生成，或输入「退出」放弃任务。".to_string(),
                );
            }
            PendingKind::GradConfirm { confirmed } => {
                if confirmed {
                    // 执行轮被中止 → 回到执行阶段，用户可重发指令或宣告完成
                    self.set_flow_phase(FlowPhase::Executing);
                    self.add_message(
                        MessageRole::System,
                        "任务执行已中止。可输入「完成」结束任务，或继续输入指令。".to_string(),
                    );
                } else {
                    self.set_flow_phase(FlowPhase::GradConfirm);
                    self.add_message(MessageRole::System, "流程图修订已中止。".to_string());
                }
            }
            PendingKind::ChatRound { .. } => {
                self.add_message(MessageRole::System, "已中止本次请求。".to_string());
            }
            PendingKind::AskGccp { .. } => {
                self.set_flow_phase(FlowPhase::Chat);
                self.add_message(MessageRole::System, "任务事实确认已中止，任务放弃。".to_string());
                self.task_mode = false;
            }
            PendingKind::Distill => {
                self.finish_task(false);
            }
            PendingKind::CheckConnect { .. } => {
                self.add_message(MessageRole::System, "已中止连接等待。".to_string());
            }
        }
        self.add_log("INFO", "任务已人工中止（Ctrl+X）".to_string());
    }

    /// 暂停后台请求轮询（Ctrl+Z）：冻结等待渲染，请求继续在网关执行。
    pub fn pause_task(&mut self) {
        if self.pending.is_some() {
            self.set_task_control(TaskControl::Paused);
            log::info!("pause_task: 已暂停（Ctrl+Z，请求仍在后台执行）");
            self.add_message(
                MessageRole::System,
                "⏸ 已暂停等待（Ctrl+Z 恢复，Ctrl+X 中止）。请求仍在后台执行。".to_string(),
            );
            self.add_log("INFO", "任务已暂停（Ctrl+Z）".to_string());
        }
    }

    /// 恢复暂停（Ctrl+Z）。
    pub fn resume_task(&mut self) {
        if self.task_control == TaskControl::Paused {
            self.set_task_control(TaskControl::Running);
            log::info!("resume_task: 已恢复等待（Ctrl+Z）");
            self.add_message(
                MessageRole::System,
                "▶ 已恢复，继续等待回复。".to_string(),
            );
            self.add_log("INFO", "任务已恢复（Ctrl+Z）".to_string());
        }
    }

    /// 是否有进行中的后台请求（主循环据此进入渲染等待）。
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// 应用后台请求结果（按请求类型分派）。
    fn apply_result(&mut self, kind: PendingKind, outcome: PendingOutcome) {
        match kind {
            PendingKind::ChatRound { input } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_chat_result(input, res);
                }
            }
            PendingKind::AskGccp { round } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_ask_gccp(round, res);
                }
            }
            PendingKind::GradPlan => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_grad_plan(res);
                }
            }
            PendingKind::GradConfirm { confirmed } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_grad_confirm(confirmed, res);
                }
            }
            PendingKind::Distill => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_distill_result(res);
                }
            }
            PendingKind::CheckConnect { kind, prompt, agent, history } => match outcome {
                PendingOutcome::Connect(true) => {
                    // 连接成功：继续执行真实请求（loading 由 start_pending 重新置位）
                    log::info!("apply_result: 连接检查通过，继续执行请求（kind={:?}）", kind);
                    self.connected = true;
                    self.start_pending(*kind, &prompt, agent, history);
                }
                _ => {
                    // 连接检查失败：若正在进行任务收尾（技能蒸馏），仍需结束任务流，
                    // 否则会卡在 Executing 阶段——「技能沉淀」被跳过且无法继续对话。
                    log::warn!("apply_result: 连接检查失败（kind={:?}）", kind);
                    if matches!(&*kind, PendingKind::Distill) {
                        self.add_message(
                            MessageRole::System,
                            "网关不可达，技能蒸馏请求失败：本次任务已完成，但经验未能沉淀。"
                                .to_string(),
                        );
                        self.finish_task(false);
                    } else {
                        self.add_message(
                            MessageRole::System,
                            "Not connected to gateway. Run 'agentrt' to start the server."
                                .to_string(),
                        );
                    }
                }
            },
        }
    }

    /// 构造发送给 LLM 的增强 prompt。
    ///
    /// 结构：系统判定指令 → 召回的可复用技能 → 相关记忆 → 用户输入。
    /// "是否进入任务集"由 LLM 判断：回复以 [MODE:TASK]/[MODE:CHAT]/[MODE:TASK:GCCP] 开头。
    /// 对话历史已改由 messages 数组承载（build_history_messages），不再挤进
    /// prompt 文本（M1/M2/M3 修复：网络层透传真实多轮上下文，上限 40 条）。
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

        ctx.push_str(&format!("用户: {}\n", input));
        ctx
    }

    /// 构造完整对话历史（OpenAI messages 数组）随请求透传 gateway。
    ///
    /// 来源：本地记忆库最近 40 条记录（倒序 → 正序），过滤 system 角色，
    /// 连续同角色消息合并（OpenAI 要求 user/assistant 交替）。跳过最新一条
    /// user 记录——它正是 submit_input 刚写入的当前输入，由 `final_content`
    /// （增强 prompt）作为末条 user 消息承载，避免输入双注入。
    ///
    /// 仅剩当前输入时返回 None（退化为单条 prompt，走 gateway 旧路径）。
    fn build_history_messages(&self, final_content: &str) -> Option<serde_json::Value> {
        let recent = self.memory.recent(40);
        if recent.is_empty() {
            return None;
        }
        let mut msgs: Vec<serde_json::Value> = Vec::with_capacity(recent.len() + 1);
        let mut last_role: Option<&str> = None;
        // 倒序 → 正序（时间从早到晚），同时跳过最新一条 user 记录
        // （recent.first() = 最新 = submit_input 刚写入的当前输入）
        let newest_is_user =
            recent.first().map(|r| r.role == "user").unwrap_or(false);
        for rec in recent.iter().rev() {
            if rec.role == "system" {
                continue;
            }
            if rec.role == "user" && newest_is_user && std::ptr::eq(rec, recent.first().unwrap()) {
                continue;
            }
            let role = if rec.role == "assistant" { "assistant" } else { "user" };
            if last_role == Some(role) {
                // 连续同角色（如用户连发多条）：合并进上一条，保持交替约束
                if let Some(last) = msgs.last_mut() {
                    let prev = last["content"].as_str().unwrap_or("").to_string();
                    last["content"] =
                        serde_json::json!(format!("{}\n{}", prev, rec.content));
                }
                continue;
            }
            msgs.push(serde_json::json!({ "role": role, "content": rec.content }));
            last_role = Some(role);
        }
        msgs.push(serde_json::json!({ "role": "user", "content": final_content }));
        if msgs.len() == 1 {
            return None;
        }
        Some(serde_json::Value::Array(msgs))
    }

    /// Check gateway connection.
    pub async fn check_connection(&mut self) -> Result<()> {
        match self.gateway.health_check().await {
            Ok(health) => {
                self.connected = true;
                self.gateway_version = health.version.clone();
                self.status_message = format!("Connected to AgentRT v{}", health.version.as_deref().unwrap_or("unknown"));
                self.add_log("INFO", format!("已连接网关 v{}", health.version.as_deref().unwrap_or("unknown")));
            }
            Err(e) => {
                self.connected = false;
                self.status_message = format!("Gateway unreachable: {}", e);
                self.add_log("ERROR", format!("网关不可达：{}", e));
            }
        }
        Ok(())
    }

    /// Add a chat message.
    pub fn add_message(&mut self, role: MessageRole, content: String) {
        // 时间戳：HH:MM:SS（消息气泡头部展示）
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let msg = ChatMessage {
            role,
            content,
            timestamp,
        };

        if self.messages.len() >= MAX_CHAT_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
        // 新消息不强制回底（P2-B）：用户正在向上滚动阅读时不打断视线。
        // scroll_offset 语义为「距底部向上滚的行数」，0 = 最新位置；
        // 用户在底部（0）时无需改动——lines 增长使 max_offset 增加，
        // from_top 自然跟随，视口保持跟随最新；滚离底部（>0）时保持原位。
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

// ─────────────────────────── 模型配置持久化 ───────────────────────────

/// 用户配置目录：$AIRY_HOME/tui → ~/.airymaxrt/tui（与 wizard.toml 同目录约定）
fn tui_config_dir() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("AIRY_HOME") {
        return std::path::PathBuf::from(home).join("tui");
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".airymaxrt").join("tui");
    }
    std::path::PathBuf::from(".airymaxrt").join("tui")
}

/// AIRY_HOME（用于展示 model.yaml 用户覆盖配置路径）
fn airy_home() -> String {
    if let Ok(home) = std::env::var("AIRY_HOME") {
        return home;
    }
    if let Ok(home) = std::env::var("HOME") {
        return format!("{}/.airymaxrt", home);
    }
    ".airymaxrt".to_string()
}

/// TUI 本地配置（config.toml）：目前持久化当前模型名。
#[derive(serde::Serialize, serde::Deserialize)]
struct TuiConfig {
    model: String,
    version: String,
}

fn config_path() -> std::path::PathBuf {
    tui_config_dir().join("config.toml")
}

/// 加载上次保存的模型名（config.toml 不存在或损坏时返回 None）。
fn load_saved_model() -> Option<String> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    let cfg: TuiConfig = toml::from_str(&raw).ok()?;
    if cfg.model.is_empty() {
        None
    } else {
        Some(cfg.model)
    }
}

/// 持久化当前模型名到 config.toml（保留版本字段，未来可扩展）。
fn persist_model(model: &str) {
    let cfg = TuiConfig {
        model: model.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Some(parent) = config_path().parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("model config: create dir failed: {}", e);
            return;
        }
    }
    match toml::to_string(&cfg) {
        Ok(s) => {
            if let Err(e) = std::fs::write(config_path(), s) {
                log::warn!("model config: persist failed: {}", e);
            } else {
                log::info!("model config saved to {}", config_path().display());
            }
        }
        Err(e) => log::warn!("model config: serialize failed: {}", e),
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
        "  Alt+Up/Down - 浏览输入历史（Alt+↓ 可回到手输状态）".to_string(),
        "  PgUp/PgDn   - 滚动对话（翻页）".to_string(),
        "  End         - 回到底部（最新消息）".to_string(),
        "  Ctrl+X      - 中止当前请求（任务执行/对话等待）".to_string(),
        "  Ctrl+Z      - 暂停/恢复等待（请求继续在后台执行）".to_string(),
        "  /hiairy     - 重新打开首次启动向导".to_string(),
        "  /model      - 查看当前模型；/model <模型名> 切换并持久化".to_string(),
        String::new(),
        "任务流:".to_string(),
        "  是否进入任务集由 LLM 判断，状态栏显示当前阶段徽章。".to_string(),
        "  GCCP（任务事实确认）：大任务集启动时共 5 问，逐一询问，".to_string(),
        "    每问之间 LLM 基于已答事实思考后再提下一问。".to_string(),
        "  GRAD（任务流程图确认）：五问齐备后生成流程图与结构化依赖图（DAG），".to_string(),
        "    确认后开始执行；执行中可 Ctrl+X 中止、Ctrl+Z 暂停。".to_string(),
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

/// 按字符数截断长文本（工具参数/结果展示用，避免对话面板被长 JSON 撑满）。
fn truncate_for_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{}…", cut)
}

/// 工具参数摘要：JSON 对象 → "key=value key=value"（每个值截断，易读且紧凑）
fn format_tool_args(args: &str, max: usize) -> String {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(args) {
        let parts: Vec<String> = map
            .iter()
            .map(|(k, v)| {
                let vs = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}={}", truncate_for_display(&vs, 60))
            })
            .collect();
        if !parts.is_empty() {
            return truncate_for_display(&parts.join(" "), max);
        }
    }
    truncate_for_display(args, max)
}

/// 双思考（GCCP+GRAD）轨迹 → 一行计划摘要。
/// 输入 gateway 回传的 thinking 对象 {plan:{task_plan_id,node_count,nodes[]},feedback,stats}，
/// 输出如「双思考计划 5 节点：S_01 使用 web_fetch 抓取…（GRAD 2 轮收敛）」。
fn format_thinking_summary(
    th: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let plan = th.get("plan")?;
    let node_count = plan
        .get("node_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let first_goal = plan
        .get("nodes")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|n| n.get("goal"))
        .and_then(|g| g.as_str())
        .map(|s| truncate_for_display(s, 48));
    let grad_rounds = th
        .get("feedback")
        .and_then(|f| f.get("rounds"))
        .and_then(|v| v.as_u64());
    let corrections = th
        .get("stats")
        .and_then(|s| s.get("corrections"))
        .and_then(|v| v.as_u64());

    let mut summary = format!("双思考计划 {} 节点", node_count);
    if let Some(g) = first_goal {
        summary.push_str(&format!("：{}", g));
    }
    match (grad_rounds, corrections) {
        (Some(r), Some(c)) => summary.push_str(&format!("（GRAD {} 轮 / 修正 {} 次）", r, c)),
        (Some(r), None) => summary.push_str(&format!("（GRAD {} 轮）", r)),
        _ => {}
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量测试互斥锁（并行测试共享进程内 AIRY_HOME，必须串行）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 模型名持久化往返：persist_model → load_saved_model 一致。
    #[test]
    fn model_persist_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("AIRY_HOME", dir.path());
        persist_model("deepseek-v4-flash");
        assert_eq!(load_saved_model().as_deref(), Some("deepseek-v4-flash"));
        // 再次切换覆盖
        persist_model("gpt-4-turbo");
        assert_eq!(load_saved_model().as_deref(), Some("gpt-4-turbo"));
    }

    /// config.toml 缺失或损坏时 load_saved_model 返回 None（回落默认模型）。
    #[test]
    fn model_load_missing_or_corrupt() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("AIRY_HOME", dir.path());
        assert_eq!(load_saved_model(), None);
        let cfg_dir = tui_config_dir();
        std::fs::create_dir_all(&cfg_dir).expect("create dir");
        std::fs::write(cfg_dir.join("config.toml"), "not-valid-toml{{").expect("write");
        assert_eq!(load_saved_model(), None);
    }

    /// /model 命令：设置模型并持久化；空参显示（不修改）。
    #[test]
    fn cmd_model_set_and_query() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("AIRY_HOME", dir.path());
        let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
            .expect("gateway client");
        let mut app = App::new("agents/main.agent.yaml", gw);
        assert!(app.model.is_empty());
        app.cmd_model("/model deepseek-v4-flash");
        assert_eq!(app.model, "deepseek-v4-flash");
        assert_eq!(load_saved_model().as_deref(), Some("deepseek-v4-flash"));
        app.cmd_model("/model");
        // 查询不改变当前模型
        assert_eq!(app.model, "deepseek-v4-flash");
    }
}