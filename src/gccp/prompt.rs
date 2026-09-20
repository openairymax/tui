// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP/GRAD 提示词构造子域（0.1.18 拆分自 gccp.rs）。
//
// 职责边界：仅把已确认状态（目标 / 逐轮问答 / 流程图）装配为发给 LLM 的提示词。
// 五问五答采用「逐一询问」模式：每轮只问 1 问，轮次之间让 LLM 基于已答事实思考，
// 使下一问更精准——因此第 N 问提示词必须携带前 N-1 轮的 Q/A 上下文。

use super::GccpState;

/// 第 n 问生成提示词（基于前 n-1 问的回答；LLM 思考后再提下一个问题）。
///
/// round = 1..=5。每轮只问 1 个问题，确保「问一个 → 思考 → 再问下一个」的逐一模式。
pub fn build_qn_prompt(state: &GccpState, round: u8) -> String {
    let mut ctx = format!(
        "你是「任务事实确认」（GCCP）主持人。当前任务目标：\n{}\n\n",
        state.goal
    );

    if round > 1 {
        ctx.push_str("用户已回答以下问题：\n");
        let qa: [(u32, &str, &str); 5] = [
            (1, &state.q1, &state.a1),
            (2, &state.q2, &state.a2),
            (3, &state.q3, &state.a3),
            (4, &state.q4, &state.a4),
            (5, &state.q5, &state.a5),
        ];
        for (n, q, a) in qa.iter().take((round - 1) as usize) {
            ctx.push_str(&format!("Q{}: {}\nA{}: {}\n", n, q, n, a));
        }
        ctx.push_str("\n请思考以上回答（隐含的约束、盲点与歧义），");
    } else {
        ctx.push('请');
    }

    ctx.push_str(&format!(
        "提出任务事实确认的第 {} 个问题（必须直接决定任务成败的关键事实：目标边界、约束、输入、环境、验收标准等）。要求：\n\
         - 只输出一个问题，严格以 Q{}: 开头\n\
         - 不要输出其他任何内容\n",
        round, round
    ));
    ctx
}

/// GRAD（任务流程图确认）生成提示词（基于全部 5 项事实）
pub fn build_grad_prompt(state: &GccpState) -> String {
    format!(
        "「任务事实确认」已全部完成，5 项事实如下：\n{}\n\n\
         请生成「任务流程图确认」（GRAD）文档，以 [GRAD] 开头，包含三部分：\n\
         1. 任务目标（一句话，基于已确认事实）\n\
         2. 执行步骤（Step 1..N，每步含前置条件、动作、输出）\n\
         3. 验收标准（可验证的完成条件）\n\
         用户将据此确认是否开始执行。\n\n\
         除 [GRAD] 文本外，必须额外输出结构化任务依赖图（DAG），格式如下（严格 JSON）：\n\
         [DAG]\n\
         {{\"nodes\":[{{\"id\":\"n1\",\"label\":\"步骤一动作\"}},{{\"id\":\"n2\",\"label\":\"步骤二动作\"}}],\
         \"edges\":[{{\"from\":\"n1\",\"to\":\"n2\"}}]}}\n\
         [/DAG]\n\
         - nodes 的 id 从 n1 起递增，label 为步骤一句话动作（≤24 字）\n\
         - edges 表达依赖：from 完成是 to 开始的前置条件；无依赖关系的步骤可并行（同层）\n\
         - DAG 必须与上述执行步骤一致，包含全部步骤及其依赖关系\n",
        state.facts()
    )
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
