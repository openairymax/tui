// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 实验五件套（0.1.21 X9-01a）：X 面每一个渲染改动都必须按同一套流程落地，
// 少一件不得合入。五件 = 开关 / 指标 / 回滚 / 评审 / 退役。
//
// 目录与命名约定：
//   feature/gate.rs      开关：唯一放行闸门，默认关，非法名 fail-closed。
//   feature/metrics.rs   指标：有界记录面，次数 + 最近观测，无堆增长。
//   feature/tpl.rs       回滚 / 评审 / 退役三段模板与填写范例（X9-01b）。
//   feature/<name>.rs    单个实验：导出 `pub(crate) const NAME: &str`，
//                        在 mod.rs 登记一行，不放行逻辑以外的实现。
//
// 纪律：开关只决定「新路径是否生效」，不得承载业务语义；指标只记录事实，
// 不得反向影响渲染决策；回滚 = 关开关即回到旧路径，不保留双实现之外的
// 分支；退役 = 删开关、删旧路径、删本模块登记项，三处同一提交完成。
// 此四段合起来保证「功能可大规模拔插而不影响核心」。

#![allow(dead_code)]

pub(crate) mod gate;
pub(crate) mod metrics;
