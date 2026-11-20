// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 记忆呈现快照（0.1.21 X1-26b：记忆读取路径只读快照化）。
//
// 分层：本模块属 L1 视图模型层——纯数据，禁止 IO、加锁与时钟读取，也不引用
// 任何后端类型。后端记录由 app 层投影成 MemoryEntry 后按值传入，渲染侧只
// 消费快照，不再直呼 App 的记忆后端。

/// 单条记忆的只读呈现视图：只保留面板需要的字段。
pub(crate) struct MemoryEntry {
    role: String,
    content: String,
    timestamp: String,
    tags: String,
    has_reasoning: bool,
}

impl MemoryEntry {
    pub(crate) fn new(
        role: String,
        content: String,
        timestamp: String,
        tags: String,
        has_reasoning: bool,
    ) -> Self {
        Self {
            role,
            content,
            timestamp,
            tags,
            has_reasoning,
        }
    }

    pub(crate) fn role(&self) -> &str {
        &self.role
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn timestamp(&self) -> &str {
        &self.timestamp
    }

    pub(crate) fn tags(&self) -> &str {
        &self.tags
    }

    /// 是否带思考链（面板只做「含思考链」标记，不读正文）。
    pub(crate) fn has_reasoning(&self) -> bool {
        self.has_reasoning
    }
}

/// 记忆库头部快照：条数与后端名，面板每帧读取。
pub(crate) struct MemorySnapshot {
    total: usize,
    backend: &'static str,
}

impl MemorySnapshot {
    pub(crate) fn new(total: usize, backend: &'static str) -> Self {
        Self {
            total,
            backend,
        }
    }

    pub(crate) fn total(&self) -> usize {
        self.total
    }

    pub(crate) fn backend(&self) -> &'static str {
        self.backend
    }
}
