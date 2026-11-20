// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话布局落盘与历史还原（0.1.21 D-14）。
//
// sessions.json 只存布局——每个会话的稳定标识 conv 与标题，不存消息正文：
// 正文的唯一权威是 mem_d（与 CLI 同一记忆库），运行侧写入时以
// metadata.conv 归属会话。布局与内容职责分离，避免第二份历史副本；删掉
// sessions.json 也不丢对话——启动水合完成后按 conv 从 mem_d 重建各 tab 的
// 消息。TUI 侧只读（0.1.21 X1-28a），写记忆归运行侧机制。

use super::*;

use crate::memory::MemoryRecord;

/// 会话布局文件：`$AIRY_HOME/data/agentrt/tui/sessions.json`。
fn sessions_path() -> std::path::PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui", "sessions.json"])
}

/// 布局上限：与 Alt+1..9 切换上限一致，超出丢最旧的非当前会话。
pub(super) const TABS_MAX: usize = 9;

/// conv 序号（同毫秒内多次新建会话的唯一性来源）。
static CONV_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionsFile {
    /// 当前会话在 tabs 数组中的下标，恢复时以其为活动 tab
    #[serde(default)]
    current: usize,
    #[serde(default)]
    tabs: Vec<TabDto>,
}

/// 单 tab 布局。旧版文件里的 `messages` 字段在此无对应字段，反序列化时
/// 直接忽略——升级后正文改由 mem_d 提供。
#[derive(serde::Serialize, serde::Deserialize)]
struct TabDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    conv: String,
}

/// 生成会话标识：毫秒时间 + 进程内序号。非密码学用途，只求跨进程唯一。
pub(super) fn new_conv() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = CONV_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("conv_{:012x}{:04x}", now_ms & 0xFFFF_FFFF_FFFF, n & 0xFFFF)
}

/// 按 tab 顺序重建消息队列（id 由 msg_seq 单调重分配，落盘不存 id）。
fn rebuild(recs: &[MemoryRecord], seq: &mut u64) -> VecDeque<ChatMessage> {
    let mut out = VecDeque::new();
    for r in recs {
        let role = match r.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Agent,
            _ => continue,
        };
        *seq += 1;
        out.push_back(ChatMessage {
            role,
            content: r.content.clone(),
            timestamp: r.timestamp.clone(),
            id: *seq,
            original_bytes: r.content.len(),
        });
    }
    out
}

/// 把记忆记录按 conv 分组（组内旧→新，组间最近活跃在前）。
fn group_by_conv(recent: Vec<MemoryRecord>) -> Vec<(String, Vec<MemoryRecord>)> {
    let mut out: Vec<(String, Vec<MemoryRecord>)> = Vec::new();
    for r in recent.into_iter().rev() {
        let Some(conv) = r.conv.as_ref().filter(|c| !c.is_empty()) else {
            continue;
        };
        match out.iter_mut().find(|(c, _)| c == conv) {
            Some((_, v)) => v.push(r),
            None => out.push((conv.clone(), vec![r])),
        }
    }
    out.sort_by_key(|(_, v)| {
        std::cmp::Reverse(v.last().map(|r| r.timestamp.clone()).unwrap_or_default())
    });
    out
}

impl App {
    /// 会话布局落盘：只写 conv + 标题，不写消息正文。无标识的 tab 在此
    /// 分配标识——只要入布局就必须有稳定身份，重启后正文才找得回来。
    pub fn persist_tabs(&mut self) {
        for t in self.session_tabs.iter_mut() {
            if t.conv.is_empty() {
                t.conv = new_conv();
            }
        }
        let cur = self.current_tab_index();
        let mut tabs: Vec<TabDto> = Vec::new();
        let mut current = 0usize;
        for (i, t) in self.session_tabs.iter().enumerate() {
            if t.conv.is_empty() {
                continue;
            }
            if i == cur {
                current = tabs.len();
            }
            tabs.push(TabDto {
                title: t.title.clone(),
                conv: t.conv.clone(),
            });
        }
        if tabs.is_empty() {
            return;
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
        match serde_json::to_string(&SessionsFile { current, tabs }) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&p, s) {
                    log::warn!("persist_tabs: write {} failed: {}", p.display(), e);
                }
            }
            Err(e) => log::warn!("persist_tabs: serialize failed: {}", e),
        }
    }

    /// 会话布局恢复：只还原 tab 结构与标题，消息由 mem_d 还原（见
    /// `poll_history_restore`）。文件缺失/损坏保持初始单 tab。
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
        let tabs: Vec<TabDto> = doc
            .tabs
            .into_iter()
            .filter(|t| !t.conv.is_empty())
            .take(TABS_MAX)
            .collect();
        if tabs.is_empty() {
            return;
        }
        let current = doc.current.min(tabs.len() - 1);
        self.session_tabs = tabs
            .into_iter()
            .map(|t| SessionTab {
                title: t.title,
                conv: t.conv,
                messages: VecDeque::new(),
                input: String::new(),
                cursor: 0,
                scroll_offset: 0,
                pastes: Vec::new(),
            })
            .collect();
        self.load_tab(current);
        log::info!("restore_tabs: {} 个会话（仅布局）", self.session_tabs.len());
    }

    /// 历史还原闸门：镜像水合就绪后一次性执行，返回是否改动了界面。
    pub fn poll_history_restore(&mut self) -> bool {
        if self.history_restored {
            return false;
        }
        if !self.memory.is_ready() {
            return false;
        }
        self.history_restored = true;
        self.restore_history()
    }

    /// 按 conv 从记忆镜像重建各 tab 的消息。
    ///
    /// 无布局（sessions.json 被删/首次运行）时采用最近活跃的会话组——
    /// 删档只丢布局，不丢对话。
    fn restore_history(&mut self) -> bool {
        if !self.replay.is_empty() {
            return false;
        }
        let recent = self.memory.recent(crate::memory::HYDRATE_LIMIT);
        if recent.is_empty() {
            return false;
        }
        let groups = group_by_conv(recent);
        if groups.is_empty() {
            return false;
        }
        let cur = self.current_tab_index();
        let mut filled = 0usize;
        if self.session_tabs.iter().all(|t| t.conv.is_empty()) {
            let (conv, recs) = &groups[0];
            if let Some(t) = self.session_tabs.get_mut(0) {
                t.conv = conv.clone();
                t.messages = rebuild(recs, &mut self.msg_seq);
                filled += 1;
            }
        } else {
            for t in self.session_tabs.iter_mut() {
                if !t.messages.is_empty() || t.conv.is_empty() {
                    continue;
                }
                if let Some((_, recs)) = groups.iter().find(|(c, _)| *c == t.conv) {
                    t.messages = rebuild(recs, &mut self.msg_seq);
                    filled += 1;
                }
            }
        }
        if filled == 0 {
            return false;
        }
        if self.messages.is_empty() {
            if let Some(t) = self.session_tabs.get(cur) {
                self.messages = t.messages.clone();
            }
        }
        log::info!("restore_history: 经 mem_d 还原 {} 个会话", filled);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(role: &str, content: &str, conv: Option<&str>, ts: &str) -> MemoryRecord {
        MemoryRecord {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: ts.to_string(),
            tags: "chat".to_string(),
            reasoning: None,
            conv: conv.map(str::to_string),
        }
    }

    #[test]
    fn groups_split_by_conv_and_order_by_recency() {
        let recent = vec![
            rec("assistant", "a2", Some("conv_a"), "2026-01-02T00:00:00"),
            rec("user", "b1", Some("conv_b"), "2026-01-03T00:00:00"),
            rec("user", "a1", Some("conv_a"), "2026-01-01T00:00:00"),
            rec("assistant", "legacy", None, "2026-01-04T00:00:00"),
        ];
        let groups = group_by_conv(recent);
        assert_eq!(groups.len(), 2, "无 conv 的记录不参与会话还原");
        assert_eq!(groups[0].0, "conv_b", "最近活跃的会话在首位");
        assert_eq!(groups[1].1.len(), 2);
        assert_eq!(groups[1].1[0].content, "a1", "组内按时间旧→新");
    }

    #[test]
    fn conv_ids_are_unique() {
        let a = new_conv();
        let b = new_conv();
        assert_ne!(a, b);
        assert!(a.starts_with("conv_"));
    }

    #[test]
    fn rebuild_skips_unknown_role_and_preserves_text() {
        let recs = vec![
            rec("user", "问题", Some("c"), "2026-01-01T00:00:00"),
            rec("system", "系统提示", Some("c"), "2026-01-01T00:00:01"),
            rec("assistant", "回答", Some("c"), "2026-01-01T00:00:02"),
        ];
        let mut seq = 0;
        let msgs = rebuild(&recs, &mut seq);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[1].role, MessageRole::Agent);
        assert_eq!(seq, 2);
    }
}
