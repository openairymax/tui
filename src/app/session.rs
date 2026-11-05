// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话与多标签：项目上下文加载、历史会话恢复、向导接入、tab 槽位存取。

use super::*;

/// 会话落盘文件：`$AIRY_HOME/data/agentrt/tui/sessions.json`。
///
/// 落盘口径与 tab 槽位一致（title + 对话消息），只存有内容的会话；
/// 变更即存（新建/切换/关闭/重命名/退出），不依赖单一退出路径——
/// F8 交还 CLI、Ctrl+C、进程崩溃三种出口都能保住已发生的会话。
fn sessions_path() -> std::path::PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui", "sessions.json"])
}

/// 落盘上限：与 Alt+1..9 切换上限一致，超出丢最旧的非当前会话。
const TABS_MAX: usize = 9;

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionsFile {
    /// 当前会话在 tabs 数组（已过滤空会话）中的下标，恢复时以其为活动 tab
    current: usize,
    tabs: Vec<TabDto>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TabDto {
    title: String,
    messages: Vec<MsgDto>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MsgDto {
    role: String,
    content: String,
    timestamp: String,
}

fn role_to_str(r: MessageRole) -> &'static str {
    match r {
        MessageRole::User => "user",
        MessageRole::Agent => "agent",
        MessageRole::System => "system",
        MessageRole::ToolCall => "tool_call",
        MessageRole::ToolResult => "tool_result",
    }
}

fn role_from_str(s: &str) -> Option<MessageRole> {
    match s {
        "user" => Some(MessageRole::User),
        "agent" => Some(MessageRole::Agent),
        "system" => Some(MessageRole::System),
        "tool_call" => Some(MessageRole::ToolCall),
        "tool_result" => Some(MessageRole::ToolResult),
        _ => None,
    }
}

impl App {
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

    /// 会话恢复（--resume）：把记忆库最近的对话入回放队列，由主循环
    /// 100ms 节流逐条上屏（§6.1 #2），历史时间戳原样复用（§6.6 #5）。
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
            /* 0.1.18 B4：恢复会话不得回灌思考链原文——历史 assistant 记录
             * 的 reasoning 字段只取 content，推理原文不得借恢复通道上屏。 */
            self.replay
                .push_back((role, rec.content.clone(), rec.timestamp.clone()));
            count += 1;
        }
        self.replay_total = count;
        self.replay_at_ms = 0;
        log::info!("resume_session: queued {} messages", count);
        count
    }

    /// 回放泵（§6.1 #2）：每 `REPLAY_INTERVAL_MS` 上屏一条；排空时落
    /// 「已补账恢复 N 条」。时钟由主循环注入，模块内不读墙钟。
    pub fn pump_replay(&mut self, now_ms: u64) {
        if self.replay.is_empty() {
            return;
        }
        if now_ms.saturating_sub(self.replay_at_ms) < crate::budget::REPLAY_INTERVAL_MS {
            return;
        }
        self.replay_at_ms = now_ms;
        if let Some((role, text, stamp)) = self.replay.pop_front() {
            self.add_message_at(role, text, &stamp);
        }
        if self.replay.is_empty() {
            let total = self.replay_total;
            self.replay_total = 0;
            self.replay_at_ms = 0;
            self.add_message(
                MessageRole::System,
                format!("已补账恢复 {total} 条历史。继续对话或发送新消息。"),
            );
        }
    }

    /// 回放剩余条数（渲染骨架行「回放中 · N 条」）。
    pub fn replay_left(&self) -> usize {
        self.replay.len()
    }

    /// 重新打开首次启动向导（/hiairy 命令触发）。
    pub fn open_wizard(&mut self) {
        self.wizard.reopen();
    }

    /// 应用首次启动向导的快速配置结果：模型名写回 model.yaml 的 default_model
    /// （统一配置权威源，随 agent.run 请求下发）。
    ///
    /// API Key 与模型表已由向导直接写回 secrets.env / model.yaml（llm_d 热加载），
    /// 此处仅同步当前会话模型并记录日志。
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
                format!(
                    "向导配置：API Key 已写入 secrets.env（provider={}）",
                    r.provider
                ),
            );
        }
        self.add_log(
            "INFO",
            format!(
                "向导配置：双思考系统{}（模型见 model.yaml think 段）",
                if r.think_enabled {
                    "已启用"
                } else {
                    "已关闭"
                }
            ),
        );
    }

    /// 当前会话在 tab 列表中的索引（None = 主会话，即槽 0）。
    pub(crate) fn current_tab_index(&self) -> usize {
        self.active_tab.unwrap_or(0)
    }

    /// 主字段内容写回当前 tab 槽位（title 保留，搬移对话状态）。
    pub(super) fn save_current_tab(&mut self) {
        let i = self.current_tab_index();
        if let Some(tab) = self.session_tabs.get_mut(i) {
            tab.messages = std::mem::take(&mut self.messages);
            tab.input = std::mem::take(&mut self.input);
            tab.cursor = self.cursor;
            tab.scroll_offset = self.scroll_offset;
            tab.pastes = std::mem::take(&mut self.paste_folds);
            self.cursor = 0;
            self.scroll_offset = 0;
        }
    }

    /// 加载 tab 槽位内容到主字段（None = 主会话槽 0）。
    pub(super) fn load_tab(&mut self, i: usize) {
        let Some(tab) = self.session_tabs.get(i) else {
            return;
        };
        self.messages = tab.messages.clone();
        self.input = tab.input.clone();
        self.cursor = tab.cursor;
        self.scroll_offset = tab.scroll_offset;
        self.paste_folds = tab.pastes.clone();
        self.active_tab = if i == 0 { None } else { Some(i) };
    }

    /// tab 总数（含主会话，恒 ≥1）。
    pub fn tab_count(&self) -> usize {
        self.session_tabs.len()
    }

    /// 指定 tab 的展示标题（无标题回退「会话 N」；tab 栏渲染用）。
    pub fn tab_title(&self, i: usize) -> String {
        self.session_tabs
            .get(i)
            .map(|t| {
                if t.title.is_empty() {
                    format!("会话 {}", i + 1)
                } else {
                    t.title.clone()
                }
            })
            .unwrap_or_default()
    }

    /// Ctrl+T：新建会话 tab。当前对话（有内容时）保留为 tab，开启空白新会话。
    ///
    /// 执行态保护：请求进行中（loading/busy）不可新建——GCCP/GRAD/执行流
    /// 绑定主字段，切换会破坏进行中任务的上下文连续性。
    pub fn new_session_tab(&mut self) {
        if self.loading || self.pending.is_some() {
            self.add_message(
                MessageRole::System,
                "任务执行中不可新建会话（Ctrl+X 中止当前请求后可操作）。".to_string(),
            );
            return;
        }
        if !self.messages.is_empty() || !self.input.trim().is_empty() {
            self.save_current_tab();
        }
        let tab = SessionTab {
            title: String::new(),
            messages: VecDeque::new(),
            input: String::new(),
            cursor: 0,
            scroll_offset: 0,
            pastes: Vec::new(),
        };
        self.session_tabs.push(tab);
        self.active_tab = Some(self.session_tabs.len() - 1);
        // 主字段切入新会话（save_current_tab 已搬空；无内容时兜底清空）
        self.messages.clear();
        self.input.clear();
        self.cursor = 0;
        self.paste_folds.clear();
        self.scroll_offset = 0;
        self.streaming_text.clear();
        self.stream_reasoning.clear();
        self.stream_tool_events.clear();
        self.persist_tabs();
        self.add_message(
            MessageRole::System,
            "已新建会话。Ctrl+T 再开新会话 · Alt+1..9 切换 · /sessions 查看列表。".to_string(),
        );
        self.add_log(
            "INFO",
            format!(
                "新建会话 tab {}（共 {} 个）",
                self.session_tabs.len(),
                self.session_tabs.len()
            ),
        );
    }

    /// Alt+1..9：切换会话。Alt+1 = 主会话；Alt+N（N≥2）= 第 N 个 tab。
    pub fn switch_tab(&mut self, n: usize) {
        if n == 0 || n > self.session_tabs.len() {
            return;
        }
        if self.loading || self.pending.is_some() {
            self.add_message(
                MessageRole::System,
                "任务执行中不可切换会话（Ctrl+X 中止当前请求后可操作）。".to_string(),
            );
            return;
        }
        let target = n - 1;
        if self.current_tab_index() == target {
            return;
        }
        self.save_current_tab();
        self.load_tab(target);
        // 切换后清空流式残留，避免上一会话的增量污染新视口
        self.streaming_text.clear();
        self.stream_reasoning.clear();
        self.stream_tool_events.clear();
        self.add_log(
            "INFO",
            format!("切换到会话 tab {}（{}）", n, self.tab_title(target)),
        );
        self.persist_tabs();
    }

    /// 生成客户端预分配会话 ID（sess_ 前缀，gateway 校验后采用）。
    pub(super) fn new_session_id(&self) -> String {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        // 时间 + 会话序号 + 伪随机位（非密码学用途，仅保证唯一性）
        let seq = self.turn as u128;
        let mix = (now_ms ^ (seq << 32)) * 6364136223846793005;
        format!(
            "sess_{:016x}_{:04x}",
            mix & 0xFFFFFFFFFFFFFFFF,
            (seq & 0xFFFF) as u16
        )
    }

    /// 会话落盘：把全部 tab（当前会话取主字段实时内容）写入 sessions.json。
    ///
    /// 当前 tab 的槽位快照在切换时才回写，落盘时若直接用快照会把进行中的
    /// 对话存成上一帧状态，故当前槽一律以主字段为准。空会话不落盘——
    /// 重启后不该看到一排没有内容的标签。
    pub fn persist_tabs(&mut self) {
        let cur = self.current_tab_index();
        // 当前槽的实时消息在主字段（切换时才回写快照），落盘前先以主字段
        // 刷新当前槽——否则进行中的会话在文件里永远是上一帧。标题不回写：
        // 显式标题（派生/重命名）本就直写槽位，把 tab_title 的展示回落
        // （"会话 N"）实体化会堵死 submit_input 的首条消息派生路径。
        if let Some(t) = self.session_tabs.get_mut(cur) {
            t.messages = self.messages.clone();
        }
        let mut tabs: Vec<TabDto> = Vec::new();
        let mut current = 0usize;
        for (i, t) in self.session_tabs.iter().enumerate() {
            if t.messages.is_empty() {
                continue;
            }
            if i == cur {
                current = tabs.len();
            }
            tabs.push(TabDto {
                title: t.title.clone(),
                messages: t
                    .messages
                    .iter()
                    .map(|m| MsgDto {
                        role: role_to_str(m.role).to_string(),
                        content: m.content.clone(),
                        timestamp: m.timestamp.clone(),
                    })
                    .collect(),
            });
        }
        // 超上限丢最旧（当前会话除外），current 随之左移
        while tabs.len() > TABS_MAX {
            let drop_at = if current == 0 { 1 } else { 0 };
            tabs.remove(drop_at);
            if drop_at < current {
                current -= 1;
            }
        }
        let p = sessions_path();
        if let Some(dir) = p.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("persist_tabs: mkdir {} failed: {}", dir.display(), e);
                return;
            }
        }
        let doc = SessionsFile { current, tabs };
        match serde_json::to_string(&doc) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&p, s) {
                    log::warn!("persist_tabs: write {} failed: {}", p.display(), e);
                }
            }
            Err(e) => log::warn!("persist_tabs: serialize failed: {}", e),
        }
    }

    /// 会话恢复：从 sessions.json 还原 tab 列表（含当前会话内容）。
    ///
    /// 文件缺失/损坏时保持初始单 tab 并留日志——启动不因历史数据形态失败。
    /// 消息 id 由 msg_seq 重新单调分配（落盘不存 id，id 只服务本进程虚拟滚动）。
    pub fn restore_tabs(&mut self) {
        let p = sessions_path();
        let Ok(raw) = std::fs::read_to_string(&p) else {
            return;
        };
        let doc: SessionsFile = match serde_json::from_str(&raw) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("restore_tabs: parse {} failed: {}", p.display(), e);
                return;
            }
        };
        if doc.tabs.is_empty() {
            return;
        }
        let current = doc.current.min(doc.tabs.len() - 1);
        self.session_tabs.clear();
        for t in doc.tabs.into_iter().take(TABS_MAX) {
            let mut msgs: VecDeque<ChatMessage> = VecDeque::new();
            for m in t.messages {
                let Some(role) = role_from_str(&m.role) else {
                    continue;
                };
                self.msg_seq += 1;
                let original_bytes = m.content.len();
                msgs.push_back(ChatMessage {
                    role,
                    content: m.content,
                    timestamp: m.timestamp,
                    id: self.msg_seq,
                    original_bytes,
                });
            }
            self.session_tabs.push(SessionTab {
                title: t.title,
                messages: msgs,
                input: String::new(),
                cursor: 0,
                scroll_offset: 0,
                pastes: Vec::new(),
            });
        }
        // 退出时的活动会话即恢复后的当前会话：内容搬进主字段（与 load_tab 同口径）
        self.load_tab(current);
        log::info!("restore_tabs: {} 个会话", self.session_tabs.len());
    }

    /// /close：关闭当前会话 tab。
    ///
    /// 只剩主会话时清空对话即关闭语义（主会话槽 0 不可删——删光所有槽
    /// 会让 current_tab_index 悬空）。关闭后切到前一 tab 并落盘。
    pub fn close_tab(&mut self) {
        if self.loading || self.pending.is_some() {
            self.add_message(
                MessageRole::System,
                "任务执行中不可关闭会话（Ctrl+X 中止当前请求后可操作）。".to_string(),
            );
            return;
        }
        let cur = self.current_tab_index();
        if cur == 0 && self.session_tabs.len() == 1 {
            self.messages.clear();
            self.input.clear();
            self.cursor = 0;
            self.scroll_offset = 0;
            self.paste_folds.clear();
            if let Some(t) = self.session_tabs.get_mut(0) {
                t.title.clear();
                t.messages.clear();
            }
            self.persist_tabs();
            self.add_message(MessageRole::System, "当前会话已关闭（/new 新建）。".to_string());
            return;
        }
        let title = self.tab_title(cur);
        self.session_tabs.remove(cur);
        let next = cur.min(self.session_tabs.len() - 1);
        self.messages.clear();
        self.load_tab(next);
        self.streaming_text.clear();
        self.stream_reasoning.clear();
        self.stream_tool_events.clear();
        self.persist_tabs();
        self.add_message(
            MessageRole::System,
            format!("已关闭会话「{}」，切换到 {}。", title, self.tab_title(next)),
        );
    }

    /// /rename <标题>：重命名当前会话（tab 栏与落盘同步生效）。
    pub fn rename_tab(&mut self, title: &str) {
        let title = title.trim().to_string();
        if title.is_empty() {
            self.add_message(
                MessageRole::System,
                "用法：/rename <会话标题>".to_string(),
            );
            return;
        }
        let i = self.current_tab_index();
        if let Some(t) = self.session_tabs.get_mut(i) {
            t.title = title;
        }
        self.add_message(
            MessageRole::System,
            format!("会话已重命名为「{}」。", self.tab_title(i)),
        );
        self.persist_tabs();
    }

    /// /sessions：列出全部会话（序号 · 标题 · 消息数，当前会话标记）。
    pub fn list_tabs(&self) -> String {
        let cur = self.current_tab_index();
        self.session_tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let live = if i == cur {
                    self.messages.len()
                } else {
                    t.messages.len()
                };
                let mark = if i == cur { "◂" } else { " " };
                format!(
                    "{} [{}] {}（{} 条）",
                    mark,
                    i + 1,
                    self.tab_title(i),
                    live
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
