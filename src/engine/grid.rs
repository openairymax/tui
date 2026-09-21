// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 渲染引擎 L2 排版层：终端显示宽度的唯一裁决点（0.1.18 A 轨 W2）。
//
// 设计依据（§3.3 / §5A.0 / §5A.3 W2）：
//   - 宽度权威唯一：全仓只在本模块调用 unicode-width。截断、填充、折行、
//     表格列宽四项排版判定全部下沉至此，上层不得自行测量后截断。
//     此前 markdown / gccp / wizard 三处各自实现同一套宽度循环，口径漂移
//     直接造成 R4（CJK 混排右边界溢出）：gccp 用字符数当列宽、再按显示
//     宽度填充，全角标签必然错位；markdown 表格按 Σ(列宽+2)+1 估算总宽，
//     而实渲染每列占 列宽+3 列，窄屏收缩量少算 列数 列。
//   - 物理边界（§5A.0 裁定）：L2 的存储载体是 ratatui 的 Buffer/Cell，
//     本层不另建紧凑存储。故此处只提供纯函数式的测量与编排原语。
//   - Buffer 直写原语（put_string）与其消费者（迁移后的合成路径）同批落位：
//     P0 阶段渲染仍走 ui.rs 直绘路径，此时落该原语既无调用方，又会被
//     `-D warnings` 的 dead_code 门禁判失败（§15.5.1 不建空壳层）。
//   - §3.3 的"驻留池"以 Cell 内嵌池索引为前提，而 Cell 由 ratatui 提供且
//     无池索引字段；该设计随 §5A.0 一并收敛，本版不引入第二份字符串池。
//
// 不变量（属性测试实证）：对任意 max ≥ 2，折行结果每段显示宽度 ≤ max 且
// 拼接无损；对任意 max，截断结果显示宽度 ≤ max；列宽分配结果的实渲染总宽
// （Σ(列宽+unit)+fixed）≤ 可用宽度。

use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 字符串显示宽度（CJK/全角按 2 列，组合字符按 0 列）。
pub(crate) fn width(s: &str) -> usize {
    s.width()
}

/// 一行的显示宽度（各片段内容宽度之和）。
///
/// 行的居中/对齐预算必须由此推出：上层若改用字符数或自行遍历，全角片段
/// 下居中量会偏小（同 R4 口径漂移）。
pub(crate) fn line_w(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width(s.content.as_ref())).sum()
}

/// 单字符显示宽度（不可测字符按 0 列，与 ratatui 单元格语义一致）。
pub(crate) fn char_w(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// 按显示宽度截断，超宽时以省略号收尾；结果宽度恒 ≤ `max`。
///
/// `max` 为 0 时返回空串（省略号本身也需 1 列，无法容纳）。
pub(crate) fn clip(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // 预留 1 列给省略号，故正文预算为 max-1
    let budget = max - 1;
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = char_w(ch);
        if w + cw > budget {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// 右侧补齐到 `cols` 列；已达标（含超宽）时原样返回，不做截断。
pub(crate) fn pad(s: &str, cols: usize) -> String {
    let w = width(s);
    if w >= cols {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(cols - w))
    }
}

/// 按显示宽度折行（无断词语义，逐字符硬折）。
///
/// `max` < 2 时单列无法容纳任何双列字符，原样返回单段（调用方已用
/// `.max(8)` 兜底，此处仅保证确定性且不丢字符）。
pub(crate) fn wrap(s: &str, max: usize) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    if max < 2 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in s.chars() {
        let w = char_w(ch);
        if cur_w + w > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 按显示宽度折行一组带样式片段：结果每段显示宽度 ≤ `max`，拼接无损。
///
/// markdown 的行内样式在解析期已定型为片段序列，折行不能再退化为纯字符串
/// 处理（否则样式丢失或需二次解析）；此处按 `char_w` 逐字符切分并保留原样式，
/// 使其与 [`wrap`] 共享同一宽度口径。
pub(crate) fn wrap_spans(spans: &[Span<'_>], max: usize) -> Vec<Vec<Span<'static>>> {
    if max < 2 {
        return vec![spans
            .iter()
            .map(|s| Span::styled(s.content.to_string(), s.style))
            .collect()];
    }
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    for sp in spans {
        let style = sp.style;
        let mut buf = String::new();
        for ch in sp.content.chars() {
            let w = char_w(ch);
            if cur_w + w > max && cur_w > 0 {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), style));
                }
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            buf.push(ch);
            cur_w += w;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// 按显示宽度截断一组带样式片段（超宽以省略号收尾）；结果宽度恒 ≤ `max`。
///
/// 省略号沿用末片段样式，保持视觉连续；`max` 为 0 时返回空（省略号亦需 1 列）。
pub(crate) fn clip_spans(spans: &[Span<'_>], max: usize) -> Vec<Span<'static>> {
    if max == 0 {
        return Vec::new();
    }
    if line_w(spans) <= max {
        return spans
            .iter()
            .map(|s| Span::styled(s.content.to_string(), s.style))
            .collect();
    }
    let budget = max - 1;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut w = 0usize;
    'outer: for sp in spans {
        let mut buf = String::new();
        for ch in sp.content.chars() {
            let cw = char_w(ch);
            if w + cw > budget {
                break 'outer;
            }
            buf.push(ch);
            w += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, sp.style));
        }
    }
    let tail = spans.last().map(|s| s.style).unwrap_or_default();
    out.push(Span::styled("…", tail));
    out
}

/// 在 `avail` 列内为各列分配内容宽度。
///
/// 每列除内容外另占 `unit` 列（边框与内边距），整体再含 `fixed` 列（行首尾）。
/// 分配结果保证 `Σ(列宽 + unit) + fixed ≤ avail`——这是表格/网格右边界
/// 零溢出的充分条件。连"每列 1 列内容"的最小布局都放不下时返回 `None`，
/// 由调用方降级为纯文本（不画半个表格）。
pub(crate) fn alloc_cols(
    want: &[usize],
    unit: usize,
    fixed: usize,
    avail: usize,
) -> Option<Vec<usize>> {
    let cols = want.len();
    if cols == 0 {
        return Some(Vec::new());
    }
    if avail < cols * (1 + unit) + fixed {
        return None;
    }
    let budget = avail - cols * unit - fixed;
    let need: usize = want.iter().sum();
    if need <= budget {
        return Some(want.to_vec());
    }
    // 等比收缩，每列不低于 1 列
    let mut out: Vec<usize> = want.iter().map(|w| (w * budget / need).max(1)).collect();
    let mut sum: usize = out.iter().sum();
    // 取整抬高可能超预算：从最宽列逐列回收。budget ≥ cols 已由前置检查保证，
    // 故最宽列必然可减，循环必然收敛到 sum == budget（≥ cols）。
    while sum > budget {
        let i = (0..cols).max_by_key(|&i| out[i]).unwrap_or(0);
        if out[i] <= 1 {
            break;
        }
        out[i] -= 1;
        sum -= 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(wrap("hello world", 5), vec!["hello", " worl", "d"]);
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

    #[test]
    fn wrap_is_lossless() {
        for max in 2..=200usize {
            for s in SAMPLES {
                let joined: String = wrap(s, max).concat();
                assert_eq!(joined, *s, "折行丢字符: s={s:?} max={max}");
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
}
