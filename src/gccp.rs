// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP（任务事实确认）与 GRAD（任务流程图确认）。
//
// 大任务集启动时进入 GCCP：任务事实确认共 5 个问题，**逐一询问**（5 轮，每轮 1 问），
// 每轮之间让 LLM 基于已答事实思考，使下一个问题更精准：
//   第 1 轮：提出第 1 问，用户作答 → LLM 思考
//   第 2 轮：提出第 2 问，用户作答 → LLM 思考
//   ……
//   第 5 轮：提出第 5 问，用户作答
// 五问齐备后进入 GRAD（任务流程图确认）：LLM 生成执行流程图，用户确认后开始执行。

/// 任务流阶段状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPhase {
    /// 普通对话（非任务集）
    Chat,
    /// 任务事实确认（GCCP）第 N 轮：等待用户回答第 N 问（N = 1..=5）
    GccpRound(u8),
    /// 服务端 GCCP 目标澄清（P-A 两段式交互）：think.process 返回
    /// gccp_need_interaction，等待用户回答问题集（见 GccpPending）
    GccpClarify,
    /// 任务流程图确认（GRAD）：等待用户确认流程图
    GradConfirm,
    /// 任务集执行中
    Executing,
}

/// 任务集执行控制状态（人工中止/暂停）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskControl {
    /// 正常运行
    Running,
    /// 用户暂停（Ctrl+Z）：轮询挂起，可恢复
    Paused,
    /// 用户中止（Ctrl+X）：后台请求已取消，状态徽章展示中止态；
    /// 下次发起新交互（start_pending 等）时自动复位为 Running
    Aborted,
}

impl TaskControl {
    pub fn label(self) -> &'static str {
        match self {
            TaskControl::Running => "运行中",
            TaskControl::Paused => "已暂停",
            TaskControl::Aborted => "已中止",
        }
    }
}

impl FlowPhase {
    /// 状态栏展示名（中文术语）
    #[allow(dead_code)] // 单测使用；UI 按阶段匹配颜色自行展示
    pub fn label(self) -> &'static str {
        match self {
            FlowPhase::Chat => "对话",
            FlowPhase::GccpRound(_) => "任务事实确认",
            FlowPhase::GccpClarify => "目标澄清",
            FlowPhase::GradConfirm => "任务流程图确认",
            FlowPhase::Executing => "任务集",
        }
    }

    /// 输入框提示（引导用户作答）
    pub fn input_hint(self) -> String {
        match self {
            FlowPhase::GccpRound(n) => format!("回答第 {} 问：", n),
            FlowPhase::GccpClarify => {
                "回答 GCCP 澄清问题（空行逐条，或「跳过」放弃本轮问答）".to_string()
            }
            FlowPhase::GradConfirm => "输入「确认」通过流程图，或输入修改意见：".to_string(),
            // 普通对话 / 执行中：无引导语，前缀 ❯ 已足够
            FlowPhase::Chat | FlowPhase::Executing => String::new(),
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
    /// GRAD 结构化 DAG（由 [DAG] 块解析，用于可视化；解析失败时为 None）
    pub dag: Option<TaskDag>,
    /// DAG 节点执行状态（顺序与 dag.nodes 一致；无 dag 时为空数组）
    pub node_states: Vec<NodeState>,
}

/// DAG 节点执行状态（P2-C 过程可视化：Executing 阶段持续渲染）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeState {
    /// 未开始
    #[default]
    Pending,
    /// 执行中
    Running,
    /// 已完成
    Done,
    /// 执行失败/跳过（预留：逐节点失败反馈接线后构造）
    #[allow(dead_code)]
    Failed,
}

/// GRAD 结构化 DAG 节点（任务步骤）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DagNode {
    /// 节点 ID（如 "n1"）
    pub id: String,
    /// 步骤名称（一句话动作描述）
    pub label: String,
}

/// GRAD 结构化 DAG 边（from → to 依赖：to 依赖 from 完成）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// 任务 DAG（依赖图，用于对话中的可视化展示）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskDag {
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
}

impl TaskDag {
    /// 是否为空（无有效节点）
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 节点总数（执行进度展示用）
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// 从 LLM 的 GRAD 输出中提取并解析结构化 DAG。
///
/// 支持三种 [DAG] 块形态（LLM 输出变体容忍）：
///   1. 围栏 JSON：```json\n{...}\n```
///   2. 显式标记：[DAG]\n{...}\n[/DAG]
///   3. 裸 JSON（响应中出现 nodes/edges 键的 JSON 对象）
/// 解析失败返回 None（调用方降级为纯文本流程图，不影响流程）。
pub fn parse_dag(resp: &str) -> Option<TaskDag> {
    let body = match extract_dag_block(resp) {
        Some(b) => b,
        None => {
            log::debug!("parse_dag: 未找到 [DAG] 块（显式标记/围栏/裸 JSON 均未命中）");
            return None;
        }
    };
    log::trace!(
        "parse_dag: 提取 DAG 块成功（{} 字符）: {}",
        body.len(),
        body.chars().take(120).collect::<String>()
    );
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("parse_dag: DAG 块 JSON 解析失败: {}（降级为纯文本流程图）", e);
            return None;
        }
    };
    let obj = match v.as_object() {
        Some(o) => o,
        None => {
            log::warn!("parse_dag: DAG 块不是 JSON 对象（降级为纯文本流程图）");
            return None;
        }
    };

    let mut dag = TaskDag::default();

    // 节点：nodes: [{id,label}]（label 缺失时回退 name/title/description）
    if let Some(nodes) = obj.get("nodes").and_then(|v| v.as_array()) {
        for n in nodes.iter() {
            let o = n.as_object()?;
            let id = o
                .get("id")
                .or_else(|| o.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())?;
            let label = o
                .get("label")
                .or_else(|| o.get("title"))
                .or_else(|| o.get("description"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            dag.nodes.push(DagNode { id, label });
        }
    }

    // 边：edges: [{from,to}]（兼容 source/target 与 from/to）
    if let Some(edges) = obj.get("edges").and_then(|v| v.as_array()) {
        for e in edges.iter() {
            let o = e.as_object()?;
            let from = o
                .get("from")
                .or_else(|| o.get("source"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())?;
            let to = o
                .get("to")
                .or_else(|| o.get("target"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())?;
            dag.edges.push(DagEdge { from, to });
        }
    }

    // 依赖边引用的节点必须在节点表中（容忍 LLM 输出引用了未定义节点：丢弃该边）
    let known: std::collections::HashSet<&str> =
        dag.nodes.iter().map(|n| n.id.as_str()).collect();
    let edges_before = dag.edges.len();
    dag.edges.retain(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()));
    let dropped_edges = edges_before - dag.edges.len();
    if dropped_edges > 0 {
        log::warn!(
            "parse_dag: 丢弃 {} 条引用未定义节点的边（{} 条边中）",
            dropped_edges,
            edges_before
        );
    }

    if dag.is_empty() {
        log::warn!(
            "parse_dag: DAG 解析完成但无有效节点（nodes={} edges={}），降级为纯文本流程图",
            dag.nodes.len(),
            dag.edges.len()
        );
        None
    } else {
        log::info!(
            "parse_dag: DAG 解析成功（nodes={} edges={}，丢弃边={}）",
            dag.nodes.len(),
            dag.edges.len(),
            dropped_edges
        );
        for n in &dag.nodes {
            log::trace!("  node: {} = {}", n.id, n.label);
        }
        for e in &dag.edges {
            log::trace!("  edge: {} -> {}", e.from, e.to);
        }
        Some(dag)
    }
}

/// 提取响应中的 [DAG] 块内容（含围栏 JSON / 显式标记 / 裸 JSON）。
fn extract_dag_block(resp: &str) -> Option<String> {
    // 1) 显式标记 [DAG] ... [/DAG]
    if let Some(start) = resp.find("[DAG]") {
        let rest = &resp[start + 5..];
        if let Some(end) = rest.find("[/DAG]") {
            return Some(rest[..end].trim().to_string());
        }
        // 标记后直到响应末尾
        return Some(rest.trim().to_string());
    }
    // 2) JSON 围栏 ```json ... ```
    let lines: Vec<&str> = resp.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.starts_with("```") && t.to_ascii_lowercase().contains("json") {
            for j in (i + 1)..lines.len() {
                if lines[j].trim().starts_with("```") {
                    return Some(lines[i + 1..j].join("\n"));
                }
            }
        }
    }
    // 3) 裸 JSON：响应整体是含 nodes/edges 的对象
    let t = resp.trim();
    if t.starts_with('{') && t.contains("\"nodes\"") {
        return Some(t.to_string());
    }
    None
}

/// 渲染任务 DAG 为 ASCII 依赖图（按拓扑深度分层，乔布斯式克制：仅框线 + 节点名）。
///
/// 输出形如：
///   ┌─ 任务依赖图 ────────────────────┐
///   │  n1 准备环境                    │
///   │   ↓                            │
///   │  n2 收集数据   n3 生成报告      │
///   │   └──────┬───────┘             │
///   │          ↓                     │
///   │  n4 交付验收                    │
///   └────────────────────────────────┘
///
/// 行尾自动按节点标签宽度补齐，便于 chat.rs 逐行追加。
pub fn render_dag_lines(dag: &TaskDag, max_width: usize) -> Vec<String> {
    use std::collections::HashMap;

    if dag.is_empty() {
        return Vec::new();
    }

    // 1) 拓扑深度：depth[id] = 最长路径长度（入度为 0 的节点 = 0）
    let mut depth: HashMap<&str, usize> = dag.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    for _ in 0..=dag.nodes.len() {
        let mut changed = false;
        for e in &dag.edges {
            let d = depth.get(e.from.as_str()).copied().unwrap_or(0) + 1;
            let cur = depth.entry(e.to.as_str()).or_insert(0);
            if d > *cur {
                *cur = d;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 2) 按深度分层
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<&DagNode>> = vec![Vec::new(); max_depth + 1];
    for n in &dag.nodes {
        let d = depth.get(n.id.as_str()).copied().unwrap_or(0);
        layers[d].push(n);
    }
    layers.retain(|l| !l.is_empty());

    // 3) 计算每层是否有边跨到更深层（决定层间是否画 ↓）
    let mut has_down: Vec<bool> = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let ids: std::collections::HashSet<&str> =
            layer.iter().map(|n| n.id.as_str()).collect();
        let cross = dag
            .edges
            .iter()
            .any(|e| ids.contains(e.from.as_str()) && depth[e.to.as_str()] > i);
        has_down.push(cross);
    }

    // 4) 组装行
    let mut out: Vec<String> = Vec::new();
    // 顶部框线（宽度 = 最宽层行 + 2 边框）
    let inner_w = layers
        .iter()
        .map(|l| {
            l.iter()
                .map(|n| node_cell(n, 0))
                .collect::<Vec<_>>()
                .join("   ")
                .chars()
                .count()
        })
        .max()
        .unwrap_or(0)
        .min(max_width.saturating_sub(4));
    let top = format!("┌─ 任务依赖图 {}", "─".repeat(inner_w.saturating_sub(5).max(1)));
    out.push(truncate_pad(&top, max_width));

    for (i, layer) in layers.iter().enumerate() {
        // 节点行：同层并排（列对齐：每个节点占 max_cell_w 列）
        let max_cell = layer
            .iter()
            .map(|n| node_cell(n, 0).chars().count())
            .max()
            .unwrap_or(4);
        let row: String = layer
            .iter()
            .map(|n| pad_right(&node_cell(n, max_cell), max_cell))
            .collect::<Vec<_>>()
            .join("   ");
        out.push(truncate_pad(&format!("│  {}", row), max_width));

        // 层间连接（若存在跨层边）
        if i < layers.len() - 1 && has_down[i] {
            out.push(truncate_pad("│   ↓", max_width));
        }
    }
    let bottom = format!("└{}", "─".repeat(inner_w.saturating_sub(2).max(1)));
    out.push(truncate_pad(&bottom, max_width));

    log::trace!(
        "render_dag_lines: 渲染完成（层数={} 行数={} 宽度={} max_width={}）",
        layers.len(),
        out.len(),
        inner_w,
        max_width
    );
    out
}

/// 节点单元格文本：`id 标签`（标签截断到 24 列）
fn node_cell(n: &DagNode, _max_cell: usize) -> String {
    let label = truncate_display(&n.label, 24);
    if label.is_empty() {
        n.id.clone()
    } else {
        format!("{} {}", n.id, label)
    }
}

/// 按显示宽度截断（中文按 2 列计）
fn truncate_display(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if s.width() <= max_cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max_cols {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// 右侧补齐到指定列宽（不足补空格）
fn pad_right(s: &str, cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let w = s.width();
    if w >= cols {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(cols - w))
    }
}

/// 截断到最大列宽（超宽则裁掉，保证布局不溢出）
fn truncate_pad(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if s.width() <= max_width {
        s.to_string()
    } else {
        truncate_display(s, max_width.saturating_sub(1))
    }
}

impl GccpState {
    /// 清空状态
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 初始化 DAG 节点状态（解析出 dag 后调用）：全部 Pending
    pub fn init_node_states(&mut self) {
        if let Some(dag) = &self.dag {
            self.node_states = vec![NodeState::Pending; dag.nodes.len()];
        } else {
            self.node_states.clear();
        }
    }

    /// 任务开始执行：全部节点进入 Running（无逐节点反馈时的诚实中间态）
    pub fn mark_all_running(&mut self) {
        for s in self.node_states.iter_mut() {
            if *s == NodeState::Pending {
                *s = NodeState::Running;
            }
        }
    }

    /// 任务完成：全部节点标记 Done
    pub fn mark_all_done(&mut self) {
        for s in self.node_states.iter_mut() {
            *s = NodeState::Done;
        }
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

    /// 已作答的问题数（a1-a5 非空计数，用于状态栏进度展示）
    pub fn answered(&self) -> usize {
        [&self.a1, &self.a2, &self.a3, &self.a4, &self.a5]
            .iter()
            .filter(|a| !a.trim().is_empty())
            .count()
    }
}

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
        ctx.push_str("请");
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
        assert_eq!(FlowPhase::GccpRound(1).label(), "任务事实确认");
        assert_eq!(FlowPhase::GccpRound(5).label(), "任务事实确认");
        assert_eq!(FlowPhase::GccpClarify.label(), "目标澄清");
        assert_eq!(FlowPhase::GradConfirm.label(), "任务流程图确认");
        assert_eq!(FlowPhase::Executing.label(), "任务集");
    }

    #[test]
    fn prompts_use_correct_rounds() {
        let s = GccpState::default();
        assert!(build_qn_prompt(&s, 1).contains("Q1:"));
        // 第 2 轮须带上第 1 问的回答上下文
        let mut s2 = GccpState::default();
        s2.q1 = "目标".into();
        s2.a1 = "部署系统".into();
        assert!(build_qn_prompt(&s2, 2).contains("Q2:"));
        assert!(build_qn_prompt(&s2, 2).contains("A1: 部署系统"));
        assert!(build_qn_prompt(&s2, 5).contains("Q5:"));
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

    #[test]
    fn parse_dag_explicit_marker() {
        let resp = "[GRAD]\n任务目标：部署\n\n[DAG]\n\
            {\"nodes\":[{\"id\":\"n1\",\"label\":\"准备环境\"},\
            {\"id\":\"n2\",\"label\":\"收集数据\"},\
            {\"id\":\"n3\",\"label\":\"生成报告\"}],\
            \"edges\":[{\"from\":\"n1\",\"to\":\"n2\"},{\"from\":\"n2\",\"to\":\"n3\"}]}\n[/DAG]";
        let dag = parse_dag(resp).expect("dag");
        assert_eq!(dag.node_count(), 3);
        assert_eq!(dag.nodes[0].id, "n1");
        assert_eq!(dag.nodes[0].label, "准备环境");
        assert_eq!(dag.edges.len(), 2);
        assert_eq!(dag.edges[0], DagEdge { from: "n1".into(), to: "n2".into() });
    }

    #[test]
    fn parse_dag_fenced_json() {
        let resp = "流程如下：\n```json\n{\"nodes\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"b\",\"label\":\"B\"}],\"edges\":[{\"from\":\"a\",\"to\":\"b\"}]}\n```";
        let dag = parse_dag(resp).expect("dag");
        assert_eq!(dag.node_count(), 2);
        assert_eq!(dag.nodes[1].id, "b");
    }

    #[test]
    fn parse_dag_source_target_alias() {
        // 兼容 source/target 别名字段
        let resp = "[DAG]\n{\"nodes\":[{\"id\":\"x\",\"label\":\"X\"},{\"id\":\"y\",\"label\":\"Y\"}],\"edges\":[{\"source\":\"x\",\"target\":\"y\"}]}\n[/DAG]";
        let dag = parse_dag(resp).expect("dag");
        assert_eq!(dag.edges[0], DagEdge { from: "x".into(), to: "y".into() });
    }

    #[test]
    fn parse_dag_unknown_edge_node_dropped() {
        // 边引用了未定义的节点 → 丢弃该边
        let resp = "[DAG]\n{\"nodes\":[{\"id\":\"n1\",\"label\":\"一\"}],\"edges\":[{\"from\":\"n1\",\"to\":\"ghost\"}]}\n[/DAG]";
        let dag = parse_dag(resp).expect("dag");
        assert_eq!(dag.edges.len(), 0);
        assert_eq!(dag.node_count(), 1);
    }

    #[test]
    fn parse_dag_invalid_returns_none() {
        assert!(parse_dag("没有 DAG 块").is_none());
        assert!(parse_dag("[DAG]\nnot-json[/DAG]").is_none());
        assert!(parse_dag("[DAG]\n{\"edges\":[]}[/DAG]").is_none()); // 无节点
    }

    #[test]
    fn render_dag_layers_and_arrows() {
        let dag = TaskDag {
            nodes: vec![
                DagNode { id: "n1".into(), label: "准备".into() },
                DagNode { id: "n2".into(), label: "收集".into() },
                DagNode { id: "n3".into(), label: "交付".into() },
            ],
            edges: vec![
                DagEdge { from: "n1".into(), to: "n2".into() },
                DagEdge { from: "n2".into(), to: "n3".into() },
            ],
        };
        let lines = render_dag_lines(&dag, 60);
        assert!(lines.len() >= 5, "应含框线+3 节点行+2 箭头行: {:?}", lines);
        assert!(lines[0].contains("任务依赖图"));
        assert!(lines.iter().any(|l| l.contains("n1 准备")));
        assert!(lines.iter().any(|l| l.contains("n3 交付")));
        // 层间箭头
        assert!(lines.iter().any(|l| l.contains('↓')));
    }

    #[test]
    fn render_dag_parallel_layer_same_row() {
        // n2/n3 无依赖 → 同层并排
        let dag = TaskDag {
            nodes: vec![
                DagNode { id: "n1".into(), label: "根".into() },
                DagNode { id: "n2".into(), label: "左支".into() },
                DagNode { id: "n3".into(), label: "右支".into() },
            ],
            edges: vec![
                DagEdge { from: "n1".into(), to: "n2".into() },
                DagEdge { from: "n1".into(), to: "n3".into() },
            ],
        };
        let lines = render_dag_lines(&dag, 60);
        // n2 与 n3 出现在同一行（同层）
        let row = lines.iter().find(|l| l.contains("n2 左支")).expect("row");
        assert!(row.contains("n3 右支"), "并行节点应同层并排");
    }

    #[test]
    fn task_control_labels() {
        assert_eq!(TaskControl::Running.label(), "运行中");
        assert_eq!(TaskControl::Paused.label(), "已暂停");
        assert_eq!(TaskControl::Aborted.label(), "已中止");
    }
}
