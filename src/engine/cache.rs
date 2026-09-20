// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L1 解析结果缓存（0.1.18 A 轨 §5A.3 W1）：消除 E4。
//
// E4 缺陷：每条消息、每一帧都重新调用 crate::markdown::render，解析成本与
// 内容增量完全脱钩——长会话"越用越卡"的直接机制。本模块以「身份键 + 内容
// 指纹 + 换行宽度 + 基础样式 + 主题态」为键记忆化解析结果，内容未变的旧
// 消息永不重解析。
//
// 键必须覆盖全部影响输出的维度，缺一维即串味（同键不同结果，且被静默复用）：
//   - Key           节点身份（0.1.9 W8 稳定消息 id，禁止用数组下标）；
//   - content_hash  内容指纹（FxHash，非加密场景）；
//   - indent/width  宽度是排版的唯一裁决点，宽度变化必须重排；
//   - base          基础样式（push_content 内由 msg.role 决定）；
//   - depth/mode    主题态：markdown 输出内嵌 theme::* 颜色，色深或明暗变化
//                   后必须重算。此处取调用时快照而非自建世代计数器——主题
//                   访问器是无锁纯读，且运行期不存在热切换路径。
//
// 状态归属：thread_local + RefCell。渲染发生在事件循环单线程内，无可竞争
// 的对端，用线程本地而非全局锁可省去锁与中毒处理（L1 不加锁、不做 IO）。
// 若未来渲染移出主线程，退化为缓存不命中——仅性能退化，不损坏正确性。
//
// 容量：LRU 256 条 / 8MB 双界（§5A.3），防长会话内存膨胀（R3）。

use std::cell::RefCell;
use std::hash::Hasher;

use ratatui::style::Style;
use ratatui::text::Line;
use rustc_hash::{FxHashMap, FxHasher};

use crate::theme::{ColorDepth, ThemeMode};

use super::view::Key;

/// 条目数上限：超限按最近使用次序淘汰最久未用条目。
const MAX_ENTRIES: usize = 256;
/// 字节上限（估算口径：行/片段结构开销 + 片段文本长度）。
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// 命中率日志节拍：每 N 次查询输出一条 debug 日志（按次数而非时间，确定性）。
const LOG_EVERY: u64 = 4096;

/// 解析缓存键：身份 + 内容 + 排版 + 样式 + 主题，五维齐备才可复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    node: Key,
    content_hash: u64,
    indent: usize,
    width: usize,
    base: Style,
    depth: u8,
    mode: u8,
}

/// 缓存条目：行序列 + 占用估算 + 最近使用刻度。
struct Entry {
    lines: Vec<Line<'static>>,
    bytes: usize,
    tick: u64,
}

/// 有界解析缓存（LRU 淘汰，命中率自省）。
#[derive(Default)]
struct ParseCache {
    entries: FxHashMap<CacheKey, Entry>,
    bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
    parses: u64,
}

thread_local! {
    static CACHE: RefCell<ParseCache> = RefCell::new(ParseCache::default());
}

/// 解析（或复用）一段 markdown 为终端行序列。
///
/// `key` 为节点身份；`None` 表示瞬态内容——流式尾段（`ChatMessage::NO_ID`）
/// 每帧都在增长，缓存只会有害无益，直接旁路。
///
/// 调用方须保证 `content` 与 `key` 指向同一条消息：键内已含内容指纹，
/// 内容变化会自然失配，不会返回陈旧结果。
pub(crate) fn render_md(
    key: Option<Key>,
    content: &str,
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let Some(node) = key else {
        return crate::markdown::render(content, indent, width, base);
    };
    let ck = CacheKey {
        node,
        content_hash: hash_content(content),
        indent,
        width,
        base,
        depth: ColorDepth::current() as u8,
        mode: ThemeMode::current() as u8,
    };
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        let lines = match cache.fetch(ck) {
            Some(lines) => lines,
            None => {
                let parsed = crate::markdown::render(content, indent, width, base);
                cache.store(ck, &parsed);
                parsed
            }
        };
        cache.note();
        lines
    })
}

/// 内容指纹：FxHash 足够（键不落外部输入边界，无需抗碰撞）。
fn hash_content(content: &str) -> u64 {
    let mut hasher = FxHasher::default();
    hasher.write(content.as_bytes());
    hasher.finish()
}

/// 行序列占用估算：结构开销 + 片段文本字节。
fn line_bytes(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| {
            std::mem::size_of_val(line)
                + line
                    .spans
                    .iter()
                    .map(|span| std::mem::size_of_val(span) + span.content.len())
                    .sum::<usize>()
        })
        .sum()
}

impl ParseCache {
    /// 查表：命中则刷新使用刻度并交出克隆，未命中返回 `None`。
    fn fetch(&mut self, key: CacheKey) -> Option<Vec<Line<'static>>> {
        self.clock = self.clock.wrapping_add(1);
        match self.entries.get_mut(&key) {
            Some(entry) => {
                entry.tick = self.clock;
                self.hits += 1;
                Some(entry.lines.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// 落库并回收超额条目。
    fn store(&mut self, key: CacheKey, lines: &[Line<'static>]) {
        self.parses += 1;
        self.clock = self.clock.wrapping_add(1);
        let bytes = line_bytes(lines);
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                lines: lines.to_vec(),
                bytes,
                tick: self.clock,
            },
        ) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
        self.evict(MAX_ENTRIES, MAX_BYTES);
    }

    /// 淘汰最久未用条目至双界之内。单条即超字节预算时连同自身淘汰——
    /// 超大内容不入缓存，但仍由调用方拿到解析结果。
    fn evict(&mut self, max_entries: usize, max_bytes: usize) {
        while self.entries.len() > max_entries || self.bytes > max_bytes {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.tick)
                .map(|(key, _)| *key);
            match victim.and_then(|key| self.entries.remove(&key)) {
                Some(entry) => self.bytes = self.bytes.saturating_sub(entry.bytes),
                None => break,
            }
        }
    }

    /// 命中率自省（§5A.3 验收：命中率先升后稳，日志可查）。
    fn note(&self) {
        let queries = self.hits + self.misses;
        if queries == 0 || !queries.is_multiple_of(LOG_EVERY) {
            return;
        }
        log::debug!(
            "engine/cache: 查询 {queries} 命中 {} 未命中 {} 解析 {} 条目 {}/{} 字节 {}/{}",
            self.hits,
            self.misses,
            self.parses,
            self.entries.len(),
            MAX_ENTRIES,
            self.bytes,
            MAX_BYTES
        );
    }
}

#[cfg(test)]
fn reset() {
    CACHE.with(|cell| *cell.borrow_mut() = ParseCache::default());
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct CacheStats {
    hits: u64,
    misses: u64,
    parses: u64,
    entries: usize,
    bytes: usize,
}

#[cfg(test)]
fn stats() -> CacheStats {
    CACHE.with(|cell| {
        let cache = cell.borrow();
        CacheStats {
            hits: cache.hits,
            misses: cache.misses,
            parses: cache.parses,
            entries: cache.entries.len(),
            bytes: cache.bytes,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(node: u64, hash: u64, indent: usize, width: usize, base: Style) -> CacheKey {
        CacheKey {
            node: Key::msg(node),
            content_hash: hash,
            indent,
            width,
            base,
            depth: ColorDepth::TrueColor as u8,
            mode: ThemeMode::Dark as u8,
        }
    }

    /// 键维度齐备性：任一维度变化都必须产生不同键，否则会串味复用。
    #[test]
    fn key_holds_every_output_dimension() {
        let base = Style::default();
        let other = Style::default().fg(ratatui::style::Color::Red);
        let a = ck(1, 9, 2, 40, base);
        assert_ne!(a, ck(2, 9, 2, 40, base), "节点身份");
        assert_ne!(a, ck(1, 8, 2, 40, base), "内容指纹");
        assert_ne!(a, ck(1, 9, 0, 40, base), "缩进");
        assert_ne!(a, ck(1, 9, 2, 41, base), "换行宽度");
        assert_ne!(a, ck(1, 9, 2, 40, other), "基础样式");
        assert_ne!(
            a,
            CacheKey {
                depth: ColorDepth::Color256 as u8,
                ..a
            },
            "色深"
        );
        assert_ne!(
            a,
            CacheKey {
                mode: ThemeMode::Light as u8,
                ..a
            },
            "明暗"
        );
    }

    /// W1 验收核心：同键同内容二次渲染解析计数 +0，且输出与直绘逐字节一致。
    #[test]
    fn repeat_render_parses_once() {
        reset();
        let base = Style::default().fg(crate::theme::text());
        let key = Some(Key::msg(7));
        let first = render_md(key, "# 标题\n\n正文段落", 2, 48, base);
        assert_eq!(stats().parses, 1, "首帧解析一次");
        assert_eq!(stats().misses, 1, "首帧必为未命中");
        let second = render_md(key, "# 标题\n\n正文段落", 2, 48, base);
        assert_eq!(stats().parses, 1, "同键同内容二次渲染零解析");
        assert_eq!(stats().hits, 1);
        assert_eq!(stats().misses, 1, "命中不累加未命中");
        assert_eq!(second, first, "缓存输出与首帧不一致");
        assert_eq!(
            second,
            crate::markdown::render("# 标题\n\n正文段落", 2, 48, base),
            "缓存输出与直绘逐字节一致"
        );
    }

    /// 缓存输出必须与直绘完全等价（golden 基线，覆盖各类语法分支）。
    #[test]
    fn cached_output_equals_direct_parse() {
        reset();
        let base = Style::default().fg(crate::theme::text());
        let long = "中文全角与半角混排 abc ".repeat(30);
        let samples: [&str; 6] = [
            "",
            "# 一级标题\n正文",
            "- 甲\n- 乙\n  - 丙",
            "```rust\nfn f() {}\n```",
            "| a | b |\n| - | - |\n| 1 | 2 |",
            long.as_str(),
        ];
        for (i, src) in samples.iter().enumerate() {
            let direct = crate::markdown::render(src, 3, 52, base);
            let cached = render_md(Some(Key::msg(i as u64)), src, 3, 52, base);
            assert_eq!(cached, direct, "缓存与直绘不一致: {src:?}");
        }
    }

    /// 内容或排版维度变化必须失效重解析，旧条目此后仍可命中。
    #[test]
    fn dimension_change_invalidates_entry() {
        reset();
        let base = Style::default();
        let key = Some(Key::msg(11));
        render_md(key, "同一段内容", 2, 40, base);
        assert_eq!(stats().parses, 1);

        render_md(key, "同一段内容", 2, 41, base);
        assert_eq!(stats().parses, 2, "宽度变化须重解析");
        render_md(key, "同一段内容", 4, 40, base);
        assert_eq!(stats().parses, 3, "缩进变化须重解析");
        render_md(
            key,
            "同一段内容",
            2,
            40,
            Style::default().fg(ratatui::style::Color::Red),
        );
        assert_eq!(stats().parses, 4, "基础样式变化须重解析");
        render_md(key, "另一段内容", 2, 40, base);
        assert_eq!(stats().parses, 5, "内容变化须重解析");
        render_md(Some(Key::msg(12)), "同一段内容", 2, 40, base);
        assert_eq!(stats().parses, 6, "节点身份变化须重解析");

        render_md(key, "同一段内容", 2, 40, base);
        assert_eq!(stats().parses, 6, "回到原维度组合应命中旧条目");
    }

    /// 主题态是键维度之一：快照于调用时刻，切走再切回应仍命中。
    #[test]
    fn theme_snapshot_participates_in_key() {
        let _guard = crate::test_env::lock_env();
        reset();
        let base = Style::default();
        let key = Some(Key::msg(21));
        ThemeMode::Dark.set();
        render_md(key, "# 主题", 0, 30, base);
        assert_eq!(stats().parses, 1);

        ThemeMode::Light.set();
        render_md(key, "# 主题", 0, 30, base);
        assert_eq!(stats().parses, 2, "主题态变化须重解析（颜色内嵌于输出）");

        ThemeMode::Dark.set();
        render_md(key, "# 主题", 0, 30, base);
        assert_eq!(stats().parses, 2, "回到原主题应命中旧条目");
        reset();
    }

    /// 瞬态内容（流式尾段）旁路缓存：不入库、不占预算。
    #[test]
    fn transient_content_bypasses_cache() {
        reset();
        let base = Style::default();
        let a = render_md(None, "流式增量", 0, 20, base);
        let b = render_md(None, "流式增量", 0, 20, base);
        assert_eq!(stats().parses, 0, "旁路不应触发缓存解析计数");
        assert_eq!(stats().entries, 0, "旁路不应入库");
        assert_eq!(a, b);
        assert_eq!(a, crate::markdown::render("流式增量", 0, 20, base));
    }

    /// LRU 条目上限：超额淘汰最久未用者，最近使用者保留命中。
    #[test]
    fn lru_evicts_oldest_beyond_entry_cap() {
        reset();
        let base = Style::default();
        let total = MAX_ENTRIES as u64 + 32;
        for id in 0..total {
            render_md(Some(Key::msg(id)), "同内容", 0, 20, base);
        }
        assert!(
            stats().entries <= MAX_ENTRIES,
            "条目数应受限于上限: {}",
            stats().entries
        );
        assert!(
            stats().bytes <= MAX_BYTES,
            "字节数应受限于上限: {}",
            stats().bytes
        );
        let parses = stats().parses;
        render_md(Some(Key::msg(total - 1)), "同内容", 0, 20, base);
        assert_eq!(stats().parses, parses, "最近使用条目应仍命中");
        render_md(Some(Key::msg(0)), "同内容", 0, 20, base);
        assert_eq!(stats().parses, parses + 1, "最久未用条目应已被淘汰");
    }

    /// LRU 字节上限：按最近使用次序淘汰至预算内。
    #[test]
    fn lru_evicts_to_byte_budget() {
        let mut cache = ParseCache::default();
        for id in 0..8u64 {
            let lines = vec![Line::raw("x".repeat(256))];
            cache.store(ck(id, id, 0, 20, Style::default()), &lines);
        }
        let before = cache.entries.len();
        assert!(cache.bytes > 1024, "前置条件：应超出测试预算");
        cache.evict(MAX_ENTRIES, 1024);
        assert!(cache.entries.len() < before, "超预算应淘汰条目");
        assert!(cache.bytes <= 1024, "淘汰后应回到预算内: {}", cache.bytes);
    }

    /// 单条即超字节预算：连同自身淘汰，不留不可用条目。
    #[test]
    fn oversized_single_entry_is_dropped() {
        let mut cache = ParseCache::default();
        let lines = vec![Line::raw("y".repeat(4096))];
        cache.store(ck(1, 1, 0, 20, Style::default()), &lines);
        cache.evict(MAX_ENTRIES, 1024);
        assert!(cache.entries.is_empty(), "超预算单条不应驻留");
        assert_eq!(cache.bytes, 0);
    }
}
