// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 异步轮询：chain / ops / hall / pending / approvals 的状态推进与结果落地。

use super::*;

/// 事件流面板本地缓存上限（M5 W4）：SSE 增量 + RPC 兜底两路数据合并后
/// 保留最新 N 条（渲染 take 256，缓存留余量防抖动）。
const HALL_EVENTS_MAX: usize = 1024;

/// 合并本地事件与拉取/推送事件（M5 W4）。
///
/// hall_events 保持升序（渲染层 rev 展示最新在前）；合并按 (ts_utc, seq)
/// 稳定全局序（跨进程 gseq 会重启，不能作排序键），以 file_id 去重
/// （事件文件 id 全局唯一）。SSE 增量与 RPC 全量两路数据在 gateway 侧
/// 同为扁平事件形态（SSoT），此处即可用同一合并逻辑收敛。
fn merge_hall_events(local: &mut Vec<HallEvent>, incoming: Vec<HallEvent>) {
    if !incoming.is_empty() {
        local.extend(incoming);
        local.sort_by(|a, b| a.ts_utc.cmp(&b.ts_utc).then_with(|| a.seq.cmp(&b.seq)));
        local.dedup_by(|a, b| !a.file_id.is_empty() && a.file_id == b.file_id);
    }
    if local.len() > HALL_EVENTS_MAX {
        let excess = local.len() - HALL_EVENTS_MAX;
        local.drain(..excess);
    }
}

impl App {
    /// 消费 /chain 的异步结果并渲染进对话区。
    pub fn poll_chain(&mut self) {
        let Some(mut rx) = self.chain_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => match outcome {
                ChainOutcome::Tasks(Ok(tasks)) => {
                    if tasks.is_empty() {
                        self.add_message(
                            MessageRole::System,
                            "决策链为空：暂无任务文件（$AIRY_HOME/data/agentrt/hall）。"
                                .to_string(),
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
                        text.push_str(&format!("\n  … 另有 {} 个（/chain <task_id> 查看决策链）", tasks.len() - 20));
                    } else {
                        text.push_str("\n  /chain <task_id> 查看决策链");
                    }
                    self.add_message(MessageRole::System, text);
                }
                ChainOutcome::Tasks(Err(e)) => {
                    self.add_message(MessageRole::System, format!("任务列表读取失败：{}", e));
                }
                ChainOutcome::Events(Ok(events)) => {
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
                        text.push_str(&format!("\n  … 另有 {} 条（详见 F7 事件流面板）", events.len() - 64));
                    }
                    self.add_message(MessageRole::System, text);
                }
                ChainOutcome::Events(Err(e)) => {
                    self.add_message(MessageRole::System, format!("决策链读取失败：{}", e));
                }
            },
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.chain_pending = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// 消费运维命令的异步结果并渲染进对话区。
    pub fn poll_ops(&mut self) {
        let Some(mut rx) = self.ops_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => match outcome {
                OpsOutcome::Daemons(results) => {
                    let online = results.iter().filter(|(_, r)| r.is_ok()).count();
                    let mut text = format!(
                        "daemon 在线状态（{} / {} 在线）",
                        online,
                        results.len()
                    );
                    for (ns, r) in results {
                        let (icon, st) = if r.is_ok() { ("✓", "在线") } else { ("✗", "离线") };
                        text.push_str(&format!("\n  {} {} {}", icon, st, ns));
                    }
                    self.add_message(MessageRole::System, text);
                }
                OpsOutcome::Call(Ok(v)) => {
                    let pretty = serde_json::to_string_pretty(&v)
                        .unwrap_or_else(|_| v.to_string());
                    self.add_message(
                        MessageRole::System,
                        format!("{} 结果：\n{}", self.ops_label, pretty),
                    );
                }
                OpsOutcome::Call(Err(e)) => {
                    self.add_message(
                        MessageRole::System,
                        format!("{} 调用失败：{}", self.ops_label, e),
                    );
                }
            },
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.ops_pending = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// hall 面板（看板/事件流）数据拉取：面板激活时刷新。
    ///
    /// 看板 = hall.board（work_hall 持久化实例 + 在线 agent）；
    /// 事件流 = hall.stream（全局 gseq 因果序，最新 512 条）。
    /// 数据经 gateway 统一转发，任何前端看到同一份状态。
    ///
    /// M5 W4 数据源统一：hall.watch SSE 推送内容被真实消费——事件流面板
    /// 增量插入（实时上屏，RPC pull 仅作 1s 兜底合并）；看板是聚合快照，
    /// 推送只作刷新触发（提前 pull），与 RPC 轮询互补。
    pub fn poll_hall(&mut self) {
        if self.active_panel != ActivePanel::Board && self.active_panel != ActivePanel::Events {
            return;
        }
        let want_board = self.active_panel == ActivePanel::Board;
        // hall.watch 推送消费（2026-08-21 事件驱动；M5 W4 内容级消费）
        if let Some(rx) = &mut self.hall_watch_rx {
            if want_board {
                // 看板为聚合快照：任何推送只作刷新触发（跳过节流提前 pull）
                let mut pushed = false;
                while rx.try_recv().is_ok() {
                    pushed = true;
                }
                if pushed {
                    self.last_hall_poll = Instant::now() - std::time::Duration::from_secs(10);
                }
            } else {
                // 事件流：推送内容即事件（与 hall.stream 同形态），增量入列
                let mut incoming: Vec<HallEvent> = Vec::new();
                while let Ok(raw) = rx.try_recv() {
                    if let Ok(evt) = serde_json::from_str::<HallEvent>(&raw) {
                        incoming.push(evt);
                    }
                }
                merge_hall_events(&mut self.hall_events, incoming);
            }
        }
        // 消费在途结果（pull 结果与本地合并去重，避免覆盖 SSE 增量）
        if let Some(mut rx) = self.hall_poll_rx.take() {
            match rx.try_recv() {
                Ok(HallPollOutcome::Board(r)) => match r {
                    Ok(b) => self.hall_board = Some(b),
                    Err(e) => log::warn!("hall.board 拉取失败: {}", e),
                },
                Ok(HallPollOutcome::Events(r)) => match r {
                    Ok(evts) => merge_hall_events(&mut self.hall_events, evts),
                    Err(e) => log::warn!("hall.stream 拉取失败: {}", e),
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    self.hall_poll_rx = Some(rx);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
            }
        }
        // 节流：1s（事件流面板的实时增量由 SSE 驱动，pull 仅兜底）
        let now = Instant::now();
        if now.duration_since(self.last_hall_poll) < std::time::Duration::from_millis(1000) {
            return;
        }
        self.last_hall_poll = now;
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if want_board {
                let _ = tx.send(HallPollOutcome::Board(gw.hall_board().await));
            } else {
                let _ = tx.send(HallPollOutcome::Events(gw.hall_stream(512).await));
            }
        });
        self.hall_poll_rx = Some(rx);
    }

    /// 强制下次 poll_hall 立即拉取（F6/F7 进入面板时调用）。
    pub fn force_hall_refresh(&mut self) {
        self.last_hall_poll = Instant::now() - std::time::Duration::from_secs(10);
    }

    /// 订阅 hall.watch SSE 推送流（2026-08-21 事件流驱动；Board/Events 面板
    /// 激活时调用）。收到任何推送事件 → 立即刷新（跳过 1s 节流），轮询保留
    /// 为断连/离线兜底。
    pub fn start_hall_watch(&mut self) {
        if self.hall_watch_rx.is_none() {
            self.hall_watch_rx = Some(self.gateway.hall_watch_events());
            log::debug!("hall.watch: SSE 推送订阅已启动");
        }
    }

    /// 停止 hall.watch SSE 订阅（离开面板时调用；drop 接收端使 watch 任务退出）。
    pub fn stop_hall_watch(&mut self) {
        if self.hall_watch_rx.take().is_some() {
            log::debug!("hall.watch: SSE 推送订阅已停止");
        }
    }

    /* ---- 2026-08-17：F6/F7 面板交互（光标 + 过滤）---- */

    /// 主循环轮询：后台请求完成则应用结果（返回是否有待办请求，供渲染循环判断）。
    ///
    /// 用户暂停（Ctrl+Z）期间不消费结果——请求继续在网关执行，恢复后结果照常应用。
    /// 流式请求（StreamRound）：先消费 mpsc 增量块实时追加到 streaming_text，
    /// 再检查 oneshot 完整结果（结束信号）。
    pub fn poll_pending(&mut self) -> bool {
        if self.task_control == TaskControl::Paused {
            return true;
        }
        // 审批轮询：请求在途时查询 tool.pending（在借用 self.pending 前调用，
        // 且 poll_approvals 内部自判 pending 是否存在并做 1.5s 节流）
        self.poll_approvals();
        let Some(p) = &mut self.pending else {
            return false;
        };
        // ── 流式增量消费：SSE 块 → streaming_text（chat.rs 逐帧渲染）──
        if let Some(stream_rx) = &mut p.stream_rx {
            let mut got = false;
            while let Ok(chunk) = stream_rx.try_recv() {
                got = true;
                self.streaming_text.push_str(&chunk);
            }
            if got {
                log::trace!("stream chunk: total={} chars", self.streaming_text.len());
            }
        }
        // ── 打字机上屏：伪流式下制造逐字动效（每 tick 推进若干字符）──
        // reveal 只增不减；消费完一轮后再推进，避免与渲染竞争。
        // 字段级操作（非方法调用）：p 已借用 self.pending，避免整体借用冲突。
        {
            let total = self.streaming_text.chars().count();
            if self.streaming_reveal < total {
                let since = self.last_reveal_tick.elapsed().as_millis();
                if since >= 24 {
                    self.last_reveal_tick = Instant::now();
                    // 长文本提速：目标 8s 内上屏完，至少 1 字符/步
                    let speed =
                        (total as f64 / 8000.0 * 24.0).ceil().max(1.0) as usize;
                    self.streaming_reveal = (self.streaming_reveal + speed).min(total);
                }
            }
        }
        // ── 流式工具事件消费：tool_call/tool_result → 工具状态行 ──
        if let Some(tool_rx) = &mut p.tool_rx {
            while let Ok(evt) = tool_rx.try_recv() {
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
                }
                if let Some(line) = App::render_tool_event(&evt) {
                    self.stream_tool_events.push(line);
                }
            }
        }
        // 先检查暂存结果：打字机上屏完成（reveal 追平）才落定
        if let Some(finish) = p.finish.take() {
            if self.streaming_reveal >= self.streaming_text.chars().count() {
                // reveal 追平：取走 kind，清 pending/loading，应用结果
                let kind =
                    std::mem::replace(&mut p.kind, PendingKind::ChatRound { input: String::new() });
                self.pending = None;
                self.loading = false;
                self.set_task_control(TaskControl::Running);
                log::info!("poll_pending: 打字机上屏完成，消费流式结果（kind={:?}）", kind);
                self.last_turn_elapsed = Some(self.turn_started);
                self.apply_result(kind, finish);
                return self.pending.is_some();
            }
            // 尚未追平：放回，下一 tick 继续推进 reveal
            p.finish = Some(finish);
            return true;
        }
        let outcome = match p.rx.try_recv() {
            Ok(o) => o,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return true,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                PendingOutcome::Run(Err(anyhow!("LLM 后台任务异常终止")))
            }
        };
        // 流式请求：结果已到达但打字机尚未上屏完 → 暂存 finish，
        // 等 reveal 追平（下一 tick）再落定，保证逐字动效完整走完
        // （0.1.9 M5 W1：StreamRound 与 agent.run_stream 执行轮均带 streamed）
        let is_stream = p.streamed;
        let reveal_pending = self.streaming_reveal < self.streaming_text.chars().count();
        if is_stream && reveal_pending {
            log::debug!(
                "poll_pending: 流式结果已到，打字机尚未上屏完（{}/{}），暂存",
                self.streaming_reveal,
                self.streaming_text.chars().count()
            );
            p.finish = Some(outcome);
            return true;
        }
        // 取走 kind（避免 move 出借用），先清 pending/loading，再应用结果
        let kind = std::mem::replace(&mut p.kind, PendingKind::ChatRound { input: String::new() });
        self.pending = None;
        self.loading = false;
        self.set_task_control(TaskControl::Running);
        log::info!("poll_pending: 消费后台请求结果（kind={:?}）", kind);
        // 回合计时结算：回合分隔线展示本回合耗时（Worked for Ns）
        self.last_turn_elapsed = Some(self.turn_started);
        self.apply_result(kind, outcome);
        self.pending.is_some()
    }

    /// 审批轮询：后台请求进行中时查询 tool_d pending 审批列表。
    ///
    /// 被静态审批拒绝（如 shell_run）的工具会阻塞等待 tool.approve 决议
    /// （AIRY_TOOL_APPROVAL_MODE=interactive）。此处轮询工具侧 pending，
    /// 有新请求则上屏提示用户按 a/A/n 决议（Claude Code 风格 permission prompt）。
    pub(super) fn poll_approvals(&mut self) {
        // 仅当后台请求进行中（工具执行可能正在等待审批）
        if self.pending.is_none() {
            return;
        }
        // 消费在途轮询结果（上次 spawn 的异步查询）
        if let Some(mut rx) = self.approval_poll_rx.take() {
            match rx.try_recv() {
                Ok(pending_list) => {
                    // 新请求：去重后上屏提示
                    for a in pending_list {
                        if !self.approvals.iter().any(|x| x.request_id == a.request_id) {
                            self.approvals.push(a.clone());
                            self.add_message(
                                MessageRole::System,
                                format!(
                                    "工具「{}」请求权限执行（agent: {}，参数: {}）\n按 A=始终允许 · a=允许本次 · n=拒绝",
                                    a.tool, a.agent_id, truncate_for_display(&a.params, 160)
                                ),
                            );
                            self.add_log("INFO",
                                format!("权限审批待决议: {} ({})", a.tool, a.request_id));
                        }
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    // 尚未返回：存回，下次继续
                    self.approval_poll_rx = Some(rx);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    // 查询任务异常结束：忽略，下个周期重新发起
                }
            }
        }
        // 节流发起新查询：每 1.5s 一次
        let now = std::time::Instant::now();
        if now.duration_since(self.last_approval_poll) < std::time::Duration::from_millis(1500) {
            return;
        }
        self.last_approval_poll = now;
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let list = gw.list_pending_approvals().await.unwrap_or_default();
            let _ = tx.send(list);
        });
        self.approval_poll_rx = Some(rx);
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
            PendingKind::Distill => {
                if let PendingOutcome::Run(res) = outcome {
                    self.apply_distill_result(res);
                }
            }
            PendingKind::CheckConnect { kind, prompt, agent, history, gccp_answers } => {
                match outcome {
                    PendingOutcome::Connect(true) => {
                        // 连接成功：继续执行真实请求（loading 由 start_pending 重新置位）
                        log::info!(
                            "apply_result: 连接检查通过，继续执行请求（kind={:?}）",
                            kind
                        );
                        self.connected = true;
                        self.start_pending(*kind, &prompt, agent, history, gccp_answers);
                    }
                _ => {
                    // 连接检查失败：若正在进行任务收尾（技能蒸馏），仍需结束任务流，
                    // 否则会卡在 Executing 阶段——「技能沉淀」被跳过且无法继续对话。
                    log::warn!("apply_result: 连接检查失败（kind={:?}）", kind);
                    if matches!(&*kind, PendingKind::Distill) {
                        self.add_message(
                            MessageRole::System,
                            "网关不可达，技能蒸馏请求失败：本次任务已完成，但经验未能沉淀。"
                                .to_string(),
                        );
                        self.finish_task(false);
                    } else {
                        self.add_message(
                            MessageRole::System,
                            "Not connected to gateway. Run 'agentrt' to start the server."
                                .to_string(),
                        );
                    }
                }
                }
            },
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
        let mut local = vec![evt("a.20260901T000000000.0001.json", "20260901T000000000", 1, "progress")];
        let incoming = vec![
            evt("b.20260901T000000100.0001.json", "20260901T000000100", 1, "result"),
            evt("c.20260901T000000200.0001.json", "20260901T000000200", 1, "result"),
        ];
        merge_hall_events(&mut local, incoming);
        assert_eq!(local.len(), 3);
        assert_eq!(local[2].file_id, "c.20260901T000000200.0001.json");
    }

    #[test]
    fn merge_dedups_overlap_between_pull_and_sse() {
        /* pull 兜底可能包含 SSE 已推事件：同 file_id 只保留一份 */
        let mut local = vec![
            evt("a.20260901T000000000.0001.json", "20260901T000000000", 1, "progress"),
            evt("b.20260901T000000100.0001.json", "20260901T000000100", 1, "result"),
        ];
        let incoming = vec![
            evt("b.20260901T000000100.0001.json", "20260901T000000100", 1, "result"),
            evt("c.20260901T000000200.0001.json", "20260901T000000200", 1, "issue"),
        ];
        merge_hall_events(&mut local, incoming);
        assert_eq!(local.len(), 3);
    }

    #[test]
    fn merge_sorts_out_of_order_increments() {
        /* SSE 重连回放偶发乱序：按 (ts_utc, seq) 收敛 */
        let mut local = vec![evt("c.20260901T000000200.0001.json", "20260901T000000200", 1, "result")];
        let incoming = vec![evt("a.20260901T000000000.0001.json", "20260901T000000000", 1, "progress")];
        merge_hall_events(&mut local, incoming);
        assert_eq!(local.len(), 2);
        assert!(local[0].ts_utc < local[1].ts_utc);
    }

    #[test]
    fn merge_trims_to_cap_keeping_newest() {
        /* 容量上限：超出保留最新（头部弹出，渲染层期望最新在尾） */
        let mut local: Vec<HallEvent> = Vec::new();
        for i in 0..(HALL_EVENTS_MAX + 50) {
            let ts = format!("20260901T{:06}000", 0);
            local.push(evt(&format!("f{}.json", i), &ts, i as u64, "progress"));
        }
        merge_hall_events(&mut local, Vec::new());
        assert_eq!(local.len(), HALL_EVENTS_MAX);
        // 保留的是最大 seq（最新）
        assert_eq!(local.last().unwrap().seq, (HALL_EVENTS_MAX + 49) as u64);
        assert_eq!(local.first().unwrap().seq, 50);
    }
}
