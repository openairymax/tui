// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 任务态落盘（D-13-04）：阻断态跨进程续跑凭据，与 sessions.json 分离。

use super::*;

/// 任务态文件：`$AIRY_HOME/data/agentrt/tui/task_state.json`。
///
/// 与 D-12 断点同目录、同原子写（临时文件 + rename）。会话内容归
/// sessions.json，运行事实归本文件——两者混存会让「消息落盘」顺带
/// 改写任务状态，恢复时无法判断哪份才是真的。
fn task_state_path() -> std::path::PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui", "task_state.json"])
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TaskStateFile {
    state: String,
    #[serde(default)]
    resume_token: Option<String>,
    #[serde(default)]
    saved_at_ms: u64,
}

fn state_name(s: TaskState) -> &'static str {
    match s {
        TaskState::Executing => "executing",
        TaskState::Completed => "completed",
        TaskState::Paused => "paused",
        TaskState::Failed => "failed",
        TaskState::Unverified => "unverified",
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl App {
    /// 阻断态落盘；其余状态清除旧文件。
    ///
    /// 只有阻断态需要跨进程三选：执行中/已完成/已取消留在盘上，重启后
    /// 会误报「有任务在跑」，用户被提示去续跑一个早已结束的任务。
    pub(super) fn save_task_state(&self) {
        let path = task_state_path();
        if !self.task_state.is_blocked() {
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    log::debug!("task_state 清理失败: {e}");
                }
            }
            return;
        }
        let dto = TaskStateFile {
            state: state_name(self.task_state).to_string(),
            resume_token: self.resume_token.clone(),
            saved_at_ms: now_ms(),
        };
        let bytes = match serde_json::to_vec_pretty(&dto) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("task_state 序列化失败: {e}");
                return;
            }
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("task_state 目录创建失败: {e}");
                return;
            }
        }
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            log::warn!("task_state 写入失败: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            log::warn!("task_state 原子替换失败: {e}");
        }
    }

    /// 重启恢复：只认阻断态，恢复后给出三选；已收口状态直接丢弃。
    pub fn restore_task_state(&mut self) {
        let raw = match std::fs::read_to_string(task_state_path()) {
            Ok(s) => s,
            Err(_) => return,
        };
        let dto: TaskStateFile = match serde_json::from_str(&raw) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("task_state 解析失败（已忽略）: {e}");
                return;
            }
        };
        let Some(st) = gccp::task_state_of(&dto.state) else {
            return;
        };
        if !st.is_blocked() {
            return;
        }
        self.resume_token = dto.resume_token;
        self.task_state = st;
        log::info!(
            "task_state 恢复：{:?}（token={}）",
            st,
            self.resume_token.is_some()
        );
        self.add_message(
            MessageRole::System,
            format!(
                "上次退出时有未完成任务（{}）：输入{}。",
                st.label(),
                TaskState::choices()
            ),
        );
    }
}
