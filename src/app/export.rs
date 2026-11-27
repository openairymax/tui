// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 线性文本导出（0.1.21 X7-01a）：把会话快照摊成无装饰的纯文本。
//
// 读屏用户与管道消费（重定向、grep、二次加工）需要的是「谁说了什么」的
// 线性序列，而不是带边框与色块的屏上排版。本模块只做一件事：逐条消息
// 输出「角色> 正文」，多行正文续行缩进，控制字符经统一净化剥离；不写
// ANSI、不画表格、不依赖终端宽度，因此同一会话在任何终端上导出同一份
// 文本。
//
// 导出是只读旁路：不改会话状态、不触发 RPC；格式化与落盘分离，前者是
// 纯函数（可在 golden 中逐字节断言），后者只在命令入口发生。

use std::collections::VecDeque;

use super::*;
use crate::engine::sanitize;

/// 一条消息的角色标签（线性文本的唯一词表，避免各处自造译名）。
fn label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "用户",
        MessageRole::Agent => "助手",
        MessageRole::System => "系统",
        MessageRole::ToolCall => "工具调用",
        MessageRole::ToolResult => "工具结果",
    }
}

/// 多行正文续行缩进：让「角色> 」前缀只在首行出现一次。
const INDENT: &str = "  ";

/// 正文续行：首行随前缀，其余行缩进对齐；行尾空白与 CR 一并剥除。
fn push_body(out: &mut String, body: &str) {
    let clean = sanitize::strip(body);
    let mut lines = clean.lines();
    if let Some(first) = lines.next() {
        out.push_str(first.trim_end());
    }
    for line in lines {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str(line.trim_end());
    }
}

/// 会话快照 → 线性文本。空会话返回空串，调用方据此拒绝落盘。
fn linear(msgs: &VecDeque<ChatMessage>) -> String {
    let mut out = String::with_capacity(msgs.len() * 64);
    for msg in msgs {
        if msg.content.trim().is_empty() {
            continue;
        }
        out.push_str(label(msg.role));
        out.push_str("> ");
        push_body(&mut out, &msg.content);
        out.push('\n');
    }
    out
}

impl App {
    /// `/export`：把当前会话写入日志目录下的线性文本文件（X7-01a）。
    pub(super) fn cmd_export(&mut self) {
        let text = linear(&self.messages);
        if text.trim().is_empty() {
            self.toast("会话为空，没有可导出的内容");
            return;
        }
        let dir = crate::paths::log_dir();
        let name = format!(
            "session-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        let path = dir.join(name);
        let res = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text));
        let shown = path.display().to_string();
        match res {
            Ok(()) => {
                self.add_log("INFO", format!("会话文本已导出：{shown}"));
                self.add_message(MessageRole::System, format!("会话文本已导出：{shown}"));
                self.toast("会话文本已导出");
            }
            Err(e) => {
                self.add_log("ERROR", format!("会话文本导出失败：{e}"));
                self.add_message(MessageRole::System, format!("会话文本导出失败：{e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: MessageRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            timestamp: "12:00:00".to_string(),
            id: 1,
            original_bytes: content.len(),
        }
    }

    /// 角色词表稳定，多行正文续行缩进且前缀只出现一次。
    #[test]
    fn linear_keeps_roles_and_indent() {
        let mut msgs = VecDeque::new();
        msgs.push_back(msg(MessageRole::User, "第一行\n第二行"));
        msgs.push_back(msg(MessageRole::Agent, "答复"));
        let got = linear(&msgs);
        assert_eq!(got, "用户> 第一行\n  第二行\n助手> 答复\n");
    }

    /// 空消息不产出空行；控制字符被剥离，导出文本可直接进管道。
    #[test]
    fn empty_and_control_are_dropped() {
        let mut msgs = VecDeque::new();
        msgs.push_back(msg(MessageRole::Agent, "   "));
        msgs.push_back(msg(MessageRole::ToolResult, "a\u{0007}b"));
        let got = linear(&msgs);
        assert_eq!(got, "工具结果> ab\n");
    }
}
