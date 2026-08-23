// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Chat panel rendering: 对话主面板（随终端大小自适应）。

use ratatui::{
    layout::Rect,
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::app::{App, MessageRole};
use crate::gccp::FlowPhase;
use crate::theme;

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

/// 长回复折叠（2026-08-17，与 C 版 airy_cli 对齐）：最新 Agent 回复渲染
/// 行数超过 FOLD_MAX_LINES 时，live 视口只显示前 FOLD_KEEP_LINES 行 +
/// 折叠尾；向上滚动（↑）浏览时显示全量。
/// 阈值与 C 版 CLI_REPLY_FOLD_MAX=6 保持一致（节省屏幕空间，适配端侧小屏）。
const FOLD_MAX_LINES: usize = 6;
const FOLD_KEEP_LINES: usize = 3;

/// 构建折叠视图（纯函数，可测）：把 [start, end) 行区间折叠为前
/// `keep` 行 + 折叠尾。以下情况返回 None（不折叠）：
///   - 正在浏览（scroll_offset > 0）：滚动时显示全量，可看完整回复
///   - 区间为空或行数未超阈值（短回复无折叠开销）
fn build_fold_view<'a>(
    lines: &[Line<'a>],
    start: usize,
    end: usize,
    scroll_offset: u16,
    keep: usize,
) -> Option<Vec<Line<'a>>> {
    if scroll_offset > 0 || end <= start || end - start <= FOLD_MAX_LINES {
        return None;
    }
    let more = end - start - keep;
    let mut v = Vec::with_capacity(lines.len());
    v.extend(lines.iter().take(start).cloned());
    v.extend(lines.iter().skip(start).take(keep).cloned());
    v.push(Line::from(Span::styled(
        format!("  └ … {more} more lines · ↑ 浏览展开"),
        Style::default().fg(theme::faint()),
    )));
    v.extend(lines.iter().skip(end).cloned());
    Some(v)
}

/// 渲染对话主面板。
///
/// 参考 Claude Code 的简洁：无边框、内容直接铺开（靠留白分层），
/// 行级滚动 + 右侧滚动条。
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let viewport = area.height as usize;

    let mut lines: Vec<Line> = Vec::new();

    // 任务流阶段引导区（GCCP/GRAD/执行中展示进度与提示）
    render_flow_header(&mut lines, app, width);

    // 消息列表（全量拼行；每条用户消息前插入回合分隔线，Claude Code 惯例）
    // 同时记录全部 System 思考链消息的行区间（折叠区；2.3.9：多轮对话
    // 中每一轮 [Dual Think] 思考链都折叠——此前仅折叠最后一条，历史
    // 思考链碎片全文展开占屏，用户反馈"看不懂、没有价值"。思考链长文本
    // 折叠为前几行 + 浏览展开，最终答复完整展示——用户诉求「折叠流式
    // 输出的思考链，完整展示结果」）
    let mut fold_spans: Vec<(usize, usize)> = Vec::new();
    if app.messages.is_empty() && app.flow_phase == FlowPhase::Chat {
        append_welcome(&mut lines, width, viewport, app);
    } else {
        let mut is_first = true;
        let mut prev_is_tool = false;
        for msg in app.messages.iter() {
            if msg.role == MessageRole::User && !is_first {
                push_turn_separator(&mut lines, app);
            }
            is_first = false;
            if msg.role == MessageRole::System {
                fold_spans.push((lines.len(), lines.len()));
            }
            // 连续工具调用/结果紧凑呈现（Claude Code 风格）：工具与结果之间
            // 不留空行，形成一组紧凑的 `[Sub xxx Agent]` 状态行
            let is_tool =
                matches!(msg.role, MessageRole::ToolCall | MessageRole::ToolResult);
            append_message(&mut lines, msg, width, prev_is_tool && is_tool);
            prev_is_tool = is_tool;
            if msg.role == MessageRole::System {
                if let Some(span) = fold_spans.last_mut() {
                    span.1 = lines.len();
                }
            }
        }
    }

    // 流式工具状态行（SSE tool_call/tool_result 事件，Claude Code 风格：
    // 工具调用时显示 [Sub <tool> Agent] 状态，不污染正文气泡）
    for evt_line in app.stream_tool_events.iter() {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(evt_line.as_str(), Style::default().fg(theme::TOOL_FG)),
        ]));
    }

    // 流式思考链状态行（SSE __airy_evt:reasoning 事件 → stream_reasoning）：
    // thinking 模型先思考后回答。思考内容为模型内部推理碎片，逐块上屏
    // 无展示价值（用户反馈"看不懂、没有价值"）——流式期间仅显示一行
    // 状态（字数 + 耗时进度），完整思考链落定后折叠为摘要行，浏览（↑）
    // 时展开全量。标签按模型轨区分（2.3.14）：t2/t1-f/t1-p → [Dual
    // Slow/Fast/Prof Think]。正文首片到达（streaming_text 非空）即隐藏，
    // 与 C 版 CLI 的思考进度行竞态门控对齐（2026-08-19）。
    if app.loading
        && !app.stream_reasoning.is_empty()
        && app.streaming_text.is_empty()
    {
        let secs = app
            .stream_reasoning_start
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::styled(
                dual_think_label(&app.stream_reasoning_model).to_string(),
                Style::default().fg(theme::WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  思考中… {} 字 · {:.1}s（↑ 浏览查看思考链）",
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
        let streaming_msg = crate::app::ChatMessage {
            role: crate::app::MessageRole::Agent,
            content: revealed,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        };
        append_message(&mut lines, &streaming_msg, width, false);
    }

    if app.loading {
        // 思考动效：Braille spinner 旋转（0.1s 一帧，与 ui.rs 同一时钟；
        // Claude 风格轻量旋转——单字符宽度，不跳动文本，克制优雅）
        let frame = (app.session_start.elapsed().as_millis() / 100) as usize % THINKING.len();
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {}", THINKING[frame]),
                Style::default().fg(theme::faint()),
            ),
        ]));
    }

    // 思考链折叠（live 视口）：全部 System（[Dual Think]）消息渲染行数
    // 超阈值 → 折叠视图只保留头部行（角色名 + 时间戳）+ 折叠尾，思考链
    // 碎片正文不上屏（用户反馈"看不懂、没有价值"）；浏览（scroll_offset
    // > 0）时回退全量，滚动可看完整思考链。最终答复（Agent）不折叠，
    // 完整展示（用户诉求「折叠思考链，完整展示结果」）。
    // 从后往前逐个折叠（行号由后向前替换保持稳定），任一区间成功折叠
    // 即用折叠视图渲染。
    let mut folded_lines: Vec<Line> = lines.clone();
    let mut any_fold = false;
    for &(s, e) in fold_spans.iter().rev() {
        if let Some(sub) = build_fold_view(&folded_lines, s, e, app.scroll_offset, FOLD_KEEP_LINES) {
            folded_lines = sub;
            any_fold = true;
        }
    }
    let src: &[Line] = if any_fold { &folded_lines } else { &lines };

    // 行级滚动：scroll_offset 语义为「距底部（最新）向上滚的行数」。
    // Paragraph::scroll 的 offset 语义是「距顶部滚过的行数」，需反转：
    //   scroll_offset=0 → offset=max_offset → 视口落在底部（跟随最新消息）
    //   scroll_offset=max_offset → offset=0 → 视口落在顶部（最早消息）
    let total = src.len();
    let max_offset = total.saturating_sub(viewport);
    let from_top = max_offset
        .saturating_sub((app.scroll_offset as usize).min(max_offset));

    // 视口裁剪后渲染（行已手动裁剪到宽度内，无需 wrap）
    let visible: Vec<Line> = src.iter().skip(from_top).take(viewport).cloned().collect();
    f.render_widget(Paragraph::new(Text::from(visible)), area);

    // 滚动条：内容超出视口且有对话时显示；窄屏（<44 列）隐藏避免挤占
    if max_offset > 0 && !app.messages.is_empty() && area.width >= 44 {
        let sb_area = Rect {
            x: area.right().saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut state = ScrollbarState::new(total)
            .position(from_top)
            .viewport_content_length(viewport);
        let sb = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("█")
            .thumb_style(Style::default().fg(theme::PRIMARY))
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(theme::faint()));
        f.render_stateful_widget(sb, sb_area, &mut state);
    }
}

/// 任务流阶段引导：GCCP 五问进度 / GRAD 确认（含 DAG 依赖图）/ 执行中（含进度与中止提示）。
fn render_flow_header(lines: &mut Vec<Line>, app: &App, width: usize) {
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
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled("任务事实确认", Style::default().fg(theme::PRIMARY)),
                Span::styled(" ", Style::default()),
                Span::styled(dots, Style::default().fg(theme::PRIMARY)),
                Span::styled(format!("  {answered}/5"), Style::default().fg(theme::faint())),
            ]));
            lines.push(Line::raw(""));
        }
        FlowPhase::GccpClarify => {
            // 服务端 GCCP 目标澄清（P-A）：问题集已由 think.process 回传，
            // 逐条展示（id + 问题 + 提示），等待用户作答后携带 gccp_answers 重发。
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "目标澄清",
                    Style::default().fg(theme::PRIMARY),
                ),
                Span::styled(
                    "  请回答以下问题（逐行作答，或输入「跳过」放弃本轮问答）",
                    Style::default().fg(theme::dim()),
                ),
            ]));
            lines.push(Line::raw(""));
            if let Some(p) = &app.gccp_pending {
                for (i, q) in p.questions.iter().enumerate() {
                    let done = p.answers.contains_key(&q.id);
                    let marker = if done { "●" } else { "○" };
                    let mut spans = vec![Span::styled(
                        format!("  {marker} "),
                        Style::default().fg(theme::PRIMARY),
                    )];
                    spans.push(Span::styled(
                        format!("{}. {}", i + 1, q.question),
                        Style::default().fg(if done { theme::faint() } else { theme::ACCENT }),
                    ));
                    if !q.hint.is_empty() {
                        spans.push(Span::styled(
                            format!("  （{}）", q.hint),
                            Style::default().fg(theme::faint()),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                lines.push(Line::raw(""));
            }
        }
        FlowPhase::GradConfirm => {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    "任务流程图确认",
                    Style::default().fg(theme::MAGENTA).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  输入「确认」开始执行，或输入修改意见", Style::default().fg(theme::dim())),
            ]));
            lines.push(Line::raw(""));
            // 结构化 DAG 依赖图（LLM 生成 [DAG] 块 → 解析成功时渲染）
            if let Some(dag) = &app.gccp.dag {
                let dag_width = width.saturating_sub(6).max(40);
                for line in crate::gccp::render_dag_lines(dag, dag_width) {
                    lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(line, Style::default().fg(theme::ACCENT)),
                    ]));
                }
                lines.push(Line::raw(""));
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
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("任务集执行中{node_hint}"),
                    // 苹果轻量字重风格：无粗体，暖黄
                    Style::default().fg(theme::WARNING),
                ),
                Span::styled(control_hint, Style::default().fg(theme::dim())),
            ]));

            // DAG 节点状态持续渲染（P2-C 过程可视化；2026-08-21 增甘特式进度条）：
            // 总体进度条（▰ 完成 / ▱ 未完成）+ 逐节点状态（◐ 运行中 · ● 完成
            // · ✕ 失败 · ○ 未达）；分支符号（├/└/│）标注节点层级与先后，
            // 与 C 版 airy_cli 一致。
            if let Some(dag) = &app.gccp.dag {
                if !dag.is_empty() {
                    lines.push(Line::raw(""));
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
                    lines.push(Line::from(vec![
                        Span::styled("    ", Style::default()),
                        Span::styled("进度", Style::default().fg(theme::PRIMARY)),
                        Span::styled(" ", Style::default()),
                        Span::styled(
                            bar.clone(),
                            Style::default().fg(if done == n { theme::SUCCESS } else { theme::WARNING }),
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
                            crate::gccp::NodeState::Running => (spin, theme::WARNING),
                            crate::gccp::NodeState::Done => ("●", theme::SUCCESS),
                            crate::gccp::NodeState::Failed => ("✕", theme::DANGER),
                        };
                        // 分支符号：首节点 ├，末节点 └，中间 │（层级先导）
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
                            spans.push(Span::styled(
                                node_state_bar(state),
                                Style::default().fg(color),
                            ));
                        }
                        lines.push(Line::from(spans));
                    }
                }
            }
            lines.push(Line::raw(""));
        }
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

/// 回合分隔线：`── Worked for Ns · N tok · $x ──`（Claude Code 惯例，C 版 airy_cli 同款）。
///
/// 展示上一回合耗时 + 会话累计用量；无已结算回合（首回合）时降级为细分隔线。
fn push_turn_separator(lines: &mut Vec<Line>, app: &App) {
    let (elapsed, metrics) = match app.last_turn_elapsed {
        Some(started) => {
            let secs = started.elapsed().as_secs_f64();
            (
                format!(" Worked for {:.1}s", secs),
                format!(" · {} tok · ${:.4}", app.tokens, app.cost),
            )
        }
        None => (String::new(), String::new()),
    };
    let sep = "─".repeat(6);
    let text = format!("{}{}{}{}", sep, elapsed, metrics, sep);
    lines.push(Line::from(Span::styled(
        text,
        Style::default().fg(theme::faint()),
    )));
    lines.push(Line::raw(""));
}

/// 追加一条消息：角色名 + 时间戳头部 + markdown 渲染内容（P2-A/P2-B 修复）。
///
/// 角色命名规范（与 C 版 airy_cli / 文档 03-cli-reference 对齐）：
///   [For Thee]       青   用户（操作者）输入
///   [Super Agent]    绿   agentrt 本体：最终答复与决策
///   [Dual Think]     黄   系统级思考：GCCP / 双思考 / 蓝图路由轨迹
///   [Sub xxx Agent]  品红 子代理与执行体（xxx = 代理类型，取自工具名）
/// 时间戳展示在头部行；内容固定缩进 4 列；用户消息以气泡背景色块区分，
/// 其余角色左对齐流式。
///
/// 2.2.1.5 任务 2 强化：
///   - 角色着色：用户 [For Thee] 晶蓝（PRIMARY，与助手绿区分）、助手绿、思考链 dim、
///     工具调用品红 / 工具结果青（Claude Code 风格双色）；
///   - 思考链（System）整体以 dim 色呈现（live 视口折叠为 dim 色块，浏览展开）；
///   - 工具调用/结果单行紧凑预览（截断 + …），连续工具消息间不留空行（compact）。
fn append_message(
    lines: &mut Vec<Line>,
    msg: &crate::app::ChatMessage,
    width: usize,
    compact: bool,
) {
    let (name, color) = match msg.role {
        MessageRole::User => ("[For Thee]".to_string(), theme::PRIMARY),
        MessageRole::Agent => ("[Super Agent]".to_string(), theme::SUCCESS),
        MessageRole::System => ("[Dual Think]".to_string(), theme::dim()),
        MessageRole::ToolCall | MessageRole::ToolResult => {
            // [Sub <tag> Agent]：tag 取工具名（ToolCall 首 token）；ToolResult
            // 是工具结果（JSON 等），无工具名可识别时回退 "exec"
            let first = msg.content.split_whitespace().next().unwrap_or("");
            let is_jsonish =
                first.starts_with('{') || first.starts_with('[') || first.starts_with('"');
            let tag = if first.is_empty() || is_jsonish { "exec" } else { first };
            let tag: String = tag.chars().take(12).collect();
            // 工具调用品红（调用侧）· 工具结果青（回传侧），Claude Code 风格
            let c = if msg.role == MessageRole::ToolCall {
                theme::MAGENTA
            } else {
                theme::CYAN
            };
            (format!("[Sub {} Agent]", tag), c)
        }
    };

    // 工具调用/结果状态图标（头部行右侧，语义色）：
    //   工具调用  ✓ 成功 / ✗ 失败（content 含「（失败）」标记判定，2026-08-17
    //   适配过程化格式：失败行可能附短错误，不再依赖行尾判定）
    //   工具结果  ▸ 结果回传
    let (icon, icon_color) = match msg.role {
        MessageRole::ToolCall => {
            if msg.content.contains("（失败）") {
                (" ✗", theme::DANGER)
            } else {
                (" ✓", theme::SUCCESS)
            }
        }
        MessageRole::ToolResult => (" ▸", theme::faint()),
        _ => ("", theme::faint()),
    };

    // 头部行：角色名（加粗语义色）+ 状态图标 + 时间戳（极弱色）
    lines.push(Line::from(vec![
        Span::styled(
            name,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            icon.to_string(),
            Style::default().fg(icon_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", msg.timestamp),
            Style::default().fg(theme::faint()),
        ),
    ]));

    // 内容统一固定缩进 4 列（Claude 式克制排版；此前随角色名长度参差）
    const INDENT: usize = 4;

    // 工具调用/结果与思考链为次要信息：轻量 dim 色
    let base = match msg.role {
        MessageRole::ToolCall | MessageRole::ToolResult | MessageRole::System => {
            Style::default().fg(theme::dim())
        }
        _ => Style::default().fg(theme::text()),
    };

    if msg.content.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{:width$}（空）", "", width = INDENT),
            Style::default().fg(theme::faint()),
        )));
    } else if matches!(msg.role, MessageRole::ToolCall | MessageRole::ToolResult) {
        // 工具调用/结果单行紧凑预览（Claude Code 风格）：首行截断 + …
        let max = width.saturating_sub(INDENT + 3).max(8);
        let first_line = msg.content.lines().next().unwrap_or("").trim();
        let mut preview: String = first_line.chars().take(max).collect();
        if first_line.chars().count() > max || msg.content.lines().count() > 1 {
            preview.push('…');
        }
        lines.push(Line::from(Span::styled(
            format!("{:width$}{}", "", preview, width = INDENT),
            base,
        )));
    } else {
        let mut rendered = crate::markdown::render(&msg.content, INDENT, width, base);
        // 用户消息：气泡背景色块（Claude 式左右分层）；Airymax 回复左对齐流式
        if msg.role == MessageRole::User {
            for line in rendered.iter_mut() {
                let bg = theme::surface_active();
                let spans: Vec<Span> = line
                    .spans
                    .iter()
                    .map(|s| s.clone().bg(bg))
                    .collect();
                *line = Line::from(spans);
            }
        }
        lines.extend(rendered);
    }

    // 消息间留白（替代粗分隔线，视觉更轻）；连续工具消息（compact）不插空行
    if !compact {
        lines.push(Line::raw(""));
    }
}

/// 欢迎页品牌英雄区（无消息时，顶部对齐，随终端大小自适应）。
/// 2.2.1.5 任务 1 强化（2026-08-23）：完整品牌英雄区——
///   · 品牌：◈ AirymaxRT + 版本 + tagline「极境智能体运行平台」
///   · 运行时状态：连接灯 + 在线 daemon 计数 + 在线 Agent +
///     核心链路 llm · think · agent · tool
///   · 会话信息：会话数 / 模型 / token / 成本 / 记忆条数
///   · 硬件快照：架构 / 内存总量·可用 / 加速器（探测失败显示占位）
///   · 快捷键提示行
/// 布局轻盈（留白呼吸感）、theme 函数取色、顶部对齐不居中。
fn append_welcome<'a>(lines: &mut Vec<Line<'a>>, width: usize, height: usize, app: &'a App) {
    let ver = env!("CARGO_PKG_VERSION");
    let proj = if app.project_context.is_empty() {
        "未加载项目上下文（F2 配置 / /project 加载）".to_string()
    } else {
        app.project_context
            .lines()
            .next()
            .unwrap_or("已加载项目上下文")
            .to_string()
    };

    // 高度过小（<8 行）：仅输出精简品牌行，不画边框，避免挤占
    if height < 8 {
        lines.push(Line::from(vec![
            Span::styled(
                "◈ AirymaxRT",
                Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  v{ver}"), Style::default().fg(theme::faint())),
            Span::styled(
                "  ·  极境智能体运行平台 · 输入消息开始对话",
                Style::default().fg(theme::dim()),
            ),
        ]));
        return;
    }

    let border_w = width.saturating_sub(2).min(72);
    let content_max = border_w.saturating_sub(2).max(8);
    let mut hero: Vec<Line> = Vec::new();

    // 品牌行：名称 + 版本 + tagline
    hero.push(Line::from(vec![
        Span::styled(
            "◈ AirymaxRT",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  v{ver}"), Style::default().fg(theme::faint())),
        Span::styled("   ·   极境智能体运行平台", Style::default().fg(theme::dim())),
    ]));
    hero.push(Line::raw(""));

    // 运行时状态：连接灯 + 在线 daemon 计数 + 在线 Agent
    let (light, label, color) = if app.connected {
        ("●", "ONLINE", theme::SUCCESS)
    } else if app.loading {
        ("◐", "WAITING", theme::WARNING)
    } else {
        ("●", "OFFLINE", theme::DANGER)
    };
    // 在线 daemon 计数：无持久状态时显示占位（/daemons 可查 16 个 daemon）
    let agents = app.hall_board.as_ref().map(|b| b.agents.len()).unwrap_or(0);
    hero.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(light, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label}"), Style::default().fg(color)),
        Span::styled(
            format!(
                "   ·   daemon 在线 —   ·   在线 Agent {}",
                if agents > 0 {
                    agents.to_string()
                } else {
                    "—".to_string()
                }
            ),
            Style::default().fg(theme::dim()),
        ),
        Span::styled(
            "   ·   （/daemons 查看）",
            Style::default().fg(theme::faint()),
        ),
    ]));

    // 核心链路 chips：llm · think · agent · tool
    let chain = [
        ("llm", theme::ACCENT),
        ("think", theme::WARNING),
        ("agent", theme::SUCCESS),
        ("tool", theme::MAGENTA),
    ];
    let mut chain_spans: Vec<Span> =
        vec![Span::styled("  核心链路  ", Style::default().fg(theme::dim()))];
    for (i, (name, c)) in chain.iter().enumerate() {
        if i > 0 {
            chain_spans.push(Span::styled(" · ", Style::default().fg(theme::faint())));
        }
        chain_spans.push(Span::styled(
            *name,
            Style::default().fg(*c).add_modifier(Modifier::BOLD),
        ));
    }
    hero.push(Line::from(chain_spans));
    hero.push(Line::raw(""));

    // 会话信息：会话数 / 模型 / token / 成本 / 记忆
    let model_text = if app.model.is_empty() {
        "默认模型".to_string()
    } else {
        app.model.clone()
    };
    let model_disp: String = model_text.chars().take(24).collect();
    hero.push(Line::from(vec![
        Span::styled("  会话  ", Style::default().fg(theme::faint())),
        Span::styled(format!("{}", app.tab_count()), Style::default().fg(theme::text())),
        Span::styled("  ·  模型  ", Style::default().fg(theme::faint())),
        Span::styled(
            model_disp,
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::faint())),
        Span::styled(format!("{} tok", app.tokens), Style::default().fg(theme::dim())),
        Span::styled("  ·  ", Style::default().fg(theme::faint())),
        Span::styled(format!("${:.4}", app.cost), Style::default().fg(theme::dim())),
        Span::styled("  ·  记忆  ", Style::default().fg(theme::faint())),
        Span::styled(
            format!("{}", app.memory.len()),
            Style::default().fg(theme::SUCCESS).add_modifier(Modifier::BOLD),
        ),
    ]));

    // 硬件快照：架构 / 内存 / 加速器（数据探测失败显示占位）
    let (mem_total, mem_avail) = crate::panels::config::mem_snapshot();
    let hw = format!(
        "{} · 内存 {} / {} · 加速器 {}",
        crate::panels::config::arch_snapshot(),
        mem_total,
        mem_avail,
        crate::panels::config::accelerator_snapshot(),
    );
    let hw_disp: String = hw.chars().take(content_max.saturating_sub(6)).collect();
    hero.push(Line::from(vec![
        Span::styled("  硬件  ", Style::default().fg(theme::faint())),
        Span::styled(hw_disp, Style::default().fg(theme::dim())),
    ]));

    // 项目上下文行（未加载时给出引导）
    let proj_disp: String = proj.chars().take(content_max.saturating_sub(6)).collect();
    hero.push(Line::from(vec![
        Span::styled("  项目  ", Style::default().fg(theme::faint())),
        Span::styled(proj_disp, Style::default().fg(theme::text())),
    ]));
    hero.push(Line::raw(""));

    // 快捷键提示行
    let hint = "输入消息 Enter 发送 · F1 帮助 · F2 配置 · F3 日志 · F4 记忆 · F5 插件 · F6 看板 · F7 事件 · F8 CLI · Ctrl+C 退出";
    let hint_disp: String = hint.chars().take(content_max.saturating_sub(2)).collect();
    hero.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(hint_disp, Style::default().fg(theme::faint())),
    ]));

    // 顶部对齐：先输出 1 行呼吸空间，不垂直居中
    lines.push(Line::raw(""));

    // 边框（上下线 + 左右竖线），主题色为晶蓝
    let border = "─".repeat(border_w);
    if width >= 44 {
        lines.push(Line::from(vec![
            Span::styled("╭", Style::default().fg(theme::PRIMARY)),
            Span::styled(border.clone(), Style::default().fg(theme::PRIMARY)),
            Span::styled("╮", Style::default().fg(theme::PRIMARY)),
        ]));
        for h in hero.iter() {
            let mut hline = Line::from(vec![
                Span::styled("│", Style::default().fg(theme::PRIMARY)),
                Span::styled(" ", Style::default()),
            ]);
            hline.extend(h.spans.clone());
            lines.push(hline);
        }
        lines.push(Line::from(vec![
            Span::styled("╰", Style::default().fg(theme::PRIMARY)),
            Span::styled(border, Style::default().fg(theme::PRIMARY)),
            Span::styled("╯", Style::default().fg(theme::PRIMARY)),
        ]));
    } else {
        lines.extend(hero);
    }
}

#[cfg(test)]
mod tests {
    // wrap_line 实现已迁至 markdown 模块（P2-A 渲染器共用）
    use super::append_message;
    use super::build_fold_view;
    use super::gantt_bar;
    use super::node_state_bar;
    use super::FOLD_KEEP_LINES;
    use crate::markdown::wrap_line;
    use ratatui::text::Line;

    /// 长回复折叠：超阈值折叠为 KEEP 行 + 折叠尾；浏览（↑）时展开全量。
    #[test]
    fn fold_view_collapses_long_reply() {
        let mut lines: Vec<Line> = Vec::new();
        for i in 0..20 {
            lines.push(Line::raw(format!("line {i}")));
        }
        let folded = build_fold_view(&lines, 2, 20, 0, FOLD_KEEP_LINES).expect("long reply folds");
        // 保留：前 2 行 + KEEP 行 + 折叠尾 1 行 = 2 + 3 + 1 = 6 行
        assert_eq!(folded.len(), 2 + FOLD_KEEP_LINES + 1, "折叠后行数");
        assert!(folded.iter().any(|l| l.to_string().contains("浏览展开")), "折叠尾存在");
        let tail = folded.last().unwrap().to_string();
        assert!(tail.contains("more lines"), "折叠尾含行数提示: {tail}");
        // 思考链折叠（keep=1）：只保留头部行 + 折叠尾
        let think_folded = build_fold_view(&lines, 2, 20, 0, 1).expect("think fold");
        assert_eq!(think_folded.len(), 2 + 1 + 1, "思考链折叠后行数（头部+折叠尾）");
        assert!(think_folded.iter().any(|l| l.to_string().contains("more lines")), "思考链折叠尾");
    }

    #[test]
    fn fold_view_keeps_short_reply_and_browse_mode() {
        let mut lines: Vec<Line> = Vec::new();
        for i in 0..6 {
            lines.push(Line::raw(format!("line {i}")));
        }
        // 未超阈值：不折叠
        assert!(build_fold_view(&lines, 0, 6, 0, 3).is_none(), "短回复不折叠");
        // 超阈值但正在浏览：展开全量（滚动可看完整回复）
        let mut long: Vec<Line> = Vec::new();
        for i in 0..20 {
            long.push(Line::raw(format!("line {i}")));
        }
        assert!(build_fold_view(&long, 0, 20, 5, 3).is_none(), "浏览时不折叠");
        // 空区间 / 非法区间：不折叠
        assert!(build_fold_view(&long, 5, 5, 0, 3).is_none(), "空区间不折叠");
        assert!(build_fold_view(&long, 8, 5, 0, 3).is_none(), "非法区间不折叠");
    }

    #[test]
    fn wrap_line_short_text_single_line() {
        assert_eq!(wrap_line("你好", 10), vec!["你好"]);
        assert_eq!(wrap_line("abc", 10), vec!["abc"]);
    }

    #[test]
    fn wrap_line_splits_by_display_width() {
        // 中文按 2 列计：宽度 5 时 "你好世" = 6 列超宽 → 拆行
        assert_eq!(wrap_line("你好世界", 5), vec!["你好", "世界"]);
        // 半角按 1 列计
        assert_eq!(wrap_line("abcdef", 3), vec!["abc", "def"]);
    }

    #[test]
    fn wrap_line_empty_and_narrow() {
        assert!(wrap_line("", 10).is_empty());
        // 极窄宽度兜底：整行返回，不产生空片段
        assert_eq!(wrap_line("abc", 1), vec!["abc"]);
    }

    #[test]
    fn wrap_line_mixed_widths() {
        // "a你好b" = 1+2+2+1 = 6 列，宽度 4 → "a你"（3 列）+ "好b"（3 列）
        assert_eq!(wrap_line("a你好b", 4), vec!["a你", "好b"]);
    }

    /// 角色命名规范：头部行必须为 [For Thee]/[Super Agent]/[Dual Think]/[Sub xxx Agent]。
    #[test]
    fn role_headers_follow_naming_scheme() {
        use crate::app::{ChatMessage, MessageRole};

        let cases = [
            (MessageRole::User, "[For Thee]", "hi"),
            (MessageRole::Agent, "[Super Agent]", "hello!"),
            (MessageRole::System, "[Dual Think]", "GCCP 提示"),
            (MessageRole::ToolCall, "[Sub web_fetch Agent]", "web_fetch {\"url\":\"x\"}"),
            (MessageRole::ToolResult, "[Sub exec Agent]", "{\"ok\":1}"),
        ];
        for (role, expect_head, content) in cases.iter() {
            let mut lines = Vec::new();
            let msg = ChatMessage {
                role: role.clone(),
                content: content.to_string(),
                timestamp: "00:00:00".to_string(),
            };
            append_message(&mut lines, &msg, 80, false);
            assert!(
                !lines.is_empty(),
                "role {:?}: 应至少渲染头部行",
                role
            );
            let head = lines[0].to_string();
            assert!(
                head.starts_with(expect_head),
                "role {:?}: 头部应为 '{}'，实际 '{}'",
                role,
                expect_head,
                head
            );
        }
    }

    /// 子代理 tag 兜底：空内容 / 无工具名时回退 "exec"。
    #[test]
    fn sub_agent_tag_fallback() {
        use crate::app::{ChatMessage, MessageRole};
        let mut lines = Vec::new();
        let msg = ChatMessage {
            role: MessageRole::ToolResult,
            content: String::new(),
            timestamp: "00:00:00".to_string(),
        };
        append_message(&mut lines, &msg, 80, false);
        assert!(lines[0].to_string().starts_with("[Sub exec Agent]"));
    }

    /// 超长工具名 tag 截断到 12 字符，保持 "[Sub xxx Agent]" 结构完整。
    #[test]
    fn sub_agent_tag_truncated() {
        use crate::app::{ChatMessage, MessageRole};
        let mut lines = Vec::new();
        let msg = ChatMessage {
            role: MessageRole::ToolCall,
            content: "very_long_tool_name_that_exceeds_budget args".to_string(),
            timestamp: "00:00:00".to_string(),
        };
        append_message(&mut lines, &msg, 80, false);
        assert!(lines[0].to_string().starts_with("[Sub very_long_to Agent]"));
    }

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
