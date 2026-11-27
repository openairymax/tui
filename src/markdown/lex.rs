// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 词法层（0.1.21 X4-05a）：按语言把代码正文切分为带作用域栈的 token 行。
//
// 只产出「文本 + 作用域名」，不决定颜色：样式映射留在 code.rs 的
// classify（X4-05b），折行与缓存键在其上叠加（X4-06）。分层的理由是同一
// 段文本可能按不同主题与色深复染，词法层若直接产出 SGR，缓存就得按主题
// 分裂，命中率无从谈起。
//
// 语言标记已经过 markdown::lang 探测归一，本层只接受规范 token；查不到
// 语法时整块按纯文本产出，上层照常按默认样式渲染，不丢内容。

use std::sync::OnceLock;
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

/// 一个词法片段：文本 + 从通用到具体的作用域栈（空栈表示未分类）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) text: String,
    pub(crate) scopes: Vec<String>,
}

/// 语法定义集：进程内一次加载（packdump 反序列化在百毫秒量级）。
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

/// 按行切分：行数恒等于 `body.split('\n')` 的段数，空行产出空 token 行。
///
/// 解析错误只影响当前行：该行整行作为一个未分类 token 产出，后续行继续
/// 用既有解析状态，避免一处异常吞掉整块代码。
pub(crate) fn lines(token: &str, body: &str) -> Vec<Vec<Token>> {
    let set = syntaxes();
    let syntax = if token.is_empty() {
        set.find_syntax_plain_text()
    } else {
        set.find_syntax_by_token(token)
            .unwrap_or_else(|| set.find_syntax_plain_text())
    };

    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<Vec<Token>> = Vec::new();

    for line in body.split('\n') {
        match state.parse_line(line, set) {
            Ok(ops) => {
                let mut row: Vec<Token> = Vec::new();
                for (text, op) in ScopeRegionIterator::new(&ops, line) {
                    if stack.apply(op).is_err() {
                        stack = ScopeStack::new();
                    }
                    if !text.is_empty() {
                        row.push(Token {
                            text: text.to_string(),
                            scopes: stack
                                .as_slice()
                                .iter()
                                .map(|scope| scope.build_string())
                                .collect(),
                        });
                    }
                }
                out.push(row);
            }
            Err(_) => out.push(if line.is_empty() {
                Vec::new()
            } else {
                vec![Token {
                    text: line.to_string(),
                    scopes: Vec::new(),
                }]
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 八种语言的样本：围栏标注 + 含字符串字面量的正文。token 先经
    /// markdown::lang 探测归一——这正是生产路径，用例不得绕过探测层。
    const SAMPLES: &[(&str, &str)] = &[
        ("rust", "fn main() { let s = \"hi\"; }"),
        ("python", "def f():\n    return \"hi\""),
        ("javascript", "const s = \"hi\";"),
        ("typescript", "const s: string = \"hi\";"),
        ("go", "var s = \"hi\""),
        ("c", "char *s = \"hi\";"),
        ("sh", "echo \"hi\""),
        ("json", "{\"s\": \"hi\"}"),
    ];

    #[test]
    fn eight_languages_lex_without_loss() {
        for (tag, body) in SAMPLES {
            let Some(found) = crate::markdown::lang::detect(tag) else {
                panic!("未探测到语言: {tag}");
            };
            let rows = lines(found.token, body);
            let rows_n = body.split('\n').count();
            assert_eq!(rows.len(), rows_n, "行数不一致: {tag}");
            let joined = rows
                .iter()
                .map(|r| r.iter().map(|t| t.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(joined, *body, "token 文本必须无损: {tag}");
            let has_string = rows
                .iter()
                .flatten()
                .any(|t| t.scopes.iter().any(|s| s.starts_with("string")));
            assert!(has_string, "未分出字符串作用域: {tag}");
        }
    }

    #[test]
    fn unknown_token_keeps_text() {
        let rows = lines("no-such-lang-xyz", "a b");
        assert_eq!(rows.len(), 1);
        let joined: String = rows[0].iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, "a b");
    }

    #[test]
    fn blank_lines_are_preserved() {
        let rows = lines("rust", "fn a() {}\n\nfn b() {}");
        assert_eq!(rows.len(), 3);
        assert!(rows[1].is_empty());
    }
}
