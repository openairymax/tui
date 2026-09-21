// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 行内层（0.1.18 W6）：pulldown-cmark 行内事件 → 带样式片段序列。
//
// 未闭合标记由解析器按 CommonMark 语义自行回退为普通文本，本层不再做字符级
// 兜底识别——旧实现自行扫描 `**` 与反引号并在未闭合时原样回吐，与解析器语义
// 存在漂移（转义、嵌套强调、代码跨度内标记三处行为不一致）。
//
// 数学降级归 math 层；本层只负责把降级结果放进正确的样式槽位。

use crate::markdown::math;
use crate::theme;
use pulldown_cmark::{Event, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

/// 行内事件累加器。样式以计数栈表示：`**a *b* c**` 的内层 `*b*` 同时具备粗斜体。
pub(super) struct Inline {
    spans: Vec<Span<'static>>,
    base: Style,
    bold: u32,
    italic: u32,
    strike: u32,
    link: Option<String>,
    link_at: usize,
}

impl Inline {
    pub(super) fn new(base: Style) -> Self {
        Self {
            spans: Vec::new(),
            base,
            bold: 0,
            italic: 0,
            strike: 0,
            link: None,
            link_at: 0,
        }
    }

    /// 改写基础样式（标题、已完成任务项等整段语义在此注入）。
    pub(super) fn set_base(&mut self, base: Style) {
        self.base = base;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// 取走已累积的片段并复位嵌套状态。
    pub(super) fn take(&mut self) -> Vec<Span<'static>> {
        self.bold = 0;
        self.italic = 0;
        self.strike = 0;
        self.link = None;
        std::mem::take(&mut self.spans)
    }

    /// 消费行内事件；返回 `false` 表示该事件属块级，交还调用方处理。
    pub(super) fn on(&mut self, ev: &Event<'_>) -> bool {
        match ev {
            Event::Text(t) => {
                self.text(t);
                true
            }
            Event::Code(c) => {
                self.spans.push(Span::styled(
                    c.to_string(),
                    self.base.fg(theme::cyan()).bg(theme::surface_2()),
                ));
                true
            }
            Event::InlineMath(t) => {
                let d = math::degrade(t);
                self.spans
                    .push(Span::styled(d, self.base.fg(theme::magenta())));
                true
            }
            Event::DisplayMath(t) => {
                let d = math::degrade(t);
                self.spans
                    .push(Span::styled(format!(" {d} "), self.base.fg(theme::magenta())));
                true
            }
            Event::SoftBreak => {
                self.push_str(" ");
                true
            }
            Event::FootnoteReference(name) => {
                self.spans.push(Span::styled(
                    format!("[{name}]"),
                    self.base.fg(theme::accent()),
                ));
                true
            }
            // 原始 HTML 不进入终端文本（无浏览器语义，透出标签只会干扰阅读）
            Event::Html(_) | Event::InlineHtml(_) => true,
            Event::Start(tag) => self.open(tag),
            Event::End(tag) => self.close(tag),
            Event::Rule | Event::TaskListMarker(_) | Event::HardBreak => false,
        }
    }

    fn open(&mut self, tag: &Tag<'_>) -> bool {
        match tag {
            Tag::Emphasis => {
                self.italic += 1;
                true
            }
            Tag::Strong => {
                self.bold += 1;
                true
            }
            Tag::Strikethrough => {
                self.strike += 1;
                true
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.link_at = self.spans.len();
                true
            }
            Tag::Image { dest_url, .. } => {
                self.spans
                    .push(Span::styled("[图片: ".to_string(), self.base.fg(theme::dim())));
                self.link = Some(dest_url.to_string());
                self.link_at = self.spans.len();
                true
            }
            _ => false,
        }
    }

    fn close(&mut self, tag: &TagEnd) -> bool {
        match tag {
            TagEnd::Emphasis => {
                self.italic = self.italic.saturating_sub(1);
                true
            }
            TagEnd::Strong => {
                self.bold = self.bold.saturating_sub(1);
                true
            }
            TagEnd::Strikethrough => {
                self.strike = self.strike.saturating_sub(1);
                true
            }
            TagEnd::Link => {
                self.link_tail();
                true
            }
            TagEnd::Image => {
                self.spans
                    .push(Span::styled("]".to_string(), self.base.fg(theme::dim())));
                self.link_tail();
                true
            }
            _ => false,
        }
    }

    /// 链接/图片收尾：附上弱化 URL；自动链接（文本即 URL）不重复输出。
    fn link_tail(&mut self) {
        let url = self.link.take().unwrap_or_default();
        let text: String = self.spans[self.link_at..]
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        if !url.is_empty() && text != url {
            self.spans
                .push(Span::styled(format!(" ({url})"), self.base.fg(theme::faint())));
        }
    }

    /// 普通文本：其中裸 URL 单独着色（解析器不自动链接裸 URL）。
    fn text(&mut self, s: &str) {
        let style = self.style();
        let mut rest = s;
        while !rest.is_empty() {
            let Some(at) = find_url(rest) else {
                self.push_styled(rest, style);
                return;
            };
            self.push_styled(&rest[..at], style);
            let tail = &rest[at..];
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            self.spans.push(Span::styled(
                tail[..end].to_string(),
                self.base
                    .fg(theme::accent())
                    .add_modifier(Modifier::UNDERLINED),
            ));
            rest = &tail[end..];
        }
    }

    fn push_str(&mut self, s: &str) {
        let style = self.style();
        self.push_styled(s, style);
    }

    fn push_styled(&mut self, s: &str, style: Style) {
        if !s.is_empty() {
            self.spans.push(Span::styled(s.to_string(), style));
        }
    }

    fn style(&self) -> Style {
        // 基础样式已带前景色时不得覆盖：标题的层次色由 block 层经 set_base 注入，
        // 若在此强制回落 theme::text() 会抹掉标题与普通文本的区分。
        let mut s = if self.base.fg.is_none() {
            self.base.fg(theme::text())
        } else {
            self.base
        };
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link.is_some() {
            s = s.fg(theme::accent()).add_modifier(Modifier::UNDERLINED);
        }
        s
    }
}

/// 定位首个 `http://` 或 `https://` 的字节下标（仅在字符边界上取值）。
fn find_url(s: &str) -> Option<usize> {
    let mut at = 0;
    while let Some(off) = s[at..].find("http") {
        let i = at + off;
        let tail = &s[i..];
        if tail.starts_with("https://") || tail.starts_with("http://") {
            return Some(i);
        }
        at = i + 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldown_cmark::{Options, Parser};

    /// 用真实解析器驱动行内层，避免测试与解析语义脱节。
    fn spans(md: &str) -> Vec<Span<'static>> {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        let mut inline = Inline::new(Style::default());
        for ev in Parser::new_ext(md, opts) {
            if matches!(ev, Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph)) {
                continue;
            }
            inline.on(&ev);
        }
        inline.take()
    }

    fn text_of(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn bold_and_italic_nest() {
        let s = spans("**粗 *斜* 粗**");
        assert_eq!(text_of(&s), "粗 斜 粗");
        let inner = s.iter().find(|sp| sp.content == "斜");
        let style = inner.map(|sp| sp.style);
        assert!(
            style.is_some_and(|st| st
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)),
            "嵌套强调应同时具备粗体与斜体: {s:?}"
        );
    }

    #[test]
    fn strikethrough_is_marked() {
        let s = spans("~~废弃~~");
        assert_eq!(text_of(&s), "废弃");
        assert!(s[0].style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn link_shows_text_then_weakened_url() {
        let s = spans("[文档](https://example.com/doc)");
        assert_eq!(text_of(&s), "文档 (https://example.com/doc)");
        assert!(s[0].style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn autolink_does_not_duplicate_url() {
        let s = spans("<https://example.com>");
        assert_eq!(text_of(&s), "https://example.com");
    }

    #[test]
    fn bare_url_is_highlighted() {
        let s = spans("见 https://example.com/a 说明");
        assert_eq!(text_of(&s), "见 https://example.com/a 说明");
        let url = s.iter().find(|sp| sp.content.starts_with("https://"));
        assert!(
            url.is_some_and(|sp| sp.style.add_modifier.contains(Modifier::UNDERLINED)),
            "裸 URL 应着色: {s:?}"
        );
    }

    #[test]
    fn inline_code_keeps_content_verbatim() {
        let s = spans("`a_b_c`");
        assert_eq!(text_of(&s), "a_b_c");
    }

    #[test]
    fn image_shows_alt_and_url() {
        let s = spans("![图](https://example.com/a.png)");
        assert_eq!(text_of(&s), "[图片: 图] (https://example.com/a.png)");
    }

    #[test]
    fn escaped_markup_stays_literal() {
        let s = spans("\\*不是斜体\\*");
        assert_eq!(text_of(&s), "*不是斜体*");
    }

    #[test]
    fn math_event_is_degraded() {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_MATH);
        let mut inline = Inline::new(Style::default());
        for ev in Parser::new_ext("$x^2$", opts) {
            inline.on(&ev);
        }
        assert_eq!(text_of(&inline.take()), "x²");
    }
}
