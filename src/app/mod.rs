// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Application state for the AgentRT TUI.

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::time::Instant;

use crate::client::{
    GatewayClient, GccpQuestion, HallBoard, HallBoardEntry, HallEvent, HallTask, PendingApproval,
    RunResponse,
};
use crate::engine::sched::Lane;
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

use config::*;
pub use mode::{sanitize_reply, ModeMarker, StreamSanitizer};
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
    /// 思考链独立视图（0.1.18 B4）：模型推理原文默认不上屏、不落记忆，
    /// 仅在用户显式请求（Alt+E）时于本视图中按需取用。
    Think,
    /// 焦点视图（0.1.18 B11/W14）：最近一条回复的全屏只读覆盖层，
    /// 独立滚动，Esc 退出恢复原视口。
    Focus,
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
    /// 翻页步长 SSoT（0.1.18 B11/W14）：= 对话区当前视口高度，渲染每帧
    /// 回写，终端 resize 后自动跟随（V11.2）；PgUp/PgDn 与鼠标翻页均消费
    /// 此值，固定常量翻页已废除。首帧前缺省值仅作兜底。
    pub page_step: u16,
    /// 对话内容可滚总量（0.1.18 B11/W14）：总行数 - 视口高度，渲染每帧
    /// 回写。滚动步进据此钳位：内容未超出视口时（=0）滚动为 no-op，
    /// 不再积累脏偏移（V11.3）。
    pub chat_scroll_max: u16,
    /// 鼠标滚轮捕获开关（0.1.18 B11/W14）：默认关（不牺牲终端原生文本
    /// 选择）；Ctrl+M 会话级切换。开启后滚轮滚动对话（Shift 加速 /
    /// Ctrl 细粒度），关闭即还原捕获（V11.1）。
    pub mouse_capture: bool,
    /// 焦点视图消息快照（0.1.18 B11/W14）：Alt+F 打开时取最近一条回复
    /// 克隆，渲染与滚动均基于快照，不受后续消息影响；Esc 退出即弃。
    pub focus_msg: Option<ChatMessage>,
    /// 焦点视图滚动偏移（正文首行下标，0 = 顶部）。
    pub focus_scroll: usize,
    /// 焦点视图可滚总量（总行数 - 视口高度，渲染每帧回写，钳位用）。
    pub focus_scroll_max: usize,
    /// 渲染降级标志（0.1.18 §5A.3 W10）：合成层连续故障（渲染回调 panic /
    /// 后端写失败）时为真，状态条显示降级横幅；成功一帧即自愈转假。由主循环
    /// 每轮从合成层回写——本字段只作呈现，不参与任何判定。
    pub render_degraded: bool,
    /// 浏览态是否展开全部折叠（长系统消息）。0.1.7：折叠与滚动解耦——
    /// 滚动基于稳定的折叠视图，不再"一滚就展开导致视口跳变"。
    /// 0.1.18 B4：Alt+E 改用于打开思考链独立视图，本开关改由 Alt+O 切换。
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
    /// F3 日志面板滚动偏移（距最新的条目数，0 = 最新在顶；↑/↓ 调整）
    pub logs_scroll: usize,
    /// Help text cached
    pub help_text: Vec<String>,
    /// 当前对话/任务模型（/model <name> 设置并持久化；空 = 由网关/llm_d 回落默认）
    pub model: String,
    /// 用户模型配置文件路径（展示用）
    pub config_file: String,
    /// Whether we are currently loading (waiting for response)
    pub loading: bool,
    /// 本次请求发出时刻（0.1.18 B2-7：`begin_busy` 置位）。状态行据此
    /// 即时展示「已受理 · N.Ns」，使请求发出后立刻有可见反馈并给出
    /// 分段时间证据（V2.3；与上一回合结算的 `last_turn_elapsed` 分开）。
    pub busy_started: Instant,
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
    /// Skills 共享技能库（经网关 mem.*，与 CLI 同源；任务成功后自动沉淀经验）
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
    /// 流式输出：当前正在流式追加的 Agent 回复文本（chat.rs 增量渲染）。
    /// 增量到达即整块上屏，无本地上屏动画（0.1.18 §5A.3 W4 B2）。
    pub streaming_text: String,
    /// 流式工具循环事件（SSE __airy_evt 渲染行，如 `[Sub web_search Agent] …`）
    pub stream_tool_events: Vec<String>,
    /// 流式思考链（SSE `__airy_evt:reasoning` 事件携带的 reasoning_content，
    /// thinking 模型的思考过程）。
    /// 0.1.18 B4：原文**默认不上屏**（chat 区仅显示一行"思考中…"进度），
    /// 也不再生成 System 消息；仅由用户显式请求（Alt+E / `/think`）打开的
    /// 思考链独立视图取用，流式期间即该视图的实时数据源。
    /// 2026-08-17 F6 新增（gateway 透传 reasoning_content）。
    pub stream_reasoning: String,
    /// 流式思考链的模型轨（SSE reasoning 事件 model 字段，2.3.14）：
    /// 匹配 AIRY_MODEL_T2/T1F/T1P 显示 [Dual Slow/Fast/Prof Think]。
    pub stream_reasoning_model: String,
    /// 思考阶段开始时刻（首个 reasoning 增量到达时记录；chat 流式状态行
    /// 显示耗时，2026-08-19 与 C 版 CLI 的 "N 字 · T.Ts" 进度对齐）。
    pub stream_reasoning_start: Option<Instant>,
    /// 0.1.18 B4：最近一轮思考链原文（仅内存副本，会话结束即弃）。
    /// 思考链不再上屏、不再随 assistant 记录落盘，本字段只供思考链独立
    /// 视图（ActivePanel::Think）按需取用，保证 Alt+E 查看能力不丢失。
    pub last_reasoning: Option<String>,
    /// 0.1.18 B4：思考链独立视图的正文滚动偏移（正文首行下标，0 = 顶部）。
    /// 打开视图（Alt+E）时归零；↑/↓ 步进、PgUp/PgDn 翻页。
    pub think_scroll: usize,
    /// 0.1.8：本轮流式错误（SSE `__airy_evt:error` 事件携带的 message，
    /// gateway 把 llm_d 错误信封/不可达转为可读文本）。落定时以 Err 形式
    /// 呈现（System 一行摘要），杜绝原始 JSON 上屏。
    pub stream_error: Option<String>,
    /// 0.1.18 B3：流式控制面净化器——[MODE:*] 标记与前导元话语在流式
    /// 中间态亦零上屏（V3.1），剥出文本暂存 protocol_buf，落定时入
    /// F3 诊断通道（V3.2）。
    stream_sanitizer: StreamSanitizer,
    /// 待人工决议的工具审批请求（tool.pending 轮询；Claude Code 风格 permission prompt）
    pub approvals: Vec<PendingApproval>,
    /// 项目上下文文件内容（AGENTS.md / CLAUDE.md，注入 build_context_prompt）
    pub project_context: String,
    /// 审批轮询在途请求（spawn 后异步返回，下次 poll 消费结果）
    approval_poll_rx: Option<tokio::sync::oneshot::Receiver<Vec<PendingApproval>>>,
    /// 2026-08-17：F8 请求切换到 CLI（airy_cli）——主循环收到标志后
    /// 恢复终端并以 exec 语义替换当前进程（见 main.rs run_tui）。
    pub switch_to_cli: bool,
    /// 任务看板缓存（hall.board 最近一次成功拉取；Board 面板 1s 节流刷新）
    pub hall_board: Option<HallBoard>,
    /// 事件流缓存（hall.stream 最近一次拉取，最新在前）
    pub hall_events: Vec<HallEvent>,
    /// hall 面板显式刷新请求（面板切换 / SSE 推送触发）。置位后由主循环取走并
    /// 把 Hall 节拍提前到下一帧——拉取的**时刻判定仍归 L4 调度器**，此处只记
    /// 「有刷新需求」，不另存一份时钟（0.1.18 §5A.3 W4 节拍权威）。
    hall_force: bool,
    /// hall 面板在途请求（spawn 后异步返回，下次 poll_hall 消费结果）
    hall_poll_rx: Option<tokio::sync::oneshot::Receiver<HallPollOutcome>>,
    /// hall.watch SSE 推送流接收端（2026-08-21：事件流驱动，替代纯轮询；
    /// Board/Events 面板激活时订阅，离开时 drop 以结束 watch 任务）
    hall_watch_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// hall 面板最近一次拉取失败信息（None = 尚无失败或上次已成功；
    /// board.rs 据此区分"正在加载"与"拉取失败/离线"两种空态）
    pub hall_error: Option<String>,
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
    /// 异步数据落地档位（0.1.18 A 轨 §5A.3 W4）：poll_* 消费到新内容
    /// （流式增量/工具事件/结果落定/面板数据/后台轮询）时按来源记档，由主循环
    /// 取走并清零，作为 L4 调度器的成帧输入——静止界面因此不再产出绘制帧。
    /// 同一批内多来源落地只保留最高优先级档位（渲染读最新状态，一帧足矣）。
    landed: Option<Lane>,
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

/// /daemons 探测清单（0.1.9 M4 整编后口径）：14 个可经 gateway FWD 探测
/// 的业务 daemon 命名空间。plugin/info/observe 为整编兼容别名（→tool/
/// monit），列入即同一 daemon 重复计数；gateway_d 自身以顶部连接状态呈现
/// （连接断开时 /daemons 本就不可达）；maths_d 随 0.1.9 M4 补入。
const OPS_DAEMON_NS: [&str; 14] = [
    "agent", "tool", "think", "monit", "sched", "channel", "market", "llm", "cupolas", "mem",
    "notify", "hook", "a2a", "maths",
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
        // 记忆后端需在 gateway move 进 Self 之前取一份 clone
        // （TUI 记忆统一走网关 mem.*，与 CLI 同一后端）。
        let memory_backend = memory::build_memory(&gateway);
        log::info!(
            "memory: backend={} {} records hydrated",
            memory_backend.backend_name(),
            memory_backend.len()
        );
        // 技能库同源：统一走网关 mem.*（metadata.kind="skill" 分区），
        // 与 CLI 共享同一记忆后端；同样需在 gateway move 前进 Self 前构造。
        let skills_backend = skills::build_skill_store(&gateway);
        log::info!(
            "skills: backend={} {} skills loaded",
            skills_backend.backend_name(),
            skills_backend.len()
        );
        Self {
            agent_file: agent_file.to_string(),
            messages: VecDeque::with_capacity(MAX_CHAT_MESSAGES),
            input: String::new(),
            cursor: 0,
            active_panel: ActivePanel::Chat,
            scroll_offset: 0,
            page_step: 10,
            chat_scroll_max: 0,
            mouse_capture: false,
            focus_msg: None,
            focus_scroll: 0,
            focus_scroll_max: 0,
            render_degraded: false,
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
            logs_scroll: 0,
            help_text: build_help_text(),
            model: load_saved_model().unwrap_or_default(),
            config_file: format!("{}/config/model.yaml", airy_home()),
            loading: false,
            busy_started: Instant::now(),
            status_message: "Press Enter to start".to_string(),
            task_mode: false,
            memory: memory_backend,
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
            skills: skills_backend,
            wizard: wizard::WizardState::new(),
            pending: None,
            task_control: TaskControl::Running,
            input_history: Vec::with_capacity(16),
            history_pos: None,
            streaming_text: String::new(),
            stream_tool_events: Vec::new(),
            stream_reasoning: String::new(),
            stream_reasoning_model: String::new(),
            stream_reasoning_start: None,
            last_reasoning: None,
            think_scroll: 0,
            stream_error: None,
            stream_sanitizer: StreamSanitizer::new(),
            approvals: Vec::new(),
            project_context: String::new(),
            approval_poll_rx: None,
            switch_to_cli: false,
            hall_board: None,
            hall_events: Vec::new(),
            hall_force: false,
            hall_poll_rx: None,
            hall_watch_rx: None,
            hall_error: None,
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
            landed: None,
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

    /// 记一次异步落地（0.1.18 A 轨 §5A.3 W4）：按**数据来源**记档，档位即 L4
    /// 调度器里的渲染优先级（输入回显 > 流式输出 > 面板刷新 > 后台轮询）。
    /// 同一批内多来源落地时保留最高优先级档位——一帧渲染读的是最新状态。
    pub(crate) fn mark_landed(&mut self, lane: Lane) {
        self.landed = Some(match self.landed {
            Some(prev) => prev.min(lane),
            None => lane,
        });
    }

    /// 取走异步数据落地档位（读后清零）：主循环据此把对应优先级档位入调度队列
    /// （0.1.18 A 轨 §5A.3 W4，L4 调度器的成帧输入之一）。无落地返回 `None`。
    pub fn take_landed(&mut self) -> Option<Lane> {
        self.landed.take()
    }
}

#[cfg(test)]
mod tests;
