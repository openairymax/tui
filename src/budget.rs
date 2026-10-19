// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L7 预算常量 SSoT（0.1.19 TUI 建设方案 §5.3）。
//
// 一切有界：渲染、投影、缓存、队列、输入输出五域的界值全部登记于此，
// 各实现模块只引用本模块，禁止在别处复制同义字面量。界值不得为无界
// 表达（0 / usize::MAX），也不得随手调大——调界属于方案变更，须先改
// 方案再改此文件。
//
// 分域与消费点（括号内为落点，随 L7 各域生长逐一接线）：
//   渲染 / 帧   engine/sched（帧窗、帧预算）、ui/term（写帧、思考预览）、
//               main（首帧预算）、keys（leader 提示）
//   投影窗口    panels/chat（turn / cell 投影上限）
//   缓存        engine/cache（解析缓存）、term（宽度缓存、OSC52）、
//               panels/events（去重键）
//   队列 / 历史 app（会话与日志）、panels/chat（保留回合）、gccp（问询单）
//   输入 / 输出 main（粘贴折叠、双击退出）、client（流式发布节拍）
//
// 裁剪语义（§5.3）：超 MAX_TOOL_TEXT_BYTES 时保留头尾各半，中间插入
// "\n… [truncated: N bytes total] …\n"，按 UTF-8 字节安全截断，并记
// originalBytes 供增量合并前缀对齐。
//
// 未接线常量在对应机制落地前暂无消费点；集中登记是 §5.3 的明确要求，
// 故本模块允许 dead_code，但不得借此放入无界或演示性常量。
#![allow(dead_code)]

// ── 渲染 / 帧 ────────────────────────────────────────────────────────────

/// 最小成帧间隔（ms）：帧界合并的硬下限，对应 §5.2 机制 #3。
pub(crate) const MIN_RENDER_INTERVAL_MS: u64 = 16;
/// 单次写帧字符上限（1MiB）：超出分片写出，避免超长转义序列阻塞终端。
pub(crate) const MAX_RENDER_WRITE_CHARS: usize = 1024 * 1024;
/// thinking 折叠态预览字符数（§6.1 #3）。
pub(crate) const THINKING_PREVIEW_MAX_CHARS: usize = 2_000;
/// 首帧预算（ms）：超过即记降级日志（§6.1 #1）。
pub(crate) const FIRST_FRAME_BUDGET_MS: u64 = 100;
/// leader 键提示浮层延迟（ms，§6.4 #7）。
pub(crate) const LEADER_HINT_DELAY_MS: u64 = 500;
/// 终端能力探测窗口（ms，§5.5 #1）：OSC11/DEC2031 应答等待上限，与首帧并发。
pub(crate) const CAP_PROBE_TIMEOUT_MS: u64 = 250;

// ── 外壳行高（TUI 方案 §4.2 布局计划）────────────────────────────────────

/// 顶部系统状态条总高：状态文字 1 行 + 主色细分隔线 1 行。
pub(crate) const SHELL_STATUS_ROWS: u16 = 2;
/// 状态文字行数（品牌/连接/时钟）。
pub(crate) const SHELL_STATUS_TEXT_ROWS: u16 = 1;
/// 状态条下细分隔线行数。
pub(crate) const SHELL_STATUS_RULE_ROWS: u16 = 1;
/// 依赖降级横幅行数（单行常显）。
pub(crate) const SHELL_BANNER_ROWS: u16 = 1;
/// 主内容区最小高度：低于该值面板无法表达任何结构。
pub(crate) const SHELL_MAIN_MIN_ROWS: u16 = 3;
/// 工具审批条行数。
pub(crate) const SHELL_APPROVAL_ROWS: u16 = 2;
/// leader 提示浮层行数。
pub(crate) const SHELL_LEADER_ROWS: u16 = 1;
/// 输入栏常规高度（输入行 + 底部分隔线）。
pub(crate) const SHELL_INPUT_ROWS: u16 = 2;
/// 输入栏 IME 拼音态高度（输入行 + 候选区 + 分隔线）。
pub(crate) const SHELL_INPUT_IME_ROWS: u16 = 3;
/// 快捷键行高度。
pub(crate) const SHELL_SHORTCUT_ROWS: u16 = 1;

// ── 投影窗口 ────────────────────────────────────────────────────────────

/// 首屏投影回合数上限（§5.1 拉模型）。
pub(crate) const MAX_INITIAL_TURNS: usize = 30;
/// 单回合物化 cell 上限：超出折叠为摘要（§6.10 #6 同源）。
pub(crate) const MAX_CELLS_PER_TURN: usize = 120;
/// 视口外投影回合上限（§6.10 #3）。
pub(crate) const MAX_PROJECTED_TURNS: usize = 60;
/// 投影 cell 总量上限：渲染树总规模的有界保证。
pub(crate) const MAX_PROJECTED_CELLS: usize = 1_000;

// ── 缓存 ────────────────────────────────────────────────────────────────

/// 宽度测量缓存条目上限（§5.1 cell diff）。
pub(crate) const WIDTH_CACHE_SIZE: usize = 512;
/// OSC52 编码后长度上限（§5.5 #4）：超出改走文件/剪贴板降级。
pub(crate) const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;
/// 事件去重键上限（§6.10 #5）：防长会话内存爬坡。
pub(crate) const MAX_DEDUPE_KEYS: usize = 256;

// ── 队列 / 历史 ──────────────────────────────────────────────────────────

/// 视口外保留回合摘要数（§6.10 #3）。
pub(crate) const RETAINED_TURNS_LIMIT: usize = 8;
/// 已关闭问询单 id 记忆上限（防重复弹窗内存增长）。
pub(crate) const MAX_CLOSED_QUESTIONNAIRE_IDS: usize = 128;
/// 多 agent 嵌套深度钳位（§5.2 机制 #9）：防渲染环。
pub(crate) const MAX_PARENT_DEPTH: usize = 32;
/// 子代理历史注入条数上限（§6.4 #4 历史 recall 供数）。
pub(crate) const CHILD_HISTORY_LIMIT: usize = 200;
/// 同屏活跃 toast 上限（§6.3 #4）。
pub(crate) const MAX_ACTIVE_TOASTS: usize = 3;
/// toast 存活时长（ms，§6.3 #4）：到点自动消失。
pub(crate) const TOAST_TTL_MS: u64 = 1_500;

// ── 输入 / 输出 ──────────────────────────────────────────────────────────

/// 单次输出字符上限（64KiB）：与工具文本同界。
pub(crate) const MAX_OUTPUT_CHARACTERS: usize = 64 * 1024;
/// 工具文本落库上限（64KiB，§6.3 #6）：超限按裁剪语义折叠。
pub(crate) const MAX_TOOL_TEXT_BYTES: usize = 64 * 1024;
/// 双击退出窗口（ms，§6.5 #3）。
pub(crate) const DOUBLE_ESCAPE_WINDOW_MS: u64 = 500;
/// 流式增量发布节拍（ms，§6.6）。
pub(crate) const DEFAULT_LIVE_PUBLISH_INTERVAL_MS: u64 = 250;
/// 大粘贴折叠阈值（字节，§6.4 #3）：超过即折叠为占位。
pub(crate) const MAX_PASTE_FOLD_BYTES: usize = 16 * 1024;
/// 通知正文/标题字符上限（§5.5 #5）：超长截断，防超长 OSC 序列。
pub(crate) const NOTIFY_TEXT_MAX: usize = 120;

// ── 降级态与可观测性 ─────────────────────────────────────────────────────

/// 健康面板条目上限（§6.12 #3）。
pub(crate) const MAX_HEALTH_PANEL_ITEMS: usize = 16;
/// 错误聚合表条目上限（§6.13 #2/#4）。
pub(crate) const MAX_ERROR_AGG_ENTRIES: usize = 32;
/// 降级横幅恢复冷却（ms，§6.12 #6）：抖动期不闪烁。
pub(crate) const DEGRADED_BANNER_COOLDOWN_MS: u64 = 5_000;

// ── 存量界值（自各实现模块收编，行为零变化）──────────────────────────────

/// 调度空闲等待上限（ms）：无事件时最长挂起（engine/sched）。
pub(crate) const IDLE_WAIT_MS: u64 = 250;
/// 单帧时间预算（μs）：超限置降级态（engine/sched）。
pub(crate) const FRAME_BUDGET_US: u64 = 8_000;
/// 待办帧队列上限（engine/sched 背压）。
pub(crate) const FRAME_QUEUE_CAP: usize = 64;
/// 帧率上限（engine/sched，`AIRY_TUI_MAX_FPS` 收敛上界）。
pub(crate) const MAX_FPS: u32 = 240;
/// 解析缓存条目上限（engine/cache）。
pub(crate) const COMPOSE_CACHE_ENTRIES: usize = 256;
/// 解析缓存字节上限（engine/cache）。
pub(crate) const COMPOSE_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// 事件流单屏物化行数（panels/events）。
pub(crate) const MAX_EVENT_ROWS: usize = 256;
/// 看板单屏物化行数（panels/board）。
pub(crate) const MAX_BOARD_ROWS: usize = 64;
/// 会话消息保留上限（app）：超限淘汰最老消息。
pub(crate) const MAX_CHAT_MESSAGES: usize = 2_000;

// 界值自洽在编译期守护：任一关系被改坏即构建失败，不依赖测试是否被执行。
const _: () = {
    // 全部界值须为正：0 即无界（或退化为不可用），不得登记。
    assert!(MIN_RENDER_INTERVAL_MS > 0);
    assert!(MAX_RENDER_WRITE_CHARS > 0);
    assert!(THINKING_PREVIEW_MAX_CHARS > 0);
    assert!(FIRST_FRAME_BUDGET_MS > 0);
    assert!(LEADER_HINT_DELAY_MS > 0);
    assert!(CAP_PROBE_TIMEOUT_MS > 0 && CAP_PROBE_TIMEOUT_MS <= IDLE_WAIT_MS);
    assert!(MAX_INITIAL_TURNS > 0);
    assert!(MAX_CELLS_PER_TURN > 0);
    assert!(MAX_PROJECTED_TURNS > 0);
    assert!(MAX_PROJECTED_CELLS > 0);
    assert!(WIDTH_CACHE_SIZE > 0);
    assert!(MAX_OSC52_ENCODED_LENGTH > 0);
    assert!(MAX_DEDUPE_KEYS > 0);
    assert!(RETAINED_TURNS_LIMIT > 0);
    assert!(MAX_CLOSED_QUESTIONNAIRE_IDS > 0);
    assert!(MAX_PARENT_DEPTH > 0);
    assert!(CHILD_HISTORY_LIMIT > 0);
    assert!(MAX_ACTIVE_TOASTS > 0);
    assert!(TOAST_TTL_MS > 0);
    assert!(MAX_OUTPUT_CHARACTERS > 0);
    assert!(MAX_TOOL_TEXT_BYTES > 0);
    assert!(DOUBLE_ESCAPE_WINDOW_MS > 0);
    assert!(DEFAULT_LIVE_PUBLISH_INTERVAL_MS > 0);
    assert!(MAX_PASTE_FOLD_BYTES > 0);
    assert!(NOTIFY_TEXT_MAX > 0 && NOTIFY_TEXT_MAX <= MAX_OUTPUT_CHARACTERS);
    assert!(MAX_HEALTH_PANEL_ITEMS > 0);
    assert!(MAX_ERROR_AGG_ENTRIES > 0);
    assert!(DEGRADED_BANNER_COOLDOWN_MS > 0);
    assert!(IDLE_WAIT_MS > 0);
    assert!(FRAME_BUDGET_US > 0);
    assert!(FRAME_QUEUE_CAP > 0);
    assert!(MAX_FPS > 0);
    assert!(COMPOSE_CACHE_ENTRIES > 0);
    assert!(COMPOSE_CACHE_BYTES > 0);
    assert!(MAX_EVENT_ROWS > 0);
    assert!(MAX_BOARD_ROWS > 0);
    assert!(MAX_CHAT_MESSAGES > 0);

    // 外壳行高：状态条由文字行与分隔线组成，IME 态输入栏须高于常规态，
    // 主内容最小高度不得低于输入栏——否则小终端下面板先于输入栏被压扁。
    assert!(SHELL_STATUS_TEXT_ROWS + SHELL_STATUS_RULE_ROWS == SHELL_STATUS_ROWS);
    assert!(SHELL_INPUT_IME_ROWS > SHELL_INPUT_ROWS);
    assert!(SHELL_MAIN_MIN_ROWS >= SHELL_INPUT_IME_ROWS);
    assert!(SHELL_BANNER_ROWS > 0 && SHELL_APPROVAL_ROWS > 0);
    assert!(SHELL_LEADER_ROWS > 0 && SHELL_SHORTCUT_ROWS > 0);

    // 帧界：最小成帧间隔不得长于发布节拍，单帧预算须落在间隔之内。
    assert!(MIN_RENDER_INTERVAL_MS <= DEFAULT_LIVE_PUBLISH_INTERVAL_MS);
    assert!(FRAME_BUDGET_US <= MIN_RENDER_INTERVAL_MS * 1_000);

    // 投影界：单回合 cell 不得超出总量，首屏不得超出可投影回合。
    assert!(MAX_PROJECTED_CELLS >= MAX_CELLS_PER_TURN);
    assert!(MAX_PROJECTED_TURNS >= MAX_INITIAL_TURNS);
    assert!(MAX_PROJECTED_TURNS > RETAINED_TURNS_LIMIT);

    // 输入输出界：粘贴折叠 ≤ 工具文本 ≤ 输出，会话保留须容纳历史注入窗口。
    assert!(MAX_PASTE_FOLD_BYTES <= MAX_TOOL_TEXT_BYTES);
    assert!(MAX_TOOL_TEXT_BYTES <= MAX_OUTPUT_CHARACTERS);
    assert!(MAX_CHAT_MESSAGES >= CHILD_HISTORY_LIMIT);

    // 缓存与队列界：去重键表不得大于解析缓存条目上限，行数层级须单调。
    assert!(MAX_DEDUPE_KEYS <= COMPOSE_CACHE_ENTRIES);
    assert!(MAX_EVENT_ROWS <= MAX_CHAT_MESSAGES);
    assert!(MAX_BOARD_ROWS <= MAX_EVENT_ROWS);
    assert!(MAX_ACTIVE_TOASTS <= MAX_ERROR_AGG_ENTRIES);
};
