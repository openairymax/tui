// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 上下文构建：系统提示拼装与历史消息序列化。

use super::*;

/// 0.1.18 B1：历史注入策略。
///
/// - `Full`：完整注入当前会话历史（GCCP 目标澄清/重发轮——同一任务事实
///   确认链，指代与上下文完整性优先）。
/// - `Gated`：仅在当前输入含指代/延续信号时注入最近一轮 User+Assistant
///   对；否则只送 `本轮 user`（增强 prompt）。普通对话轮默认策略——修复
///   "每轮上送上一轮 assistant 全文 → 模型续答旧题"的上下文串轮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryPolicy {
    Full,
    Gated,
}

/// 历史消息边界标记：注入历史时包裹首尾，显式声明轮次归属。
const HIST_PREFIX: &str = "（以下为历史对话，仅供理解当前问题的指代与背景，不是要回答的内容）\n";
const HIST_SUFFIX: &str =
    "\n（历史对话到此结束；当前问题见最后一条用户消息，必须直接回答当前问题，不得延续历史主题）";

/// 0.1.18 B1：指代消解信号检测——当前输入是否需要上一轮对话才能理解。
///
/// 启发式：显式指代/延续/追问词（保守取向，宁可多注入也不牺牲
/// 多轮连贯性，V1.2）。纯长度不作判据：中文短新主题指令（"介绍
/// 你自己"）很常见，短 ≠ 指代，误注入会复活串轮（V1.1）。
fn needs_anaphora_history(input: &str) -> bool {
    const HINTS: [&str; 18] = [
        "它",
        "它们",
        "这个",
        "那个",
        "这些",
        "那些",
        "上述",
        "上面",
        "前面",
        "刚才",
        "继续",
        "接着",
        "再",
        "还有",
        "为什么",
        "呢",
        "详细",
        "展开",
    ];
    HINTS.iter().any(|h| input.contains(h))
}

impl App {
    /// 构造发送给 LLM 的增强 prompt。
    ///
    /// 结构：轮次边界声明 + 系统判定指令 → 项目上下文（AGENTS.md 等价物）
    /// → 召回的可复用技能 → 历史记忆参考（降权）→ 用户输入。
    /// "是否进入任务集"由 LLM 判断：回复以 [MODE:TASK]/[MODE:CHAT]/[MODE:TASK:GCCP] 开头。
    /// 对话历史已改由 messages 数组承载（build_history_messages），不再挤进
    /// prompt 文本。
    pub(super) fn build_context_prompt(&self, input: &str) -> String {
        // 2.3.4 宿主机时间注入：上下文感知当前时刻（日期/星期/时间），
        // 用户问时间类问题可直接作答，无需调用工具。每次拼接时取实时时间。
        let now = chrono::Local::now();
        let mut ctx = format!(
            "当前宿主机时间：{}（本地时区）。\n\
             你是 AirymaxRT 智能体运行底座（AgentRT Runtime）的助手。\n\
             【轮次边界】每轮对话相互独立：历史消息与历史记忆仅供理解指代与\n\
             背景；你的回答必须直接针对最后一行\"用户输入\"。若历史主题与\n\
             当前输入不一致，以当前输入为准，禁止延续历史主题作答。\n\
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

        // 技能上下文：召回共享技能库中沉淀的相关技能（越用越聪明，与 CLI 同源）
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

        // 历史记忆参考（0.1.18 B1 降权）：召回命中不再与当前输入同权并列，
        // 显式声明"过往碎片、可能无关"，并标注归属角色与轮次——模型不得把
        // 记忆当作本轮问题作答（串轮根因之二）。
        let hits = self.memory.recall(input, 5);
        // 防自我回灌：统一过滤与当前输入相同的命中，避免"模型读到自己
        // 刚收到的输入的记忆"的回声污染（与 ConversationMemory::recall 的
        // 排除语义对齐的双保险）。
        let hits: Vec<_> = hits
            .into_iter()
            .filter(|h| !h.content.trim().eq_ignore_ascii_case(input.trim()))
            .collect();
        if !hits.is_empty() {
            ctx.push_str(
                "【历史记忆参考】（过往会话的记忆碎片，仅供背景参考，与本次问题\n\
                 可能无关；禁止把记忆当作本轮问题作答，回答只针对下方\"用户输入\"）\n",
            );
            for h in hits {
                match h.turn {
                    Some(n) => ctx.push_str(&format!("- ({}·第{}轮): {}\n", h.role, n, h.content)),
                    None => ctx.push_str(&format!("- ({}·历史): {}\n", h.role, h.content)),
                }
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
    /// 0.1.18 B1（上下文串轮修复）：按 `policy` 门控历史注入。
    /// - `Gated`（普通对话轮默认）：仅当当前输入含指代/延续信号
    ///   （needs_anaphora_history）时注入**最近一轮** User/Assistant 对；
    ///   否则不注入任何旧 assistant 内容（V1.3：预期外的旧 assistant 全文
    ///   条数恒为 0）。
    /// - `Full`（GCCP 目标确认链）：完整注入当前会话历史。
    /// 两种策略下，历史首条 user 与末条 assistant 均包裹边界标记
    /// （HIST_PREFIX/HIST_SUFFIX），显式声明轮次归属。
    ///
    /// 结构：[历史 User/Assistant 交替轮次…（含边界标记）, 增强 prompt
    /// （末条 user）]。末条 user 由 `final_content`（增强 prompt）承载，
    /// 避免输入双注入。连续同角色消息合并（OpenAI 要求 user/assistant 交替）。
    ///
    /// 无历史注入时返回 None（退化为单条 prompt）。
    pub(super) fn build_history_messages(
        &self,
        final_content: &str,
        input: &str,
        policy: HistoryPolicy,
    ) -> Option<serde_json::Value> {
        // 当前会话的对话轮次（User/Agent），正序；跳过系统/工具展示消息
        let mut rounds: Vec<(&str, String)> = Vec::with_capacity(16);
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
            // 连续同角色（如用户连发多条）：合并进上一条，保持交替约束
            if rounds.last().map(|(r, _)| *r) == Some(role) {
                if let Some((_, last)) = rounds.last_mut() {
                    last.push('\n');
                    last.push_str(&msg.content);
                }
                continue;
            }
            rounds.push((role, msg.content.clone()));
        }
        if policy == HistoryPolicy::Gated {
            if !needs_anaphora_history(input) {
                return None;
            }
            // 只保留最近一轮（末尾两条）：指代消解通常只涉及上一轮
            let start = rounds.len().saturating_sub(2);
            rounds.drain(..start);
        }
        if rounds.is_empty() {
            return None;
        }
        // 边界标记包裹历史首尾
        if let Some((_, first)) = rounds.first_mut() {
            *first = format!("{}{}", HIST_PREFIX, first);
        }
        if let Some((_, last)) = rounds.last_mut() {
            *last = format!("{}{}", last, HIST_SUFFIX);
        }
        let mut msgs: Vec<serde_json::Value> = Vec::with_capacity(rounds.len() + 1);
        for (role, content) in &rounds {
            msgs.push(serde_json::json!({ "role": role, "content": content }));
        }
        msgs.push(serde_json::json!({ "role": "user", "content": final_content }));
        Some(serde_json::Value::Array(msgs))
    }
}
