// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 瞬态提示队列（0.1.19 TUI 方案 §6.3 #4）：同屏最多三条、1.5s 自动消失。
//
// 队列容量与存活时长取自 budget.rs 的单一来源，本模块只实现「有界 + 到期」
// 两条语义。时刻由调用方注入（与调度层同一铁律），故边界行为可在单测里
// 用虚拟时刻确定性重放，不依赖真实时钟。
//
// 溢出语义：队列满时淘汰最旧一条——提示是「最近发生了什么」的短暂回显，
// 保留最新事实比保留完整历史更有价值；淘汰发生在入队瞬间，不会先撑破
// 同屏上限再回收。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::budget::{MAX_ACTIVE_TOASTS, TOAST_TTL_MS};

/// 一条瞬态提示：文本 + 入队时刻。
struct Toast {
    text: String,
    born: Instant,
}

impl Toast {
    /// 已存活时长（时刻回拨时按零计，避免减法下溢）。
    fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.born)
    }
}

/// 有界瞬态提示队列（先进先出，满则淘汰最旧）。
pub(crate) struct ToastQueue {
    items: VecDeque<Toast>,
}

impl ToastQueue {
    pub(crate) fn new() -> Self {
        Self {
            items: VecDeque::with_capacity(MAX_ACTIVE_TOASTS),
        }
    }

    /// 入队一条提示：先清到期项（不占容量），满则淘汰最旧一条。
    pub(crate) fn push(&mut self, text: impl Into<String>, now: Instant) {
        self.expire(now);
        if self.items.len() >= MAX_ACTIVE_TOASTS {
            self.items.pop_front();
        }
        self.items.push_back(Toast {
            text: text.into(),
            born: now,
        });
    }

    /// 清除全部到期提示；有变化返回真（调用方据此补一帧）。
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        let ttl = Duration::from_millis(TOAST_TTL_MS);
        self.items.retain(|t| t.age(now) < ttl);
        before != self.items.len()
    }

    /// 活跃提示文本（先进先出），渲染层只读消费。
    pub(crate) fn active(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|t| t.text.as_str())
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// 距最近一条到期还剩多久；无提示时为 None。
    ///
    /// 主循环以本值收窄事件等待，使消隐时刻不被空闲节拍（250ms）拖长。
    pub(crate) fn wait(&self, now: Instant) -> Option<Duration> {
        let ttl = Duration::from_millis(TOAST_TTL_MS);
        self.items
            .iter()
            .map(|t| ttl.saturating_sub(t.age(now)))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §6.3 #4 容量：同屏不超过三条，溢出淘汰最旧。
    #[test]
    fn overflow_evicts_oldest() {
        let t0 = Instant::now();
        let mut q = ToastQueue::new();
        for text in ["a", "b", "c"] {
            q.push(text, t0);
        }
        assert_eq!(q.len(), MAX_ACTIVE_TOASTS);
        q.push("d", t0);
        assert_eq!(q.len(), MAX_ACTIVE_TOASTS, "溢出后仍守住同屏上限");
        let got: Vec<&str> = q.active().collect();
        assert_eq!(got, ["b", "c", "d"], "淘汰最旧、保留最新三条");
    }

    /// §6.3 #4 存活：1.5s 边界内保留，到点即消失。
    #[test]
    fn expiry_hits_ttl_boundary() {
        let t0 = Instant::now();
        let ttl = Duration::from_millis(TOAST_TTL_MS);
        let mut q = ToastQueue::new();
        q.push("copy ok", t0);
        assert!(
            !q.expire(t0 + ttl - Duration::from_millis(1)),
            "界内不提前清除"
        );
        assert_eq!(
            q.wait(t0 + ttl - Duration::from_millis(1)),
            Some(Duration::from_millis(1)),
            "剩余时长按最早到期项给出"
        );
        assert!(q.expire(t0 + ttl), "满 1.5s 即到期");
        assert_eq!(q.len(), 0);
        assert_eq!(q.wait(t0 + ttl), None, "空队列不参与等待收窄");
    }

    /// 到期项不占容量：清退后仍可满额入队，不会误淘汰有效提示。
    #[test]
    fn expired_do_not_hold_capacity() {
        let t0 = Instant::now();
        let ttl = Duration::from_millis(TOAST_TTL_MS);
        let mut q = ToastQueue::new();
        q.push("old", t0);
        q.push("b", t0 + ttl);
        q.push("c", t0 + ttl);
        q.push("d", t0 + ttl);
        let got: Vec<&str> = q.active().collect();
        assert_eq!(got, ["b", "c", "d"], "过期项先清退，不挤占同屏名额");
    }

    /// 最近到期决定等待时长：取各提示剩余存活的最小值。
    #[test]
    fn wait_tracks_earliest() {
        let t0 = Instant::now();
        let ttl = Duration::from_millis(TOAST_TTL_MS);
        let mut q = ToastQueue::new();
        q.push("first", t0);
        q.push("second", t0 + Duration::from_millis(500));
        assert_eq!(
            q.wait(t0 + Duration::from_millis(500)),
            Some(ttl - Duration::from_millis(500)),
            "最先入队者最早到期"
        );
    }
}
