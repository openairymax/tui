// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP（任务事实确认）与 GRAD（任务流程图确认）。
//
// 大任务集启动时进入 GCCP：任务事实确认共 5 个问题，分三轮交互式提问，
// 每轮之间让 LLM 基于已答事实思考，使问题更精准：
//   第 1 轮：提出第 1-2 问，用户作答
//   → LLM 思考
//   第 2 轮：提出第 3-4 问，用户作答
//   → LLM 思考
//   第 3 轮：提出第 5 问，用户作答
// 五问齐备后进入 GRAD（任务流程图确认）：LLM 生成执行流程图，用户确认后开始执行。

/// 任务流阶段状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPhase {
    /// 普通对话（非任务集）
    Chat,
    /// 任务事实确认（GCCP）第 1 轮：等待用户回答第 1-2 问
    GccpRound1,
    /// 任务事实确认（GCCP）第 2 轮：等待用户回答第 3-4 问
    GccpRound2,
    /// 任务事实确认（GCCP）第 3 轮：等待用户回答第 5 问
    GccpRound3,
    /// 任务流程图确认（GRAD）：等待用户确认流程图
    GradConfirm,
    /// 任务集执行中
    Executing,
}

impl FlowPhase {
    /// 状态栏展示名（中文术语）
    #[allow(dead_code)] // 单测使用；UI 按阶段匹配颜色自行展示
    pub fn label(self) -> &'static str {
        match self {
            FlowPhase::Chat => "对话",
            FlowPhase::GccpRound1 | FlowPhase::GccpRound2 | FlowPhase::GccpRound3 => {
                "任务事实确认"
            }
            FlowPhase::GradConfirm => "任务流程图确认",
            FlowPhase::Executing => "任务集",
        }
    }

    /// 输入框提示（引导用户作答）
    pub fn input_hint(self) -> &'static str {
        match self {
            FlowPhase::GccpRound1 => "回答第 1-2 问（每行一个）：",
            FlowPhase::GccpRound2 => "回答第 3-4 问（每行一个）：",
            FlowPhase::GccpRound3 => "回答第 5 问：",
            FlowPhase::GradConfirm => "输入「确认」通过流程图，或输入修改意见：",
            _ => "> ",
        }
    }
}

/// GCCP 五问五答 + GRAD 流程图状态
#[derive(Debug, Clone, Default)]
pub struct GccpState {
    /// 任务目标（LLM 判定为大任务集时的原始输入）
    pub goal: String,
    pub q1: String,
    pub a1: String,
    pub q2: String,
    pub a2: String,
    pub q3: String,
    pub a3: String,
    pub q4: String,
    pub a4: String,
    pub q5: String,
    pub a5: String,
    /// GRAD 任务流程图（用户确认后进入执行）
    pub grad_plan: String,
}

impl GccpState {
    /// 清空状态
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 汇总 5 项已确认事实（Q+A 交错，供 GRAD 与后续执行使用）
    pub fn facts(&self) -> String {
        let mut out = String::new();
        for (q, a) in [
            (&self.q1, &self.a1),
            (&self.q2, &self.a2),
            (&self.q3, &self.a3),
            (&self.q4, &self.a4),
            (&self.q5, &self.a5),
        ] {
            if q.trim().is_empty() {
                continue;
            }
            out.push_str(&format!("Q: {}\nA: {}\n", q.trim(), a.trim()));
        }
        out
    }
}

/// 第 1-2 问生成提示词
pub fn build_q12_prompt(goal: &str) -> String {
    format!(
        "你是「任务事实确认」（GCCP）主持人。当前任务目标：\n{}\n\n\
         请提出任务事实确认的第 1、2 个问题。要求：\n\
         - 问题必须直接决定任务成败的关键事实（目标边界、约束、输入、环境、验收标准等）\n\
         - 每个问题一行，严格以 Q1: 与 Q2: 开头\n\
         - 不要输出其他任何内容\n",
        goal
    )
}

/// 第 3-4 问生成提示词（基于用户对 1-2 问的回答）
pub fn build_q34_prompt(state: &GccpState) -> String {
    format!(
        "你是「任务事实确认」（GCCP）主持人。当前任务目标：\n{}\n\n\
         用户已回答第 1-2 问：\nQ1: {}\nA1: {}\nQ2: {}\nA2: {}\n\n\
         请思考以上回答（隐含的约束、盲点与歧义），然后提出第 3、4 个问题。要求：\n\
         - 每个问题一行，严格以 Q3: 与 Q4: 开头\n\
         - 不要输出其他任何内容\n",
        state.goal, state.q1, state.a1, state.q2, state.a2
    )
}

/// 第 5 问生成提示词（基于 1-4 问的回答）
pub fn build_q5_prompt(state: &GccpState) -> String {
    format!(
        "你是「任务事实确认」（GCCP）主持人。当前任务目标：\n{}\n\n\
         用户已回答第 1-4 问：\nQ1: {}\nA1: {}\nQ2: {}\nA2: {}\nQ3: {}\nA3: {}\nQ4: {}\nA4: {}\n\n\
         请思考以上回答，提出最后一个（第 5 个）问题，用于补全仍缺失的关键事实。要求：\n\
         - 只输出一个问题，严格以 Q5: 开头\n\
         - 不要输出其他任何内容\n",
        state.goal, state.q1, state.a1, state.q2, state.a2, state.q3, state.a3, state.q4, state.a4
    )
}

/// GRAD（任务流程图确认）生成提示词（基于全部 5 项事实）
pub fn build_grad_prompt(state: &GccpState) -> String {
    format!(
        "「任务事实确认」已全部完成，5 项事实如下：\n{}\n\n\
         请生成「任务流程图确认」（GRAD）文档，以 [GRAD] 开头，包含三部分：\n\
         1. 任务目标（一句话，基于已确认事实）\n\
         2. 执行步骤（Step 1..N，每步含前置条件、动作、输出）\n\
         3. 验收标准（可验证的完成条件）\n\
         用户将据此确认是否开始执行。\n",
        state.facts()
    )
}

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
    if lower_t.starts_with('a')
        && lower_t.len() >= 2
        && lower_t.as_bytes()[1].is_ascii_digit()
    {
        if let Some(idx) = t.find(':') {
            return t[idx + 1..].trim().to_string();
        }
    }
    line.to_string()
}

/// 用户输入是否为确认指令（GRAD 通过）
pub fn is_confirm(input: &str) -> bool {
    let t = input.trim().to_lowercase();
    matches!(t.as_str(), "确认" | "同意" | "ok" | "okay" | "yes" | "y" | "通过" | "确认执行")
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

/// 拼接执行阶段的上下文（目标 + 事实 + 流程图）
pub fn build_execute_prompt(state: &GccpState) -> String {
    format!(
        "【任务目标】\n{}\n\n【已确认事实】\n{}\n【任务流程图（已确认）】\n{}\n\n\
         请按照已确认的流程图开始执行任务，每一步完成后简要汇报进展。",
        state.goal,
        state.facts(),
        state.grad_plan
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_questions_works() {
        let resp = "Q1: 目标是什么\nQ2: 有哪些约束\n\nQ3: 输入数据格式";
        let qs = parse_questions(resp);
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0], (1, "目标是什么".to_string()));
        assert_eq!(qs[1], (2, "有哪些约束".to_string()));
        assert_eq!(qs[2], (3, "输入数据格式".to_string()));
    }

    #[test]
    fn parse_questions_tolerates_garbage() {
        let resp = "好的，我来提问：\nQ1: 第一步做什么\n无关内容\nQ2: 第二步做什么";
        let qs = parse_questions(resp);
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].0, 1);
        assert_eq!(qs[1].0, 2);
    }

    #[test]
    fn parse_questions_lowercase_and_spaces() {
        let resp = "q1: a\nq 2: b";
        let qs = parse_questions(resp);
        // q1 识别；"q 2"（空格分隔）不识别
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0], (1, "a".to_string()));
    }

    #[test]
    fn parse_answers_splits_lines() {
        let ans = parse_answers("目标是部署系统\n约束是保持兼容");
        assert_eq!(ans.len(), 2);
        assert_eq!(ans[0], "目标是部署系统");
        assert_eq!(ans[1], "约束是保持兼容");
    }

    #[test]
    fn parse_answers_strips_prefixes() {
        let ans = parse_answers("1: 第一个回答\nA2: 第二个回答\n3. 第三个回答");
        assert_eq!(ans.len(), 3);
        assert_eq!(ans[0], "第一个回答");
        assert_eq!(ans[1], "第二个回答");
        assert_eq!(ans[2], "第三个回答");
    }

    #[test]
    fn confirm_and_done_detection() {
        assert!(is_confirm("确认"));
        assert!(is_confirm(" OK "));
        assert!(is_confirm("同意"));
        assert!(!is_confirm("再想想"));
        assert!(is_task_done_input("完成"));
        assert!(is_task_done_input("任务完成！"));
        assert!(is_task_done_input("done"));
        assert!(!is_task_done_input("继续"));
    }

    #[test]
    fn task_done_marker_helpers() {
        assert!(has_task_done_marker("已完成\n[TASK:DONE]"));
        assert!(!has_task_done_marker("已完成"));
        assert_eq!(
            strip_task_done("任务完成\n[TASK:DONE]"),
            "任务完成"
        );
    }

    #[test]
    fn flow_phase_labels() {
        assert_eq!(FlowPhase::Chat.label(), "对话");
        assert_eq!(FlowPhase::GccpRound1.label(), "任务事实确认");
        assert_eq!(FlowPhase::GradConfirm.label(), "任务流程图确认");
        assert_eq!(FlowPhase::Executing.label(), "任务集");
    }

    #[test]
    fn prompts_use_correct_rounds() {
        let s = GccpState::default();
        assert!(build_q12_prompt("部署服务").contains("Q1:"));
        assert!(build_q34_prompt(&s).contains("Q3:"));
        assert!(build_q5_prompt(&s).contains("Q5:"));
        assert!(build_grad_prompt(&s).contains("[GRAD]"));
        assert!(build_execute_prompt(&s).contains("已确认事实"));
    }

    #[test]
    fn facts_concatenates_q_and_a() {
        let mut s = GccpState::default();
        s.q1 = "目标".into();
        s.a1 = "部署".into();
        s.q2 = "约束".into();
        s.a2 = "兼容".into();
        let facts = s.facts();
        assert!(facts.contains("Q: 目标"));
        assert!(facts.contains("A: 部署"));
        assert!(facts.contains("Q: 约束"));
        assert!(!facts.contains("Q5"));
    }
}
