// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 时间线与诊断导出：墙钟与单调钟配对，回拨按单调序修正（§6.13 #1）；
// 诊断快照只含依赖名、状态、计数、时长与错误码，不读会话正文（§6.13 #5）。

use super::agg::ErrAgg;
use super::table::HealthTable;

/// 判定时钟回拨的容差：小于该幅度的抖动视为正常调度误差。
const CLOCK_SKEW_TOL_MS: u64 = 1_000;
const MS_PER_S: u64 = 1_000;
const S_PER_MIN: u64 = 60;
const MIN_PER_H: u64 = 60;

/// 一次时间注入的处理结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stamp {
    /// 单调序时刻（用于所有时序展示）。
    pub(crate) order_ms: u64,
    /// 本次注入检测到墙钟回拨并已修正。
    pub(crate) corrected: bool,
}

/// 时间线：墙钟与单调钟配对，回拨时按单调序修正并累计异常量。
#[derive(Debug, Default)]
pub(crate) struct Timeline {
    last_wall: Option<u64>,
    order: u64,
    skew_ms: u64,
    corrections: u32,
}

impl Timeline {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 注入一对时刻（墙钟毫秒、单调毫秒），返回单调序时刻与修正标记。
    pub(crate) fn observe(&mut self, wall_ms: u64, mono_ms: u64) -> Stamp {
        let corrected = self
            .last_wall
            .is_some_and(|prev| wall_ms.saturating_add(CLOCK_SKEW_TOL_MS) < prev);
        if let Some(prev) = self.last_wall {
            if corrected {
                self.skew_ms = self.skew_ms.saturating_add(prev.saturating_sub(wall_ms));
                self.corrections = self.corrections.saturating_add(1);
            }
        }
        self.last_wall = Some(wall_ms);
        self.order = self.order.max(mono_ms);
        Stamp {
            order_ms: self.order,
            corrected,
        }
    }

    /// 回拨异常的人话标注；从未回拨则无值。
    pub(crate) fn note(&self) -> Option<String> {
        if self.corrections == 0 {
            return None;
        }
        let secs = self.skew_ms.div_ceil(MS_PER_S);
        Some(format!("时钟已校准（回拨 ≈{secs}s），时序已按单调序修正"))
    }
}

/// 时长格式化：10s / 3m12s / 2h5m。
pub(crate) fn dur_cn(ms: u64) -> String {
    let secs = ms / MS_PER_S;
    if secs < S_PER_MIN {
        return format!("{secs}s");
    }
    let mins = secs / S_PER_MIN;
    if mins < MIN_PER_H {
        return format!("{}m{}s", mins, secs % S_PER_MIN);
    }
    format!("{}h{}m", mins / MIN_PER_H, mins % MIN_PER_H)
}

/// 诊断快照（§6.13 #5）：只含时间线异常、依赖健康与错误聚合，
/// 结构上不引用会话正文与密钥；原始错误串亦不入报告。
pub(crate) fn diag_report(
    stamp: &Stamp,
    timeline: &Timeline,
    health: &HealthTable,
    errs: &ErrAgg,
    wall_text: &str,
    now_ms: u64,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("AirymaxRT 诊断快照\n");
    out.push_str(&format!("采集时刻：{wall_text}\n"));
    out.push_str(&format!("单调序：{}ms\n", stamp.order_ms));
    match timeline.note() {
        Some(note) => out.push_str(&format!("时间线：{note}\n")),
        None => out.push_str("时间线：正常\n"),
    }

    let down = health.deps().iter().filter(|d| !d.up).count();
    out.push_str(&format!(
        "依赖健康（{} 项，{} 项降级）\n",
        health.deps().len(),
        down
    ));
    for dep in health.deps() {
        let mark = if dep.up { "✓" } else { "✗" };
        let span = if dep.up {
            match dep.ok_age(now_ms) {
                Some(age) => format!("正常 · {} 前", dur_cn(age)),
                None => "正常".to_string(),
            }
        } else {
            match dep.down_age(now_ms) {
                Some(age) => format!("不可达 · {} 前起", dur_cn(age)),
                None => "不可达".to_string(),
            }
        };
        out.push_str(&format!(
            "  {mark} {} · {span}（失败 {} 次 / 抖动 {} 次）\n",
            dep.name, dep.fails, dep.flaps
        ));
    }

    out.push_str(&format!(
        "错误聚合（{} 类，共 {} 次）\n",
        errs.len(),
        errs.total()
    ));
    for e in errs.entries() {
        out.push_str(&format!(
            "  ×{} {} · {} · 最近 {} 前\n",
            e.count,
            e.code,
            e.label,
            dur_cn(now_ms.saturating_sub(e.last_ms))
        ));
    }
    if errs.overflow() > 0 {
        out.push_str(&format!("  其他合并 {} 次\n", errs.overflow()));
    }
    out
}
