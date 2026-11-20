// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 记忆呈现快照组装（0.1.21 X1-26b）：面板只读快照，不直呼记忆后端。
// 召回归因、写入与再水合属编排/写路径，仍在 App 内直接使用后端。

use crate::engine::memory::{MemoryEntry, MemorySnapshot};

use super::*;

impl App {
    /// 记忆库头部快照：条数与后端名（每帧读取，不克隆记录）。
    pub(crate) fn memory_snapshot(&self) -> MemorySnapshot {
        MemorySnapshot::new(self.memory.len(), self.memory.backend_name())
    }

    /// 最近 `take` 条记忆的呈现投影（面板重建时整段取用，最新在前）。
    ///
    /// 思考链只投影「有无」，与面板的「含思考链」标记同口径；正文不进入
    /// 快照，避免推理原文借呈现路径留在内存里。
    pub(crate) fn memory_records(&self, take: usize) -> Vec<MemoryEntry> {
        self.memory
            .recent(take)
            .into_iter()
            .map(|r| {
                let has_reasoning = r
                    .reasoning
                    .as_deref()
                    .map(|x| !x.trim().is_empty())
                    .unwrap_or(false);
                MemoryEntry::new(r.role, r.content, r.timestamp, r.tags, has_reasoning)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::GatewayMemory;

    /// 以 volatile 记忆库替换默认后端：投影口径与网关后端无关。
    fn app_with(records: &[(&str, &str, &str)]) -> App {
        let _home = crate::test_env::Home::new("memory-snapshot");
        let gw = crate::client::GatewayClient::new("http://127.0.0.1:1").expect("gateway client");
        let mut app = App::new("agents/main.agent.yaml", gw);
        let mut mem = GatewayMemory::volatile();
        for (role, text, tags) in records {
            mem.push(role, text, tags).expect("push");
        }
        app.memory = Box::new(mem);
        app
    }

    #[test]
    fn snapshot_reports_total_and_backend() {
        let app = app_with(&[("user", "问", "chat"), ("assistant", "答", "task")]);
        let snap = app.memory_snapshot();
        assert_eq!(snap.total(), 2);
        assert_eq!(snap.backend(), "volatile");
    }

    #[test]
    fn records_project_newest_first_without_reasoning() {
        let app = app_with(&[
            ("user", "第一问", "chat"),
            ("assistant", "第一答", "task,chat"),
        ]);
        let recs = app.memory_records(2);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].role(), "assistant");
        assert_eq!(recs[0].content(), "第一答");
        assert_eq!(recs[0].tags(), "task,chat");
        assert!(!recs[0].has_reasoning());
        assert_eq!(recs[1].content(), "第一问");
    }

    #[test]
    fn records_respect_take_limit() {
        let app = app_with(&[
            ("user", "a", "chat"),
            ("user", "b", "chat"),
            ("user", "c", "chat"),
        ]);
        let recs = app.memory_records(2);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].content(), "c");
        assert_eq!(recs[1].content(), "b");
    }
}
