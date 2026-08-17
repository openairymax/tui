// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Application state for the AgentRT TUI.

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::time::Instant;

use crate::client::{GatewayClient, HallBoard, HallBoardEntry, HallEvent, HallTask, PendingApproval, RunResponse};
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
    /// 任务看板（hall.board：work_hall 执行实例 + 在线 agent，实时刷新）
    Board,
    /// 事件流（hall.stream：全局 gseq 因果序回放）
    Events,
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
    /// 输入光标位置（UTF-8 字节索引；←→ 移动、Backspace/Delete 删除、字符插入点）
    pub cursor: usize,
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
    /// 当前回合开始时刻（submit_input 记录，结果消费时结算）
    turn_started: Instant,
    /// 上一回合耗时（对话区回合分隔线展示：Worked for Ns）
    pub last_turn_elapsed: Option<Instant>,
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
    /// 流式输出：当前正在流式追加的 Agent 回复文本（chat.rs 增量渲染）
    pub streaming_text: String,
    /// 打字机上屏进度：streaming_text 已"显示"的字符数（伪流式时制造
    /// 逐字动效；< len 表示仍在逐字上屏中）。2026-08-17 F5 新增。
    pub streaming_reveal: usize,
    /// 打字机推进节拍（距上次 reveal 推进的时长；50ms tick 一字符）
    last_reveal_tick: Instant,
    /// 流式工具循环事件（SSE __airy_evt 渲染行，如 `[Sub web_search Agent] …`）
    pub stream_tool_events: Vec<String>,
    /// 流式思考链（SSE `__airy_evt:reasoning` 事件携带的 reasoning_content，
    /// thinking 模型的思考过程）。默认折叠为一行，浏览时展开全量。
    /// 2026-08-17 F6 新增（gateway 透传 reasoning_content）。
    pub stream_reasoning: String,
    /// 待人工决议的工具审批请求（tool.pending 轮询；Claude Code 风格 permission prompt）
    pub approvals: Vec<PendingApproval>,
    /// 项目上下文文件内容（AGENTS.md / CLAUDE.md，注入 build_context_prompt）
    pub project_context: String,
    /// 审批轮询节流（上次查询 tool.pending 的时刻）
    last_approval_poll: Instant,
    /// 审批轮询在途请求（spawn 后异步返回，下次 poll 消费结果）
    approval_poll_rx: Option<tokio::sync::oneshot::Receiver<Vec<PendingApproval>>>,
    /// 2026-08-17：F8 请求切换到 CLI（airy_cli）——主循环收到标志后
    /// 恢复终端并以 exec 语义替换当前进程（见 main.rs run_tui）。
    pub switch_to_cli: bool,
    /// 任务看板缓存（hall.board 最近一次成功拉取；Board 面板 1s 节流刷新）
    pub hall_board: Option<HallBoard>,
    /// 事件流缓存（hall.stream 最近一次拉取，最新在前）
    pub hall_events: Vec<HallEvent>,
    /// hall 面板轮询节流（上次拉取时刻）
    last_hall_poll: Instant,
    /// hall 面板在途请求（spawn 后异步返回，下次 poll_hall 消费结果）
    hall_poll_rx: Option<tokio::sync::oneshot::Receiver<HallPollOutcome>>,
    /// /chain 在途请求（task_id 为空 = 任务列表）
    chain_pending: Option<tokio::sync::oneshot::Receiver<ChainOutcome>>,
    /// /chain 请求的任务 id（" " 空串 = 任务列表，非空 = 该任务决策链）
    chain_task: String,
    /// 运维命令（/daemons /agents /tools /models /mem /rpc）在途请求
    ops_pending: Option<tokio::sync::oneshot::Receiver<OpsOutcome>>,
    /// 运维命令的展示标签（方法名，错误渲染用）
    ops_label: String,
    /// 2026-08-17：F6 看板选中行索引（↑↓ 移动，Enter 查看决策链）
    pub board_cursor: usize,
    /// 2026-08-17：F6 看板状态过滤（空 = 全部；running/completed/failed/...）
    pub board_filter: String,
    /// 2026-08-17：F7 事件流选中行索引（↑↓ 移动，Enter 展开完整内容）
    pub events_cursor: usize,
    /// 2026-08-17：F7 事件流类别过滤（空 = 全部；blueprint/command/progress/...）
    pub events_filter: String,
    /// 2026-08-17：任务执行期间（busy）插入对话队列——Enter 提交后先入队，
    /// 任务完成后主循环自动逐条处理（submit_input），对话不被打断、体验连续。
    pub insert_queue: VecDeque<String>,
}

/// hall 面板轮询结果（看板/事件流二选一）。
enum HallPollOutcome {
    Board(Result<HallBoard>),
    Events(Result<Vec<HallEvent>>),
}

/// /chain 决策链查询结果（任务列表 / 单任务事件链）。
enum ChainOutcome {
    Tasks(Result<Vec<HallTask>>),
    Events(Result<Vec<HallEvent>>),
}

/// 运维命令结果（/daemons 聚合 / 通用方法调用）。
enum OpsOutcome {
    /// 16 个 daemon 的 health_check 结果（ns, 结果）
    Daemons(Vec<(String, Result<serde_json::Value>)>),
    /// 单个 gateway 方法调用结果
    Call(Result<serde_json::Value>),
}

/// gateway 转发的 16 个 daemon 命名空间（与 C CLI CLI_DAEMONS 对齐）。
const OPS_DAEMON_NS: [&str; 16] = [
    "agent", "tool", "hook", "plugin", "think", "monit", "sched", "channel", "market", "llm",
    "cupolas", "mem", "info", "notify", "observe", "a2a",
];

/// 后台 LLM 请求的类型（决定结果如何应用）。
#[derive(Debug)]
enum PendingKind {
    /// 普通对话 / 任务执行轮（原 chat_round）
    ChatRound { input: String },
    /// 流式对话轮（SSE 增量渲染，普通对话走此路径）
    StreamRound { input: String },
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
    /// 流式输出接收端（StreamRound）：SSE 增量块（option：非流式请求为 None）
    stream_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// 流式工具事件接收端（tool_call/tool_result JSON，option：非流式请求为 None）
    tool_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// 流式最终结果暂存（打字机上屏完成前收到结果时先存这里，
    /// 等 reveal 追平文本长度后再 apply——保证逐字动效完整走完）。
    finish: Option<PendingOutcome>,
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
            cursor: 0,
            active_panel: ActivePanel::Chat,
            scroll_offset: 0,
            gateway,
            connected: false,
            gateway_version: None,
            turn: 0,
            tokens: 0,
            cost: 0.0,
            session_start: Instant::now(),
            turn_started: Instant::now(),
            last_turn_elapsed: None,
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
            streaming_text: String::new(),
            streaming_reveal: 0,
            last_reveal_tick: Instant::now(),
            stream_tool_events: Vec::new(),
            stream_reasoning: String::new(),
            approvals: Vec::new(),
            project_context: String::new(),
            last_approval_poll: Instant::now(),
            approval_poll_rx: None,
            switch_to_cli: false,
            hall_board: None,
            hall_events: Vec::new(),
            last_hall_poll: Instant::now(),
            hall_poll_rx: None,
            chain_pending: None,
            chain_task: String::new(),
            ops_pending: None,
            ops_label: String::new(),
            board_cursor: 0,
            board_filter: String::new(),
            events_cursor: 0,
            events_filter: String::new(),
            insert_queue: VecDeque::new(),
        }
    }

    /// 加载项目上下文文件（AGENTS.md / CLAUDE.md 等价物）。
    ///
    /// 从 start_dir（默认 cwd）向上逐级查找，首个命中即注入。与
    /// openlab/core/project_context.py 的行为对齐（跨进程一致的约定来源）。
    /// 返回是否找到。
    pub fn load_project_context(&mut self, start_dir: Option<&std::path::Path>) -> bool {
        let mut dir = start_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        loop {
            for name in ["AGENTS.md", "agents.md", "CLAUDE.md", ".airymax/AGENTS.md"] {
                let p = dir.join(name);
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if !content.trim().is_empty() {
                        self.project_context =
                            format!("项目约定（{}）:\n{}", p.display(), content.trim());
                        log::info!("project context loaded from {}", p.display());
                        return true;
                    }
                }
            }
            // .git 目录视为项目根，检查后停止（与 Python 侧一致）
            if dir.join(".git").is_dir() || !dir.pop() {
                break;
            }
        }
        log::info!("project context: no AGENTS.md/CLAUDE.md found");
        false
    }

    /// 会话恢复（--resume）：把记忆库最近的对话注入消息列表，还原上次会话。
    ///
    /// 对标 Codex sessions / Claude /resume：恢复最近 user/assistant 轮次
    /// （上限 30 条），新会话开篇即可看到上次上下文。
    pub fn resume_session(&mut self) -> usize {
        let recent = self.memory.recent(30);
        if recent.is_empty() {
            self.add_message(
                MessageRole::System,
                "没有可恢复的历史会话（--resume 未找到记忆）。".to_string(),
            );
            return 0;
        }
        let mut count = 0;
        for rec in recent.iter().rev() {
            let role = match rec.role.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Agent,
                _ => continue,
            };
            self.add_message(role, rec.content.clone());
            count += 1;
        }
        self.add_message(
            MessageRole::System,
            format!("已恢复上次会话（{} 条历史）。继续对话或发送新消息。", count),
        );
        log::info!("resume_session: restored {} messages", count);
        count
    }

    /// 重新打开首次启动向导（/hiairy 命令触发）。
    pub fn open_wizard(&mut self) {
        self.wizard.reopen();
    }

    /// 应用首次启动向导的快速配置结果：模型名持久化（config.toml，随请求下发）。
    ///
    /// API Key 已由向导写回 secrets.env（llm_d 热加载），此处仅记录日志。
    pub fn apply_wizard_result(&mut self, r: &crate::wizard::WizardResult) {
        if !r.model.is_empty() {
            self.model = r.model.clone();
            persist_model(&self.model);
            self.add_log(
                "INFO",
                format!("向导配置：模型 {}（provider={}）", self.model, r.provider),
            );
        } else {
            self.add_log("INFO", "向导配置：未指定模型，使用网关默认模型".to_string());
        }
        if r.api_key_set {
            self.add_log(
                "INFO",
                format!("向导配置：API Key 已写入 secrets.env（provider={}）", r.provider),
            );
        }
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
        // 历史回填后光标置于末尾（readline 惯例）
        self.cursor = self.input.len();
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
        // 历史回填后光标置于末尾（readline 惯例）
        self.cursor = self.input.len();
    }

    // ─────────── 输入编辑（光标感知，readline 风格） ───────────

    /// 在光标处插入一个字符。
    pub fn input_insert_char(&mut self, c: char) {
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.insert(pos, c);
        self.cursor = pos + c.len_utf8();
    }

    /// 在光标处插入多字节文本（Alt+Enter 换行用）。
    pub fn input_insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.insert_str(pos, text);
        self.cursor = pos + text.len();
    }

    /// Backspace：删除光标前一个字符。
    pub fn input_backspace(&mut self) {
        if self.cursor == 0 || self.input.is_empty() {
            self.cursor = 0;
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        // 回退一个字符边界
        let start = match self.input[..pos].char_indices().next_back() {
            Some((i, _)) => i,
            None => return,
        };
        self.input.drain(start..pos);
        self.cursor = start;
    }

    /// Delete：删除光标后一个字符。
    pub fn input_delete_after(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos >= self.input.len() || !self.input.is_char_boundary(pos) {
            return;
        }
        let end = match self.input[pos..].char_indices().nth(1) {
            Some((i, _)) => pos + i,
            None => self.input.len(),
        };
        self.input.drain(pos..end);
    }

    /// ←：光标左移一个字符。
    pub fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.cursor = match self.input[..pos].char_indices().next_back() {
            Some((i, _)) => i,
            None => 0,
        };
    }

    /// →：光标右移一个字符。
    pub fn cursor_right(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos >= self.input.len() || !self.input.is_char_boundary(pos) {
            return;
        }
        self.cursor = match self.input[pos..].char_indices().nth(1) {
            Some((i, _)) => pos + i,
            None => self.input.len(),
        };
    }

    /// Home / Ctrl+A：光标到开头。
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// End / Ctrl+E：光标到末尾。
    pub fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Ctrl+W：删除光标前一个词（空白分隔）。
    pub fn input_delete_word_before(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos == 0 || !self.input.is_char_boundary(pos) {
            return;
        }
        let before = &self.input[..pos];
        // 先跳过词尾空白，再删到词首
        let mut end = before.len();
        while end > 0 {
            let prev = before[..end].chars().next_back().unwrap();
            if !prev.is_whitespace() {
                break;
            }
            end -= prev.len_utf8();
        }
        while end > 0 {
            let prev = before[..end].chars().next_back().unwrap();
            if prev.is_whitespace() {
                break;
            }
            end -= prev.len_utf8();
        }
        self.input.drain(end..pos);
        self.cursor = end;
    }

    /// Ctrl+U：删除光标前全部内容。
    pub fn input_delete_to_start(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos == 0 || !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.drain(..pos);
        self.cursor = 0;
    }

    /// Tab 补全：补全 / 命令或技能名。
    ///
    /// 取光标前的当前词，按前缀匹配候选；再次 Tab 在当前候选间循环
    /// （当前词已等于某候选时取下一个，天然支持循环，无需额外状态）。
    pub fn tab_complete(&mut self) {
        // 候选：/ 命令 + 本地技能名
        let mut cands: Vec<String> = vec![
            "/model".into(),
            "/hiairy".into(),
            "/help".into(),
            "/clear".into(),
            "/status".into(),
            "/memory".into(),
            "/skills".into(),
            "/board".into(),
            "/events".into(),
            "/chain".into(),
            "/daemons".into(),
            "/agents".into(),
            "/tools".into(),
            "/models".into(),
            "/mem".into(),
            "/rpc".into(),
        ];
        cands.extend(self.skills.list().into_iter().map(|s| s.name));
        if cands.is_empty() {
            return;
        }

        // 光标前的当前词（最后一段空白分隔 token）
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        let before = &self.input[..pos];
        let word_start = before.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        let prefix = before[word_start..].to_string();

        let matches: Vec<&String> = cands
            .iter()
            .filter(|c| c.starts_with(&prefix))
            .collect();
        if matches.is_empty() {
            return;
        }
        // 当前词已等于某候选 → 取下一个；否则取第一个匹配
        let next = match matches.iter().position(|c| c.as_str() == prefix) {
            Some(p) => matches[(p + 1) % matches.len()].clone(),
            None => matches[0].clone(),
        };
        self.input.replace_range(word_start..pos, &next);
        self.cursor = word_start + next.len();
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

    /// /status 命令：展示运行时状态总览（连接/版本/模型/用量/记忆/技能）。
    fn cmd_status(&mut self) {
        let conn = if self.connected {
            "ONLINE"
        } else {
            "OFFLINE"
        };
        let model = if self.model.is_empty() {
            "默认（网关 / llm_d 自动回落）".to_string()
        } else {
            self.model.clone()
        };
        let phase = self.flow_phase.label();
        let text = format!(
            "运行时状态\n  \
             连接: {}  v{}\n  \
             模型: {}\n  \
             阶段: {} · 回合: {} · Token: {} · 成本: ${:.4} · 耗时: {}\n  \
             记忆: {} 条 · 技能: {} 条\n  \
             配置: {}",
            conn,
            self.gateway_version.as_deref().unwrap_or("unknown"),
            model,
            phase,
            self.turn,
            self.tokens,
            self.cost,
            self.elapsed_time(),
            self.memory.len(),
            self.skills.len(),
            self.config_file
        );
        self.add_message(MessageRole::System, text);
        self.add_log("INFO", "状态查询（/status）".to_string());
    }

    /// /skills 命令：列出本地技能库（任务成功后自动沉淀的可复用技能）。
    fn cmd_skills(&mut self) {
        let list = self.skills.list();
        if list.is_empty() {
            self.add_message(
                MessageRole::System,
                "本地技能库为空：任务完成后经验会自动沉淀为可复用技能。".to_string(),
            );
            return;
        }
        let mut text = format!("本地技能库（{} 条）", list.len());
        for s in list.iter().take(12) {
            text.push_str(&format!(
                "\n  ✓ {}（{} · 复用 {} 次）：{}",
                s.name, s.category, s.success_count, s.summary
            ));
        }
        if list.len() > 12 {
            text.push_str(&format!("\n  … 另有 {} 条", list.len() - 12));
        }
        self.add_message(MessageRole::System, text);
    }

    /// /chain 命令：无参数列出 hall_store 任务（最新在前）；带 task_id 回放该任务
    /// 全部类别事件（按 gseq 因果序 = 决策链）。数据经 gateway hall.tasks/hall.replay。
    ///
    /// 结果异步返回：先给"读取中"提示，poll_chain 消费后渲染进对话区。
    fn cmd_chain(&mut self, input: &str) {
        let arg = input[6..].trim().to_string();
        if self.chain_pending.is_some() {
            self.add_message(
                MessageRole::System,
                "决策链查询进行中，请稍候…".to_string(),
            );
            return;
        }
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.chain_task = arg.clone();
        if arg.is_empty() {
            self.add_message(MessageRole::System, "正在读取任务列表…".to_string());
            tokio::spawn(async move {
                let _ = tx.send(ChainOutcome::Tasks(gw.hall_tasks().await));
            });
        } else {
            self.add_message(
                MessageRole::System,
                format!("正在回放决策链（task_id={}）…", arg),
            );
            tokio::spawn(async move {
                let _ = tx.send(ChainOutcome::Events(gw.hall_replay(&arg, None).await));
            });
        }
        self.chain_pending = Some(rx);
    }

    /// 消费 /chain 的异步结果并渲染进对话区。
    pub fn poll_chain(&mut self) {
        let Some(mut rx) = self.chain_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => match outcome {
                ChainOutcome::Tasks(Ok(tasks)) => {
                    if tasks.is_empty() {
                        self.add_message(
                            MessageRole::System,
                            "决策链为空：暂无任务文件（$AIRY_HOME/data/agentrt/hall）。"
                                .to_string(),
                        );
                        return;
                    }
                    let mut text = format!("任务列表（{} 个，最新在前）", tasks.len());
                    for t in tasks.iter().take(20) {
                        text.push_str(&format!(
                            "\n  {} · {} 事件 · {}",
                            t.task_id, t.event_count, t.latest_ts
                        ));
                    }
                    if tasks.len() > 20 {
                        text.push_str(&format!("\n  … 另有 {} 个（/chain <task_id> 查看决策链）", tasks.len() - 20));
                    } else {
                        text.push_str("\n  /chain <task_id> 查看决策链");
                    }
                    self.add_message(MessageRole::System, text);
                }
                ChainOutcome::Tasks(Err(e)) => {
                    self.add_message(MessageRole::System, format!("任务列表读取失败：{}", e));
                }
                ChainOutcome::Events(Ok(events)) => {
                    let tid = self.chain_task.clone();
                    if events.is_empty() {
                        self.add_message(
                            MessageRole::System,
                            format!("决策链为空：任务「{}」暂无事件。", tid),
                        );
                        return;
                    }
                    let mut text = format!("决策链「{}」（{} 条事件，gseq 因果序）", tid, events.len());
                    for e in events.iter().take(64) {
                        text.push_str(&format!("\n  {}", crate::panels::events::event_line(e, 96)));
                    }
                    if events.len() > 64 {
                        text.push_str(&format!("\n  … 另有 {} 条（详见 F7 事件流面板）", events.len() - 64));
                    }
                    self.add_message(MessageRole::System, text);
                }
                ChainOutcome::Events(Err(e)) => {
                    self.add_message(MessageRole::System, format!("决策链读取失败：{}", e));
                }
            },
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.chain_pending = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// /daemons：16 个 daemon 命名空间经 gateway health_check 聚合在线状态。
    ///
    /// 结果异步返回：先给"检查中"提示，poll_ops 消费后渲染进对话区。
    fn cmd_daemons(&mut self) {
        if self.ops_pending.is_some() {
            self.add_message(
                MessageRole::System,
                "运维命令执行中，请稍候…".to_string(),
            );
            return;
        }
        self.ops_label = "daemons".to_string();
        self.add_message(MessageRole::System, "正在检查 daemon 在线状态…".to_string());
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut results = Vec::new();
            for ns in OPS_DAEMON_NS {
                let r = gw
                    .rpc_call(&format!("{}.health_check", ns), serde_json::json!({}))
                    .await;
                results.push((ns.to_string(), r));
            }
            let _ = tx.send(OpsOutcome::Daemons(results));
        });
        self.ops_pending = Some(rx);
    }

    /// 通用运维方法调用（/agents /tools /models /mem /rpc 共用）。
    fn cmd_ops_call(&mut self, method: &str, params: serde_json::Value) {
        if self.ops_pending.is_some() {
            self.add_message(
                MessageRole::System,
                "运维命令执行中，请稍候…".to_string(),
            );
            return;
        }
        self.ops_label = method.to_string();
        self.add_message(
            MessageRole::System,
            format!("正在调用 {} …", method),
        );
        let method_owned = method.to_string();
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(OpsOutcome::Call(gw.rpc_call(&method_owned, params).await));
        });
        self.ops_pending = Some(rx);
    }

    /// /rpc <ns>.<method> [json]：通用 JSON-RPC 直调（对齐 C CLI /rpc）。
    fn cmd_rpc(&mut self, input: &str) {
        let rest = input[5..].trim().to_string();
        if rest.is_empty() {
            self.add_message(
                MessageRole::System,
                "用法：/rpc <ns>.<method> [json]（如 /rpc tool.list_tools）".to_string(),
            );
            return;
        }
        let (method, params) = match rest.split_once(char::is_whitespace) {
            Some((m, p)) => (m.trim().to_string(), p.trim().to_string()),
            None => (rest.clone(), String::new()),
        };
        if method.is_empty() || !method.contains('.') {
            self.add_message(
                MessageRole::System,
                format!("方法格式应为 <ns>.<method>，收到：{}", method),
            );
            return;
        }
        let params_val = if params.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&params)
                .unwrap_or_else(|_| serde_json::json!({ "raw": params }))
        };
        self.cmd_ops_call(&method, params_val);
    }

    /// 消费运维命令的异步结果并渲染进对话区。
    pub fn poll_ops(&mut self) {
        let Some(mut rx) = self.ops_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => match outcome {
                OpsOutcome::Daemons(results) => {
                    let online = results.iter().filter(|(_, r)| r.is_ok()).count();
                    let mut text = format!(
                        "daemon 在线状态（{} / {} 在线）",
                        online,
                        results.len()
                    );
                    for (ns, r) in results {
                        let (icon, st) = if r.is_ok() { ("✓", "在线") } else { ("✗", "离线") };
                        text.push_str(&format!("\n  {} {} {}", icon, st, ns));
                    }
                    self.add_message(MessageRole::System, text);
                }
                OpsOutcome::Call(Ok(v)) => {
                    let pretty = serde_json::to_string_pretty(&v)
                        .unwrap_or_else(|_| v.to_string());
                    self.add_message(
                        MessageRole::System,
                        format!("{} 结果：\n{}", self.ops_label, pretty),
                    );
                }
                OpsOutcome::Call(Err(e)) => {
                    self.add_message(
                        MessageRole::System,
                        format!("{} 调用失败：{}", self.ops_label, e),
                    );
                }
            },
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.ops_pending = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// hall 面板（看板/事件流）数据拉取：面板激活时 1s 节流刷新。
    ///
    /// 看板 = hall.board（work_hall 持久化实例 + 在线 agent）；
    /// 事件流 = hall.stream（全局 gseq 因果序，最新 512 条）。
    /// 数据经 gateway 统一转发，任何前端看到同一份状态。
    pub fn poll_hall(&mut self) {
        if self.active_panel != ActivePanel::Board && self.active_panel != ActivePanel::Events {
            return;
        }
        // 消费在途结果
        if let Some(mut rx) = self.hall_poll_rx.take() {
            match rx.try_recv() {
                Ok(HallPollOutcome::Board(r)) => match r {
                    Ok(b) => self.hall_board = Some(b),
                    Err(e) => log::warn!("hall.board 拉取失败: {}", e),
                },
                Ok(HallPollOutcome::Events(r)) => match r {
                    Ok(evts) => self.hall_events = evts,
                    Err(e) => log::warn!("hall.stream 拉取失败: {}", e),
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    self.hall_poll_rx = Some(rx);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
            }
        }
        // 节流：1s
        let now = Instant::now();
        if now.duration_since(self.last_hall_poll) < std::time::Duration::from_millis(1000) {
            return;
        }
        self.last_hall_poll = now;
        let gw = self.gateway.clone();
        let want_board = self.active_panel == ActivePanel::Board;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if want_board {
                let _ = tx.send(HallPollOutcome::Board(gw.hall_board().await));
            } else {
                let _ = tx.send(HallPollOutcome::Events(gw.hall_stream(512).await));
            }
        });
        self.hall_poll_rx = Some(rx);
    }

    /// 强制下次 poll_hall 立即拉取（F6/F7 进入面板时调用）。
    pub fn force_hall_refresh(&mut self) {
        self.last_hall_poll = Instant::now() - std::time::Duration::from_secs(10);
    }

    /* ---- 2026-08-17：F6/F7 面板交互（光标 + 过滤）---- */

    /// F6 看板光标下移（循环）。
    pub fn board_cursor_down(&mut self) {
        let n = self.board_visible_count();
        if n == 0 {
            self.board_cursor = 0;
            return;
        }
        self.board_cursor = (self.board_cursor + 1) % n;
    }

    /// F6 看板光标上移（循环）。
    pub fn board_cursor_up(&mut self) {
        let n = self.board_visible_count();
        if n == 0 {
            self.board_cursor = 0;
            return;
        }
        self.board_cursor = (self.board_cursor + n - 1) % n;
    }

    /// F6 看板状态过滤：空 = 全部；点按过滤后光标回零。
    pub fn board_set_filter(&mut self, filter: &str) {
        self.board_filter = filter.to_string();
        self.board_cursor = 0;
    }

    /// F6 看板当前可见条目数（应用过滤后；与面板渲染同序：最新在前）。
    pub fn board_visible_count(&self) -> usize {
        let Some(board) = &self.hall_board else {
            return 0;
        };
        let n = if self.board_filter.is_empty() {
            board.entries.len()
        } else {
            board
                .entries
                .iter()
                .filter(|e| e.state == self.board_filter)
                .count()
        };
        n
    }

    /// F6 看板当前选中条目的 execution_id（无则返回空；与渲染同序）。
    pub fn board_selected_exec(&self) -> String {
        let Some(board) = &self.hall_board else {
            return String::new();
        };
        let mut visible: Vec<&HallBoardEntry> = if self.board_filter.is_empty() {
            board.entries.iter().collect()
        } else {
            board
                .entries
                .iter()
                .filter(|e| e.state == self.board_filter)
                .collect()
        };
        visible.reverse(); // 与面板渲染一致：最新在前
        visible
            .get(self.board_cursor % visible.len().max(1))
            .map(|e| e.execution_id.clone())
            .unwrap_or_default()
    }

    /// F6 看板选中行 → 切回对话并回放该任务决策链（复用 /chain 逻辑）。
    pub fn board_view_selected(&mut self) {
        let exec = self.board_selected_exec();
        if exec.is_empty() {
            return;
        }
        self.active_panel = ActivePanel::Chat;
        self.cmd_chain(&format!("/chain {}", exec));
    }

    /// F7 事件流光标下移（循环）。
    pub fn events_cursor_down(&mut self) {
        let n = self.events_visible_count();
        if n == 0 {
            self.events_cursor = 0;
            return;
        }
        self.events_cursor = (self.events_cursor + 1) % n;
    }

    /// F7 事件流光标上移（循环）。
    pub fn events_cursor_up(&mut self) {
        let n = self.events_visible_count();
        if n == 0 {
            self.events_cursor = 0;
            return;
        }
        self.events_cursor = (self.events_cursor + n - 1) % n;
    }

    /// F7 事件流类别过滤：空 = 全部；点按过滤后光标回零。
    pub fn events_set_filter(&mut self, filter: &str) {
        self.events_filter = filter.to_string();
        self.events_cursor = 0;
    }

    /// F7 事件流当前可见条数（应用过滤后）。
    pub fn events_visible_count(&self) -> usize {
        if self.events_filter.is_empty() {
            return self.hall_events.len();
        }
        self.hall_events
            .iter()
            .filter(|e| e.category == self.events_filter)
            .count()
    }

    /// F7 事件流选中行 → 对话区展示完整事件 JSON（方便深读）。
    pub fn events_view_selected(&mut self) {
        let Some(e) = self.events_selected() else {
            return;
        };
        self.active_panel = ActivePanel::Chat;
        let pretty = serde_json::to_string_pretty(&e.content).unwrap_or_else(|_| e.content.to_string());
        self.add_message(
            MessageRole::System,
            format!("[{}:{}] 事件详情（task={}）\n{}", events_category_cn(&e.category), e.gseq, e.task_id, pretty),
        );
    }

    /// F7 事件流当前选中事件（与面板渲染同序：最新在前）。
    pub fn events_selected(&self) -> Option<HallEvent> {
        let mut visible: Vec<&HallEvent> = if self.events_filter.is_empty() {
            self.hall_events.iter().collect()
        } else {
            self.hall_events
                .iter()
                .filter(|e| e.category == self.events_filter)
                .collect()
        };
        visible.reverse(); // 与面板渲染一致：最新在前
        visible
            .get(self.events_cursor % visible.len().max(1))
            .map(|e| (*e).clone())
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
    fn chat_round(&mut self, input: &str) -> Result<()> {
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
    fn apply_stream_result(&mut self, input: String, res: Result<RunResponse>) {
        // 流式工具状态行 → 落为正式消息（先于最终回答；流式路径无 tool_trace，
        // 工具事件仅此一处可见）
        for line in std::mem::take(&mut self.stream_tool_events) {
            self.add_message(MessageRole::ToolCall, line);
        }
        // 思考链（reasoning_content）→ 落为 [Dual Think] 正式消息（折叠展示）
        if !self.stream_reasoning.is_empty() {
            let reasoning = std::mem::take(&mut self.stream_reasoning);
            self.add_message(MessageRole::System, reasoning);
        }
        // 流式结束：把已渲染的 streaming_text 落为正式消息（防止与 result 双写）
        if !self.streaming_text.is_empty() {
            // 内容已实时渲染在占位消息上；此处仅清理占位，避免重复上屏
            self.streaming_text.clear();
        }
        self.streaming_reveal = 0;
        // 复用普通对话的结果应用逻辑（模式判定/技能/记忆/GCCP 入口）
        self.apply_chat_result(input, res);
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
                    self.add_message(
                        MessageRole::Agent,
                        "（未产生回复：模型可能仅生成了思考内容，请重试）".to_string(),
                    );
                } else {
                    self.add_message(MessageRole::Agent, cleaned.clone());
                }
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
            stream_rx: None,
            tool_rx: None,
            finish: None,
        });
    }

    /// 发起流式对话请求（普通对话路径）：SSE 增量块经 channel 逐块渲染，
    /// 完整结果经 oneshot 回传后按 ChatRound 相同逻辑应用。
    fn start_stream_pending(&mut self, kind: PendingKind, messages: serde_json::Value) {
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
            finish: None,
        });
        // 占位消息：流式输出目标（chat.rs 按 streaming_text 增量渲染）
        self.streaming_text.clear();
        self.streaming_reveal = 0;
        self.stream_reasoning.clear();
        self.last_reveal_tick = Instant::now();
        self.stream_tool_events.clear();
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
            stream_rx: None,
            tool_rx: None,
            finish: None,
        });
    }

    /// 动作短语映射（与 C 版 airy_cli cli_tool_action 对齐）：对话只展示
    /// "正在做什么"，不暴露工具参数与返回内容；未知工具保留原名。
    fn tool_action(tool: &str) -> String {
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
    fn render_tool_event(evt_json: &str) -> Option<String> {
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
    /// 流式请求（StreamRound）：先消费 mpsc 增量块实时追加到 streaming_text，
    /// 再检查 oneshot 完整结果（结束信号）。
    pub fn poll_pending(&mut self) -> bool {
        if self.task_control == TaskControl::Paused {
            return true;
        }
        // 审批轮询：请求在途时查询 tool.pending（在借用 self.pending 前调用，
        // 且 poll_approvals 内部自判 pending 是否存在并做 1.5s 节流）
        self.poll_approvals();
        let Some(p) = &mut self.pending else {
            return false;
        };
        // ── 流式增量消费：SSE 块 → streaming_text（chat.rs 逐帧渲染）──
        if let Some(stream_rx) = &mut p.stream_rx {
            let mut got = false;
            while let Ok(chunk) = stream_rx.try_recv() {
                got = true;
                self.streaming_text.push_str(&chunk);
            }
            if got {
                log::trace!("stream chunk: total={} chars", self.streaming_text.len());
            }
        }
        // ── 打字机上屏：伪流式下制造逐字动效（每 tick 推进若干字符）──
        // reveal 只增不减；消费完一轮后再推进，避免与渲染竞争。
        // 字段级操作（非方法调用）：p 已借用 self.pending，避免整体借用冲突。
        {
            let total = self.streaming_text.chars().count();
            if self.streaming_reveal < total {
                let since = self.last_reveal_tick.elapsed().as_millis();
                if since >= 24 {
                    self.last_reveal_tick = Instant::now();
                    // 长文本提速：目标 8s 内上屏完，至少 1 字符/步
                    let speed =
                        (total as f64 / 8000.0 * 24.0).ceil().max(1.0) as usize;
                    self.streaming_reveal = (self.streaming_reveal + speed).min(total);
                }
            }
        }
        // ── 流式工具事件消费：tool_call/tool_result → 工具状态行 ──
        if let Some(tool_rx) = &mut p.tool_rx {
            while let Ok(evt) = tool_rx.try_recv() {
                // 思考链事件（__airy_evt:reasoning）→ 追加到 stream_reasoning
                // （增量块；gateway 逐块透传，实时上屏 + 落定折叠）
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&evt) {
                    if v.get("__airy_evt").and_then(|k| k.as_str()) == Some("reasoning") {
                        if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                            self.stream_reasoning.push_str(c);
                        }
                        continue;
                    }
                }
                if let Some(line) = App::render_tool_event(&evt) {
                    self.stream_tool_events.push(line);
                }
            }
        }
        // 先检查暂存结果：打字机上屏完成（reveal 追平）才落定
        if let Some(finish) = p.finish.take() {
            if self.streaming_reveal >= self.streaming_text.chars().count() {
                // reveal 追平：取走 kind，清 pending/loading，应用结果
                let kind =
                    std::mem::replace(&mut p.kind, PendingKind::ChatRound { input: String::new() });
                self.pending = None;
                self.loading = false;
                self.set_task_control(TaskControl::Running);
                log::info!("poll_pending: 打字机上屏完成，消费流式结果（kind={:?}）", kind);
                self.last_turn_elapsed = Some(self.turn_started);
                self.apply_result(kind, finish);
                return self.pending.is_some();
            }
            // 尚未追平：放回，下一 tick 继续推进 reveal
            p.finish = Some(finish);
            return true;
        }
        let outcome = match p.rx.try_recv() {
            Ok(o) => o,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return true,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                PendingOutcome::Run(Err(anyhow!("LLM 后台任务异常终止")))
            }
        };
        // 流式请求：结果已到达但打字机尚未上屏完 → 暂存 finish，
        // 等 reveal 追平（下一 tick）再落定，保证逐字动效完整走完
        let is_stream = matches!(p.kind, PendingKind::StreamRound { .. });
        let reveal_pending = self.streaming_reveal < self.streaming_text.chars().count();
        if is_stream && reveal_pending {
            log::debug!(
                "poll_pending: 流式结果已到，打字机尚未上屏完（{}/{}），暂存",
                self.streaming_reveal,
                self.streaming_text.chars().count()
            );
            p.finish = Some(outcome);
            return true;
        }
        // 取走 kind（避免 move 出借用），先清 pending/loading，再应用结果
        let kind = std::mem::replace(&mut p.kind, PendingKind::ChatRound { input: String::new() });
        self.pending = None;
        self.loading = false;
        self.set_task_control(TaskControl::Running);
        log::info!("poll_pending: 消费后台请求结果（kind={:?}）", kind);
        // 回合计时结算：回合分隔线展示本回合耗时（Worked for Ns）
        self.last_turn_elapsed = Some(self.turn_started);
        self.apply_result(kind, outcome);
        self.pending.is_some()
    }

    /// 审批轮询：后台请求进行中时查询 tool_d pending 审批列表。
    ///
    /// 被静态审批拒绝（如 shell_run）的工具会阻塞等待 tool.approve 决议
    /// （AIRY_TOOL_APPROVAL_MODE=interactive）。此处轮询工具侧 pending，
    /// 有新请求则上屏提示用户按 a/A/n 决议（Claude Code 风格 permission prompt）。
    fn poll_approvals(&mut self) {
        // 仅当后台请求进行中（工具执行可能正在等待审批）
        if self.pending.is_none() {
            return;
        }
        // 消费在途轮询结果（上次 spawn 的异步查询）
        if let Some(mut rx) = self.approval_poll_rx.take() {
            match rx.try_recv() {
                Ok(pending_list) => {
                    // 新请求：去重后上屏提示
                    for a in pending_list {
                        if !self.approvals.iter().any(|x| x.request_id == a.request_id) {
                            self.approvals.push(a.clone());
                            self.add_message(
                                MessageRole::System,
                                format!(
                                    "工具「{}」请求权限执行（agent: {}，参数: {}）\n按 A=始终允许 · a=允许本次 · n=拒绝",
                                    a.tool, a.agent_id, truncate_for_display(&a.params, 160)
                                ),
                            );
                            self.add_log("INFO",
                                format!("权限审批待决议: {} ({})", a.tool, a.request_id));
                        }
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    // 尚未返回：存回，下次继续
                    self.approval_poll_rx = Some(rx);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    // 查询任务异常结束：忽略，下个周期重新发起
                }
            }
        }
        // 节流发起新查询：每 1.5s 一次
        let now = std::time::Instant::now();
        if now.duration_since(self.last_approval_poll) < std::time::Duration::from_millis(1500) {
            return;
        }
        self.last_approval_poll = now;
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let list = gw.list_pending_approvals().await.unwrap_or_default();
            let _ = tx.send(list);
        });
        self.approval_poll_rx = Some(rx);
    }

    /// 决议一个审批请求（Claude Code 风格权限确认）。
    ///
    /// `decision` ∈ {"allow", "always", "deny"}。决议成功后从待办列表移除
    /// 并回显结果；失败（已超时/已决议）提示用户。
    pub fn approve_request(&mut self, decision: &str) {
        let Some(a) = self.approvals.first() else {
            self.add_message(
                MessageRole::System,
                "当前没有待决议的权限请求。".to_string(),
            );
            return;
        };
        let request_id = a.request_id.clone();
        let tool = a.tool.clone();
        let gw = self.gateway.clone();
        let decision = decision.to_string();
        let mut approvals = std::mem::take(&mut self.approvals);
        approvals.remove(0);
        self.approvals = approvals;
        let label = match decision.as_str() {
            "always" => "始终允许",
            "allow" => "允许本次",
            _ => "拒绝",
        };
        self.add_message(MessageRole::System, format!("已{label}工具「{}」", tool));
        self.add_log("INFO", format!("权限决议: {} → {} ({})", request_id, label, tool));
        tokio::spawn(async move {
            if let Err(e) = gw.resolve_approval(&request_id, &decision).await {
                log::warn!("tool.approve 请求失败 ({}): {}", request_id, e);
            }
        });
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
            PendingKind::StreamRound { .. } => {
                self.streaming_text.clear();
                self.streaming_reveal = 0;
                self.stream_reasoning.clear();
                self.add_message(MessageRole::System, "已中止流式回复。".to_string());
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
            PendingKind::StreamRound { input } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_stream_result(input, res);
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
    /// 结构：系统判定指令 → 项目上下文（AGENTS.md 等价物）→ 召回的可复用技能
    /// → 相关记忆 → 用户输入。
    /// "是否进入任务集"由 LLM 判断：回复以 [MODE:TASK]/[MODE:CHAT]/[MODE:TASK:GCCP] 开头。
    /// 对话历史已改由 messages 数组承载（build_history_messages），不再挤进
    /// prompt 文本（M1/M2/M3 修复：网络层透传真实多轮上下文，上限 40 条）。
    fn build_context_prompt(&self, input: &str) -> String {
        // 2.3.4 宿主机时间注入：上下文感知当前时刻（日期/星期/时间），
        // 用户问时间类问题可直接作答，无需调用工具。每次拼接时取实时时间。
        let now = chrono::Local::now();
        let mut ctx = format!(
            "当前宿主机时间：{}（本地时区）。\n\
             你是 AirymaxRT 智能体运行底座（AgentRT Runtime）的助手。\n\
             请先判断本次请求意图，然后正常回答：\n\
             - 若属于普通对话（闲聊、问答、寒暄），回复以 [MODE:CHAT] 开头；\n\
             - 若属于需要多步执行、工具调用或复杂编排的任务集，回复以 [MODE:TASK] 开头；\n\
             - 若属于大型/高复杂度任务集（需先确认任务事实再执行），回复以 [MODE:TASK:GCCP] 开头；\n\
             - 任务集执行完成时，可在回复末尾追加 [TASK:DONE]。\n\n",
            now.format("%Y-%m-%d %H:%M:%S %:z")
        );

        // 项目上下文（AGENTS.md / CLAUDE.md 等价物）：工作目录约定最先注入，
        // 让 LLM 一开始就了解项目规范（P1 项，与 openlab 侧一致）
        if !self.project_context.is_empty() {
            ctx.push_str("【项目约定】\n");
            ctx.push_str(&self.project_context);
            ctx.push_str("\n\n");
        }

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
    /// 2026-08-17 F4 修复：改为基于**当前会话**消息（self.messages 的
    /// User/Agent 轮次）构建历史，不再注入跨会话 memory.recent(40)——后者
    /// 会把历史会话的旧记忆塞进上下文（msgs_len 高达 20+），污染当前问题，
    /// 导致「agentrt 不能理解我发送的信息」。
    ///
    /// 结构：[历史 User/Agent 交替轮次…, 增强 prompt（末条 user）]。
    /// 历史轮次来自当前会话；系统/工具消息不进入 LLM 上下文（工具过程
    /// 结果由 gateway 工具循环自行维护，前端消息仅作展示）。
    ///
    /// 连续同角色消息合并（OpenAI 要求 user/assistant 交替）。末条 user
    /// 记录——即 submit_input 刚写入的当前输入——由 `final_content`
    /// （增强 prompt）作为末条 user 消息承载，避免输入双注入。
    ///
    /// 仅剩当前输入时返回 None（退化为单条 prompt，走 gateway 旧路径）。
    fn build_history_messages(&self, final_content: &str) -> Option<serde_json::Value> {
        // 当前会话的对话轮次（User/Agent），正序；跳过系统/工具展示消息
        let mut msgs: Vec<serde_json::Value> = Vec::with_capacity(16);
        let mut last_role: Option<&str> = None;
        // 末条 user 消息是 submit_input 刚写入的当前输入（增强 prompt 的
        // 原始版），构建历史时跳过它，避免与 final_content 双写。
        let skip_last_user = self
            .messages
            .back()
            .map(|m| m.role == MessageRole::User)
            .unwrap_or(false);
        let total = self.messages.len();
        for (i, msg) in self.messages.iter().enumerate() {
            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Agent => "assistant",
                _ => continue,
            };
            if skip_last_user && i == total - 1 {
                continue;
            }
            if last_role == Some(role) {
                // 连续同角色（如用户连发多条）：合并进上一条，保持交替约束
                if let Some(last) = msgs.last_mut() {
                    let prev = last["content"].as_str().unwrap_or("").to_string();
                    last["content"] = serde_json::json!(format!("{}\n{}", prev, msg.content));
                }
                continue;
            }
            msgs.push(serde_json::json!({ "role": role, "content": msg.content }));
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

/// 事件类别中文化（F7 详情展示用，与 panels/events.rs category_label 对齐）。
fn events_category_cn(cat: &str) -> &'static str {
    match cat {
        "blueprint" => "蓝图",
        "command" => "命令",
        "progress" => "进度",
        "result" => "结果",
        "issue" => "问题",
        "verify" => "复核",
        "chain" => "决策",
        _ => "事件",
    }
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
        "  F6          - 任务看板（work_hall 执行实例 + 在线 agent，实时刷新）".to_string(),
        "  F7          - 事件流（全局 gseq 因果序回放）".to_string(),
        "  F8          - 切换到 CLI（airy_cli；CLI 中 /tui 切回）".to_string(),
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
        "  /status     - 运行时状态总览（连接/版本/模型/用量/记忆/技能）".to_string(),
        "  /skills     - 列出本地技能库（任务成功自动沉淀）".to_string(),
        "  /memory     - 记忆统计面板（F4 等价）".to_string(),
        "  /clear      - 清空对话区".to_string(),
        "  /help       - 显示帮助面板（F1 等价）".to_string(),
        "  Tab         - 补全 / 命令（Tab 再次循环候选）".to_string(),
        "  /board      - 任务看板面板（F6 等价）".to_string(),
        "  /events     - 事件流面板（F7 等价）".to_string(),
        "  /chain      - 决策链：无参列任务，/chain <task_id> 回放该任务决策链".to_string(),
        "  /daemons    - 16 个 daemon 在线状态（经 gateway health_check）".to_string(),
        "  /agents     - 已注册智能体（agent.list）".to_string(),
        "  /tools      - 可用工具（tool.list_tools）".to_string(),
        "  /models     - LLM 模型（llm.list_models）".to_string(),
        "  /mem        - 记忆统计；/mem <query> 语义检索".to_string(),
        "  /rpc        - 通用调用：/rpc <ns>.<method> [json]（如 /rpc tool.list_tools）".to_string(),
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
    use crate::memory::{JsonlMemory, MemoryRecord};

    /// 环境变量测试互斥锁（并行测试共享进程内 AIRY_HOME，必须串行）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// SSE 工具事件渲染：tool_call / tool_result JSON → 过程化状态行。
    /// 只展示动作名与成败，不暴露参数与返回内容（2026-08-17）。
    #[test]
    fn render_tool_event_parses_sse_json() {
        let call = r#"{"__airy_evt":"tool_call","tool":"web_search","args":{"query":"hello"}}"#;
        let line = App::render_tool_event(call).expect("tool_call renders");
        assert!(line.contains("web_search"), "line={}", line);
        assert!(line.contains("搜索网络"), "line={}", line);
        assert!(!line.contains("hello"), "参数不得暴露: line={}", line);
        assert!(!line.contains("调用工具"), "过程化后无旧文案: line={}", line);

        let result =
            r#"{"__airy_evt":"tool_result","tool":"web_search","call_id":"c1","ok":1,"summary":"3 results"}"#;
        let line = App::render_tool_event(result).expect("tool_result renders");
        assert!(line.contains("web_search"), "line={}", line);
        assert!(line.contains("完成"), "line={}", line);
        assert!(!line.contains("3 results"), "成功结果内容不得暴露: line={}", line);

        let fail =
            r#"{"__airy_evt":"tool_result","tool":"shell_run","call_id":"c2","ok":0,"summary":"boom"}"#;
        let line = App::render_tool_event(fail).expect("failed tool_result renders");
        assert!(line.contains("失败"), "line={}", line);
        assert!(line.contains("boom"), "失败应附短错误: line={}", line);

        // 非工具事件 / 非法 JSON → None（不污染对话）
        assert!(App::render_tool_event(r#"{"type":"ping"}"#).is_none());
        assert!(App::render_tool_event("not json").is_none());
    }

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

    /// --resume 会话恢复：记忆库 user/assistant 记录还原到消息列表。
    #[test]
    fn resume_session_restores_history() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("AIRY_HOME", dir.path());
        let mem_dir = dir.path().join("tui");
        std::fs::create_dir_all(&mem_dir).expect("create mem dir");
        let recs = [
            MemoryRecord {
                role: "user".into(),
                content: "上次的问题".into(),
                timestamp: "2026-08-08T10:00:00".into(),
                tags: "chat".into(),
            },
            MemoryRecord {
                role: "assistant".into(),
                content: "上次的回答".into(),
                timestamp: "2026-08-08T10:00:01".into(),
                tags: "chat".into(),
            },
            MemoryRecord {
                role: "system".into(),
                content: "不应恢复的系统消息".into(),
                timestamp: "2026-08-08T10:00:02".into(),
                tags: "chat".into(),
            },
        ];
        let path = mem_dir.join("memory.jsonl");
        let mut lines = String::new();
        for r in &recs {
            lines.push_str(&serde_json::to_string(r).expect("serialize"));
            lines.push('\n');
        }
        std::fs::write(&path, lines).expect("write memory");

        let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
            .expect("gateway client");
        let mut app = App::new("agents/main.agent.yaml", gw);
        // build_memory 在 memoryrovol feature 下优先 MemoryRovol 后端（不读 JSONL），
        // 此处显式注入 JsonlMemory 以验证 resume_session 的恢复逻辑本身。
        app.memory = Box::new(JsonlMemory::new(Some(&mem_dir)).expect("jsonl memory"));
        let n = app.resume_session();
        // user + assistant 共 2 条恢复；system 跳过
        assert_eq!(n, 2);
        let contents: Vec<String> =
            app.messages.iter().map(|m| m.content.clone()).collect();
        assert!(contents.iter().any(|c| c.contains("上次的问题")));
        assert!(contents.iter().any(|c| c.contains("上次的回答")));
        assert!(contents.iter().any(|c| c.contains("已恢复上次会话")));
        assert!(!contents.iter().any(|c| c.contains("不应恢复的系统消息")));
    }

    /// 项目上下文：AGENTS.md 等价物向上查找并注入。
    #[test]
    fn load_project_context_finds_agents_md() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        // 模拟项目根：.git 目录 + AGENTS.md
        std::fs::create_dir_all(dir.path().join(".git")).expect("create .git");
        std::fs::write(dir.path().join("AGENTS.md"), "项目约定：优先使用相对路径")
            .expect("write AGENTS.md");
        // 嵌套子目录：从子目录向上查找
        let sub = dir.path().join("src/sub");
        std::fs::create_dir_all(&sub).expect("create sub");

        let gw = crate::client::GatewayClient::new("http://127.0.0.1:1")
            .expect("gateway client");
        let mut app = App::new("agents/main.agent.yaml", gw);
        assert!(app.load_project_context(Some(&sub)));
        assert!(app.project_context.contains("项目约定"));
        assert!(app.project_context.contains("AGENTS.md"));
    }
}