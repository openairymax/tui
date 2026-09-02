// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话与多标签：项目上下文加载、历史会话恢复、向导接入、tab 槽位存取。

use super::*;

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
            let is_agent = matches!(role, MessageRole::Agent);
            self.add_message(role, rec.content.clone());
            /* 缺口 #7 修复：恢复 assistant 记录时还原其思考链（reasoning
             * 字段已随 JSONL 持久化，但此前恢复只取 content，恢复后的
             * 会话看不到历史思考链）。以系统消息形式注入，标注 [Dual Think]。 */
            if is_agent {
                if let Some(rz) = rec.reasoning.as_ref() {
                    if !rz.trim().is_empty() {
                        self.add_message(
                            MessageRole::System,
                            format!("[Dual Think] 上轮思考链：{}", rz),
                        );
                    }
                }
            }
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
        self.add_log(
            "INFO",
            format!(
                "向导配置：双思考系统{}（模型见 model.yaml think 段）",
                if r.think_enabled { "已启用" } else { "已关闭" }
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
        };
        self.session_tabs.push(tab);
        self.active_tab = Some(self.session_tabs.len() - 1);
        // 主字段切入新会话（save_current_tab 已搬空；无内容时兜底清空）
        self.messages.clear();
        self.input.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.streaming_text.clear();
        self.stream_reasoning.clear();
        self.stream_tool_events.clear();
        self.add_message(
            MessageRole::System,
            "已新建会话。Ctrl+T 再开新会话 · Alt+1..9 切换。".to_string(),
        );
        self.add_log(
            "INFO",
            format!("新建会话 tab {}（共 {} 个）", self.session_tabs.len(), self.session_tabs.len()),
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
        self.add_log("INFO", format!("切换到会话 tab {}（{}）", n, self.tab_title(target)));
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
        format!("sess_{:016x}_{:04x}", mix & 0xFFFFFFFFFFFFFFFF, (seq & 0xFFFF) as u16)
    }
}
