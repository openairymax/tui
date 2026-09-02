// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 上下文构建：系统提示拼装与历史消息序列化。

use super::*;

impl App {
    /// 构造发送给 LLM 的增强 prompt。
    ///
    /// 结构：系统判定指令 → 项目上下文（AGENTS.md 等价物）→ 召回的可复用技能
    /// → 相关记忆 → 用户输入。
    /// "是否进入任务集"由 LLM 判断：回复以 [MODE:TASK]/[MODE:CHAT]/[MODE:TASK:GCCP] 开头。
    /// 对话历史已改由 messages 数组承载（build_history_messages），不再挤进
    /// prompt 文本（M1/M2/M3 修复：网络层透传真实多轮上下文，上限 40 条）。
    pub(super) fn build_context_prompt(&self, input: &str) -> String {
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
        // 防自我回灌（MemoryRovol 后端在 C 库内检索，无法在 recall 层排除；
        // 此处统一过滤与当前输入相同的命中，避免"模型读到自己刚收到的输入
        // 的记忆"的回声污染，与 JsonlMemory::recall 的排除语义对齐）。
        let hits: Vec<_> = hits
            .into_iter()
            .filter(|h| !h.content.trim().eq_ignore_ascii_case(input.trim()))
            .collect();
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
    pub(super) fn build_history_messages(&self, final_content: &str) -> Option<serde_json::Value> {
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
}
