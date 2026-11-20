// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件入口（0.1.21 X1 面，§23.1.5 子进程模型）：TUI 只消费 CLI 事件流。
//
// 分层：CLI 是语义层与唯一权威（持有 gateway 客户端、任务状态机、重试与
// 鉴权策略），TUI 是渲染层（订阅事件 → 投影 → 布局 → 合成 → 绘制）。
// 本模块只承载「怎么把事件拿进来」，不承载任何业务语义；事件解码、序列
// 断言、投影分别在后续子模块落位，避免一个文件同时背管道、解析与状态。

pub(crate) mod pipe;
