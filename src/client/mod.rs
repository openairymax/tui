// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT 通信契约（TUI 侧）。
//
// 运行轮已全部改由事件管道承载；观察面读也统一走 query 帧（D-15）。本模块
// 不再持有任何 HTTP/RPC 传输，只保留 wire → Rust 的反序列化契约：
//   - event：事件/投影 API（D-15 子进程），运行轮与审批的唯一入口
//   - types：wire → Rust 反序列化契约
//
// 原 `GatewayClient`（reqwest 直连 health_check / rpc_call）已随读侧清零删除，
// 避免再出现绕过事件管道的旁路。

pub(crate) mod event;
mod types;

pub use types::*;
