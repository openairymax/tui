// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 任务流头部与流式尾段（0.1.9 W8）：GCCP/GRAD/执行中引导区 +
// 流式工具/思考链/打字机气泡尾段。头部与消息块行数无关（恒物化，
// 规模有界）；尾段随流式状态每帧现算（本就 O(流长)，与旧行为一致）。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::app::{App, ChatMessage, MessageRole};
use crate::gccp::FlowPhase;
use crate::theme;

use super::block;

/// 思考动效帧（Braille spinner，Claude 风格轻量旋转；0.1s 一帧）。
/// 相较文字循环（t→th→…）更克制优雅：仅一个字符宽度，不跳动文本。
const THINKING: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 任务节点运行动效（◐◓◑◒ 顺时针旋转，0.5s 一帧）。
/// 执行期间多个节点并行 Running 时同步旋转，传达"进行中"的层次动态
/// （2.3.9：层级动态细致处理；2.3.15 Claude 美学——状态可见且不喧宾夺主）。
const NODE_SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// 2.3.14：根据思考链模型轨映射双思考标签（区分实时思考状态）。
///   t2 模型思考 → [Dual Slow Think]（慢思考）
///   t1-f 模型思考 → [Dual Fast Think]（快思考）
///   t1-p 模型思考 → [Dual Prof Think]（专业思考）
/// 未识别（通用对话/llm_d 默认模型）→ 通用 [Dual Think]。
/// 模型轨来自 gateway reasoning 事件 model 字段，与 env 配置
/// AIRY_MODEL_T2 / AIRY_MODEL_T1F / AIRY_MODEL_T1P 双向模糊匹配
/// （env 值可为模型名或端点 URL，model 为 llm_d 实际请求模型名）。
fn dual_think_label(model: &str) -> &'static str {
    if model.is_empty() {
        return "[Dual Think]";
    }
    const TRACKS: [(&str, &str); 3] = [
        ("AIRY_MODEL_T2", "[Dual Slow Think]"),
        ("AIRY_MODEL_T1F", "[Dual Fast Think]"),
        ("AIRY_MODEL_T1P", "[Dual Prof Think]"),
    ];
    for (env, label) in TRACKS {
        if let Ok(cfg) = std::env::var(env) {
            let cfg = cfg.trim();
            if !cfg.is_empty() && (model.contains(cfg) || cfg.contains(model)) {
                return label;
            }
        }
    }
    "[Dual Think]"
}

/// 任务流阶段头部：GCCP 五问进度 / GRAD 确认（含 DAG 依赖图）/ 执行中（含进度与中止提示）。
pub(super) fn render_header(out: &mut Vec<Line<'static>>, app: &App, width: usize) {
    match app.flow_phase {
        FlowPhase::Chat => {}
        FlowPhase::GccpRound(_) => {
            let answered = app.gccp.answered();
            let dots: String = (1..=5)
                .map(|i| {
                    let done = match i {
                        1 => !app.gccp.a1.trim().is_empty(),
                        2 => !app.gccp.a2.trim().is_empty(),
                        3 => !app.gccp.a3.trim().is_empty(),
                        4 => !app.gccp.a4.trim().is_empty(),
                        _ => !app.gccp.a5.trim().is_empty(),
                    };
                    if done { "●" } else { "○" }
                })
                .collect();
            out.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled("任务事实确认", Style::default().fg(theme::primary())),
                Span::styled(" ", Style::default()),
                Span::styled(dots, Style::default().fg(theme::primary())),
                Span::styled(format!("  {answered}/5"), Style::default().fg(theme::faint())),
            ]));
            out.push(Line::raw(""));
        }
        FlowPhase::GccpClarify => {
            // 服务端 GCCP 目标澄清（P-A）：问题集已由 think.process 回传，
            // 逐条展示（id + 问题 + 提示），等待用户作答后携带 gccp_answers 重发。
            out.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled("目标澄清", Style::default().fg(theme::primary())),
                Span::styled(
                    "  请回答以下问题（逐行作答，或输入「跳过」放弃本轮问答）",
                    Style::default().fg(theme::dim()),
                ),
            ]));
            out.push(Line::raw(""));
            if let Some(p) = &app.gccp_pending {
                for (i, q) in p.questions.iter().enumerate() {
                    let done = p.answers.contains_key(&q.id);
                    let marker = if done { "●" } else { "○" };
                    let mut spans = vec![Span::styled(
                        format!("  {marker} "),
                        Style::default().fg(theme::primary()),
                    )];
                    spans.push(Span::styled(
                        format!("{}. {}", i + 1, q.question),
                        Style::default().fg(if done { theme::faint() } else { theme::accent() }),
                    ));
                    if !q.hint.is_empty() {
                        spans.push(Span::styled(
                            format!("  （{}）", q.hint),
                            Style::default().fg(theme::faint()),
                        ));
                    }
                    out.push(Line::from(spans));
                }
                out.push(Line::raw(""));
            }
        }
        FlowPhase::GradConfirm => {
            out.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "任务流程图确认",
                    Style::default().fg(theme::magenta()).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  输入「确认」开始执行，或输入修改意见", Style::default().fg(theme::dim())),
            ]));
            out.push(Line::raw(""));
            // 结构化 DAG 依赖图（LLM 生成 [DAG] 块 → 解析成功时渲染）
            if let Some(dag) = &app.gccp.dag {
                let dag_width = width.saturating_sub(6).max(40);
                for line in crate::gccp::render_dag_lines(dag, dag_width) {
                    out.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(line, Style::default().fg(theme::accent())),
                    ]));
                }
                out.push(Line::raw(""));
            }
        }
        FlowPhase::Executing => {
            // 任务集执行中：阶段徽章 + DAG 节点数（若有）+ 人工控制状态提示
            let node_hint = app
                .gccp
                .dag
                .as_ref()
                .filter(|d| !d.is_empty())
                .map(|d| format!(" · {} 个步骤", d.node_count()))
                .unwrap_or_default();
            let control_hint = match app.task_control {
                crate::gccp::TaskControl::Paused => "  ⏸ 已暂停（Ctrl+Z 恢复）",
                crate::gccp::TaskControl::Aborted => "  ✕ 已中止",
                crate::gccp::TaskControl::Running => "  Ctrl+X 中止 · Ctrl+Z 暂停",
            };
            out.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("任务集执行中{node_hint}"),
                    // 苹果轻量字重风格：无粗体，暖黄
                    Style::default().fg(theme::warning()),
                ),
                Span::styled(control_hint, Style::default().fg(theme::dim())),
            ]));

            // DAG 节点状态持续渲染（P2-C 过程可视化；2026-08-21 增甘特式进度条）：
            // 总体进度条（▰ 完成 / ▱ 未完成）+ 逐节点状态（◐ 运行中 · ● 完成
            // · ✕ 失败 · ○ 未达）；分支符号（├/└）标注节点层级，与 C 版 airy_cli 一致。
            if let Some(dag) = &app.gccp.dag {
                if !dag.is_empty() {
                    out.push(Line::raw(""));
                    let n = dag.nodes.len();
                    // 甘特式总体进度条：已完成节点 / 总数（进度可视化）
                    let done = app
                        .gccp
                        .node_states
                        .iter()
                        .filter(|s| **s == crate::gccp::NodeState::Done)
                        .count();
                    let bar_w = width.saturating_sub(24).clamp(12, 40);
                    let bar = gantt_bar(done, n, bar_w);
                    out.push(Line::from(vec![
                        Span::styled("    ", Style::default()),
                        Span::styled("进度", Style::default().fg(theme::primary())),
                        Span::styled(" ", Style::default()),
                        Span::styled(
                            bar.clone(),
                            Style::default().fg(if done == n { theme::success() } else { theme::warning() }),
                        ),
                        Span::styled(
                            format!("  {done}/{n}"),
                            Style::default().fg(theme::faint()),
                        ),
                    ]));
                    for (i, node) in dag.nodes.iter().enumerate() {
                        let state = app
                            .gccp
                            .node_states
                            .get(i)
                            .copied()
                            .unwrap_or(crate::gccp::NodeState::Pending);
                        // 运行动效：Running 节点 ◐◓◑◒ 随时间旋转（0.5s 一帧），
                        // 并行节点同步转动，层级状态"活"起来（2.3.9）
                        let spin = if state == crate::gccp::NodeState::Running {
                            NODE_SPIN[(app.session_start.elapsed().as_millis() / 500) as usize % 4]
                        } else {
                            "◐"
                        };
                        let (mark, color) = match state {
                            crate::gccp::NodeState::Pending => ("○", theme::faint()),
                            crate::gccp::NodeState::Running => (spin, theme::warning()),
                            crate::gccp::NodeState::Done => ("●", theme::success()),
                            crate::gccp::NodeState::Failed => ("✕", theme::danger()),
                        };
                        // 分支符号：末节点 └，其余 ├（层级先导）
                        let branch = if i == n - 1 { "└─" } else { "├─" };
                        // 节点行：状态标记 + 标签 + 甘特式状态短条（宽度足够时）
                        let mut spans = vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(branch, Style::default().fg(theme::faint())),
                            Span::styled(" ", Style::default()),
                            Span::styled(mark, Style::default().fg(color)),
                            Span::styled(" ", Style::default()),
                            Span::styled(node.label.clone(), Style::default().fg(theme::text())),
                        ];
                        if width >= 56 {
                            spans.push(Span::styled("  ", Style::default()));
                            spans.push(Span::styled(node_state_bar(state), Style::default().fg(color)));
                        }
                        out.push(Line::from(spans));
                    }
                }
            }
            out.push(Line::raw(""));
        }
    }
}

/// 流式尾段：工具状态行 + 思考链进度行 + 打字机气泡 + 思考动效。
/// 均为瞬态内容（每帧现算），不参与行高缓存。
pub(super) fn render_tail(out: &mut Vec<Line<'static>>, app: &App, width: usize) {
    // 流式工具状态行（SSE tool_call/tool_result 事件，Claude Code 风格：
    // 工具调用时显示 [Sub <tool> Agent] 状态，不污染正文气泡）
    for evt in app.stream_tool_events.iter() {
        out.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(evt.clone(), Style::default().fg(theme::tool_fg())),
        ]));
    }

    // 流式思考链状态行（SSE __airy_evt:reasoning → stream_reasoning）：
    // 思考内容为模型内部推理碎片，逐块上屏无展示价值——流式期间仅显示
    // 一行状态（字数 + 耗时进度），落定后折叠为摘要行，Alt+E 展开全量。
    // 标签按模型轨区分（2.3.14）。正文首片到达即隐藏（与 C 版 CLI 的
    // 思考进度行竞态门控对齐，2026-08-19）。
    if app.loading
        && !app.stream_reasoning.is_empty()
        && app.streaming_text.is_empty()
    {
        let secs = app
            .stream_reasoning_start
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        out.push(Line::from(vec![
            Span::styled(
                dual_think_label(&app.stream_reasoning_model).to_string(),
                Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  思考中… {} 字 · {:.1}s（Alt+E 查看思考链）",
                    app.stream_reasoning.chars().count(),
                    secs
                ),
                Style::default().fg(theme::faint()),
            ),
        ]));
    }

    // 流式输出：SSE 增量块已累计在 streaming_text，实时渲染为「Airymax」气泡
    // （Claude 风格逐字上屏；完成后由 apply_stream_result 落为正式消息）
    if app.loading && !app.streaming_text.is_empty() {
        // 打字机上屏：只显示前 reveal 个字符（伪流式下制造逐字动效，
        // F5 修复：此前网关一次性返回整段文本，无任何输出动效）
        let mut revealed: String = app
            .streaming_text
            .chars()
            .take(app.streaming_reveal)
            .collect();
        if revealed.len() < app.streaming_text.len() {
            // 上屏未完成：光标块表示"正在生成"
            revealed.push('▍');
        }
        let streaming_msg = ChatMessage {
            role: MessageRole::Agent,
            content: revealed,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            id: ChatMessage::NO_ID,
        };
        block::render(out, &streaming_msg, width, false, false);
    }

    if app.loading {
        // 思考动效：Braille spinner 旋转（0.1s 一帧，与 ui.rs 同一时钟；
        // Claude 风格轻量旋转——单字符宽度，不跳动文本，克制优雅）
        let frame = (app.session_start.elapsed().as_millis() / 100) as usize % THINKING.len();
        out.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {}", THINKING[frame]),
                Style::default().fg(theme::faint()),
            ),
        ]));
    }
}

/// 甘特式总体进度条：▰ 完成 / ▱ 未完成（按完成比例填充，2026-08-21）。
fn gantt_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 || width == 0 {
        return String::new();
    }
    let filled = done.saturating_mul(width) / total;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(empty))
}

/// 节点状态短进度条（4 格，甘特式）：
/// Done=▰▰▰▰ · Running=▰▱▱▱ · Pending=▱▱▱▱ · Failed=✕✕✕✕
fn node_state_bar(state: crate::gccp::NodeState) -> &'static str {
    match state {
        crate::gccp::NodeState::Done => "▰▰▰▰",
        crate::gccp::NodeState::Running => "▰▱▱▱",
        crate::gccp::NodeState::Pending => "▱▱▱▱",
        crate::gccp::NodeState::Failed => "✕✕✕✕",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 甘特式进度条：按完成比例填充 ▰/▱，边界（0/满/除零）安全。
    #[test]
    fn gantt_bar_fills_by_ratio() {
        assert_eq!(gantt_bar(0, 4, 8), "▱▱▱▱▱▱▱▱");
        assert_eq!(gantt_bar(4, 4, 8), "▰▰▰▰▰▰▰▰");
        assert_eq!(gantt_bar(2, 4, 8), "▰▰▰▰▱▱▱▱");
        // 除零 / 零宽：安全返回空
        assert_eq!(gantt_bar(1, 0, 8), "");
        assert_eq!(gantt_bar(1, 4, 0), "");
    }

    /// 节点状态短条：四态映射与标记一致。
    #[test]
    fn node_state_bar_maps_states() {
        use crate::gccp::NodeState;
        assert_eq!(node_state_bar(NodeState::Done), "▰▰▰▰");
        assert_eq!(node_state_bar(NodeState::Running), "▰▱▱▱");
        assert_eq!(node_state_bar(NodeState::Pending), "▱▱▱▱");
        assert_eq!(node_state_bar(NodeState::Failed), "✕✕✕✕");
    }
}
