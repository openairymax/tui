// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L1 视图模型层：节点身份键（0.1.18 §5A.3）。
//
// 身份规则：节点键必须由数据身份派生，禁止用数组下标——下标随列表增删整体
// 漂移，同一条消息在两次渲染间会拿到不同身份，缓存随之失效甚至串味。
// 聊天消息取 0.1.9 W8 的稳定 id（单调分配、永不复用）；内容变化由缓存键内
// 的内容指纹区分，故不另设版本号字段。
//
// 流式哨兵 id（`ChatMessage::NO_ID`）不建键：其内容是逐帧增长的半成品，
// 由调用侧传 None 旁路缓存（见 engine::cache::render_md）。

/// 节点身份键（L1 身份原语）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key(u64);

impl Key {
    /// 聊天消息键：稳定消息 id。
    pub(crate) fn msg(id: u64) -> Self {
        Self(id)
    }
}
