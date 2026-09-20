// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 展示文本装配：会话标题、事件类别译名、帮助页、长文本截断与思考摘要。

use crate::engine::grid;

/// 会话标题：取用户输入首行，并按 24 列做一个**存储护栏**（用户可粘贴超长
/// 单行，标题在 tab 快照中长期驻留、每帧克隆，必须封顶）。该护栏大于 tab
/// 栏的 16 列展示槽，故不作展示裁决——真正上屏的宽度边界由 L2 唯一裁决点
/// `ui.rs` 的 `grid::clip(_, 16)` 决定（§3 表 N4：宽度权威唯一）。
pub(super) fn derive_session_title(input: &str) -> String {
    let t = input.trim();
    let first = t.lines().next().unwrap_or(t);
    let s = grid::clip(first, 24);
    if s.is_empty() {
        "（空会话）".to_string()
    } else {
        s
    }
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
        "  Alt+E       - 查看思考链（独立视图；思考链默认不上屏、不落长期记忆）".to_string(),
        "  Alt+F       - 焦点视图（全屏只读查看最近一条回复，Esc 返回）".to_string(),
        "  Alt+O       - 展开/折叠长系统消息".to_string(),
        "  Ctrl+M      - 鼠标滚轮捕获开关（默认关保终端文本选择；开：滚轮滚动对话，".to_string(),
        "                Shift 翻页 / Ctrl 单行）".to_string(),
        "  Enter       - 发送消息".to_string(),
        "  Alt+Enter   - 换行（多行输入）".to_string(),
        "  Ctrl+C      - 退出 TUI".to_string(),
        "  Esc         - 返回对话".to_string(),
        "  Up/Down     - 滚动对话/思考链/焦点视图".to_string(),
        "  Alt+Up/Down - 浏览输入历史（Alt+↓ 可回到手输状态）".to_string(),
        "  PgUp/PgDn   - 对话/焦点视图翻页（步长=视口高度）；记忆面板翻记录窗口；思考链翻页"
            .to_string(),
        "  Home/End    - 输入框光标到行首/行尾（readline 惯例）".to_string(),
        "  Alt+Home/End - 视口滚动到顶/底（End 在空输入时回底部）".to_string(),
        "  Ctrl+X      - 中止当前请求（任务执行/对话等待）".to_string(),
        "  Ctrl+Z      - 暂停/恢复等待（请求继续在后台执行）".to_string(),
        "  Ctrl+T      - 新建会话 tab（多会话；任务执行中不可用）".to_string(),
        "  Alt+1..9    - 切换会话（Alt+1 = 主会话，Alt+N = 第 N 个 tab）".to_string(),
        "  /hiairy     - 重新打开首次启动向导".to_string(),
        "  /model      - 查看当前模型；/model <模型名> 切换并持久化".to_string(),
        "  /set-key    - 写入模型 API Key：/set-key <KEY> <VALUE>（写回 secrets.env，chmod 600）"
            .to_string(),
        "  /status     - 运行时状态总览（连接/版本/模型/用量/记忆/技能）".to_string(),
        "  /skills     - 列出共享技能库（任务成功自动沉淀，与 CLI 同源）".to_string(),
        "  /memory     - 记忆统计面板（F4 等价）".to_string(),
        "  /clear      - 清空对话区".to_string(),
        "  /help       - 显示帮助面板（F1 等价）".to_string(),
        "  Tab         - 补全 / 命令（Tab 再次循环候选）".to_string(),
        "  /board      - 任务看板面板（F6 等价）".to_string(),
        "  /events     - 事件流面板（F7 等价）".to_string(),
        "  /think      - 思考链独立视图（Alt+E 等价；思考链默认不上屏）".to_string(),
        "  /chain      - 决策链：无参列任务，/chain <task_id> 回放该任务决策链".to_string(),
        "  /daemons    - 14 个 daemon 在线状态（gateway 自身见顶部连接灯）".to_string(),
        "  /agents     - 已注册智能体（agent.list）".to_string(),
        "  /tools      - 可用工具（tool.list_tools）".to_string(),
        "  /models     - LLM 模型（llm.list_models）".to_string(),
        "  /mem        - 记忆统计；/mem <query> 语义检索".to_string(),
        "  /rpc        - 通用调用：/rpc <ns>.<method> [json]（如 /rpc tool.list_tools）"
            .to_string(),
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
        "  对话记忆经网关记忆服务（mem.*）持久化，与 CLI 共享同一记忆库，".to_string(),
        "  跨会话可召回；TUI 不持有独立本地存储。".to_string(),
        String::new(),
        "Skills 共享技能库:".to_string(),
        "  任务成功后自动提炼经验并沉淀为可复用技能（经网关 mem.* 与 CLI 共享同一存储，"
            .to_string(),
        "  metadata.kind=skill 分区）。".to_string(),
        "  区别于社区官方技能库：本地技能是 Agent 在任务中自我总结的，".to_string(),
        "  用得多、沉淀多、可用工具就多，不用重复造技能的轮子。".to_string(),
        String::new(),
        "状态栏:".to_string(),
        "  顶部系统状态条：连接状态与时间、模型、Token/成本、阶段徽章、".to_string(),
        "  任务控制（暂停/中止）——技能数/回合数不在此栏，F4 记忆/F5 技能查看。".to_string(),
    ]
}

/// 双思考（GCCP+GRAD）轨迹 → 一行计划摘要。
/// 输入 gateway 回传的 thinking 对象 {plan:{task_plan_id,node_count,nodes[]},feedback,stats}，
/// 输出如「双思考计划 5 节点：S_01 使用 web_fetch 抓取…（GRAD 2 轮收敛）」。
pub(super) fn format_thinking_summary(
    th: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let plan = th.get("plan")?;
    let node_count = plan.get("node_count").and_then(|v| v.as_u64()).unwrap_or(0);
    let first_goal = plan
        .get("nodes")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|n| n.get("goal"))
        .and_then(|g| g.as_str())
        .map(|s| grid::clip(s, 48));
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
