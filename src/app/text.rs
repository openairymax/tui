// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 展示文本装配：会话标题、事件类别译名、帮助页、长文本截断与思考摘要。

/// 会话标题：取用户输入首行，截断到 24 字符（tab 栏展示用）。
pub(super) fn derive_session_title(input: &str) -> String {
    let t = input.trim();
    let first = t.lines().next().unwrap_or(t);
    let mut s: String = first.chars().take(24).collect();
    if first.chars().count() > 24 {
        s.push('…');
    }
    if s.is_empty() {
        s = "（空会话）".to_string();
    }
    s
}

/// 事件类别中文化（F7 详情展示用，与 panels/events.rs category_label 对齐）。
pub(super) fn events_category_cn(cat: &str) -> &'static str {
    match cat {
        "blueprint" => "蓝图",
        "command" => "命令",
        "progress" => "进度",
        "result" => "结果",
        "issue" => "问题",
        "verify" => "复核",
        "chain" => "决策",
        _ => "事件",
    }
}

pub(super) fn build_help_text() -> Vec<String> {
    vec![
        "AirymaxRT 智能体运行底座 - 帮助".to_string(),
        String::new(),
        "快捷键:".to_string(),
        "  F1          - 显示帮助面板".to_string(),
        "  F2          - 显示配置".to_string(),
        "  F3          - 显示运行时日志".to_string(),
        "  F4          - 显示记忆统计".to_string(),
        "  F5          - 显示插件列表".to_string(),
        "  F6          - 任务看板（work_hall 执行实例 + 在线 agent，实时刷新）".to_string(),
        "  F7          - 事件流（全局 gseq 因果序回放）".to_string(),
        "  F8          - 切换到 CLI（airy_cli；CLI 中 /tui 切回）".to_string(),
        "  Enter       - 发送消息".to_string(),
        "  Alt+Enter   - 换行（多行输入）".to_string(),
        "  Ctrl+C      - 退出 TUI".to_string(),
        "  Esc         - 返回对话".to_string(),
        "  Up/Down     - 滚动对话".to_string(),
        "  Alt+Up/Down - 浏览输入历史（Alt+↓ 可回到手输状态）".to_string(),
        "  PgUp/PgDn   - 滚动对话（翻页）；记忆面板翻记录窗口".to_string(),
        "  End         - 回到底部（最新消息）".to_string(),
        "  Ctrl+X      - 中止当前请求（任务执行/对话等待）".to_string(),
        "  Ctrl+Z      - 暂停/恢复等待（请求继续在后台执行）".to_string(),
        "  Ctrl+T      - 新建会话 tab（多会话；任务执行中不可用）".to_string(),
        "  Alt+1..9    - 切换会话（Alt+1 = 主会话，Alt+N = 第 N 个 tab）".to_string(),
        "  /hiairy     - 重新打开首次启动向导".to_string(),
        "  /model      - 查看当前模型；/model <模型名> 切换并持久化".to_string(),
        "  /set-key    - 写入模型 API Key：/set-key <KEY> <VALUE>（写回 secrets.env，chmod 600）".to_string(),
        "  /status     - 运行时状态总览（连接/版本/模型/用量/记忆/技能）".to_string(),
        "  /skills     - 列出本地技能库（任务成功自动沉淀）".to_string(),
        "  /memory     - 记忆统计面板（F4 等价）".to_string(),
        "  /clear      - 清空对话区".to_string(),
        "  /help       - 显示帮助面板（F1 等价）".to_string(),
        "  Tab         - 补全 / 命令（Tab 再次循环候选）".to_string(),
        "  /board      - 任务看板面板（F6 等价）".to_string(),
        "  /events     - 事件流面板（F7 等价）".to_string(),
        "  /chain      - 决策链：无参列任务，/chain <task_id> 回放该任务决策链".to_string(),
        "  /daemons    - 16 个 daemon 在线状态（经 gateway health_check）".to_string(),
        "  /agents     - 已注册智能体（agent.list）".to_string(),
        "  /tools      - 可用工具（tool.list_tools）".to_string(),
        "  /models     - LLM 模型（llm.list_models）".to_string(),
        "  /mem        - 记忆统计；/mem <query> 语义检索".to_string(),
        "  /rpc        - 通用调用：/rpc <ns>.<method> [json]（如 /rpc tool.list_tools）".to_string(),
        String::new(),
        "任务流:".to_string(),
        "  是否进入任务集由 LLM 判断，状态栏显示当前阶段徽章。".to_string(),
        "  GCCP（任务事实确认）：大任务集启动时共 5 问，逐一询问，".to_string(),
        "    每问之间 LLM 基于已答事实思考后再提下一问。".to_string(),
        "  GRAD（任务流程图确认）：五问齐备后生成流程图与结构化依赖图（DAG），".to_string(),
        "    确认后开始执行；执行中可 Ctrl+X 中止、Ctrl+Z 暂停。".to_string(),
        "  任务集执行中输入「完成」或 LLM 回复 [TASK:DONE] 即完成。".to_string(),
        String::new(),
        "记忆:".to_string(),
        "  对话历史自动持久化（$AIRY_HOME/tui/memory.jsonl），跨会话可召回。".to_string(),
        String::new(),
        "Skills 本地技能库:".to_string(),
        "  任务成功后自动提炼经验并沉淀为可复用技能（$AIRY_HOME/tui/skills.jsonl）。".to_string(),
        "  区别于社区官方技能库：本地技能是 Agent 在任务中自我总结的，".to_string(),
        "  用得多、沉淀多、可用工具就多，不用重复造技能的轮子。".to_string(),
        String::new(),
        "状态栏:".to_string(),
        "  显示阶段、技能数、回合数、Token、成本与耗时。".to_string(),
    ]
}

/// 按字符数截断长文本（工具参数/结果展示用，避免对话面板被长 JSON 撑满）。
pub(super) fn truncate_for_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{}…", cut)
}

/// 双思考（GCCP+GRAD）轨迹 → 一行计划摘要。
/// 输入 gateway 回传的 thinking 对象 {plan:{task_plan_id,node_count,nodes[]},feedback,stats}，
/// 输出如「双思考计划 5 节点：S_01 使用 web_fetch 抓取…（GRAD 2 轮收敛）」。
pub(super) fn format_thinking_summary(
    th: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let plan = th.get("plan")?;
    let node_count = plan
        .get("node_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let first_goal = plan
        .get("nodes")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|n| n.get("goal"))
        .and_then(|g| g.as_str())
        .map(|s| truncate_for_display(s, 48));
    let grad_rounds = th
        .get("feedback")
        .and_then(|f| f.get("rounds"))
        .and_then(|v| v.as_u64());
    let corrections = th
        .get("stats")
        .and_then(|s| s.get("corrections"))
        .and_then(|v| v.as_u64());

    let mut summary = format!("双思考计划 {} 节点", node_count);
    if let Some(g) = first_goal {
        summary.push_str(&format!("：{}", g));
    }
    match (grad_rounds, corrections) {
        (Some(r), Some(c)) => summary.push_str(&format!("（GRAD {} 轮 / 修正 {} 次）", r, c)),
        (Some(r), None) => summary.push_str(&format!("（GRAD {} 轮）", r)),
        _ => {}
    }
    Some(summary)
}
