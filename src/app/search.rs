// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 全文会话检索状态（0.1.21 X8-04b）：查询串、命中行表与当前命中游标。
//
// 索引按「会话版本（消息条数 + 末条长度）＋ 渲染宽度」失效：宽度变了
// 折行位置就变，行号不再指向同一处；末条长度覆盖流式追加——只比条数
// 会让正在生长的回复永远命中旧行号。索引文本只在失效时重建一次，
// 逐键输入只重跑匹配，输入延迟与会话规模解耦。

use crate::budget::MAX_SEARCH_HITS;
use crate::engine::search::hit_lines;

/// 索引失效键：消息条数、渲染宽度、末条内容长度。
type IndexKey = (usize, u16, usize);

/// 检索状态（默认关闭、空查询）。
#[derive(Default)]
pub struct Search {
    /// 搜索框是否在场（仅在全文会话视图内可用）。
    pub open: bool,
    /// 查询串（逐字符追加，Backspace 退格）。
    pub query: String,
    /// 命中行号（自顶 0 起，升序）。
    pub hits: Vec<u32>,
    /// 当前命中下标（`hits` 内）。
    pub cur: usize,
    /// 待跳转：查询变化时置位，由渲染在索引重建后兑现——首帧索引尚空，
    /// 键位面算不出目标行，跳转只能推到索引可用之后。
    pub pending: bool,
    /// 索引文本：已物化的整段会话行。
    lines: Vec<String>,
    /// 当前索引的失效键。
    key: Option<IndexKey>,
}

impl Search {
    /// 打开搜索框：清空上一次查询与游标，保留已建索引（同一会话内免重建）。
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.hits.clear();
        self.cur = 0;
    }

    /// 关闭搜索框并丢弃查询与命中；索引保留给下次打开复用。
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.hits.clear();
        self.cur = 0;
        self.pending = false;
    }

    /// 追加一个字符并重跑匹配。
    pub(crate) fn push(&mut self, c: char) {
        self.query.push(c);
        self.pending = true;
        self.recompute();
    }

    /// 退格；空查询时无动作。
    pub(crate) fn pop(&mut self) {
        if self.query.pop().is_some() {
            self.pending = true;
            self.recompute();
        }
    }

    /// 索引是否需要重建（会话或宽度变化）。
    pub(crate) fn stale(&self, msgs: usize, width: u16, last_len: usize) -> bool {
        self.key != Some((msgs, width, last_len))
    }

    /// 重建索引文本并重跑匹配。
    pub(crate) fn reindex(&mut self, key: IndexKey, lines: Vec<String>) {
        self.key = Some(key);
        self.lines = lines;
        self.recompute();
    }

    /// 用当前索引重跑匹配；查询为空即清空命中。
    fn recompute(&mut self) {
        self.hits = hit_lines(&self.lines, &self.query, MAX_SEARCH_HITS);
        if self.cur >= self.hits.len() {
            self.cur = 0;
        }
    }

    /// 跳到下一条命中（循环）；无命中不动作。
    pub(crate) fn next(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.cur = (self.cur + 1) % self.hits.len();
    }

    /// 跳到上一条命中（循环）；无命中不动作。
    pub(crate) fn prev(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.cur = if self.cur == 0 {
            self.hits.len() - 1
        } else {
            self.cur - 1
        };
    }

    /// 当前命中行号（跳转目标）。
    pub(crate) fn anchor(&self) -> Option<u32> {
        self.hits.get(self.cur).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::Search;

    fn ready(lines: &[&str]) -> Search {
        let mut s = Search::default();
        s.reindex((1, 80, 0), lines.iter().map(|l| l.to_string()).collect());
        s
    }

    #[test]
    fn query_narrows_hits_and_cursor_resets() {
        let mut s = ready(&["alpha", "beta", "alphabet"]);
        s.open();
        s.push('a');
        assert_eq!(s.hits, vec![0, 1, 2]);
        s.push('l');
        assert_eq!(s.hits, vec![0, 2]);
        s.pop();
        assert_eq!(s.hits.len(), 3);
    }

    #[test]
    fn cursor_wraps_in_both_directions() {
        let mut s = ready(&["x1", "x2"]);
        s.open();
        s.push('x');
        assert_eq!(s.anchor(), Some(0));
        s.next();
        assert_eq!(s.anchor(), Some(1));
        s.next();
        assert_eq!(s.anchor(), Some(0));
        s.prev();
        assert_eq!(s.anchor(), Some(1));
    }

    #[test]
    fn close_clears_query_and_open_state() {
        let mut s = ready(&["命中"]);
        s.open();
        s.push('命');
        assert_eq!(s.hits.len(), 1);
        s.close();
        assert!(!s.open);
        assert!(s.query.is_empty() && s.hits.is_empty());
        // 索引仍在：重新打开同宽同会话不重建，直接可搜。
        assert!(!s.stale(1, 80, 0));
        s.open();
        s.push('命');
        assert_eq!(s.hits, vec![0]);
    }

    #[test]
    fn stale_triggers_on_stream_growth_and_width_change() {
        let s = ready(&["a"]);
        assert!(!s.stale(1, 80, 0));
        assert!(s.stale(2, 80, 0));
        assert!(s.stale(1, 60, 0));
        assert!(s.stale(1, 80, 3));
    }

    #[test]
    fn empty_query_has_no_hits_and_next_is_noop() {
        let mut s = ready(&["a"]);
        s.open();
        s.next();
        assert!(s.hits.is_empty());
        assert_eq!(s.anchor(), None);
    }
}
