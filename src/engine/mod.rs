// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 渲染引擎（0.1.18 A 轨，§5A.1）：L1 视图模型层、L4 调度层与 L5 合成层的落点。
//
// 分层铁律（§3）：依赖方向 L6 → L5 → L4 → L3 → L2 → L1 → L0 严格单向。
// 本模块只承载自研层，各子模块以自身层的约束为准：L1（身份键/解析缓存）
// 只向上层暴露纯数据与原语，禁止引用任何上层，也禁止做 IO、加锁或依赖
// 时间；L4（调度）是唯一节拍权威，只输出成帧判定与到期时刻，不触碰
// ratatui 后端，且时刻一律由上层注入以便虚拟时钟重放；L5（合成）是全仓
// 唯一允许触碰 ratatui 后端的层，且只经显式失效驱动，不反向依赖 L6
// 组件（渲染回调由调用方下传）。
//
// 施工方式（§5A.2 P0→P1，经 §15.5.1 校正）：不建空壳层。每一层只有在其
// 真实消费者落位时才落文件——L1 的消费者是 chat 面板的内容解析路径，
// L5 的消费者是三处散落的 terminal.draw，L4 的消费者是主循环与面板
// 轮询。空壳类型（无消费者的 Node/ViewTree）会同时构成桩实现与
// dead_code 门禁风险，一律不做。

pub(crate) mod cache;
pub(crate) mod compose;
pub(crate) mod grid;
pub(crate) mod sched;
pub(crate) mod view;
