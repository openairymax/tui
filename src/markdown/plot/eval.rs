// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// plot 表达式求值：受限 PRN（逆波兰）解释器（0.1.18 W7，§5 W7 约束）。
//
// 安全边界（硬约束，不可放宽）：
//   - 白名单函数 20 个 + 常量 pi/e/tau，表外标识符一律拒绝；
//   - 括号与函数嵌套深度 ≤ MAX_DEPTH(32)；
//   - 单次求值步数 ≤ MAX_STEPS(10_000)，超限即失败；
//   - 无 IO、无循环、无赋值、无变量声明——表达式语法无法表达控制流。
// 故本模块不构成任意代码执行能力，也不存在"循环炸弹"面。
//
// 编译分两步：中缀 → 调度场算法 → PRN 序列；求值只消费 PRN 序列。
// 编译产物可跨采样点复用，逐点求值不含解析开销。

/// 求值失败（对调用方只表达"该点不可求值"，不泄漏内部细节）。
#[derive(Debug, Clone, Copy)]
pub(super) struct EvalErr;

/// 括号/函数嵌套深度上界。
const MAX_DEPTH: usize = 32;
/// 单次求值步数上界（每消费一个 PRN 记号计一步）。
const MAX_STEPS: usize = 10_000;
/// 编译产物长度上界（防超长表达式拖垮编译）。
const MAX_TOKS: usize = 512;

/// PRN 记号。
#[derive(Clone, Copy)]
pub(super) enum Tok {
    Num(f64),
    /// 自变量 x。
    Var,
    /// 一元负号。
    Neg,
    /// 二元运算符。
    Bin(char),
    /// 白名单函数调用。
    Call(Func),
}

/// 白名单函数（恰好 20 个）。
#[derive(Clone, Copy)]
pub(super) enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sqrt,
    Cbrt,
    Abs,
    Ln,
    Log10,
    Exp,
    Floor,
    Ceil,
    Round,
    Sign,
    Min,
    Max,
    Pow,
}

impl Func {
    fn arity(self) -> usize {
        match self {
            Func::Atan2 | Func::Min | Func::Max | Func::Pow => 2,
            _ => 1,
        }
    }

    /// 参数按源码顺序放在 `a`（首个）与 `b`（次个）。
    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Func::Sin => a.sin(),
            Func::Cos => a.cos(),
            Func::Tan => a.tan(),
            Func::Asin => a.asin(),
            Func::Acos => a.acos(),
            Func::Atan => a.atan(),
            Func::Atan2 => a.atan2(b),
            Func::Sqrt => a.sqrt(),
            Func::Cbrt => a.cbrt(),
            Func::Abs => a.abs(),
            Func::Ln => a.ln(),
            Func::Log10 => a.log10(),
            Func::Exp => a.exp(),
            Func::Floor => a.floor(),
            Func::Ceil => a.ceil(),
            Func::Round => a.round(),
            Func::Sign => a.signum(),
            Func::Min => a.min(b),
            Func::Max => a.max(b),
            Func::Pow => a.powf(b),
        }
    }
}

/// 调度场算法栈内元素。
#[derive(Clone, Copy)]
enum Op {
    Bin(char),
    Neg,
    Func(Func),
    Paren,
}

impl Op {
    fn into_tok(self) -> Option<Tok> {
        match self {
            Op::Bin(c) => Some(Tok::Bin(c)),
            Op::Neg => Some(Tok::Neg),
            Op::Func(_) | Op::Paren => None,
        }
    }

    /// (优先级, 右结合)。一元负号高于乘除、低于乘方，故 `-x^2` = `-(x^2)`。
    fn prec(self) -> Option<(u8, bool)> {
        match self {
            Op::Bin('^') => Some((4, true)),
            Op::Neg => Some((3, false)),
            Op::Bin('*') | Op::Bin('/') | Op::Bin('%') => Some((2, false)),
            Op::Bin(_) => Some((1, false)),
            Op::Func(_) | Op::Paren => None,
        }
    }
}

/// 编译表达式为中缀→PRN 序列。语法错误、深度超限、表外标识符一律失败。
pub(super) fn compile(src: &str) -> Result<Vec<Tok>, EvalErr> {
    let b = src.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    let mut expect_operand = true;
    let mut i = 0;

    while i < b.len() {
        let c = b[i] as char;

        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            // 并置操作数（`1 2`、`x y`、`2(3)`）不是合法表达式，必须显式拒绝：
            // 若放任其进入 PRN，会在求值期表现为"栈内多余值"而非语法错误。
            if !expect_operand {
                return Err(EvalErr);
            }
            let (v, ni) = num_at(b, i)?;
            out.push(Tok::Num(v));
            i = ni;
            expect_operand = false;
        } else if c.is_ascii_alphabetic() || c == '_' {
            if !expect_operand {
                return Err(EvalErr);
            }
            let (name, ni) = ident_at(b, i);
            i = ni;
            if let Some(f) = func_of(&name) {
                // 函数必须紧跟左括号；函数标记与左括号一并入栈，故深度计数含函数层
                let mut j = i;
                while j < b.len() && (b[j] as char).is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() || b[j] != b'(' {
                    return Err(EvalErr);
                }
                ops.push(Op::Func(f));
                ops.push(Op::Paren);
                i = j + 1;
                expect_operand = true;
            } else if let Some(v) = const_of(&name) {
                out.push(Tok::Num(v));
                expect_operand = false;
            } else if name == "x" {
                out.push(Tok::Var);
                expect_operand = false;
            } else {
                return Err(EvalErr);
            }
        } else if c == '(' {
            if !expect_operand {
                return Err(EvalErr);
            }
            ops.push(Op::Paren);
            i += 1;
            expect_operand = true;
        } else if c == ')' {
            if expect_operand {
                return Err(EvalErr);
            }
            loop {
                match ops.pop() {
                    Some(Op::Paren) => break,
                    Some(op) => {
                        if let Some(t) = op.into_tok() {
                            out.push(t);
                        }
                    }
                    None => return Err(EvalErr),
                }
            }
            if let Some(Op::Func(f)) = ops.last().copied() {
                ops.pop();
                out.push(Tok::Call(f));
            }
            i += 1;
            expect_operand = false;
        } else if c == ',' {
            // 只允许在函数实参之间出现：弹到最近的左括号为止
            let mut seen_paren = false;
            while let Some(op) = ops.last().copied() {
                if matches!(op, Op::Paren) {
                    seen_paren = true;
                    break;
                }
                ops.pop();
                if let Some(t) = op.into_tok() {
                    out.push(t);
                }
            }
            if !seen_paren {
                return Err(EvalErr);
            }
            i += 1;
            expect_operand = true;
        } else if matches!(c, '+' | '-' | '*' | '/' | '%' | '^') {
            if expect_operand {
                if c == '-' {
                    ops.push(Op::Neg);
                } else if c != '+' {
                    return Err(EvalErr);
                }
                i += 1;
                continue;
            }
            let (p, right) = Op::Bin(c).prec().ok_or(EvalErr)?;
            while let Some(top) = ops.last().copied() {
                let Some((tp, _)) = top.prec() else { break };
                if tp > p || (tp == p && !right) {
                    ops.pop();
                    if let Some(t) = top.into_tok() {
                        out.push(t);
                    }
                } else {
                    break;
                }
            }
            ops.push(Op::Bin(c));
            i += 1;
            expect_operand = true;
        } else {
            return Err(EvalErr);
        }

        if ops.len() > MAX_DEPTH || out.len() > MAX_TOKS {
            return Err(EvalErr);
        }
    }

    if expect_operand {
        return Err(EvalErr);
    }
    while let Some(op) = ops.pop() {
        if matches!(op, Op::Paren) {
            return Err(EvalErr);
        }
        if let Some(t) = op.into_tok() {
            out.push(t);
        }
    }
    if out.is_empty() {
        return Err(EvalErr);
    }
    Ok(out)
}

/// 在 PRN 序列上求值。栈深与步数均有界，非有限结果按"不可求值"返回。
pub(super) fn eval(prn: &[Tok], x: f64) -> Result<f64, EvalErr> {
    let mut st: Vec<f64> = Vec::with_capacity(16);
    let mut steps = 0usize;

    for t in prn {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(EvalErr);
        }
        match *t {
            Tok::Num(v) => st.push(v),
            Tok::Var => st.push(x),
            Tok::Neg => {
                let a = st.pop().ok_or(EvalErr)?;
                st.push(-a);
            }
            Tok::Bin(c) => {
                let b = st.pop().ok_or(EvalErr)?;
                let a = st.pop().ok_or(EvalErr)?;
                st.push(bin(c, a, b));
            }
            Tok::Call(f) => {
                let n = f.arity();
                let mut args = [0.0f64; 2];
                for k in (0..n).rev() {
                    args[k] = st.pop().ok_or(EvalErr)?;
                }
                st.push(f.apply(args[0], args[1]));
            }
        }
    }

    if st.len() != 1 {
        return Err(EvalErr);
    }
    let v = st[0];
    if v.is_finite() {
        Ok(v)
    } else {
        Err(EvalErr)
    }
}

fn bin(c: char, a: f64, b: f64) -> f64 {
    match c {
        '+' => a + b,
        '-' => a - b,
        '*' => a * b,
        '/' => a / b,
        '%' => a % b,
        '^' => a.powf(b),
        _ => f64::NAN,
    }
}

/// 数字字面量（含 `e`/`E` 指数，仅在后随数字时归入数字，避免与常量 e 混淆）。
fn num_at(b: &[u8], mut i: usize) -> Result<(f64, usize), EvalErr> {
    let start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let s = std::str::from_utf8(&b[start..i]).map_err(|_| EvalErr)?;
    let v: f64 = s.parse().map_err(|_| EvalErr)?;
    if v.is_finite() {
        Ok((v, i))
    } else {
        Err(EvalErr)
    }
}

fn ident_at(b: &[u8], mut i: usize) -> (String, usize) {
    let start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    (
        String::from_utf8_lossy(&b[start..i]).into_owned(),
        i,
    )
}

fn func_of(name: &str) -> Option<Func> {
    Some(match name {
        "sin" => Func::Sin,
        "cos" => Func::Cos,
        "tan" => Func::Tan,
        "asin" => Func::Asin,
        "acos" => Func::Acos,
        "atan" => Func::Atan,
        "atan2" => Func::Atan2,
        "sqrt" => Func::Sqrt,
        "cbrt" => Func::Cbrt,
        "abs" => Func::Abs,
        "ln" => Func::Ln,
        "log" => Func::Log10,
        "exp" => Func::Exp,
        "floor" => Func::Floor,
        "ceil" => Func::Ceil,
        "round" => Func::Round,
        "sign" => Func::Sign,
        "min" => Func::Min,
        "max" => Func::Max,
        "pow" => Func::Pow,
        _ => return None,
    })
}

fn const_of(name: &str) -> Option<f64> {
    match name {
        "pi" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        "tau" => Some(std::f64::consts::TAU),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(expr: &str, x: f64) -> f64 {
        let prn = compile(expr).expect("应可编译");
        eval(&prn, x).expect("应可求值")
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn arithmetic_precedence() {
        near(at("1 + 2 * 3", 0.0), 7.0);
        near(at("(1 + 2) * 3", 0.0), 9.0);
        near(at("2 ^ 3 ^ 2", 0.0), 512.0);
        near(at("10 % 3", 0.0), 1.0);
    }

    #[test]
    fn unary_minus_binds_below_power() {
        near(at("-2 ^ 2", 0.0), -4.0);
        near(at("2 ^ -1", 0.0), 0.5);
        near(at("-3 + 1", 0.0), -2.0);
    }

    #[test]
    fn whitelist_functions() {
        near(at("sin(0)", 0.0), 0.0);
        near(at("cos(0)", 0.0), 1.0);
        near(at("sqrt(9)", 0.0), 3.0);
        near(at("abs(-4)", 0.0), 4.0);
        near(at("ln(e)", 0.0), 1.0);
        near(at("log(1000)", 0.0), 3.0);
        near(at("floor(1.7)", 0.0), 1.0);
        near(at("ceil(1.2)", 0.0), 2.0);
        near(at("round(1.5)", 0.0), 2.0);
        near(at("sign(-9)", 0.0), -1.0);
        near(at("min(3, 5)", 0.0), 3.0);
        near(at("max(3, 5)", 0.0), 5.0);
        near(at("pow(2, 10)", 0.0), 1024.0);
        near(at("atan2(0, 1)", 0.0), 0.0);
        near(at("cbrt(27)", 0.0), 3.0);
        near(at("exp(0)", 0.0), 1.0);
    }

    #[test]
    fn constants_and_variable() {
        near(at("pi", 0.0), std::f64::consts::PI);
        near(at("tau", 0.0), std::f64::consts::TAU);
        near(at("x * 2", 21.0), 42.0);
        near(at("sin(x)", std::f64::consts::FRAC_PI_2), 1.0);
    }

    #[test]
    fn nested_calls_are_supported() {
        near(at("sqrt(abs(-16))", 0.0), 4.0);
        near(at("max(min(1, 2), 3)", 0.0), 3.0);
    }

    #[test]
    fn scientific_literal_is_accepted() {
        near(at("1e3", 0.0), 1000.0);
        near(at("1.5e-2", 0.0), 0.015);
    }

    #[test]
    fn unknown_identifier_is_rejected() {
        assert!(compile("system(\"ls\")").is_err());
        assert!(compile("x + y").is_err());
        assert!(compile("sin x").is_err());
        assert!(compile("").is_err());
    }

    #[test]
    fn malformed_syntax_is_rejected() {
        assert!(compile("1 +").is_err());
        assert!(compile("(1").is_err());
        assert!(compile("1)").is_err());
        assert!(compile("min(1,)").is_err());
        assert!(compile("1 2").is_err());
        assert!(compile("$x").is_err());
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        assert!(compile(&deep).is_err(), "深度 40 应超限");
        let ok = format!("{}1{}", "(".repeat(8), ")".repeat(8));
        assert!(compile(&ok).is_ok(), "深度 8 应通过");
    }

    #[test]
    fn long_expression_is_rejected() {
        let long = vec!["1"; MAX_TOKS + 10].join("+");
        assert!(compile(&long).is_err(), "记号数超限应失败");
    }

    #[test]
    fn domain_errors_are_not_values() {
        let prn = compile("sqrt(x)").expect("可编译");
        assert!(eval(&prn, -1.0).is_err(), "sqrt(-1) 非有限，应不可求值");
        let div = compile("1 / x").expect("可编译");
        assert!(eval(&div, 0.0).is_err(), "1/0 非有限，应不可求值");
    }

    #[test]
    fn evaluation_is_pure_and_repeatable() {
        let prn = compile("x ^ 2 + 2 * x + 1").expect("可编译");
        let a = eval(&prn, 3.0).expect("可求值");
        let b = eval(&prn, 3.0).expect("可求值");
        assert_eq!(a, b);
        near(a, 16.0);
    }
}
