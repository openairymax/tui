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
// 非空白字符拼接无损（断点处的空白折叠为换行）；组合字符恒随基字符同段，
// ZWJ 序列与 emoji 修饰符不拆散；对任意 max，截断结果显示宽度 ≤ max；
// 列宽分配结果的实渲染总宽（Σ(列宽+unit)+fixed）≤ 可用宽度。

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 零宽连接符：其两侧字形在终端合成为一个字形，折行不得从中间切开。
const ZWJ: char = '\u{200D}';

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

/// 按显示宽度折行（词边界优先，词超宽时退化为逐字符硬折）。
///
/// 断点选择（X4-01b）：优先在空白串之前断行，断行处的空白折叠为换行，故
/// 非空白字符拼接无损；一段内没有空白（中文、长标识符）时才按字符硬折，
/// 保证每段显示宽度恒 ≤ `max`。
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
    let units: Vec<(char, Style)> = s.chars().map(|c| (c, Style::default())).collect();
    fold(&units, max)
        .into_iter()
        .map(|line| line.into_iter().map(|(c, _)| c).collect())
        .collect()
}

/// 按显示宽度折行一组带样式片段：结果每段显示宽度 ≤ `max`，非空白字符无损。
///
/// markdown 的行内样式在解析期已定型为片段序列，折行不能再退化为纯字符串
/// 处理（否则样式丢失或需二次解析）；此处把片段摊平为带样式的字符序列，
/// 与 [`wrap`] 共用同一断点算法与宽度口径，再按样式合并回片段。
pub(crate) fn wrap_spans(spans: &[Span<'_>], max: usize) -> Vec<Vec<Span<'static>>> {
    if max < 2 {
        return vec![spans
            .iter()
            .map(|s| Span::styled(s.content.to_string(), s.style))
            .collect()];
    }
    let units: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    for line in fold(&units, max) {
        let mut spans_out: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut cur_style = Style::default();
        for (i, (c, st)) in line.into_iter().enumerate() {
            if i > 0 && st != cur_style {
                spans_out.push(Span::styled(std::mem::take(&mut buf), cur_style));
            }
            cur_style = st;
            buf.push(c);
        }
        if !buf.is_empty() {
            spans_out.push(Span::styled(buf, cur_style));
        }
        out.push(spans_out);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// 折行核心：带样式的字符序列 → 行序列。
///
/// 断点机会记录在 `brk`：最近一次「空白串起点」的下标（不含该空白），
/// 只有在它之前已有非空白内容时才可用来断行。行满时优先回退到该点，
/// 走不通则硬折；两条路径都保证结果宽度 ≤ `max`。
///
/// 输入先按「基字符 + 附加序列」切簇（X4-03a/03b）：组合符、变体选择符
/// 显示宽度为 0，逐字符断行会把附加符甩到下一行行首；ZWJ 与 emoji 修饰符
/// 则会把一个字形拆成多段。成簇后整簇参与宽度判定与搬移，字形不可拆。
fn fold(units: &[(char, Style)], max: usize) -> Vec<Vec<(char, Style)>> {
    let mut out: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut cur_w = 0usize;
    let mut brk: Option<usize> = None;
    let mut prev_ws = false;
    let mut has_text = false;
    let mut any_text = false;

    let mut idx = 0usize;
    while idx < units.len() {
        let from = idx;
        idx = cluster_end(units, from);
        let cluster = &units[from..idx];
        let w = cluster_w(cluster);
        let ch = cluster[0].0;
        let cl_ws = ch.is_whitespace();
        while !cur.is_empty() && cur_w + w > max {
            if let Some(b) = brk {
                out.push(cur.drain(..b).collect());
                // 断点处的空白折叠为换行：剩下的只可能是空白串 + 后续内容
                cur = std::mem::take(&mut cur)
                    .into_iter()
                    .skip_while(|(c, _)| c.is_whitespace())
                    .collect();
                cur_w = units_w(&cur);
                has_text = cur.iter().any(|(c, _)| !c.is_whitespace());
                prev_ws = false;
                brk = last_break(&cur);
            } else {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
                has_text = false;
                prev_ws = false;
            }
        }
        // 行首空白：上一行已在断点处折叠（或本身以空白开头），丢弃即可；
        // 否则空格会粘到新行行首，使后续词被无谓挤断（"hello world" → " worl"）。
        if cur.is_empty() && cl_ws {
            prev_ws = true;
            continue;
        }
        if cl_ws {
            if !prev_ws && has_text {
                brk = Some(cur.len());
            }
        } else {
            has_text = true;
            any_text = true;
        }
        prev_ws = cl_ws;
        cur.extend_from_slice(cluster);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    } else if out.is_empty() && !any_text {
        // 纯空白输入：保留一行，避免调用方拿不到任何行
        out.push(units.to_vec());
    }
    out
}

/// 组合簇终点：基字符 + 附加符（零宽）+ ZWJ 连接的字形 + emoji 修饰符。
///
/// ZWJ 之后的字符与前一字形合成一体（家庭/职业 emoji），肤色修饰符同理；
/// 二者若各自成簇，折行点会插进字形内部，渲染出半个 emoji 或改色字形。
fn cluster_end(units: &[(char, Style)], start: usize) -> usize {
    let mut i = start + 1;
    loop {
        while i < units.len() && char_w(units[i].0) == 0 {
            i += 1;
        }
        if i >= units.len() {
            return i;
        }
        if units[i - 1].0 == ZWJ || is_emoji_mod(units[i].0) {
            i += 1;
        } else {
            return i;
        }
    }
}

/// 簇显示宽度：ZWJ 序列由终端合成单字形，按首字符宽度计；带 emoji 呈现
/// 选择符或键帽组合的簇至少占 2 列；肤色修饰符不另占列；其余按字符宽度和。
fn cluster_w(cluster: &[(char, Style)]) -> usize {
    if cluster.iter().any(|&(c, _)| c == ZWJ) {
        return char_w(cluster[0].0).max(1);
    }
    if is_emoji_mod(cluster[0].0) {
        // 孤立修饰符（前面没有基字形）：按自身宽度保守计入
        return char_w(cluster[0].0);
    }
    let sum: usize = cluster
        .iter()
        .filter(|&&(c, _)| !is_emoji_mod(c))
        .map(|&(c, _)| char_w(c))
        .sum();
    let wide = cluster
        .iter()
        .any(|&(c, _)| c == '\u{FE0F}' || c == '\u{20E3}');
    if wide {
        sum.max(2)
    } else {
        sum
    }
}

/// 一段字符序列的总显示宽度（按簇计算，ZWJ 序列只计一次）。
fn units_w(units: &[(char, Style)]) -> usize {
    let mut w = 0usize;
    let mut i = 0usize;
    while i < units.len() {
        let end = cluster_end(units, i);
        w += cluster_w(&units[i..end]);
        i = end;
    }
    w
}

/// emoji 肤色修饰符 U+1F3FB..=U+1F3FF：并入前一字形，不另占列。
fn is_emoji_mod(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// 最近一次可断行位置：空白串的起点，且其前缀含非空白内容；否则 `None`。
fn last_break(line: &[(char, Style)]) -> Option<usize> {
    let mut pos = None;
    let mut in_ws = false;
    let mut seen_text = false;
    for (i, (c, _)) in line.iter().enumerate() {
        if c.is_whitespace() {
            if seen_text && !in_ws {
                pos = Some(i);
            }
            in_ws = true;
        } else {
            seen_text = true;
            in_ws = false;
        }
    }
    pos
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
#[path = "grid_tests.rs"]
mod tests;
