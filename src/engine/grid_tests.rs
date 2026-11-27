// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// grid 排版层的属性测试：宽度口径、折行断点、组合簇原子性、列宽分配。
// 用例单独成文件，避免生产代码文件在追加用例后越过单文件 SLOC 上限。

use super::*;
use ratatui::style::Color;

fn line_text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

/// 混排语料：ASCII / CJK / 全角标点 / 全角字母数字 / 组合字符（e+U+0301）
const SAMPLES: &[&str] = &[
    "hello world",
    "你好世界",
    "上下文窗口 hello 世界",
    "全角标点：（测试）、「引用」——破折号",
    "组合字符 e\u{0301}a\u{0301}o\u{0301} 结束",
    "混合 mix 文字 e\u{0301} 与 全角，标点。",
    "ｆｕｌｌｗｉｄｔｈ ＡＢＣ１２３",
    "a你b好c世d界e",
];

#[test]
fn line_w_sums_span_widths() {
    let spans = vec![Span::raw("◈ AirymaxRT"), Span::raw("  极境智能体运行平台")];
    assert_eq!(line_w(&spans), width("◈ AirymaxRT  极境智能体运行平台"));
    assert_eq!(line_w(&[]), 0);
}

#[test]
fn width_counts_cjk_and_fullwidth_as_two() {
    assert_eq!(width(""), 0);
    assert_eq!(width("abc"), 3);
    assert_eq!(width("中文"), 4);
    assert_eq!(width("，"), 2);
    assert_eq!(width("Ａ"), 2);
}

#[test]
fn wrap_short_text_stays_single_line() {
    assert_eq!(wrap("你好", 10), vec!["你好"]);
    assert_eq!(wrap("abc", 10), vec!["abc"]);
}

#[test]
fn wrap_prefers_word_boundary() {
    assert_eq!(wrap("hello world", 5), vec!["hello", "world"]);
    assert_eq!(
        wrap("the quick brown fox", 10),
        vec!["the quick", "brown fox"]
    );
    // 词边界优先后各段仍不越界
    for piece in wrap("the quick brown fox jumps", 10) {
        assert!(width(&piece) <= 10, "{piece:?}");
    }
}

#[test]
fn wrap_hard_breaks_overlong_word() {
    assert_eq!(wrap("abcdefghij", 3), vec!["abc", "def", "ghi", "j"]);
    assert_eq!(wrap("a  b c", 4), vec!["a", "b c"]);
}

#[test]
fn wrap_spans_keeps_style_across_break() {
    let spans = vec![
        Span::styled("the quick ", Style::default().fg(Color::Red)),
        Span::styled("brown fox", Style::default().fg(Color::Blue)),
    ];
    let lines = wrap_spans(&spans, 10);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0][0].content.as_ref(), "the quick");
    assert_eq!(lines[1][0].content.as_ref(), "brown fox");
    assert_eq!(lines[1][0].style.fg, Some(Color::Blue));
    for line in &lines {
        assert!(line_w(line) <= 10);
    }
}

#[test]
fn wrap_spans_splits_inside_single_style() {
    let spans = vec![Span::styled("abcdefghij", Style::default())];
    let lines = wrap_spans(&spans, 3);
    let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
    assert_eq!(texts, vec!["abc", "def", "ghi", "j"]);
}

#[test]
fn wrap_splits_by_display_width() {
    assert_eq!(wrap("你好世界", 5), vec!["你好", "世界"]);
    assert_eq!(wrap("abcdef", 3), vec!["abc", "def"]);
    assert_eq!(wrap("a你好b", 4), vec!["a你", "好b"]);
    let narrow = wrap("上下文窗口", 4);
    assert!(narrow.iter().all(|p| width(p) <= 4), "{narrow:?}");
}

#[test]
fn wrap_empty_and_degenerate_width() {
    assert!(wrap("", 10).is_empty());
    assert_eq!(wrap("abc", 1), vec!["abc"]);
    assert_eq!(wrap("abc", 0), vec!["abc"]);
}

#[test]
fn clip_never_exceeds_budget() {
    for max in 0..=200usize {
        for s in SAMPLES {
            let c = clip(s, max);
            assert!(
                width(&c) <= max,
                "clip 溢出: s={s:?} max={max} out={c:?} w={}",
                width(&c)
            );
            if max >= width(s) {
                assert_eq!(c, *s, "未超宽应原样返回: s={s:?} max={max}");
            }
        }
    }
}

#[test]
fn clip_keeps_prefix_and_marks_ellipsis() {
    assert_eq!(clip("abcdef", 3), "ab…");
    assert_eq!(clip("中文汉字", 5), "中文…");
    assert_eq!(clip("abc", 0), "");
    assert_eq!(clip("abc", 3), "abc");
}

#[test]
fn pad_reaches_exact_columns() {
    for cols in 0..=40usize {
        for s in SAMPLES {
            let p = pad(s, cols);
            if cols >= width(s) {
                assert_eq!(width(&p), cols, "padding 应精确到列: s={s:?} cols={cols}");
            } else {
                assert_eq!(p, *s, "超宽不截断: s={s:?} cols={cols}");
            }
        }
    }
}

/// R4 验收：宽度 20..200 全边界扫描，折行右边界零溢出（组合字符混排）。
#[test]
fn wrap_right_edge_never_overflows() {
    for max in 20..=200usize {
        for s in SAMPLES {
            for piece in wrap(s, max) {
                assert!(
                    width(&piece) <= max,
                    "折行溢出: s={s:?} max={max} piece={piece:?} w={}",
                    width(&piece)
                );
            }
        }
    }
}

/// X4-01b：词边界断行只折叠断点空白，非空白字符拼接仍无损。
#[test]
fn wrap_is_lossless() {
    for max in 2..=200usize {
        for s in SAMPLES {
            let joined: String = wrap(s, max).concat();
            let kept = |t: &str| -> String { t.chars().filter(|c| !c.is_whitespace()).collect() };
            assert_eq!(
                kept(&joined),
                kept(s),
                "折行丢非空白字符: s={s:?} max={max}"
            );
        }
    }
}

/// X4-02b（IS-01 显示侧）：全角字符按 2 列参与折行与裁剪，不切半字。
#[test]
fn fullwidth_wrap_and_clip_is01() {
    // 宽度 3 的奇数预算：汉字占 2 列，余 1 列只够一个半角字
    assert_eq!(wrap("中文ab", 3), vec!["中", "文a", "b"]);
    // 全角字母同样是 2 列，宽度 2 时每行恰一个
    assert_eq!(wrap("ＡＢＣ", 2), vec!["Ａ", "Ｂ", "Ｃ"]);

    let s = "全角标点：（测试）、「引用」——破折号";
    let kept = |t: &str| -> String { t.chars().filter(|c| !c.is_whitespace()).collect() };
    for max in 2..=16usize {
        let pieces = wrap(s, max);
        assert!(
            pieces.iter().all(|p| width(p) <= max),
            "全角折行越界: max={max} pieces={pieces:?}"
        );
        assert_eq!(kept(&pieces.concat()), kept(s), "全角折行丢失非空白字符");
    }

    // 裁剪预算减 1 留给省略号：4 列只容得下「全」+「…」
    assert_eq!(clip("全角，", 4), "全…");
    assert_eq!(clip("中a文", 3), "中…");
    for max in 0..=12usize {
        let c = clip(s, max);
        assert!(width(&c) <= max, "全角裁剪越界: max={max} out={c:?}");
    }
}

/// X4-03a：组合符零宽、与基字符同段，裁剪也不拆列。
#[test]
fn combining_marks_follow_base_char() {
    assert_eq!(char_w('\u{0301}'), 0);
    assert_eq!(width("e\u{0301}"), 1);
    assert_eq!(wrap("ae\u{0301}b", 2), vec!["ae\u{0301}", "b"]);
    assert_eq!(
        wrap("中\u{0301}文\u{0301}", 2),
        vec!["中\u{0301}", "文\u{0301}"]
    );
    assert_eq!(clip("e\u{0301}xyz", 2), "e\u{0301}…");
    assert_eq!(clip("中\u{0301}文\u{0301}", 3), "中\u{0301}…");
}

/// 附加符不得成为某段首字符：出现即说明折行把它从基字符上拆走。
#[test]
fn no_orphan_combining_marks() {
    let marks = [
        "e\u{0301}a\u{0301}o\u{0301}",
        "中\u{0301}文\u{0301}字\u{0301}",
        "a e\u{0301}b e\u{0301}c",
        "组合 e\u{0301} 与 全角，标点。",
        "a\u{0301}b\u{0301}c\u{0301}d\u{0301}",
    ];
    let kept = |t: &str| -> String { t.chars().filter(|c| !c.is_whitespace()).collect() };
    for max in 2..=16usize {
        for s in marks {
            let pieces = wrap(s, max);
            for p in &pieces {
                assert!(
                    p.chars().next().map_or(true, |c| char_w(c) > 0),
                    "附加符被拆到行首: s={s:?} max={max} piece={p:?}"
                );
                assert!(width(p) <= max, "组合折行越界: max={max} p={p:?}");
            }
            assert_eq!(kept(&pieces.concat()), kept(s), "组合符随基字符丢失");
        }
    }
}

/// X4-03b：ZWJ 序列、肤色修饰符、键帽序列整簇断行，任意预算都不拆开。
#[test]
fn zwj_sequences_are_atomic() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let wave = "\u{1F44B}\u{1F3FD}";
    let key = "\u{0031}\u{FE0F}\u{20E3}";
    // 合成字形只占一个字形宽，后续字符不被无谓挤到下一行
    assert_eq!(wrap(format!("{family}x"), 4), vec![format!("{family}x")]);
    assert_eq!(wrap(format!("{wave}x"), 4), vec![format!("{wave}x")]);
    // 键帽簇至少 2 列：预算 2 时后面的 x 必须换行，说明未被拆成裸字符
    assert_eq!(wrap(format!("{key}x"), 2), vec![key, "x"]);
    assert_eq!(wrap(family, 2), vec![family]);
    for max in 2..=20usize {
        for p in wrap(format!("a {family} {wave} b"), max) {
            assert!(!p.ends_with('\u{200D}'), "段尾悬空 ZWJ: {p:?}");
            assert!(
                p.chars().next().is_none_or(|c| !is_emoji_mod(c)),
                "段首孤立修饰符: {p:?}"
            );
            assert!(p.contains(family) || !p.contains('\u{200D}'));
        }
    }
}

#[test]
fn alloc_cols_never_exceeds_available() {
    let wants: &[Vec<usize>] = &[
        vec![0],
        vec![1, 1],
        vec![5, 3, 2],
        vec![40, 40, 40],
        vec![1, 100],
        vec![0, 0, 0],
        vec![120, 120, 120, 120, 120],
    ];
    for avail in 0..=200usize {
        for w in wants {
            let got = alloc_cols(w, 3, 1, avail);
            match got {
                Some(cols) => {
                    assert_eq!(cols.len(), w.len());
                    let total: usize = cols.iter().map(|c| c + 3).sum::<usize>() + 1;
                    assert!(
                        total <= avail,
                        "列宽分配溢出: want={w:?} avail={avail} cols={cols:?} total={total}"
                    );
                }
                None => {
                    // 仅在最小布局都放不下时才允许失败
                    assert!(
                        avail < w.len() * 4 + 1,
                        "可用宽度充足却拒绝分配: want={w:?} avail={avail}"
                    );
                }
            }
        }
    }
}

#[test]
fn alloc_cols_keeps_columns_when_room_enough() {
    assert_eq!(alloc_cols(&[5, 3], 3, 1, 40), Some(vec![5, 3]));
    assert_eq!(alloc_cols(&[], 3, 1, 40), Some(vec![]));
    // 恰好容下最小布局（每列 1 列内容 + unit 3 + fixed 1 = 9）时应保留各列
    assert_eq!(alloc_cols(&[40, 40], 3, 1, 9), Some(vec![1, 1]));
    // 再少 1 列则连最小布局都放不下，降级为 None
    assert_eq!(alloc_cols(&[40, 40], 3, 1, 8), None);
}
