// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GRAD 结构化任务依赖图（DAG）子域（0.1.18 拆分自 gccp.rs）。
//
// 职责边界：DAG 数据结构 + LLM 输出解析 + ASCII 依赖图渲染。本文件只做
// 「解析 → 分层 → 成行」三件事，不持有 GCCP 流程状态（状态在 state.rs）。
//
// 健壮性纪律：LLM 输出形态不可控，解析失败/无有效节点一律返回 None 由调用方
// 降级为纯文本流程图；引用未定义节点的边丢弃而非整体失败。
//
// 宽度纪律（0.1.18 A 轨 W2，§3.3）：显示宽度测量与截断一律委托 L2 唯一裁决点
// `crate::engine::grid`，本文件不得自行测量后截断或填充。

use crate::engine::grid;

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
///
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
            log::warn!(
                "parse_dag: DAG 块 JSON 解析失败: {}（降级为纯文本流程图）",
                e
            );
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
    let known: std::collections::HashSet<&str> = dag.nodes.iter().map(|n| n.id.as_str()).collect();
    let edges_before = dag.edges.len();
    dag.edges
        .retain(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()));
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
        let ids: std::collections::HashSet<&str> = layer.iter().map(|n| n.id.as_str()).collect();
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
            // 框线宽度按显示宽度计（CJK 标签按 2 列），此前用字符数，全角标签下框线偏窄
            grid::width(
                &l.iter()
                    .map(|n| node_cell(n, 0))
                    .collect::<Vec<_>>()
                    .join("   "),
            )
        })
        .max()
        .unwrap_or(0)
        .min(max_width.saturating_sub(4));
    let top = format!(
        "┌─ 任务依赖图 {}",
        "─".repeat(inner_w.saturating_sub(5).max(1))
    );
    out.push(grid::clip(&top, max_width));

    for (i, layer) in layers.iter().enumerate() {
        // 节点行：同层并排（列对齐：每个节点占 max_cell 列）
        let max_cell = layer
            .iter()
            .map(|n| grid::width(&node_cell(n, 0)))
            .max()
            .unwrap_or(4);
        let row: String = layer
            .iter()
            .map(|n| grid::pad(&node_cell(n, max_cell), max_cell))
            .collect::<Vec<_>>()
            .join("   ");
        out.push(grid::clip(&format!("│  {}", row), max_width));

        // 层间连接（若存在跨层边）
        if i < layers.len() - 1 && has_down[i] {
            out.push(grid::clip("│   ↓", max_width));
        }
    }
    let bottom = format!("└{}", "─".repeat(inner_w.saturating_sub(2).max(1)));
    out.push(grid::clip(&bottom, max_width));

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
    let label = grid::clip(&n.label, 24);
    if label.is_empty() {
        n.id.clone()
    } else {
        format!("{} {}", n.id, label)
    }
}
