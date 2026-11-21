// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 异步轮询：chain / ops / hall / pending / approvals 的状态推进与结果落地。

use std::collections::VecDeque;

use crate::budget::MAX_DEDUPE_KEYS;
use crate::engine::grid;

use super::*;

/// 事件流面板本地缓存上限（M5 W4）：查询节拍增量与本地合并两路数据
/// 保留最新 N 条（渲染 take 256，缓存留余量防抖动）。
const HALL_EVENTS_MAX: usize = 1024;

/// 去重键：file_id 非空时以其为键；无 file_id 的合成事件沿用既有行为
/// （不去重），避免用可能跨进程重启复位的 seq 作键造成误抑制。
fn event_key(e: &HallEvent) -> Option<&str> {
    (!e.file_id.is_empty()).then_some(e.file_id.as_str())
}

/// 时间线次序的第二关键字：gseq 是账本全局写序号（跨进程经水位线续接），
/// 同毫秒内跨类别写入时只有它能给出真实写序；旧事件缺 gseq 时退回目录内
/// seq（单类别内仍然正确，与 gateway 排序口径一致）。
fn event_order(e: &HallEvent) -> (u64, u64) {
    (if e.gseq > 0 { e.gseq } else { e.seq }, e.seq)
}

/// 合并本地事件与查询拉取事件（M5 W4）。
///
/// hall_events 保持升序（渲染层 rev 展示最新在前）；合并按 (ts_utc, gseq)
/// 稳定全局序（与 gateway hall.replay/hall.stream 同一排序口径），以
/// file_id 去重（事件文件 id 全局唯一）。每次查询拉取与本地缓存同形态
/// （gateway 侧 SSoT），此处即可用同一合并逻辑收敛。
///
/// `seen` 是跨轮次的有界去重键 LRU（§6.10 #5）：本地窗口只保留最新
/// HALL_EVENTS_MAX 条，窗口外的旧事件若被 RPC 全量重放，仅靠窗口内去重
/// 会重新入列并挤掉新事件；键 LRU 把重放挡在合并之前。容量
/// MAX_DEDUPE_KEYS，长会话不随事件总量爬坡。
fn merge_hall_events(
    local: &mut Vec<HallEvent>,
    incoming: Vec<HallEvent>,
    seen: &mut VecDeque<String>,
) {
    let mut fresh: Vec<HallEvent> = Vec::with_capacity(incoming.len());
    for ev in incoming {
        let dup = match event_key(&ev) {
            Some(k) => {
                seen.iter().any(|s| s.as_str() == k)
                    || fresh.iter().any(|f| f.file_id.as_str() == k)
            }
            None => false,
        };
        if !dup {
            fresh.push(ev);
        }
    }
    if !fresh.is_empty() {
        // 先排序再记键：LRU 按时间序淘汰，留下的是最新 256 个键
        fresh.sort_by(|a, b| {
            a.ts_utc
                .cmp(&b.ts_utc)
                .then_with(|| event_order(a).cmp(&event_order(b)))
        });
        for ev in &fresh {
            if let Some(key) = event_key(ev) {
                seen.push_back(key.to_string());
            }
        }
        while seen.len() > MAX_DEDUPE_KEYS {
            seen.pop_front();
        }
        local.extend(fresh);
        local.sort_by(|a, b| {
            a.ts_utc
                .cmp(&b.ts_utc)
                .then_with(|| event_order(a).cmp(&event_order(b)))
        });
        local.dedup_by(|a, b| !a.file_id.is_empty() && a.file_id == b.file_id);
    }
    if local.len() > HALL_EVENTS_MAX {
        let excess = local.len() - HALL_EVENTS_MAX;
        local.drain(..excess);
    }
}

impl App {
    /// 摘取 /chain 的查询应答并渲染进对话区（观察面读经 query 帧）。
    pub fn poll_chain(&mut self) {
        let Some(list) = self.chain_pending else {
            return;
        };
        let id = if list { "chain.tasks" } else { "chain.replay" };
        let Some(reply) = self.take_query(id) else {
            // 管道已退出：应答永远不会到达，清掉在途标记，避免命令永久卡住
            if !self.pipe_alive() {
                self.chain_pending = None;
            }
            return;
        };
        self.chain_pending = None;
        self.mark_landed(Lane::Poll);
        match reply {
            Ok(v) if list => {
                let tasks: Vec<HallTask> = v
                    .get("tasks")
                    .and_then(|t| serde_json::from_value(t.clone()).ok())
                    .unwrap_or_default();
                if tasks.is_empty() {
                    self.add_message(
                        MessageRole::System,
                        "决策链为空：暂无任务文件（$AIRY_HOME/data/agentrt/hall）。".to_string(),
                    );
                    return;
                }
                let mut text = format!("任务列表（{} 个，最新在前）", tasks.len());
                for t in tasks.iter().take(20) {
                    text.push_str(&format!(
                        "\n  {} · {} 事件 · {}",
                        t.task_id, t.event_count, t.latest_ts
                    ));
                }
                if tasks.len() > 20 {
                    text.push_str(&format!(
                        "\n  … 另有 {} 个（/chain <task_id> 查看决策链）",
                        tasks.len() - 20
                    ));
                } else {
                    text.push_str("\n  /chain <task_id> 查看决策链");
                }
                self.add_message(MessageRole::System, text);
            }
            Ok(v) => {
                let events: Vec<HallEvent> = v
                    .get("events")
                    .and_then(|e| serde_json::from_value(e.clone()).ok())
                    .unwrap_or_default();
                let tid = self.chain_task.clone();
                if events.is_empty() {
                    self.add_message(
                        MessageRole::System,
                        format!("决策链为空：任务「{}」暂无事件。", tid),
                    );
                    return;
                }
                let mut text = format!("决策链「{}」（{} 条事件，gseq 因果序）", tid, events.len());
                for e in events.iter().take(64) {
                    text.push_str(&format!("\n  {}", crate::panels::events::event_line(e, 96)));
                }
                if events.len() > 64 {
                    text.push_str(&format!(
                        "\n  … 另有 {} 条（详见 F7 事件流面板）",
                        events.len() - 64
                    ));
                }
                self.add_message(MessageRole::System, text);
            }
            Err(e) if list => {
                self.add_message(MessageRole::System, format!("任务列表读取失败：{}", e));
            }
            Err(e) => {
                self.add_message(MessageRole::System, format!("决策链读取失败：{}", e));
            }
        }
    }

    /// 消费运维命令的查询应答并渲染进对话区。
    ///
    /// 两类在途：/daemons（每域一条 `ops.daemon.{i}`，收齐后一次性成表）
    /// 与通用单方法（`ops.call`）。应答按 id 摘取；管道已退出则如实报错，
    /// 不让命令卡在「检查中」。
    pub fn poll_ops(&mut self) {
        match self.ops_q.take() {
            None => {}
            Some(OpsStage::Call) => match self.take_query("ops.call") {
                Some(Ok(v)) => {
                    self.mark_landed(Lane::Poll);
                    // C-19：/models 与 /rpc llm.slots_get 同一份带 slots 的结果，
                    // 顺手刷新徽标判据，避免用户必须重启 TUI 才看到新槽位。
                    self.absorb_slots(&v);
                    let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
                    self.add_message(
                        MessageRole::System,
                        format!("{} 结果：\n{}", self.ops_label, pretty),
                    );
                }
                Some(Err(e)) => {
                    self.mark_landed(Lane::Poll);
                    self.add_message(
                        MessageRole::System,
                        format!("{} 调用失败：{}", self.ops_label, e),
                    );
                }
                None if !self.pipe_alive() => {
                    self.add_message(
                        MessageRole::System,
                        format!("{} 调用失败：事件管道不可用", self.ops_label),
                    );
                }
                None => self.ops_q = Some(OpsStage::Call),
            },
            Some(st @ OpsStage::Daemons { .. }) => self.poll_ops_daemons(st),
        }
    }

    /// /daemons 聚合应答收集：全部到齐后一次性渲染，缺应答保留在途。
    fn poll_ops_daemons(&mut self, st: OpsStage) {
        let OpsStage::Daemons {
            mut pending,
            mut results,
        } = st
        else {
            return;
        };
        let mut i = 0;
        while i < pending.len() {
            if let Some(r) = self.take_query(&pending[i].1) {
                let (ns, _) = pending.remove(i);
                results.push((ns, r));
            } else {
                i += 1;
            }
        }
        if pending.is_empty() {
            self.mark_landed(Lane::Poll);
            self.land_ops_daemons(results);
            return;
        }
        if !self.pipe_alive() {
            self.add_message(MessageRole::System, "daemon 检查失败：事件管道不可用");
            return;
        }
        self.ops_q = Some(OpsStage::Daemons { pending, results });
    }

    /// /daemons 结果落地：在线统计 + 健康表回填 + 对话区成表。
    fn land_ops_daemons(&mut self, results: Vec<(String, Result<serde_json::Value, String>)>) {
        let online = results.iter().filter(|(_, r)| r.is_ok()).count();
        let mut text = format!("daemon 在线状态（{} / {} 在线）", online, results.len());
        for (ns, r) in results {
            // /daemons 与 5s 保活探测同源（都是真实查询），结果一并回填
            // 健康表；只在状态迁移时记错误，避免重复刷屏。
            let ok = r.is_ok();
            let err = r.as_ref().err().cloned();
            self.note_dep(&ns, ok, self.clock_ms, err.as_deref());
            let (icon, st) = if ok {
                ("✓", "在线")
            } else {
                ("✗", "离线")
            };
            text.push_str(&format!("\n  {} {} {}", icon, st, ns));
        }
        self.sync_banner(self.clock_ms);
        self.add_message(MessageRole::System, text);
    }

    /// hall 面板（看板/事件流）数据拉取：面板激活时按节拍下发查询帧。
    ///
    /// 看板 = hall.board（work_hall 持久化实例 + 在线 agent）；
    /// 事件流 = hall.stream（全局 gseq 因果序，最新 512 条）。
    /// 观察面读统一走 D-15 query 帧：CLI 同步取值后回 query_result，应答按
    /// id 暂存，本函数摘取后更新面板缓存（不自建 SSE/HTTP 连接）。
    ///
    /// `due` 由主循环传入（`Beats::has(Beat::Hall)`）：拉取节拍归 L4 调度表，
    /// 此处不再自持 1s 节流时钟（0.1.18 §5A.3 W4）。
    pub fn poll_hall(&mut self, due: bool) {
        if self.active_panel != ActivePanel::Board && self.active_panel != ActivePanel::Events {
            return;
        }
        let want_board = self.active_panel == ActivePanel::Board;
        // 在途应答：摘到即落地；管道已退出则该请求永远不会应答，放弃等待
        if let Some(q_board) = self.hall_q {
            let id = if q_board { "hall.board" } else { "hall.stream" };
            match self.take_query(id) {
                Some(reply) => {
                    self.hall_q = None;
                    self.land_hall(q_board, reply);
                }
                None if !self.pipe_alive() => self.hall_q = None,
                // 上一请求仍在途：即使节拍到期也不重复下发（慢请求不丢）
                None => return,
            }
        }
        // 节拍门控：Beat::Hall 到期才发起拉取（事件流面板的刷新即此查询节拍）
        if self.hall_q.is_some() || !due {
            return;
        }
        let (id, params) = if want_board {
            ("hall.board", serde_json::json!({}))
        } else {
            ("hall.stream", serde_json::json!({ "limit": 512 }))
        };
        match self.send_query(id, id, &params) {
            Ok(()) => self.hall_q = Some(want_board),
            Err(e) => self.set_hall_error(id, &e),
        }
    }

    /// 查询应答落地：看板整体替换，事件流增量合并（按 file_id 去重入列）。
    fn land_hall(&mut self, board: bool, reply: Result<serde_json::Value, String>) {
        if board {
            let parsed = reply
                .and_then(|v| serde_json::from_value::<HallBoard>(v).map_err(|e| e.to_string()));
            match parsed {
                Ok(b) => {
                    self.hall_board = Some(b);
                    self.mark_landed(Lane::Panel);
                    self.clear_hall_error("hall.board");
                }
                Err(e) => {
                    self.set_hall_error("hall.board", &e);
                    log::warn!("hall.board 拉取失败: {}", e);
                }
            }
            return;
        }
        match reply {
            Ok(v) => {
                let evts: Vec<HallEvent> = v
                    .get("events")
                    .and_then(|e| serde_json::from_value(e.clone()).ok())
                    .unwrap_or_default();
                if !evts.is_empty() {
                    self.mark_landed(Lane::Panel);
                }
                merge_hall_events(&mut self.hall_events, evts, &mut self.hall_seen);
                self.clear_hall_error("hall.stream");
            }
            Err(e) => {
                self.set_hall_error("hall.stream", &e);
                log::warn!("hall.stream 拉取失败: {}", e);
            }
        }
    }

    /// hall 拉取失败：记录失败态（board.rs 区分"正在加载"与"失败/离线"）
    /// 并在状态变化时追加一条 F3 日志——内容相同不重复刷屏（离线时轮询
    /// 每秒重试一次，日志只记一次变化，文件日志仍逐条记录）。
    fn set_hall_error(&mut self, source: &str, err: &str) {
        let msg = format!("{} 拉取失败：{}", source, err);
        let changed = self.hall_error.as_deref() != Some(msg.as_str());
        self.hall_error = Some(msg.clone());
        if changed {
            self.add_log("ERROR", msg);
        }
    }

    /// hall 拉取成功：清除失败态；此前确有失败时记一条恢复日志（F3 可见）。
    fn clear_hall_error(&mut self, source: &str) {
        if self.hall_error.take().is_some() {
            self.add_log("INFO", format!("{} 恢复刷新", source));
        }
    }

    /// 请求下次节拍判定时立即拉取（F6/F7 进入面板、`/board`、`/events` 时调用）。
    ///
    /// 只置「有刷新需求」标志：由主循环取走（`take_hall_force`）并把 Hall 节拍
    /// 提前到下一帧，拉取时刻仍由 L4 调度器裁决（0.1.18 §5A.3 W4 节拍权威）。
    pub fn force_hall_refresh(&mut self) {
        self.hall_force = true;
    }

    /// 取走 hall 显式刷新请求（读后清零）。主循环据此调用调度器的节拍提前。
    pub fn take_hall_force(&mut self) -> bool {
        std::mem::take(&mut self.hall_force)
    }

    /* ---- 2026-08-17：F6/F7 面板交互（光标 + 过滤）---- */

    /// 主循环轮询：后台请求完成则应用结果（返回是否有待办请求，供渲染循环判断）。
    ///
    /// 流式请求（StreamRound）：先消费 mpsc 增量块实时追加到 streaming_text，
    /// 再检查 oneshot 完整结果（结束信号）——**结果到达即落定**，不受上屏
    /// 动画进度影响（0.1.18 B2-6）。
    ///
    /// `approvals_due` 由主循环传入（`Beats::has(Beat::Approvals)`）：审批轮询
    /// 节拍归 L4 调度表，此处不再自持 1.5s 节流时钟（0.1.18 §5A.3 W4）。
    pub fn poll_pending(&mut self, approvals_due: bool) -> bool {
        // 审批轮询：请求在途时查询 tool.pending（在借用 self.pending 前调用，
        // 且 poll_approvals 内部自判 pending 是否存在）
        self.poll_approvals(approvals_due);
        // 取走 pending 持有所有权：消费过程中需要调用 `&mut self` 方法记落地
        // 档位，借用 `self.pending` 会产生冲突；未落定路径原样放回。
        let Some(mut p) = self.pending.take() else {
            return false;
        };
        // ── 流式增量消费：SSE 块 → 控制面净化 → streaming_text ──
        // 0.1.18 B3（V3.1）：增量块经 StreamSanitizer 剥离 [MODE:*] 标记与
        // 元话语后再上屏——流式中间态零协议渲染；剥出文本暂存 protocol_buf，
        // 落定时入 F3 诊断通道（settle_stream_tail）。
        if let Some(stream_rx) = &mut p.stream_rx {
            let mut got = false;
            while let Ok(chunk) = stream_rx.try_recv() {
                got = true;
                let (clean, _) = self.stream_sanitizer.feed(&chunk);
                self.streaming_text.push_str(&clean);
            }
            if got {
                self.mark_landed(Lane::Stream);
                log::trace!("stream chunk: total={} chars", self.streaming_text.len());
            }
        }
        // ── 上屏（0.1.18 §5A.3 W4 B2）：本地打字机已移除 ──
        // 增量到达即整块上屏，不引入任何附加上屏延迟；逐字观感由服务端真流式
        // （complete→complete_stream）给出。上屏与落定同刻，无动画门控。
        // ── 流式工具事件消费：tool_call/tool_result → 工具状态行 ──
        if let Some(tool_rx) = &mut p.tool_rx {
            while let Ok(evt) = tool_rx.try_recv() {
                self.mark_landed(Lane::Stream);
                // 思考链事件（__airy_evt:reasoning）→ 追加到 stream_reasoning
                // （增量块；gateway 逐块透传，实时上屏 + 落定折叠）
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&evt) {
                    if v.get("__airy_evt").and_then(|k| k.as_str()) == Some("reasoning") {
                        if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                            if self.stream_reasoning.is_empty() {
                                self.stream_reasoning_start = Some(Instant::now());
                            }
                            self.stream_reasoning.push_str(c);
                        }
                        // 2.3.14：思考链模型轨（t2/t1-f/t1-p → Dual Slow/Fast/Prof Think）
                        if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                            self.stream_reasoning_model = m.to_string();
                        }
                        continue;
                    }
                    // 2.1.1.5：usage 事件（llm_d 流式尾帧真实 token 消耗，gateway
                    // 透传）→ 累加到会话级统计（TUI 英雄区 tok/$ 实时真实）。
                    if v.get("__airy_evt").and_then(|k| k.as_str()) == Some("usage") {
                        if let Some(t) = v.get("total_tokens").and_then(|t| t.as_u64()) {
                            self.tokens += t;
                        }
                        if let Some(c) = v.get("cost_usd").and_then(|c| c.as_f64()) {
                            self.cost += c;
                        }
                        continue;
                    }
                    // 0.1.8：error 事件（gateway 把 llm_d 错误信封/不可达转为
                    // 可读文本）→ 记录后由 apply_stream_result 以失败形式呈现。
                    if v.get("__airy_evt").and_then(|k| k.as_str()) == Some("error") {
                        if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
                            if self.stream_error.is_none() {
                                self.stream_error = Some(m.to_string());
                            }
                        }
                        continue;
                    }
                    // C-15：task_state 事件驱动任务状态机（阶段渲染）；
                    // needs_intent 已在客户端转译为结果字段，不上工具通道。
                    if v.get("__airy_evt").and_then(|k| k.as_str()) == Some("task_state") {
                        if let Some(state) = v.get("state").and_then(|s| s.as_str()) {
                            self.observe_task_state(state);
                            log::debug!("task_state 事件: {state}");
                        }
                        continue;
                    }
                }
                if let Some(line) = App::render_tool_event(&evt) {
                    self.stream_tool_events.push(line);
                }
            }
        }
        // 消费后台结果：服务水平结果到达即落定（0.1.18 B2-6）——不再等待
        // 打字机 reveal 追平。动画只作用于流式期间，**不构成落定门控**：
        // 关闭动画（默认）时上屏与落定同刻；开启动画时落定会丢弃未播完的
        // 余量，用户感知为"结果到达即完成"，而非"等动画播完"。
        // D-15：TurnState 的结果来自查询帧应答（不经 oneshot）。应答未到
        // 即原样放回；管道已退出则按失败落定，避免 loading 悬死。
        let outcome = if matches!(&p.kind, PendingKind::TurnState) {
            match self.take_query("turns.get") {
                Some(r) => PendingOutcome::Turns(r.map_err(|e| anyhow!(e))),
                None if !self.pipe_alive() => PendingOutcome::Turns(Err(anyhow!("事件管道不可用"))),
                None => {
                    self.pending = Some(p);
                    return true;
                }
            }
        } else {
            match p.rx.try_recv() {
                Ok(o) => o,
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    self.pending = Some(p);
                    return true;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    PendingOutcome::Run(Err(anyhow!("LLM 后台任务异常终止")))
                }
            }
        };
        // 取走 kind（p 已具所有权），先清 loading，再应用结果；pending 在上方
        // take 时即已腾空。
        self.mark_landed(Lane::Stream);
        let kind = std::mem::replace(
            &mut p.kind,
            PendingKind::ChatRound {
                input: String::new(),
            },
        );
        self.loading = false;
        self.set_task_control(TaskControl::Running);
        log::info!("poll_pending: 消费后台请求结果（kind={:?}）", kind);
        // 会话完成通知（§6.7 #4）：只在对话回合落定时生产，内部多步请求
        // （GRAD/GCCP/蒸馏）不逐段打扰；正文按成功/失败区分。
        let chat_landed = matches!(
            &kind,
            PendingKind::ChatRound { .. } | PendingKind::StreamRound { .. }
        );
        if chat_landed {
            let secs = self.turn_started.elapsed().as_secs_f64();
            let body = match &outcome {
                PendingOutcome::Run(Ok(_)) => format!("回合完成 · {secs:.1}s"),
                _ => format!("回合失败 · {secs:.1}s"),
            };
            self.notice = Some(crate::app::Notice {
                title: "Airymax".to_string(),
                body,
            });
        }
        // 回合计时结算：回合分隔线展示本回合耗时（Worked for Ns）
        self.last_turn_elapsed = Some(self.turn_started);
        self.apply_result(kind, outcome);
        // 会话落盘（0.1.20 TUI 缺口②）：对话回合落定即写 sessions.json——
        // 变更即存覆盖崩溃/强杀出口，退出路径（shutdown/F8）只是补充。
        if chat_landed {
            self.persist_tabs();
        }
        self.pending.is_some()
    }

    /// 审批同步：把事件投影里的待批行同步到界面队列。
    ///
    /// 审批请求由 CLI 经事件流（control_request）直达，投影按 request_id
    /// 建行；本地决议把 Answer 折回投影，行随即收口。不再向网关轮询
    /// pending，也不存在「渠道故障」这条独立失败路径——管道断了事件流
    /// 本身就会停，队列随运行结束一并清空。
    ///
    /// `due` 为 L4 调度表给出的 `Beat::Approvals` 到期信号：只门控快照
    /// 读取（克隆整份投影有成本），不是数据可用性的前提。
    pub(super) fn poll_approvals(&mut self, due: bool) {
        if self.pending.is_none() || !due {
            return;
        }
        let Some(ev) = self.ev.clone() else {
            return;
        };
        let proj = ev.snapshot();
        for r in proj.approvals().pending() {
            if self.approvals.iter().any(|x| x.request_id == r.id) {
                continue;
            }
            self.mark_landed(Lane::Poll);
            self.approvals.push(PendingApproval {
                request_id: r.id.clone(),
                tool: r.tool.clone(),
                agent_id: String::new(),
                params: r.target.clone(),
                created_at: Some(r.asked),
            });
            self.add_message(
                MessageRole::System,
                format!(
                    "工具「{}」请求权限执行（参数: {}）\n按 y=允许本次 · a=允许本次（总是待支持） · n=拒绝（Esc 拒绝）",
                    r.tool,
                    grid::clip(&r.target, 160)
                ),
            );
            self.add_log("INFO", format!("权限审批待决议: {} ({})", r.tool, r.id));
        }
    }

    /// 应用后台请求结果（按请求类型分派）。
    pub(super) fn apply_result(&mut self, kind: PendingKind, outcome: PendingOutcome) {
        match kind {
            PendingKind::ChatRound { input } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_chat_result(input, res);
                }
            }
            PendingKind::StreamRound { input } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_stream_result(input, res);
                }
            }
            PendingKind::AskGccp { round } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_ask_gccp(round, res);
                }
            }
            PendingKind::GradPlan => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_grad_plan(res);
                }
            }
            PendingKind::GradConfirm { confirmed } => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_grad_confirm(confirmed, res);
                }
            }
            PendingKind::TurnState => {
                if let PendingOutcome::Turns(res) = outcome {
                    self.apply_turn_state(res);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条最小 HallEvent（file_id 含 ts/seq，便于断言顺序）。
    fn evt(file_id: &str, ts_utc: &str, seq: u64, category: &str) -> HallEvent {
        HallEvent {
            file_id: file_id.to_string(),
            category: category.to_string(),
            task_id: "t1".to_string(),
            tenant_id: "default".to_string(),
            node_id: String::new(),
            ts_utc: ts_utc.to_string(),
            seq,
            gseq: seq,
            content: serde_json::json!({ "event": file_id }),
        }
    }

    #[test]
    fn merge_appends_sse_increments_after_pull() {
        /* RPC pull 快照（旧）→ SSE 增量（新）：合并后按 ts 升序，全部保留 */
        let mut local = vec![evt(
            "a.20260901T000000000.0001.json",
            "20260901T000000000",
            1,
            "progress",
        )];
        let incoming = vec![
            evt(
                "b.20260901T000000100.0001.json",
                "20260901T000000100",
                1,
                "result",
            ),
            evt(
                "c.20260901T000000200.0001.json",
                "20260901T000000200",
                1,
                "result",
            ),
        ];
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, incoming, &mut seen);
        assert_eq!(local.len(), 3);
        assert_eq!(local[2].file_id, "c.20260901T000000200.0001.json");
    }

    #[test]
    fn merge_dedups_overlap_between_pull_and_sse() {
        /* pull 兜底可能包含 SSE 已推事件：同 file_id 只保留一份 */
        let mut local = vec![
            evt(
                "a.20260901T000000000.0001.json",
                "20260901T000000000",
                1,
                "progress",
            ),
            evt(
                "b.20260901T000000100.0001.json",
                "20260901T000000100",
                1,
                "result",
            ),
        ];
        let incoming = vec![
            evt(
                "b.20260901T000000100.0001.json",
                "20260901T000000100",
                1,
                "result",
            ),
            evt(
                "c.20260901T000000200.0001.json",
                "20260901T000000200",
                1,
                "issue",
            ),
        ];
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, incoming, &mut seen);
        assert_eq!(local.len(), 3);
    }

    #[test]
    fn merge_sorts_out_of_order_increments() {
        /* SSE 重连回放偶发乱序：按 (ts_utc, gseq) 收敛 */
        let mut local = vec![evt(
            "c.20260901T000000200.0001.json",
            "20260901T000000200",
            1,
            "result",
        )];
        let incoming = vec![evt(
            "a.20260901T000000000.0001.json",
            "20260901T000000000",
            1,
            "progress",
        )];
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, incoming, &mut seen);
        assert_eq!(local.len(), 2);
        assert!(local[0].ts_utc < local[1].ts_utc);
    }

    #[test]
    fn merge_orders_same_millisecond_events_by_gseq() {
        /* seq 只在类别目录内计数，同毫秒跨类别会撞号；账本全局写序 gseq
         * 才是真实次序（与 gateway hall.replay 同一口径）。 */
        let ts = "20260901T000000100";
        let mut first = evt("a.json", ts, 2, "progress");
        first.gseq = 8;
        let mut second = evt("b.json", ts, 1, "result");
        second.gseq = 9;
        let mut local = vec![second];
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, vec![first], &mut seen);
        assert_eq!(local[0].file_id, "a.json");
        assert_eq!(local[1].file_id, "b.json");
    }

    #[test]
    fn merge_legacy_events_without_gseq_fall_back_to_seq() {
        /* 旧事件文件无 gseq：退回目录内 seq（单类别内仍然正确） */
        let ts = "20260901T000000200";
        let mut first = evt("a.json", ts, 1, "progress");
        first.gseq = 0;
        let mut second = evt("b.json", ts, 2, "progress");
        second.gseq = 0;
        let mut local = vec![second];
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, vec![first], &mut seen);
        assert_eq!(local[0].file_id, "a.json");
        assert_eq!(local[1].file_id, "b.json");
    }

    #[test]
    fn merge_trims_to_cap_keeping_newest() {
        /* 容量上限：超出保留最新（头部弹出，渲染层期望最新在尾） */
        let mut local: Vec<HallEvent> = Vec::new();
        for i in 0..(HALL_EVENTS_MAX + 50) {
            let ts = format!("20260901T{:06}000", 0);
            local.push(evt(&format!("f{}.json", i), &ts, i as u64, "progress"));
        }
        let mut seen: VecDeque<String> = VecDeque::new();
        merge_hall_events(&mut local, Vec::new(), &mut seen);
        assert_eq!(local.len(), HALL_EVENTS_MAX);
        // 保留的是最大 seq（最新）
        assert_eq!(local.last().unwrap().seq, (HALL_EVENTS_MAX + 49) as u64);
        assert_eq!(local.first().unwrap().seq, 50);
    }

    #[test]
    fn merge_dedup_keys_stay_bounded() {
        /* §6.10 #5：去重键 LRU 容量 256，长会话不随事件总量爬坡 */
        let mut local: Vec<HallEvent> = Vec::new();
        let mut seen: VecDeque<String> = VecDeque::new();
        let total = HALL_EVENTS_MAX + 50;
        let batch: Vec<HallEvent> = (0..total)
            .map(|i| {
                let ts = format!("20260901T{:06}000", i);
                evt(&format!("k{i}.json"), &ts, i as u64, "progress")
            })
            .collect();
        merge_hall_events(&mut local, batch, &mut seen);
        assert_eq!(seen.len(), MAX_DEDUPE_KEYS, "去重键应有界");
        assert_eq!(local.len(), HALL_EVENTS_MAX, "事件窗口仍有界");

        let newest = format!("k{}.json", total - 1);
        let count = |v: &[HallEvent], id: &str| v.iter().filter(|e| e.file_id == id).count();
        let ts = format!("20260901T{:06}000", total - 1);
        let replay = evt(&newest, &ts, (total - 1) as u64, "progress");
        merge_hall_events(&mut local, vec![replay], &mut seen);
        assert_eq!(local.len(), HALL_EVENTS_MAX, "近期重放不入列");
        assert_eq!(count(&local, &newest), 1, "近期重放不产生重复");

        let replay = evt("k0.json", "20260901T000000000", 0, "progress");
        merge_hall_events(&mut local, vec![replay], &mut seen);
        assert_eq!(local.len(), HALL_EVENTS_MAX, "窗口外重放不挤占容量");
        assert_eq!(seen.len(), MAX_DEDUPE_KEYS, "去重键重放后仍有界");
    }
}
