// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 记忆分页台账（0.1.21 X8-01~03）。
//
// mem.recent 的应答有字节上限：一次取 1000 条把 219,264 B（M-01）或
// 438,528 B（M-02）塞进一帧，被网关截断成非法 JSON。故水合改按页推进：
// 每页 ≤ PAGE_LIMIT 条，服务端回 `next`（下一页的排他上界）与 `has_more`。
// 本模块只记账、不发请求：游标推进、区间归并、缺口报告与页数上限，供
// query.rs（派发/落地）与 session_store.rs（前插更早记录）消费。
//
// 写入序下标作为游标（而非时间戳）：同一毫秒内多条记录的时间戳相同，
// 按时间取"更早"会漏段；下标单调且排他，删除只会让后续下标前移，最坏
// 情形是重复一段、不会缺段——重复由记录身份去重吸收（memory.rs）。

use std::collections::BTreeMap;

/// 单页条数：50 条满额应答约 11 KB，远低于网关 64 KB 帧上限，也为
/// 记录较长（思考链）时留足余量。
pub(crate) const PAGE_LIMIT: usize = 50;

/// 单个会话最多翻页数：50 × 20 = 1000 条，与旧单窗口径一致，避免滚到
/// 顶部后无界拉取拖垮网关。
pub(crate) const MAX_PAGES: usize = 20;

/// 一页覆盖的写入序下标区间 `[lo, hi)`；`hi = i64::MAX` 表示首页（最新端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    hi: i64,
    lo: i64,
}

/// 单会话分页簿。
#[derive(Debug, Default, Clone)]
struct Book {
    /// 已覆盖区间，按下界降序且互不相交（见 [`merge`]）。
    spans: Vec<Span>,
    /// 服务端是否还可能给出更早的一页（首页应答的 has_more）。
    has_more: bool,
    /// 是否已收到过至少一页（未开始时不允许翻页）。
    started: bool,
    /// 已记入的页数（重复页只算一次），受 [`MAX_PAGES`] 约束。
    pages: usize,
}

/// 分会话分页台账（PageBook）。
///
/// 每个会话独立推进：切换 tab 不影响其余会话的游标；重新水合（面板
/// 打开）时 [`PageBook::clear`] 归零后从最新一页重来。
#[derive(Debug, Default)]
pub(crate) struct PageBook {
    books: BTreeMap<String, Book>,
}

impl PageBook {
    /// 重置全部会话的游标（新一轮水合开始时调用）。
    pub(crate) fn clear(&mut self) {
        self.books.clear();
    }

    /// 记入一页应答。
    ///
    /// `before` 是本次请求的排他上界（-1 = 从最新开始），`next` 是应答给出
    /// 的下一页上界，`has_more` 表示服务端仍有更早记录。乱序到达（多个
    /// 区间不相邻）由区间归并吸收；同一页重复到达只记一次页数。
    pub(crate) fn note(&mut self, conv: &str, before: i64, next: i64, has_more: bool) {
        let b = self.books.entry(conv.to_string()).or_default();
        let hi = if before < 0 { i64::MAX } else { before };
        let span = Span {
            hi,
            lo: next.max(0),
        };
        // 已被已有区间完整覆盖的页是重复到达（含归并后的超大区间），不计数。
        if !b.spans.iter().any(|s| s.lo <= span.lo && s.hi >= span.hi) {
            b.pages += 1;
        }
        merge(&mut b.spans, span);
        // 游标已到头或页数封顶都视为收口：宁可少翻，不再向网关要页。
        b.has_more = has_more && span.lo > 0 && b.pages < MAX_PAGES;
        b.started = true;
    }

    /// 下一页的请求上界（有更早记录且未达上限时）。
    pub(crate) fn older(&self, conv: &str) -> Option<i64> {
        let b = self.books.get(conv)?;
        if !b.has_more || b.pages >= MAX_PAGES {
            return None;
        }
        let lo = b.spans.iter().map(|s| s.lo).min()?;
        (lo > 0).then_some(lo)
    }

    /// 该会话是否已翻到尽头（无更早记录或页数封顶）。
    pub(crate) fn is_done(&self, conv: &str) -> bool {
        match self.books.get(conv) {
            Some(b) => b.started && (!b.has_more || b.pages >= MAX_PAGES),
            None => true,
        }
    }

    /// 已记入页数。
    pub(crate) fn pages(&self, conv: &str) -> usize {
        self.books.get(conv).map_or(0, |b| b.pages)
    }

    /// 归并后的区间个数（诊断用：应随翻页保持为 1）。
    pub(crate) fn merged(&self, conv: &str) -> usize {
        self.books.get(conv).map_or(0, |b| b.spans.len())
    }

    /// 未覆盖的区间 `(lo, hi)`，按升序返回。顺序翻页时应为空；非空即
    /// 说明有页丢失（网关拒绝/管道断开），下次翻页会从最小下界重试。
    pub(crate) fn gaps(&self, conv: &str) -> Vec<(i64, i64)> {
        let Some(b) = self.books.get(conv) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut iter = b.spans.iter().rev();
        let Some(mut prev) = iter.next().copied() else {
            return out;
        };
        for s in iter {
            if prev.hi < s.lo {
                out.push((prev.hi, s.lo));
            }
            prev = Span {
                hi: prev.hi.max(s.hi),
                lo: prev.lo.min(s.lo),
            };
        }
        out
    }
}

/// 插入区间并归并相邻/重叠者。按下界降序维护，便于最小下界即游标。
fn merge(spans: &mut Vec<Span>, span: Span) {
    spans.push(span);
    spans.sort_by_key(|s| std::cmp::Reverse(s.lo));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans.drain(..) {
        match out.last_mut() {
            Some(last) if s.hi >= last.lo => {
                last.lo = last.lo.min(s.lo);
                last.hi = last.hi.max(s.hi);
            }
            _ => out.push(s),
        }
    }
    *spans = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_resumes_page_by_page() {
        let mut pb = PageBook::default();
        pb.note("c", -1, 950, true);
        assert_eq!(pb.older("c"), Some(950), "首页游标 = 服务端给出的 next");
        pb.note("c", 950, 900, true);
        assert_eq!(pb.older("c"), Some(900));
        pb.note("c", 900, 0, false);
        assert_eq!(pb.older("c"), None, "到头不再翻页");
        assert!(pb.is_done("c"));
        assert_eq!(pb.pages("c"), 3);
        assert!(pb.gaps("c").is_empty(), "顺序翻页不留缺口");
    }

    #[test]
    fn out_of_order_pages_merge_and_report_gap() {
        let mut pb = PageBook::default();
        pb.note("c", -1, 900, true);
        pb.note("c", 100, 50, true);
        assert_eq!(pb.gaps("c"), vec![(100, 900)], "中间缺一段要能报告");
        assert_eq!(pb.older("c"), Some(50), "游标取已覆盖的最小下界");
        pb.note("c", 900, 100, true);
        assert!(pb.gaps("c").is_empty(), "补齐后归并为一整段");
        assert_eq!(pb.merged("c"), 1);
    }

    #[test]
    fn duplicate_page_counts_once() {
        let mut pb = PageBook::default();
        pb.note("c", -1, 950, true);
        pb.note("c", 950, 900, true);
        pb.note("c", 950, 900, true);
        assert_eq!(pb.pages("c"), 2, "重复到达的页不重复计数");
        assert_eq!(pb.merged("c"), 1);
        assert_eq!(pb.older("c"), Some(900));
    }

    #[test]
    fn page_budget_caps_unbounded_scroll() {
        let mut pb = PageBook::default();
        let mut before = -1;
        let mut next = 9000;
        for _ in 0..MAX_PAGES {
            pb.note("c", before, next, true);
            before = next;
            next -= 50;
        }
        assert!(pb.is_done("c"), "达到页数上限即收口");
        assert_eq!(pb.older("c"), None);
        assert_eq!(pb.pages("c"), MAX_PAGES);
    }

    #[test]
    fn sessions_keep_independent_cursors() {
        let mut pb = PageBook::default();
        pb.note("a", -1, 10, true);
        pb.note("b", -1, 7, false);
        assert_eq!(pb.older("a"), Some(10));
        assert!(pb.is_done("b"));
    }

    #[test]
    fn empty_page_ends_the_stream() {
        let mut pb = PageBook::default();
        pb.note("c", -1, 0, false);
        assert!(pb.is_done("c"));
        assert_eq!(pb.older("c"), None);
        assert!(pb.gaps("c").is_empty());
    }

    #[test]
    fn clear_restarts_from_newest() {
        let mut pb = PageBook::default();
        pb.note("c", -1, 10, true);
        pb.clear();
        assert!(pb.is_done("c"));
        assert_eq!(pb.pages("c"), 0);
        pb.note("c", -1, 12, true);
        assert_eq!(pb.older("c"), Some(12));
    }
}
