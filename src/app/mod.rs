// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Application state for the AgentRT TUI.

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::time::Instant;

use crate::client::{GccpQuestion, GatewayClient, HallBoard, HallBoardEntry, HallEvent, HallTask, PendingApproval, RunResponse};
use crate::gccp::{self, FlowPhase, GccpState, TaskControl};
use crate::ime::ImeEngine;
use crate::memory::{self, ConversationMemory};
use crate::skills::{self, SkillStore};
use crate::wizard;

// 应用状态按职责域分文件实现（0.1.9 W8c）：同一个 `impl App` 分散在子模块，
// 子模块以 `use super::*` 继承本模块的类型、常量与私有项，外部路径仍为 crate::app。
mod command;
mod config;
mod context;
mod control;
mod dispatch;
mod gccp_flow;
mod input;
mod mode;
mod panel;
mod poll;
mod session;
mod task;
mod text;

pub use mode::{parse_mode_detail, ModeMarker};
use config::*;
use text::*;

/// Maximum number of chat messages to keep in memory.
///
/// 0.1.9 W8：渲染改为按消息块虚拟滚动后，内存条数与每帧渲染行数解耦，
/// 上限从 500 提升至 2000（长会话保留更多历史，帧成本仍只与视口高度相关）。
pub(crate) const MAX_CHAT_MESSAGES: usize = 2000;

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
    /// 稳定消息 id（0.1.9 W8）：虚拟滚动的行高缓存键，单调分配、永不复用
    pub id: u64,
}

impl ChatMessage {
    /// 流式哨兵 id：不进缓存、不参与身份判断
    pub const NO_ID: u64 = u64::MAX;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// 会话 tab（2026-08-21 多会话）：对话核心状态的快照。
///
/// 轻量模型：App 主字段（messages/input/cursor/scroll）恒为"当前会话"；
/// 其他会话以快照存于 App.session_tabs。新建（Ctrl+T）/切换（Alt+1..9）
/// 时在快照与主字段间搬移，不触碰 GCCP/任务流等执行态（执行中不切换）。
pub struct SessionTab {
    pub title: String,
    pub messages: VecDeque<ChatMessage>,
    pub input: String,
    pub cursor: usize,
    pub scroll_offset: u16,
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
    /// 浏览态是否展开全部折叠（思考链/长回复）。0.1.7：折叠与滚动解耦——
    /// 滚动基于稳定的折叠视图，不再"一滚就展开导致视口跳变"；Ctrl+E 切换。
    pub browse_expanded: bool,
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
    /// 内置拼音输入法引擎（词典加载失败/库未链接时为 None → IME 禁用）
    pub ime_engine: Option<ImeEngine>,
    /// IME 拼音态：true = 输入法开启（a-z 进拼音缓冲，1-9/空格选字）
    pub ime_active: bool,
    /// 拼音缓冲（仅小写 [a-z]；ü 以 v 表示）
    pub ime_buf: String,
    /// 当前拼音的候选词（UTF-8，频次降序，0.1.3 起最多 27 个=3 页）
    pub ime_cands: Vec<String>,
    /// IME 分页（微信式，0.1.3）：当前页 / 总页数 / 页内高亮下标
    pub ime_page: usize,
    pub ime_pages: usize,
    pub ime_sel: usize,
    /// 任务流阶段（对话 / GCCP 任务事实确认 / GRAD 任务流程图确认 / 执行）
    pub flow_phase: FlowPhase,
    /// GCCP 五问状态（任务事实确认）
    pub gccp: GccpState,
    /// GCCP 两段式交互第一段挂起状态（P-A，None = 无挂起；见 GccpPending）
    pub gccp_pending: Option<GccpPending>,
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
    /// 流式思考链的模型轨（SSE reasoning 事件 model 字段，2.3.14）：
    /// 匹配 AIRY_MODEL_T2/T1F/T1P 显示 [Dual Slow/Fast/Prof Think]。
    pub stream_reasoning_model: String,
    /// 思考阶段开始时刻（首个 reasoning 增量到达时记录；chat.rs 状态行
    /// 显示耗时，2026-08-19 与 C 版 CLI 的 "N 字 · T.Ts" 进度对齐）。
    pub stream_reasoning_start: Option<Instant>,
    /// 2.1.1.6：本轮流式思考链待持久化副本（apply_stream_result 落屏后
    /// 保留，apply_chat_result 写记忆时随 assistant 记录落盘）。
    pub pending_reasoning: Option<String>,
    /// 0.1.8：本轮流式错误（SSE `__airy_evt:error` 事件携带的 message，
    /// gateway 把 llm_d 错误信封/不可达转为可读文本）。落定时以 Err 形式
    /// 呈现（System 一行摘要），杜绝原始 JSON 上屏。
    pub stream_error: Option<String>,
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
    /// hall.watch SSE 推送流接收端（2026-08-21：事件流驱动，替代纯轮询；
    /// Board/Events 面板激活时订阅，离开时 drop 以结束 watch 任务）
    hall_watch_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// 多会话 tab（2026-08-21）：其他会话快照；None = 主会话即当前会话
    pub session_tabs: Vec<SessionTab>,
    /// 当前显示的 tab 索引（None = 主会话；Some(n) = session_tabs[n]）
    pub active_tab: Option<usize>,
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
    /// 对话虚拟视图缓存（0.1.9 W8）：行高缓存随 App 生命周期，跨帧/tab 复用
    pub chat_view: crate::panels::chat::ChatView,
    /// 消息 id 单调发生器（缓存键，永不复用）
    msg_seq: u64,
    /// 记忆面板分组视图缓存（0.1.9 W8）：条数不变即复用，翻页仅移动窗口
    pub memory_view: crate::panels::memory::MemoryView,
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
        /// 待继续请求携带的 GCCP 交互答案 JSON（连接通过后透传）
        gccp_answers: Option<String>,
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

/// GCCP 两段式交互第一段挂起状态（P-A）：think.process 返回
/// gccp_need_interaction 后暂存问题集与原始请求，待用户作答后以
/// gccp_answers 重发同一 prompt 完成澄清闭环（见 think_service.h）。
pub struct GccpPending {
    /// 原始用户输入（重发时作为 ChatRound.input，保持记忆/模式判定语义）
    raw_input: String,
    /// 原始 prompt（与第一段发送的一致，重发时保持同一任务上下文）
    prompt: String,
    /// 原始完整对话历史（重发时透传 gateway）
    history: Option<serde_json::Value>,
    /// 服务端回传的问题集（id/question/hint/required；panels/chat.rs 渲染用）
    pub questions: Vec<GccpQuestion>,
    /// 已收集答案（key = 问题 id；panels/chat.rs 渲染进度用）
    pub answers: std::collections::BTreeMap<String, serde_json::Value>,
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
            browse_expanded: false,
            gateway,
            connected: false,
            gateway_version: None,
            turn: 0,
            tokens: 0,
            cost: 0.0,
            session_start: Instant::now(),
            turn_started: Instant::now(),
            last_turn_elapsed: None,
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
            ime_engine: {
                let e = ImeEngine::load();
                if e.is_none() {
                    log::warn!("ime: 输入法不可用（词典缺失或库未链接），F10 无效");
                }
                e
            },
            ime_active: false,
            ime_buf: String::new(),
            ime_cands: Vec::new(),
            ime_page: 0,
            ime_pages: 1,
            ime_sel: 0,
            flow_phase: FlowPhase::Chat,
            gccp: GccpState::default(),
            gccp_pending: None,
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
            stream_reasoning_model: String::new(),
            stream_reasoning_start: None,
            pending_reasoning: None,
            stream_error: None,
            approvals: Vec::new(),
            project_context: String::new(),
            last_approval_poll: Instant::now(),
            approval_poll_rx: None,
            switch_to_cli: false,
            hall_board: None,
            hall_events: Vec::new(),
            last_hall_poll: Instant::now(),
            hall_poll_rx: None,
            hall_watch_rx: None,
            session_tabs: vec![SessionTab {
                title: String::new(),
                messages: VecDeque::new(),
                input: String::new(),
                cursor: 0,
                scroll_offset: 0,
            }],
            active_tab: None,
            chain_pending: None,
            chain_task: String::new(),
            ops_pending: None,
            ops_label: String::new(),
            board_cursor: 0,
            board_filter: String::new(),
            events_cursor: 0,
            events_filter: String::new(),
            insert_queue: VecDeque::new(),
            chat_view: crate::panels::chat::ChatView::new(),
            msg_seq: 0,
            memory_view: crate::panels::memory::MemoryView::default(),
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

    /// Add a chat message.
    pub fn add_message(&mut self, role: MessageRole, content: String) {
        // 时间戳：HH:MM:SS（消息气泡头部展示）
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        self.msg_seq += 1;
        let msg = ChatMessage {
            role,
            content,
            timestamp,
            id: self.msg_seq,
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
}

#[cfg(test)]
mod tests;
