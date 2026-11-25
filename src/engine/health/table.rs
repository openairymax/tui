// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 依赖健康表：逐依赖的可达性、最后成功/失败时刻、失败与抖动计数。
// 条目集合由机制层枚举（gateway / llm / tool …），不随会话增长；
// 超限拒新增，保证面板与诊断快照的体量恒定。

use crate::budget::MAX_HEALTH_PANEL_ITEMS;

/// 依赖状态迁移（由 observe 返回，调用方据此决定是否记录事件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// 状态未变（含首次即正常的登记）。
    Same,
    /// 由可达转为不可达（含首次探测即失败）。
    Down,
    /// 由不可达恢复为可达。
    Up,
}

/// 单个依赖的探测状态。
#[derive(Debug, Clone)]
pub(crate) struct Dep {
    pub(crate) name: String,
    pub(crate) up: bool,
    pub(crate) last_ok_ms: Option<u64>,
    pub(crate) last_fail_ms: Option<u64>,
    pub(crate) fails: u32,
    pub(crate) flaps: u32,
    /// 该依赖自报的熔断读数（§33.2 #5）：未闭合 breaker 数、breaker 总数、
    /// 累计跳闸次数。语义唯一在 daemon 的 health_check 发布面，TUI 只读。
    pub(crate) brk_open: u16,
    pub(crate) brk_total: u16,
    pub(crate) breaks: u64,
}

impl Dep {
    fn new(name: &str, ok: bool, now_ms: u64) -> Self {
        Self {
            name: name.to_string(),
            up: ok,
            last_ok_ms: ok.then_some(now_ms),
            last_fail_ms: (!ok).then_some(now_ms),
            fails: u32::from(!ok),
            flaps: 0,
            brk_open: 0,
            brk_total: 0,
            breaks: 0,
        }
    }

    /// 吸收一次探测结果，返回状态迁移。
    fn update(&mut self, ok: bool, now_ms: u64) -> Change {
        if ok {
            self.last_ok_ms = Some(now_ms);
            if self.up {
                return Change::Same;
            }
            self.up = true;
            self.flaps = self.flaps.saturating_add(1);
            return Change::Up;
        }
        self.last_fail_ms = Some(now_ms);
        self.fails = self.fails.saturating_add(1);
        if self.up {
            self.up = false;
            return Change::Down;
        }
        Change::Same
    }

    /// 人话状态（面板与诊断共用，不暴露内部通道名）。
    pub(crate) fn state(&self) -> &'static str {
        if self.up {
            "正常"
        } else {
            "不可达"
        }
    }

    /// 距上次成功的时长（单调毫秒）；从未成功则无值。
    pub(crate) fn ok_age(&self, now_ms: u64) -> Option<u64> {
        self.last_ok_ms.map(|t| now_ms.saturating_sub(t))
    }

    /// 距本次不可达起点的时长；可达时无值。
    pub(crate) fn down_age(&self, now_ms: u64) -> Option<u64> {
        if self.up {
            return None;
        }
        self.last_fail_ms.map(|t| now_ms.saturating_sub(t))
    }
}

/// 依赖健康表（条目有界，超限拒新增：依赖集合由机制层枚举，不随会话增长）。
#[derive(Debug, Default)]
pub(crate) struct HealthTable {
    deps: Vec<Dep>,
}

impl HealthTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 吸收一次端到端探测结果。
    pub(crate) fn observe(&mut self, name: &str, ok: bool, now_ms: u64) -> Change {
        if let Some(dep) = self.deps.iter_mut().find(|d| d.name == name) {
            return dep.update(ok, now_ms);
        }
        if self.deps.len() >= MAX_HEALTH_PANEL_ITEMS {
            return Change::Same;
        }
        self.deps.push(Dep::new(name, ok, now_ms));
        if ok {
            Change::Same
        } else {
            Change::Down
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Dep> {
        self.deps.iter().find(|d| d.name == name)
    }

    pub(crate) fn deps(&self) -> &[Dep] {
        &self.deps
    }

    /// 吸收一次熔断读数：与可达性同表存放，避免面板再开一路状态源。
    /// 依赖尚未登记时忽略（可达性探测总是先于读数落地）。
    pub(crate) fn observe_brk(&mut self, name: &str, open: u16, total: u16, breaks: u64) {
        if let Some(dep) = self.deps.iter_mut().find(|d| d.name == name) {
            dep.brk_open = open;
            dep.brk_total = total;
            dep.breaks = breaks;
        }
    }

    /// 该依赖是否已知且不可达。未探测过的依赖不冒充降级。
    pub(crate) fn is_down(&self, name: &str) -> bool {
        self.get(name).is_some_and(|d| !d.up)
    }

    /// 是否存在任一降级依赖。
    pub(crate) fn degraded(&self) -> bool {
        self.deps.iter().any(|d| !d.up)
    }

    /// 最重降级依赖：取失败起点最早者（持续不可达时间最长）。
    /// 状态条与横幅只显示这一项，全量在 /health 面板。
    pub(crate) fn worst(&self) -> Option<&str> {
        self.deps
            .iter()
            .filter(|d| !d.up)
            .min_by_key(|d| d.last_fail_ms.unwrap_or(0))
            .map(|d| d.name.as_str())
    }
}
