// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 会话呈现快照（0.1.21 X1-26a：TUI 的 session 读取路径只读快照化）。
//
// 分层：本模块属 L1 视图模型层——纯数据与纯函数，禁止 IO、加锁与时钟读取，
// 也不引用任何上层类型。App 的会话状态由 app/session.rs 收拢成原始行后按值
// 传入，渲染侧只消费本模块产出的不可变快照，不再回问 App 的会话字段/访问器。
//
//   view.rs   SessionView / TabView：标题回退、当前标记与列表拼串。

mod view;

#[cfg(test)]
mod tests;

pub(crate) use view::*;
