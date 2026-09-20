// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// GCCP/GRAD 解析与流程回归（0.1.18 拆分自 gccp.rs）。
//
// 覆盖三类回归面：LLM 输出容忍度（问题/DAG 变体形态）、用户输入识别
// （前缀剥离/确认/完成指令）、DAG 分层渲染（拓扑深度 + 并行同层）。

use super::dag::{DagEdge, DagNode, TaskDag};
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
    assert_eq!(strip_task_done("任务完成\n[TASK:DONE]"), "任务完成");
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
    let s2 = GccpState {
        q1: "目标".into(),
        a1: "部署系统".into(),
        ..Default::default()
    };
    assert!(build_qn_prompt(&s2, 2).contains("Q2:"));
    assert!(build_qn_prompt(&s2, 2).contains("A1: 部署系统"));
    assert!(build_qn_prompt(&s2, 5).contains("Q5:"));
    assert!(build_grad_prompt(&s).contains("[GRAD]"));
    assert!(build_execute_prompt(&s).contains("已确认事实"));
}

#[test]
fn facts_concatenates_q_and_a() {
    let s = GccpState {
        q1: "目标".into(),
        a1: "部署".into(),
        q2: "约束".into(),
        a2: "兼容".into(),
        ..Default::default()
    };
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
    assert_eq!(
        dag.edges[0],
        DagEdge {
            from: "n1".into(),
            to: "n2".into()
        }
    );
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
    assert_eq!(
        dag.edges[0],
        DagEdge {
            from: "x".into(),
            to: "y".into()
        }
    );
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
            DagNode {
                id: "n1".into(),
                label: "准备".into(),
            },
            DagNode {
                id: "n2".into(),
                label: "收集".into(),
            },
            DagNode {
                id: "n3".into(),
                label: "交付".into(),
            },
        ],
        edges: vec![
            DagEdge {
                from: "n1".into(),
                to: "n2".into(),
            },
            DagEdge {
                from: "n2".into(),
                to: "n3".into(),
            },
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
            DagNode {
                id: "n1".into(),
                label: "根".into(),
            },
            DagNode {
                id: "n2".into(),
                label: "左支".into(),
            },
            DagNode {
                id: "n3".into(),
                label: "右支".into(),
            },
        ],
        edges: vec![
            DagEdge {
                from: "n1".into(),
                to: "n2".into(),
            },
            DagEdge {
                from: "n1".into(),
                to: "n3".into(),
            },
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
