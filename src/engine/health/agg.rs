// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 错误聚合：同一成因连续重复聚合为 ×N；条目有界，最旧条目并入溢出桶，
// 面板体量恒定且总次数不丢（§6.13 #2/#4）。原始错误串不入账。

use super::cause::{classify, from_wire, Cause};
use crate::budget::MAX_ERROR_AGG_ENTRIES;

/// 聚合后的单类错误。
#[derive(Debug, Clone)]
pub(crate) struct ErrEntry {
    pub(crate) code: &'static str,
    pub(crate) label: &'static str,
    pub(crate) remedy: &'static str,
    pub(crate) count: u32,
    pub(crate) first_ms: u64,
    pub(crate) last_ms: u64,
}

/// 错误聚合表：同一错误码连续重复聚合为 ×N；条目有界，最旧条目并入溢出桶。
#[derive(Debug, Default)]
pub(crate) struct ErrAgg {
    items: Vec<ErrEntry>,
    overflow: u32,
}

impl ErrAgg {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 记录一条原始错误，返回其归类（调用方用于错误卡片）。
    /// 无结构化载荷的旧路径；判定规则见 `record_wire`。
    /// 生产路径一律带 wire 载荷（app::health 走 `record_wire`），此便捷入口
    /// 仅测试与 golden 样本使用。
    #[cfg(test)]
    pub(crate) fn record(&mut self, raw: &str, now_ms: u64) -> Cause {
        self.record_wire(None, raw, now_ms)
    }

    /// 记录一条失败：载荷 `reason` 优先，缺失或未登记才按摘要归类。
    pub(crate) fn record_wire(&mut self, wire: Option<&str>, raw: &str, now_ms: u64) -> Cause {
        let cause = wire.and_then(from_wire).unwrap_or_else(|| classify(raw));
        self.record_cause(cause, now_ms);
        cause
    }

    pub(crate) fn record_cause(&mut self, cause: Cause, now_ms: u64) {
        if let Some(e) = self.items.iter_mut().find(|e| e.code == cause.code) {
            e.count = e.count.saturating_add(1);
            e.last_ms = now_ms;
            return;
        }
        if self.items.len() >= MAX_ERROR_AGG_ENTRIES.saturating_sub(1) {
            if let Some(idx) = self.oldest() {
                let gone = self.items.remove(idx);
                self.overflow = self.overflow.saturating_add(gone.count);
            }
        }
        self.items.push(ErrEntry {
            code: cause.code,
            label: cause.label,
            remedy: cause.remedy,
            count: 1,
            first_ms: now_ms,
            last_ms: now_ms,
        });
    }

    fn oldest(&self) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.last_ms)
            .map(|(i, _)| i)
    }

    pub(crate) fn entries(&self) -> &[ErrEntry] {
        &self.items
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// 并入溢出桶的历史错误次数（条目被淘汰时累加）。
    pub(crate) fn overflow(&self) -> u32 {
        self.overflow
    }

    /// 全部错误次数（命名条目 + 溢出桶）。
    pub(crate) fn total(&self) -> u32 {
        let named: u32 = self.items.iter().map(|e| e.count).sum();
        named.saturating_add(self.overflow)
    }
}
