// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 代码块层（0.1.18 W6）：语言徽章 + 作用域着色 + 着色后折行。
//
// 词法与样式分两层：词法（lex.rs，X4-05a）只产出文本与作用域栈，本层只消费
// syntect 的 scope 分类（keyword/string/comment/number/function/type...）并映射
// 到本仓 style::* 设计令牌，不消费其调色板——Cargo.toml 关掉 default-themes，
// 着色随深浅主题与色深切换，无第二套颜色来源；同一段词法结果可按主题复染。
//
// 宽度纪律：正文经 L2 唯一裁决点 `crate::engine::grid::wrap_spans` 折行，样式
// 随片段跨行延续；本模块不自行测量后截断。
//
// 缓存纪律（X4-06）：正文按（文本 / 宽度 / 语言）三元组 + 样式维度 memo，命中
// 即克隆；徽章只含语言名，不单独缓存。流式尾段旁路消息级缓存，但稳定围栏块
// 的正文与语言不变时逐帧重绘不再重跑词法分析。

use std::cell::RefCell;
use std::hash::Hasher;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use rustc_hash::{FxHashMap, FxHasher};

use crate::budget::{CODE_CACHE_BYTES, CODE_CACHE_ENTRIES};
use crate::engine::cache::line_bytes;
use crate::engine::grid;
use crate::style::{self, ColorDepth, ThemeMode};

use super::{lang, lex};

/// 代码渲染缓存键：文本 / 宽度 / 语言三元组，外加影响输出的样式维度。
///
/// 三元组是流式场景的复用基础：瞬态消息旁路消息级缓存（engine/cache），
/// 但稳定围栏块的正文与语言不变时，逐帧重绘不必重跑词法分析。indent、
/// 基础样式与主题态同样影响输出，缺任一维都会串味。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CodeKey {
    text_hash: u64,
    token: &'static str,
    width: usize,
    indent: usize,
    base: Style,
    depth: u8,
    mode: u8,
}

/// 缓存条目：着色折行后的正文行 + 占用估算 + 最近使用刻度。
struct CodeEntry {
    lines: Vec<Line<'static>>,
    bytes: usize,
    tick: u64,
}

/// 有界代码渲染缓存（LRU 淘汰，命中率自省）。
#[derive(Default)]
struct CodeCache {
    entries: FxHashMap<CodeKey, CodeEntry>,
    bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

thread_local! {
    static CACHE: RefCell<CodeCache> = RefCell::new(CodeCache::default());
}

/// 渲染代码块：语言徽章 + 高亮正文（超宽行折行，样式跨行延续）。
pub(super) fn render_code(
    lang: &str,
    body: &str,
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let bg = base.bg(style::surface());
    // 徽章展示探测后的规范名：```main.py 与 ```py 应显示同一个语言
    let shown = match lang::detect(lang) {
        Some(found) => found.id,
        None => lang,
    };

    if !shown.is_empty() {
        // 正文每行另占 indent + 1 列前缀，预算须先扣除，否则窄屏下必然右溢
        let budget = width.saturating_sub(indent + 1);
        out.push(Line::from(vec![
            Span::styled(" ".repeat(indent), Style::default()),
            Span::styled(
                format!("  {}  ", grid::clip(shown, budget)),
                base.fg(style::accent())
                    .bg(style::surface_active())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    out.extend(body_lines(lang, body, indent, width, bg));
    out
}

/// 正文行：命中缓存则克隆，否则着色 + 折行后落库。
///
/// 徽章与正文分开缓存：徽章只含语言名，重算代价可忽略；正文才是词法
/// 分析的成本所在。
fn body_lines(
    lang: &str,
    body: &str,
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    let token = match lang::detect(lang) {
        Some(found) => found.token,
        None => "",
    };
    let ck = CodeKey {
        text_hash: hash_text(body),
        token,
        width,
        indent,
        base,
        depth: ColorDepth::current() as u8,
        mode: ThemeMode::current() as u8,
    };
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        match cache.fetch(ck) {
            Some(lines) => lines,
            None => {
                let lines = build_body(token, body, indent, width, base);
                cache.store(ck, &lines);
                lines
            }
        }
    })
}

/// 未命中路径：词法（lex）→ 样式（token_spans）→ 折行（grid::wrap_spans）。
///
/// 折行在着色之后进行：片段样式已定型，wrap_spans 只负责按显示宽度搬移，
/// 跨行片段沿用同一 Style，避免折行把 token 颜色截断成两半。
fn build_body(
    token: &str,
    body: &str,
    indent: usize,
    width: usize,
    base: Style,
) -> Vec<Line<'static>> {
    // wrap_spans 在预算小于 2 时整行不折（无法安放双宽簇）；这里兜底到 2，
    // 否则窄屏 + 深缩进时正文会整行右溢，违反「渲染永不越过右边界」。
    let budget = width.saturating_sub(indent + 1).max(2);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in highlight(token, body, base) {
        for piece in grid::wrap_spans(&line, budget) {
            let mut spans = vec![Span::styled(" ".repeat(indent + 1), base)];
            spans.extend(piece);
            out.push(Line::from(spans));
        }
    }
    out
}

/// 逐行分词着色：词法由 lex 层产出（X4-05a），本函数只做作用域 → 设计令牌
/// 的样式映射（X4-05b）。语言无法识别时 lex 退化为纯文本，绝不放弃整块内容。
fn highlight(token: &str, body: &str, base: Style) -> Vec<Vec<Span<'static>>> {
    lex::lines(token, body)
        .into_iter()
        .map(|row| token_spans(row, base))
        .collect()
}

/// token 行 → 带样式片段：token → SGR 的唯一映射点（X4-05b）。
///
/// 词法层只给作用域栈，颜色在此裁决；后续折行（X4-07）拿到的是已定型
/// 的片段序列，折行只需保持样式，不再回到作用域判定。
fn token_spans(row: Vec<lex::Token>, base: Style) -> Vec<Span<'static>> {
    row.into_iter()
        .map(|t| Span::styled(t.text, scope_style(&t.scopes, base)))
        .collect()
}

/// 文本指纹：FxHash 足够（键不跨外部边界，无需抗碰撞）。
fn hash_text(text: &str) -> u64 {
    let mut hasher = FxHasher::default();
    hasher.write(text.as_bytes());
    hasher.finish()
}

impl CodeCache {
    /// 查表：命中则刷新使用刻度并交出克隆，未命中返回 `None`。
    fn fetch(&mut self, key: CodeKey) -> Option<Vec<Line<'static>>> {
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
    fn store(&mut self, key: CodeKey, lines: &[Line<'static>]) {
        self.clock = self.clock.wrapping_add(1);
        let bytes = line_bytes(lines);
        if let Some(old) = self.entries.insert(
            key,
            CodeEntry {
                lines: lines.to_vec(),
                bytes,
                tick: self.clock,
            },
        ) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
        self.evict(CODE_CACHE_ENTRIES, CODE_CACHE_BYTES);
    }

    /// 淘汰最久未用条目至双界之内；单条超预算时连同自身淘汰。
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
}

#[cfg(test)]
fn reset() {
    CACHE.with(|cell| *cell.borrow_mut() = CodeCache::default());
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct CodeStats {
    hits: u64,
    misses: u64,
    entries: usize,
    bytes: usize,
}

#[cfg(test)]
fn stats() -> CodeStats {
    CACHE.with(|cell| {
        let cache = cell.borrow();
        CodeStats {
            hits: cache.hits,
            misses: cache.misses,
            entries: cache.entries.len(),
            bytes: cache.bytes,
        }
    })
}

/// scope 栈 → 设计令牌样式。由最具体的作用域向外匹配，未分类作用域向上回退。
fn scope_style(scopes: &[String], base: Style) -> Style {
    for name in scopes.iter().rev() {
        if let Some(style) = classify(name, base) {
            return style;
        }
    }
    base
}

/// 单条作用域名的令牌映射；`None` 表示不表态，交外层继续匹配。
///
/// 结构性包裹作用域（`source` / `meta`）一律不表态：它们覆盖整段文本，一旦
/// 着色会把整个函数体染成同一颜色。
fn classify(name: &str, base: Style) -> Option<Style> {
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "comment" => base.fg(style::faint()).add_modifier(Modifier::ITALIC),
        "string" => base.fg(style::success()),
        "constant" => {
            if name.starts_with("constant.numeric") {
                base.fg(style::magenta())
            } else {
                base.fg(style::warning())
            }
        }
        "keyword" => base.fg(style::primary()).add_modifier(Modifier::BOLD),
        "storage" => base.fg(style::primary()),
        "entity" => {
            if name.starts_with("entity.name.function") {
                base.fg(style::cyan())
            } else {
                base.fg(style::accent())
            }
        }
        "support" => {
            if name.starts_with("support.function") {
                base.fg(style::cyan())
            } else {
                base.fg(style::accent())
            }
        }
        "variable" => base.fg(style::text()),
        "punctuation" => base.fg(style::dim()),
        "invalid" => base.fg(style::danger()),
        _ => return None,
    })
}

/// 解析一行围栏（CommonMark 形态）：行首最多 3 个空格，连续 ≥3 个反引号
/// 或波浪号。返回（围栏字符、连续长度、围栏之后的剩余文本）。
fn fence(line: &str) -> Option<(u8, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = rest.bytes().take_while(|b| *b == ch).count();
    if run < 3 {
        return None;
    }
    Some((ch, run, &rest[run..]))
}

/// 提取最后一个完整围栏代码块的原文（TUI 方案 §6.3 #5：leader → y 复制）。
///
/// 只认完整块：未闭合的尾块不算（流式中途的半块不能被复制成残缺内容）。
/// 返回内容保持原文换行，去掉闭合围栏前的行尾换行符；空块不参与选取。
pub(crate) fn last_fenced(text: &str) -> Option<String> {
    let mut open: Option<(u8, usize, usize)> = None;
    let mut found: Option<String> = None;
    let mut pos = 0usize;

    for line in text.split_inclusive('\n') {
        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match open {
            Some((ch, want, start)) => {
                if let Some((c, n, tail)) = fence(raw) {
                    if c == ch && n >= want && tail.trim().is_empty() {
                        let body = &text[start..pos];
                        let body = body.strip_suffix('\n').unwrap_or(body);
                        let body = body.strip_suffix('\r').unwrap_or(body);
                        if !body.is_empty() {
                            found = Some(body.to_string());
                        }
                        open = None;
                    }
                }
            }
            None => {
                if let Some((c, n, tail)) = fence(raw) {
                    // 反引号围栏的信息串不得含反引号（CommonMark），波浪号不受限。
                    if c == b'~' || !tail.contains('`') {
                        open = Some((c, n, pos + line.len()));
                    }
                }
            }
        }
        pos += line.len();
    }
    found
}

#[cfg(test)]
#[path = "code_tests.rs"]
mod tests;
