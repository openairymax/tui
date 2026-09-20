// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP/GRAD 输入输出解析子域（0.1.18 拆分自 gccp.rs）。
//
// 职责边界：LLM 输出 → 结构化问题列表（parse_questions）、用户输入 → 回答列表
// （parse_answers）、确认/完成指令识别与 [TASK:DONE] 标记处理。
//
// 容忍度纪律：LLM 输出与用户输入形态都不可控，解析一律「宽松匹配 + 剥离前缀」，
// 识别不出即返回空/ false，由调用方决定降级路径，禁止在此处 panic。

/// 从 LLM 输出解析问题列表。
///
/// 支持 "Q1: xxx"、"Q2: xxx"（大小写不敏感、可带空格）形式；
/// 返回 [(序号, 问题)]，序号即问题编号（1..=5）。
pub fn parse_questions(resp: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in resp.lines() {
        let line = line.trim();
        let b = line.as_bytes();
        if b.len() < 3 || (b[0] != b'Q' && b[0] != b'q') {
            continue;
        }
        // 读取编号数字（"Q1:" → 1），要求编号后紧跟冒号
        let mut i = 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == 1 || b.get(i) != Some(&b':') {
            continue;
        }
        let Ok(n) = line[1..i].parse::<u32>() else {
            continue;
        };
        if !(1..=5).contains(&n) {
            continue;
        }
        let body = line[i + 1..].trim().to_string();
        if !body.is_empty() {
            out.push((n, body));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// 拆分用户对多问题的回答（每行一个，按序对应）。
///
/// 支持 "1: xxx" / "2: xxx" 前缀或纯行拆分；返回回答列表（去除空行）。
pub fn parse_answers(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // 剥离开头 "1:" / "A1:" / "1." 等序号前缀
        let stripped = strip_answer_prefix(t);
        out.push(stripped);
    }
    out
}

fn strip_answer_prefix(line: &str) -> String {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return line.to_string();
    }
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b' ') {
        i += 1;
    }
    // 前缀形如 "1:" / "1." / "A1:"
    let lower = line[i..].trim_start().to_lowercase();
    if let Some(rest) = lower.strip_prefix(':') {
        return rest.trim().to_string();
    }
    if let Some(rest) = lower.strip_prefix('.') {
        return rest.trim().to_string();
    }
    // "A1:" 形式
    let t = line.trim_start();
    let lower_t = t.to_lowercase();
    if lower_t.starts_with('a') && lower_t.len() >= 2 && lower_t.as_bytes()[1].is_ascii_digit() {
        if let Some(idx) = t.find(':') {
            return t[idx + 1..].trim().to_string();
        }
    }
    line.to_string()
}

/// 用户输入是否为确认指令（GRAD 通过）
pub fn is_confirm(input: &str) -> bool {
    let t = input.trim().to_lowercase();
    matches!(
        t.as_str(),
        "确认" | "同意" | "ok" | "okay" | "yes" | "y" | "通过" | "确认执行"
    )
}

/// 用户输入是否为任务完成指令（触发技能沉淀）
pub fn is_task_done_input(input: &str) -> bool {
    let t = input.trim().to_lowercase();
    let t = t.trim_matches(['！', '!', '。', '.', ' ', '，', ',']);
    matches!(
        t,
        "完成" | "已完成" | "任务完成" | "完毕" | "结束" | "done" | "finish" | "all done"
    )
}

/// 检查 LLM 输出中是否含 [TASK:DONE] 标记（任务成功信号）
pub fn has_task_done_marker(resp: &str) -> bool {
    resp.contains("[TASK:DONE]")
}

/// 剥离 [TASK:DONE] 标记及其所在行（展示用）
pub fn strip_task_done(resp: &str) -> String {
    resp.lines()
        .filter(|l| !l.trim().contains("[TASK:DONE]"))
        .collect::<Vec<_>>()
        .join("\n")
}
