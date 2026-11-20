// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 降级态与可观测性投影（0.1.19 U12：TUI 建设方案 §6.12 六条 / §6.13 五条）。
//
// 分层：本模块属 L1 视图模型层——纯数据与纯函数，禁止 IO、加锁与时钟读取；
// 所有时刻由调用方注入（单调毫秒），状态机因此可在单测里确定性重放。
//
// 数据真实性：机制层当前不外发 degraded/restored 事件，健康表只由真实端到端
// 探测回填（gateway GET /health 与 {ns}.health_check RPC），不合成事件源；
// 错误聚合只吃真实错误串，分类不中一律落 unknown，不猜测成因。
//
// 按职责分文件（0.1.21 文件体量拆分，公共路径 crate::engine::health::* 不变）：
//   table.rs    HealthTable：逐依赖可达性与最后成功时刻（§6.12 #3、§6.13 #3）。
//   banner.rs   Banner：单行降级横幅、恢复冷却与抖动抑制（§6.12 #1/#2/#6）。
//   cause.rs    错误成因的稳定枚举与关键词回落路径。
//   agg.rs      ErrAgg：错误码去重聚合与有界淘汰（§6.13 #2/#4）。
//   timeline.rs Timeline 与 dur_cn/diag_report：回拨修正与脱敏导出（§6.13 #1/#5）。

mod agg;
mod banner;
mod cause;
mod table;
mod timeline;

#[cfg(test)]
mod tests;

pub(crate) use agg::*;
pub(crate) use banner::*;
pub(crate) use cause::*;
pub(crate) use table::*;
pub(crate) use timeline::*;
