// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// markdown 数学前置层（0.1.18 W6，§5 W6「数学」项）。
//
// 两层职责，彼此独立：
//
//   normalize —— 定界符归一。CommonMark 把 `\(` 当转义序列吃掉（`(` 属 ASCII
//   标点，反斜杠转义优先于一切），`\(...\)` / `\[...\]` 因此永远到不了解析器。
//   本层在解析前把它们改写为解析器唯一认识的 `$...$` / `$$...$$`；代码围栏、
//   行内代码、缩进代码块内的反斜杠原样保留（改写代码区会损坏源码显示）。
//
//   degrade —— LaTeX → 终端线性表达式。受限符号表，不引入排版引擎；产物交给
//   unicodeit 升格为 Unicode 上下标字形。
//
// 铁律：degrade 的产物绝不含反斜杠。无法识别的命令一律丢弃反斜杠只留名字，
// 绝不把 LaTeX 源码原样透到屏幕上。
//
// unicodeit 接入前提（一手取证 unicodeit 0.2.1 src/naive_replace.rs）：其
// REPLACEMENTS 是朴素子串替换且无词边界，`\l`→`ł` 会命中 `\log` 前缀而损坏
// 结果。全表扫描确认其中唯一不含反斜杠的键是 `-`→`−`，故**仅当输入无反斜杠
// 时**调用它：此时全部反斜杠锚定规则（pass 1/2/3/7）结构性不触发，只剩
// `-`→`−` 与上下标字形展开。这正是本层先自行消化掉全部反斜杠、再交给它的
// 原因，也是本层存在的主要理由。

/// 定界符归一：`\(...\)` → `$...$`，`\[...\]` → `$$...$$`。
///
/// 代码区（围栏、行内代码、缩进代码块）内的反斜杠一律原样保留；未配对或
/// 反斜杠被转义（`\\(`）的场合不改写，避免把普通文本误判成公式。
pub(super) fn norm_src(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut fence: Option<(char, usize)> = None;
    let mut prev_blank = true;

    for line in src.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let had_nl = line.len() != body.len();
        let indent = body.len() - body.trim_start_matches(' ').len();
        let blank = body.trim().is_empty();

        let fenced = fence.is_some();
        let indented_code = !fenced && prev_blank && indent >= 4 && !blank;

        if let Some(mark) = fence_mark(body) {
            if fenced {
                if matches!(fence, Some((c, n)) if c == mark.0 && mark.1 >= n) {
                    fence = None;
                }
            } else if !indented_code {
                fence = Some(mark);
            }
        }

        if fenced || fence.is_some() || indented_code {
            out.push_str(body);
        } else {
            out.push_str(&norm_line(body));
        }
        if had_nl {
            out.push('\n');
        }
        prev_blank = blank;
    }
    out
}

/// 围栏判定：缩进 ≤3 空格后连续 ≥3 个 `` ` `` 或 `~`。
///
/// 反引号围栏的信息串不得再含反引号，否则不构成围栏（CommonMark 规定）。
fn fence_mark(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = rest.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let n = rest.chars().take_while(|c| *c == ch).count();
    if n < 3 {
        return None;
    }
    if ch == '`' && rest.chars().skip(n).any(|c| c == '`') {
        return None;
    }
    Some((ch, n))
}

/// 单行定界符改写。仅改写「同行存在配对闭合符」的场合。
fn norm_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mask = code_mask(&chars);
    let mut out = String::with_capacity(line.len());
    let mut i = 0;

    while i < chars.len() {
        // 转义反斜杠（`\\(`）整体跳过：它表示字面反斜杠，不是数学定界符
        if !mask[i] && chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] == '\\' {
            out.push_str("\\\\");
            i += 2;
            continue;
        }
        let escaped = i + 1 < chars.len() && chars[i] == '\\' && !mask[i] && !mask[i + 1];
        if escaped {
            let pair = match chars[i + 1] {
                '(' => Some((')', "$")),
                '[' => Some((']', "$$")),
                _ => None,
            };
            if let Some((closer, mark)) = pair {
                if let Some(j) = close_at(&chars, &mask, i + 2, closer) {
                    out.push_str(mark);
                    out.extend(chars[i + 2..j].iter());
                    out.push_str(mark);
                    i = j + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 标记每个字符是否落在行内代码跨度内（含反引号定界符自身）。
fn code_mask(chars: &[char]) -> Vec<bool> {
    let mut mask = vec![false; chars.len()];
    let mut open: Option<usize> = None;
    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '`' {
            if open.is_some() {
                mask[i] = true;
            }
            i += 1;
            continue;
        }
        let n = chars.iter().skip(i).take_while(|c| **c == '`').count();
        match open {
            Some(t) if t == n => open = None,
            None => open = Some(n),
            _ => {}
        }
        for slot in mask.iter_mut().take((i + n).min(chars.len())).skip(i) {
            *slot = true;
        }
        i += n;
    }
    mask
}

/// 在 `from` 之后寻找未被代码跨度遮蔽的 `\` + `closer`。
fn close_at(chars: &[char], mask: &[bool], from: usize, closer: char) -> Option<usize> {
    let mut j = from;
    while j + 1 < chars.len() {
        if chars[j] == '\\' && !mask[j] && !mask[j + 1] && chars[j + 1] == closer {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// 数学片段降级：LaTeX → 终端可读线性表达式（产物无反斜杠）。
pub(super) fn degrade(tex: &str) -> String {
    let chars: Vec<char> = tex.chars().collect();
    let mut buf = String::with_capacity(tex.len());
    let mut i = 0;
    emit(&chars, &mut i, &mut buf);
    unicodeit::replace(buf.trim())
}

/// 顺序消费字符序列，把命令换成受限符号表内的终端字形。
fn emit(chars: &[char], i: &mut usize, out: &mut String) {
    while *i < chars.len() {
        let c = chars[*i];

        if c == '&' {
            out.push(' ');
            *i += 1;
            continue;
        }
        if c == '{' {
            if matches!(out.chars().last(), Some('^') | Some('_')) {
                out.push('{');
                *i += 1;
            } else {
                out.push_str(&group(chars, i));
            }
            continue;
        }
        if c != '\\' {
            out.push(c);
            *i += 1;
            continue;
        }

        *i += 1;
        if *i >= chars.len() {
            return;
        }
        let n = chars[*i];
        if n == '\\' {
            out.push_str(" ; ");
            *i += 1;
            continue;
        }
        if !n.is_ascii_alphabetic() {
            match n {
                ',' | ';' | ':' | '!' | ' ' | '/' => out.push(' '),
                _ => out.push(n),
            }
            *i += 1;
            continue;
        }

        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        let name: String = chars[start..*i].iter().collect();
        cmd(&name, chars, i, out);
    }
}

/// 单条命令的降级规则。
fn cmd(name: &str, chars: &[char], i: &mut usize, out: &mut String) {
    match name {
        "frac" | "dfrac" | "tfrac" | "cfrac" => {
            let a = group(chars, i);
            let b = group(chars, i);
            out.push_str(&paren(&a));
            out.push('/');
            out.push_str(&paren(&b));
        }
        "sqrt" => {
            let idx = bracket(chars, i);
            let x = group(chars, i);
            if let Some(k) = idx {
                out.push_str(&sup_of(&k));
            }
            out.push('√');
            out.push_str(&paren(&x));
        }
        "binom" | "dbinom" | "tbinom" => {
            let a = group(chars, i);
            let b = group(chars, i);
            out.push_str("C(");
            out.push_str(&a);
            out.push(',');
            out.push_str(&b);
            out.push(')');
        }
        "pmod" => {
            let a = group(chars, i);
            out.push_str("(mod ");
            out.push_str(&a);
            out.push(')');
        }
        "overset" | "stackrel" | "underset" => {
            let a = group(chars, i);
            let b = group(chars, i);
            out.push_str(&b);
            out.push(' ');
            out.push_str(&a);
        }
        "operatorname" | "text" | "textrm" | "textnormal" | "textup" | "mbox" | "mathrm"
        | "mathbf" | "mathit" | "mathsf" | "mathtt" | "mathcal" | "mathfrak" | "mathbb"
        | "boldsymbol" => {
            let a = group(chars, i);
            out.push_str(&a);
        }
        "begin" | "end" | "color" | "textcolor" | "class" | "style" | "label" | "tag"
        | "hspace" | "vspace" | "mspace" | "phantom" | "hphantom" | "vphantom" | "rule" => {
            let _ = group(chars, i);
        }
        "hat" | "widehat" => accent(chars, i, out, '\u{302}'),
        "bar" | "overline" => accent(chars, i, out, '\u{305}'),
        "tilde" | "widetilde" => accent(chars, i, out, '\u{303}'),
        "vec" => accent(chars, i, out, '\u{20d7}'),
        "dot" => accent(chars, i, out, '\u{307}'),
        "ddot" => accent(chars, i, out, '\u{308}'),
        "acute" => accent(chars, i, out, '\u{301}'),
        "grave" => accent(chars, i, out, '\u{300}'),
        "breve" => accent(chars, i, out, '\u{306}'),
        "check" => accent(chars, i, out, '\u{30c}'),
        "underline" => accent(chars, i, out, '\u{332}'),
        "quad" | "qquad" | "enspace" | "enskip" | "space" => out.push(' '),
        _ => {
            if let Some(s) = sym(name) {
                out.push_str(s);
            } else if !silent(name) {
                out.push_str(name);
            }
        }
    }
}

/// 取一个参数：`{...}` 分组、单条命令或单个字符。产物已降级。
fn group(chars: &[char], i: &mut usize) -> String {
    skip_ws(chars, i);
    if *i < chars.len() && chars[*i] == '{' {
        if let Some(end) = match_brace(chars, *i) {
            let inner: Vec<char> = chars[*i + 1..end].to_vec();
            let mut buf = String::new();
            let mut j = 0;
            emit(&inner, &mut j, &mut buf);
            *i = end + 1;
            return buf;
        }
        *i += 1;
        return String::new();
    }
    atom(chars, i)
}

/// 取一个原子：`{...}` 分组、单条命令或单个字符。
fn atom(chars: &[char], i: &mut usize) -> String {
    if *i >= chars.len() {
        return String::new();
    }
    if chars[*i] == '{' {
        return group(chars, i);
    }
    if chars[*i] != '\\' {
        let c = chars[*i];
        *i += 1;
        return c.to_string();
    }

    let mut buf = String::new();
    *i += 1;
    if *i >= chars.len() {
        return buf;
    }
    if chars[*i].is_ascii_alphabetic() {
        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        let name: String = chars[start..*i].iter().collect();
        cmd(&name, chars, i, &mut buf);
        return buf;
    }
    let c = chars[*i];
    *i += 1;
    if c != '\\' {
        buf.push(c);
    }
    buf
}

/// 可选方括号参数（如 `\sqrt[3]{x}` 的 `3`）。
fn bracket(chars: &[char], i: &mut usize) -> Option<String> {
    let save = *i;
    skip_ws(chars, i);
    if *i >= chars.len() || chars[*i] != '[' {
        *i = save;
        return None;
    }
    let mut j = *i + 1;
    while j < chars.len() && chars[j] != ']' {
        j += 1;
    }
    if j >= chars.len() {
        *i = save;
        return None;
    }
    let inner: Vec<char> = chars[*i + 1..j].to_vec();
    let mut buf = String::new();
    let mut k = 0;
    emit(&inner, &mut k, &mut buf);
    *i = j + 1;
    Some(buf)
}

/// 组合附加符：基字后追加 Unicode 组合标记。
fn accent(chars: &[char], i: &mut usize, out: &mut String, mark: char) {
    let base = group(chars, i);
    out.push_str(&base);
    out.push(mark);
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i] == ' ' {
        *i += 1;
    }
}

/// 花括号配对（跳过 `\{` / `\}` 转义）。`open` 必须指向 `{`。
fn match_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 分子/分母/根号内容按需加括号：单原子裸出，含运算符才包裹。
fn paren(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.chars().any(needs_paren) {
        format!("({s})")
    } else {
        s.to_string()
    }
}

fn needs_paren(c: char) -> bool {
    matches!(
        c,
        ' ' | '+' | '-' | '×' | '÷' | '·' | '*' | '/' | '=' | '<' | '>' | '≤' | '≥' | '≠'
            | '≈' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '→' | '⇒' | '±'
    )
}

/// 上标化：`x^2` → `x²`（复用 unicodeit 的字形表，避免第二套映射）。
fn sup_of(s: &str) -> String {
    unicodeit::replace(&format!("^{{{s}}}"))
}

/// 受限符号表：命令名 → 终端字形。表外命令由 `cmd` 的兜底分支处理。
fn sym(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" | "varrho" => "ρ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",

        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "bigcup" => "⋃",
        "bigcap" => "⋂",
        "bigoplus" => "⨁",
        "bigotimes" => "⨂",

        "times" => "×",
        "div" => "÷",
        "cdot" => "·",
        "ast" => "∗",
        "star" => "⋆",
        "circ" => "∘",
        "bullet" => "•",
        "pm" => "±",
        "mp" => "∓",
        "oplus" => "⊕",
        "otimes" => "⊗",

        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "simeq" => "≃",
        "cong" => "≅",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",

        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "emptyset" | "varnothing" => "∅",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",

        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",

        "angle" => "∠",
        "perp" => "⊥",
        "parallel" => "∥",
        "therefore" => "∴",
        "because" => "∵",
        "degree" => "°",
        "prime" => "′",
        "ldots" | "dots" | "dotsc" => "…",
        "cdots" | "dotsb" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "wp" => "℘",
        "surd" => "√",
        "checkmark" => "✓",
        "flat" => "♭",
        "sharp" => "♯",
        "clubsuit" => "♣",
        "heartsuit" => "♡",
        "spadesuit" => "♠",
        "diamondsuit" => "♢",
        "triangle" => "△",
        "square" => "□",
        _ => return None,
    })
}

/// 结构化命令：只影响排版，线性表达式中直接丢弃。
fn silent(name: &str) -> bool {
    matches!(
        name,
        "left"
            | "right"
            | "big"
            | "Big"
            | "bigg"
            | "Bigg"
            | "bigl"
            | "bigr"
            | "Bigl"
            | "Bigr"
            | "biggl"
            | "biggr"
            | "Biggl"
            | "Biggr"
            | "displaystyle"
            | "textstyle"
            | "scriptstyle"
            | "scriptscriptstyle"
            | "limits"
            | "nolimits"
            | "mathstrut"
            | "mathord"
            | "mathbin"
            | "mathrel"
            | "mathopen"
            | "mathclose"
            | "mathpunct"
            | "mathop"
            | "strut"
            | "smash"
            | "hline"
            | "tiny"
            | "scriptsize"
            | "footnotesize"
            | "small"
            | "normalsize"
            | "large"
            | "Large"
            | "LARGE"
            | "huge"
            | "Huge"
            | "thinspace"
            | "negthinspace"
            | "allowbreak"
            | "nobreak"
            | "nonumber"
            | "notag"
            | "middle"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_rewrites_inline_paren_delims() {
        assert_eq!(norm_src("前 \\(a+b\\) 后"), "前 $a+b$ 后");
    }

    #[test]
    fn norm_rewrites_display_bracket_delims() {
        assert_eq!(norm_src("\\[x^2\\]"), "$$x^2$$");
    }

    #[test]
    fn norm_keeps_fenced_code_untouched() {
        let out = norm_src("```\n\\(x\\)\n```\n\\(y\\)");
        assert!(out.contains("\\(x\\)"), "围栏内被改写: {out}");
        assert!(out.ends_with("$y$"), "围栏外未改写: {out}");
    }

    #[test]
    fn norm_keeps_inline_code_untouched() {
        assert_eq!(norm_src("`\\(x\\)` 与 \\(y\\)"), "`\\(x\\)` 与 $y$");
    }

    #[test]
    fn norm_keeps_indented_code_untouched() {
        let out = norm_src("段落\n\n    \\(x\\)\n");
        assert!(out.contains("\\(x\\)"), "缩进代码块被改写: {out}");
    }

    #[test]
    fn norm_ignores_unpaired_delim() {
        assert_eq!(norm_src("孤立的 \\( 符号"), "孤立的 \\( 符号");
    }

    #[test]
    fn norm_ignores_escaped_backslash() {
        assert_eq!(norm_src("\\\\(x\\\\)"), "\\\\(x\\\\)");
    }

    #[test]
    fn degrade_frac_uses_slash() {
        assert_eq!(degrade("\\frac{a}{b}"), "a/b");
        assert_eq!(degrade("\\frac{a+b}{c}"), "(a+b)/c");
    }

    #[test]
    fn degrade_sqrt_uses_radical() {
        assert_eq!(degrade("\\sqrt{x}"), "√x");
        assert_eq!(degrade("\\sqrt[3]{x}"), "³√x");
    }

    #[test]
    fn degrade_promotes_sub_super_to_glyphs() {
        assert_eq!(degrade("x^2"), "x²");
        assert_eq!(degrade("a_1"), "a₁");
        assert_eq!(degrade("x^n"), "xⁿ");
    }

    #[test]
    fn degrade_sum_with_limits() {
        assert_eq!(degrade("\\sum_{i=1}^{n}"), "∑ᵢ₌₁ⁿ");
        assert_eq!(degrade("\\int_0^1"), "∫₀¹");
    }

    #[test]
    fn degrade_greek_and_operators() {
        assert_eq!(degrade("\\alpha + \\beta"), "α + β");
        assert_eq!(degrade("a \\times b"), "a × b");
        assert_eq!(degrade("x \\leq y"), "x ≤ y");
        assert_eq!(degrade("A \\subseteq B"), "A ⊆ B");
    }

    #[test]
    fn degrade_keeps_function_names_intact() {
        assert_eq!(degrade("\\log x"), "log x");
        assert_eq!(degrade("\\ln x"), "ln x");
        assert_eq!(degrade("\\sin^2 x"), "sin² x");
    }

    #[test]
    fn degrade_drops_left_right() {
        assert_eq!(degrade("\\left( \\frac{a}{b} \\right)"), "( a/b )");
    }

    #[test]
    fn degrade_handles_text_and_operatorname() {
        assert_eq!(degrade("\\text{for all}"), "for all");
        assert_eq!(degrade("\\operatorname{argmax}_{x} f(x)"), "argmaxₓ f(x)");
    }

    #[test]
    fn degrade_handles_matrix_and_binom() {
        assert_eq!(degrade("\\binom{n}{k}"), "C(n,k)");
        assert_eq!(degrade("\\pmod{m}"), "(mod m)");
        let m = degrade("\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}");
        assert_eq!(m, "a   b  ;  c   d");
    }

    #[test]
    fn degrade_applies_combining_accents() {
        assert_eq!(degrade("\\hat{x}"), "x\u{302}");
        assert_eq!(degrade("\\bar{y}"), "y\u{305}");
        assert_eq!(degrade("\\vec{v}"), "v\u{20d7}");
    }

    #[test]
    fn degrade_never_leaks_backslash() {
        let cases = [
            "\\frac{\\alpha_1}{\\sqrt{x}}",
            "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
            "\\hat{x} \\cdot \\bar{y}",
            "\\unknowncmd{a} \\left[ \\int_0^1 f \\right]",
            "\\operatorname{argmax}_{x} f(x)",
            "\\binom{n}{k} \\pmod{m}",
            "\\sum_{i=1}^{n} \\frac{1}{i^2} \\to \\infty",
            "\\textcolor{red}{x} \\tag{1} \\label{eq}",
            "\\overset{def}{=} \\quad \\therefore",
            "\\left\\{ x \\in \\mathbb{R} \\mid x > 0 \\right\\}",
        ];
        for case in cases {
            let out = degrade(case);
            assert!(!out.contains('\\'), "残留反斜杠: {case} -> {out}");
        }
    }

    #[test]
    fn degrade_is_idempotent_on_plain_text() {
        assert_eq!(degrade("plain text 123"), "plain text 123");
    }
}
