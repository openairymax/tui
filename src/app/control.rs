// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 运行控制与视图滚动：审批、中止、启动探测、退出。

use super::*;

impl App {
    /// 决议一个审批请求（Claude Code 风格权限确认）。
    ///
    /// `decision` ∈ {"allow", "always", "deny"}。`always` 与 `allow` 一样只
    /// 放行本次：CLI 尚未提供会话级总是允许的帧语义，回显里注明，不假装
    /// 已记住。决议帧直达 CLI 并把本地裁决折进投影；写失败按 fail-closed
    /// 处理——工具侧等不到帧会超时拒绝，界面如实标注。
    pub fn approve_request(&mut self, decision: &str) {
        let Some(a) = self.approvals.first() else {
            self.add_message(
                MessageRole::System,
                "当前没有待决议的权限请求。".to_string(),
            );
            return;
        };
        let request_id = a.request_id.clone();
        let tool = a.tool.clone();
        let allow = decision != "deny";
        if let Some(ev) = self.ev.clone() {
            if let Err(e) = ev.answer(&request_id, allow) {
                self.add_message(
                    MessageRole::System,
                    format!(
                        "审批回送失败，工具「{}」将按超时拒绝（fail-closed）：{}",
                        tool, e
                    ),
                );
                self.add_log(
                    "ERROR",
                    format!("权限决议回送失败: {} ({})", request_id, tool),
                );
                self.approvals.remove(0);
                return;
            }
        }
        self.approvals.remove(0);
        let label = if allow {
            "允许本次执行"
        } else {
            "拒绝"
        };
        let tail = if allow {
            if decision == "always" {
                "（会话级总是允许待 CLI 支持，本次仅放行一次）"
            } else {
                ""
            }
        } else {
            "（rejected：用户拒绝）"
        };
        self.add_message(
            MessageRole::System,
            format!("已{label}工具「{}」{tail}", tool),
        );
        self.add_log(
            "INFO",
            format!("权限决议: {} → {} ({})", request_id, label, tool),
        );
    }

    /// 请求中断（§6.2 #6）：Esc 在等待期先呈递「已请求中断…」，再交由
    /// 中止路径清理本地待办。呈递先于结果，用户不必等后端确认。
    pub fn interrupt_task(&mut self) {
        if self.pending.is_none() {
            return;
        }
        self.add_message(MessageRole::System, "已请求中断…".to_string());
        self.abort_task();
    }

    /// 人工中止当前后台请求（Ctrl+X）：取消事件管道在途轮并清理待办状态。
    ///
    /// 取消帧直达 CLI，引擎侧随 SSE 断流释放运行资源；本地游标与 pending
    /// 立即释放，UI 按请求类型回退阶段，交互不挂死。
    pub fn abort_task(&mut self) {
        // 事件管道在途轮：取消帧直达 CLI 收口，本地游标立即释放，避免
        // 取消后的迟到终态被下一轮误当成本轮结果。
        if self.lane.take().is_some() {
            self.lane_tx = None;
            if let Some(ev) = &self.ev {
                if let Err(e) = ev.cancel() {
                    log::warn!("事件管道取消失败：{e}");
                }
            }
        }
        let mut session_id = String::new();
        if let Some(p) = &mut self.pending {
            if let Some(t) = p.task.take() {
                t.abort();
            }
            session_id = p.session_id.clone();
        }
        let kind = self
            .pending
            .take()
            .map(|p| p.kind)
            .unwrap_or(PendingKind::ChatRound {
                input: String::new(),
            });
        self.loading = false;
        // 中止后保持中止态（状态徽章显示「已中止」）；新交互发起时复位 Running
        self.set_task_control(TaskControl::Aborted);
        log::info!("abort_task: 人工中止（session={}）", session_id);

        // 按请求类型回退阶段，避免停留在等待状态
        match kind {
            PendingKind::GradPlan => {
                // GRAD 生成被中止 → 回到五问齐备后的等待（用户可重发）
                self.set_flow_phase(FlowPhase::GradConfirm);
                self.add_message(
                    MessageRole::System,
                    "流程图生成已中止。输入任意内容可重新生成，或输入「退出」放弃任务。"
                        .to_string(),
                );
            }
            PendingKind::GradConfirm { confirmed } => {
                if confirmed {
                    // 执行轮被中止 → 回到执行阶段，用户可重发指令或宣告完成
                    self.set_flow_phase(FlowPhase::Executing);
                    self.add_message(
                        MessageRole::System,
                        "任务执行已中止。可输入「完成」结束任务，或继续输入指令。".to_string(),
                    );
                } else {
                    self.set_flow_phase(FlowPhase::GradConfirm);
                    self.add_message(MessageRole::System, "流程图修订已中止。".to_string());
                }
            }
            PendingKind::ChatRound { .. } => {
                self.add_message(MessageRole::System, "已中止本次请求。".to_string());
            }
            PendingKind::StreamRound { .. } => {
                self.streaming_text.clear();
                self.stream_reasoning.clear();
                self.add_message(MessageRole::System, "已中止流式回复。".to_string());
            }
            PendingKind::AskGccp { .. } => {
                self.set_flow_phase(FlowPhase::Chat);
                self.add_message(
                    MessageRole::System,
                    "任务事实确认已中止，任务放弃。".to_string(),
                );
                self.task_mode = false;
            }
            PendingKind::TurnState => {
                // 丢弃可能已落地的查询应答，避免残留在查询表里
                let _ = self.take_query("turns.get");
                self.add_message(MessageRole::System, "已中止断点查询。".to_string());
            }
        }
        self.add_log("INFO", "任务已人工中止（Ctrl+X）".to_string());
    }

    /// 标记请求已发出（0.1.18 B2-7）：置 busy 并记录发出时刻。
    ///
    /// 各 dispatch 入口在发起网络请求的同一刻调用，使对话区状态行能在
    /// 首个 busy 帧（主循环 50ms 节拍）即显示「已受理 · N.Ns」——请求
    /// 发出与用户面可见反馈之间不再存在零反馈窗口（V2.3，≤200ms）。
    pub(super) fn begin_busy(&mut self) {
        self.loading = true;
        self.busy_started = Instant::now();
        self.busy_shown_secs = 0.0;
        self.stats_at_ms = 0;
    }

    /// 空闲态退出任务集（Ctrl+X 二次按下 / 空闲时 Ctrl+X）。
    ///
    /// 与 abort_task 的区别：abort 取消的是"在途请求"，任务集状态保留
    /// （可继续输入指令或宣告完成）；exit_task_mode 直接复位任务集状态
    /// 回到普通对话（task_mode=false / Chat / 清理 DAG），供用户在不
    /// 想继续任务时一键退出——此前任务集一旦进入只能通过「完成」收尾，
    /// 没有快捷键出口。
    pub fn exit_task_mode(&mut self) {
        if !self.task_mode {
            return;
        }
        self.task_mode = false;
        self.set_flow_phase(FlowPhase::Chat);
        self.set_task_control(TaskControl::Running);
        // C-14-07：离开任务集即复位生命周期状态与断点令牌
        self.set_task_state(crate::gccp::TaskState::Executing);
        self.resume_token = None;
        self.gccp.dag = None;
        self.gccp.grad_plan.clear();
        self.add_message(
            MessageRole::System,
            "已退出任务集，回到普通对话。".to_string(),
        );
        self.add_log("INFO", "已退出任务集（Ctrl+X 空闲态）".to_string());
    }

    /// 是否有进行中的后台请求（主循环据此进入渲染等待）。
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// 取走待投递的会话完成通知（§6.7 #4）：唯一消费点，取走即清零。
    pub fn take_notice(&mut self) -> Option<crate::app::Notice> {
        self.notice.take()
    }

    /// 启动期连接探测（§6.1 #1）：与首帧并发，ping 帧应答落地，不阻塞首绘。
    pub fn probe_connection(&mut self) {
        if self.boot_q {
            return;
        }
        match self.send_query("boot.ping", "ping", &serde_json::json!({})) {
            Ok(()) => self.boot_q = true,
            Err(e) => self.apply_probe(Err(anyhow!(e))),
        }
    }

    /// 消费启动期连接探测结果：应答到达即热修正连接态。
    pub fn poll_boot(&mut self) {
        if !self.boot_q {
            return;
        }
        match self.take_query("boot.ping") {
            Some(Ok(v)) => {
                self.boot_q = false;
                let parsed = serde_json::from_value::<crate::client::HealthResponse>(v)
                    .map_err(|e| anyhow!("健康应答无法解析：{e}"));
                self.apply_probe(parsed);
            }
            Some(Err(e)) => {
                self.boot_q = false;
                self.apply_probe(Err(anyhow!(e)));
            }
            None if !self.pipe_alive() => {
                self.boot_q = false;
                self.apply_probe(Err(anyhow!("事件管道不可用")));
            }
            None => {}
        }
    }

    /// C-19：拉取槽位 effective 快照（与首帧并发，静默不占对话区）。
    ///
    /// 徽标需要「角色 → 实际 model_id」的权威映射，只能来自 llm_d
    /// （model.yaml + env 归并后的结果）；TUI 自己读 model.yaml 会漏掉
    /// env 覆盖与空槽位回落，故统一走 `llm.slots_get`。
    pub fn probe_slots(&mut self) {
        if self.slots_q {
            return;
        }
        match self.send_query("slots.get", "llm.slots_get", &serde_json::json!({})) {
            Ok(()) => self.slots_q = true,
            Err(e) => log::debug!("slots_get 下发失败，徽标退回通用标签: {e}"),
        }
    }

    /// 消费槽位快照；失败静默丢弃（daemon 未起时徽标退回通用标签）。
    pub fn poll_slots(&mut self) {
        if !self.slots_q {
            return;
        }
        match self.take_query("slots.get") {
            Some(Ok(v)) => {
                self.slots_q = false;
                self.absorb_slots(&v);
            }
            Some(Err(e)) => {
                self.slots_q = false;
                log::debug!("slots_get 失败，徽标退回通用标签: {e}");
            }
            None if !self.pipe_alive() => self.slots_q = false,
            None => {}
        }
    }

    /// 吸收 `slots` 对象（slots_get / list_models 同形：角色 → {declared,effective}）。
    pub(crate) fn absorb_slots(&mut self, v: &serde_json::Value) {
        let Some(slots) = v.get("slots").and_then(|s| s.as_object()) else {
            return;
        };
        for (i, role) in ["t2", "t1f", "t1p"].iter().enumerate() {
            if let Some(m) = slots
                .get(*role)
                .and_then(|o| o.get("effective"))
                .and_then(|m| m.as_str())
            {
                self.slot_models[i] = m.to_string();
            }
        }
        self.slots_degraded = v.get("degraded").and_then(|d| d.as_bool()).unwrap_or(false);
    }

    /// 应用探测结果：连接态/版本/状态条/日志的唯一改写落点。
    ///
    /// 同一份真实探测同时回填健康表（U12）：连接态与健康表同源，不会出现
    /// 「状态条已连接、健康面板显示不可达」的自相矛盾。
    pub(super) fn apply_probe(&mut self, result: Result<crate::client::HealthResponse>) {
        let ok = result.is_ok();
        let mut err: Option<String> = None;
        match result {
            Ok(health) => {
                self.connected = true;
                self.gateway_version = health.version.clone();
                let ver = health.version.as_deref().unwrap_or(env!("AIRY_RT_VERSION"));
                self.status_message = format!("Connected to AgentRT v{ver}");
                self.add_log("INFO", format!("已连接网关 v{ver}"));
            }
            Err(e) => {
                self.connected = false;
                self.gateway_version = None;
                self.status_message = format!("Gateway unreachable: {}", e);
                self.add_log("ERROR", format!("网关不可达：{}", e));
                err = Some(e.to_string());
            }
        }
        self.note_dep("gateway", ok, self.clock_ms, err.as_deref());
        let resynced = self
            .health
            .deps()
            .iter()
            .filter(|d| d.up)
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        self.link.observe(ok, resynced);
        self.sync_banner(self.clock_ms);
        self.mark_landed(Lane::Poll);
    }

    /// 对话滚动（0.1.18 B11/W14 滚动契约）：偏移钳位到可滚总量
    /// `chat_scroll_max`（渲染每帧回写）。内容未超出视口时该值为 0，
    /// 滚动为 no-op——消除"按键无反馈"的脏偏移积累（V11.3）。
    pub fn scroll_up(&mut self) {
        self.scroll_offset = (self.scroll_offset + 1).min(self.chat_scroll_max);
    }

    /// 对话下滚：同上钳位，0 封底（最新消息）。
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// 上翻一页：步长 = `page_step`（= 当前视口高度，渲染每帧回写，
    /// resize 自动跟随——固定常量翻页已废除，V11.2）。
    pub fn scroll_page_up(&mut self) {
        self.scroll_offset = (self.scroll_offset + self.page_step).min(self.chat_scroll_max);
    }

    /// 下翻一页：步长同上。
    pub fn scroll_page_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.page_step);
    }

    /// 上半页（X5-09）：步长取当前视口页高的一半，resize 后自动跟随；
    /// 视口极矮时至少 1 行，避免「按了没反应」。
    pub fn scroll_half_up(&mut self) {
        let step = (self.page_step / 2).max(1);
        self.scroll_offset = (self.scroll_offset + step).min(self.chat_scroll_max);
    }

    /// 下半页：步长同上。
    pub fn scroll_half_down(&mut self) {
        let step = (self.page_step / 2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(step);
    }

    /// 统一动作落地（X5-08）：键盘与鼠标解析出同一 `Action` 后只走此处，
    /// 两侧不存在各自的副作用副本。
    pub(crate) fn apply_action(&mut self, action: crate::engine::action::Action) {
        use crate::engine::action::Action;
        match action {
            Action::Allow => self.approve_request("allow"),
            Action::AllowAlways => self.approve_request("always"),
            Action::Deny => self.approve_request("deny"),
            Action::HalfUp => self.scroll_half_up(),
            Action::HalfDown => self.scroll_half_down(),
            Action::MouseToggle => {
                let on = self.toggle_mouse_capture();
                log::info!("Mouse capture: {}", if on { "ON" } else { "OFF" });
            }
            Action::KeymapToggle => self.toggle_keymap(),
        }
    }

    /// 键位模式互切（X5-03b）：只改导航绑定偏好，输入与审批语义不变。
    pub(crate) fn toggle_keymap(&mut self) {
        self.keymap = self.keymap.toggle();
        let name = self.keymap.name();
        log::info!("Keymap mode: {name}");
        self.toast(format!("键位模式：{name}"));
    }

    /// 工具卡逐条展开/收起（X5-11a）：Alt+O 的全局展开不受影响，点击只翻
    /// 目标卡；行高缓存按逐条展开态记账，无需整表失效。
    pub(crate) fn toggle_card(&mut self, id: u64) {
        if !self.expanded_cards.remove(&id) {
            self.expanded_cards.insert(id);
        }
    }

    /// 导航意图落地（X5-03a）：模式表只描述意图，副作用只在此处发生。
    pub(crate) fn apply_nav(&mut self, nav: crate::engine::keymap::Nav) {
        use crate::engine::keymap::Nav;
        match nav {
            Nav::LineUp => self.scroll_up(),
            Nav::LineDown => self.scroll_down(),
            Nav::HalfUp => self.scroll_half_up(),
            Nav::HalfDown => self.scroll_half_down(),
            Nav::PageUp => self.scroll_page_up(),
            Nav::PageDown => self.scroll_page_down(),
            Nav::Top => self.scroll_top(),
            Nav::Bottom => self.scroll_bottom(),
        }
    }

    /// 视口滚动到顶（Alt+Home）：偏移 = 可滚总量。
    /// Home 无修饰键语义为输入光标行首（readline 惯例，不在此改写）。
    pub fn scroll_top(&mut self) {
        self.scroll_offset = self.chat_scroll_max;
    }

    /// 视口滚动到底（Alt+End）：偏移归零（最新消息）。
    pub fn scroll_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// 观测对话总量变化并驱动底部跟随浮标（§6.2 #5）。渲染每帧调用：
    /// 仅在脱离底部（scroll_offset > 0）时累计净新增行，回到未脱离态即
    /// 清零；宽度变化视为重排，只重置基线不计数。浮标值按发布节拍
    /// 节流，避免逐帧抖动导致文案不可读。
    pub fn follow_observe(&mut self, total: usize, width: u16) {
        let reflow = width != self.last_width;
        self.last_width = width;
        if self.scroll_offset > 0 && !reflow {
            let grow = total.saturating_sub(self.last_total);
            self.follow_new = self.follow_new.saturating_add(grow);
            // 锚点联动（X5-09b）：上滚阅读时新增行只落在底部，距底偏移
            // 同步增长才能让视口里的文字原地不动；重排（宽度变化）不算
            // 增长，否则一次 resize 会把读者甩到别处。
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(grow.min(u16::MAX as usize) as u16);
        }
        self.last_total = total;
        if self.scroll_offset == 0 {
            self.follow_new = 0;
            self.follow_shown = 0;
            self.follow_at_ms = self.clock_ms;
            return;
        }
        if self.clock_ms.saturating_sub(self.follow_at_ms)
            >= crate::budget::DEFAULT_LIVE_PUBLISH_INTERVAL_MS
        {
            self.follow_at_ms = self.clock_ms;
            self.follow_shown = self.follow_new;
        }
    }

    /// 底部跟随浮标（§6.2 #5）：脱离底部时返回节流后的新增行数，
    /// 未脱离底部返回 None（不呈递浮标）。
    pub fn follow_badge(&self) -> Option<usize> {
        if self.scroll_offset > 0 {
            Some(self.follow_shown)
        } else {
            None
        }
    }

    /// 鼠标滚轮捕获会话级切换（Ctrl+M）：返回切换后的状态，调用方据此
    /// 执行 Enable/DisableMouseCapture 终端序列。默认关（不牺牲终端
    /// 原生文本选择，V11.1）。
    pub fn toggle_mouse_capture(&mut self) -> bool {
        self.mouse_capture = !self.mouse_capture;
        self.mouse_capture
    }

    /// 鼠标滚轮一行滚动量：Shift 加速 / Ctrl 细粒度（W14）。
    pub fn wheel_lines(&self, shift: bool, ctrl: bool) -> u16 {
        if ctrl {
            1
        } else if shift {
            self.page_step
        } else {
            3
        }
    }

    /// 鼠标滚轮上滚：普通滚动与翻页共用钳位契约。
    pub fn wheel_up(&mut self, lines: u16) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.chat_scroll_max);
    }

    /// 鼠标滚轮下滚。
    pub fn wheel_down(&mut self, lines: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// 焦点视图（Alt+F）：快照最近一条回复进入全屏只读视图。无回复时
    /// 提示且不切换。快照与滚动独立于对话区，Esc 退出恢复原视口。
    pub fn focus_open(&mut self) {
        let Some(m) = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Agent)
        else {
            self.add_message(MessageRole::System, "暂无可全屏查看的回复。".to_string());
            return;
        };
        self.focus_msg = Some(m.clone());
        self.focus_scroll = 0;
        self.active_panel = ActivePanel::Focus;
    }

    /// 焦点视图滚动（钳位到 `focus_scroll_max`，渲染每帧回写）。
    pub fn focus_scroll_up(&mut self) {
        self.focus_scroll = (self.focus_scroll + 1).min(self.focus_scroll_max);
    }

    /// 焦点视图下滚。
    pub fn focus_scroll_down(&mut self) {
        self.focus_scroll = self.focus_scroll.saturating_sub(1);
    }

    /// 焦点视图上翻一页（步长 = 视口高度 SSoT）。
    pub fn focus_page_up(&mut self) {
        let step = self.page_step as usize;
        self.focus_scroll = (self.focus_scroll + step).min(self.focus_scroll_max);
    }

    /// 焦点视图下翻一页。
    pub fn focus_page_down(&mut self) {
        self.focus_scroll = self.focus_scroll.saturating_sub(self.page_step as usize);
    }

    /// 全量会话视图（X8-04a）：Ctrl+O 切换。每次打开都回到最新（底部），
    /// 关闭后对话区视口原样保留——两条滚动路径互不写对方的偏移。
    pub fn toggle_transcript(&mut self) {
        self.transcript_open = !self.transcript_open;
        if self.transcript_open {
            self.transcript_scroll = 0;
        }
    }

    /// 关闭全量会话视图（Esc / Ctrl+O 再按）；检索随之收束。
    pub fn close_transcript(&mut self) {
        self.transcript_open = false;
        self.search.close();
    }

    /// 全量会话视图上滚一行（钳位到 `transcript_scroll_max`，渲染每帧回写）。
    pub fn transcript_scroll_up(&mut self) {
        self.transcript_scroll = (self.transcript_scroll + 1).min(self.transcript_scroll_max);
    }

    /// 全量会话视图下滚一行，0 封底（最新）。
    pub fn transcript_scroll_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
    }

    /// 全量会话视图上翻一页（步长 = 视口高度 SSoT）。
    pub fn transcript_page_up(&mut self) {
        self.transcript_scroll =
            (self.transcript_scroll + self.page_step).min(self.transcript_scroll_max);
    }

    /// 全量会话视图下翻一页。
    pub fn transcript_page_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(self.page_step);
    }

    /// 全量会话视图直达最旧一条。
    pub fn transcript_top(&mut self) {
        self.transcript_scroll = self.transcript_scroll_max;
    }

    /// 全量会话视图直达最新一条。
    pub fn transcript_bottom(&mut self) {
        self.transcript_scroll = 0;
    }

    /// 全量会话视图按行滚动（滚轮步长由控制面给出：默认 3 行、Ctrl 1 行、
    /// Shift 一页），与键盘滚动共用同一钳位。
    pub fn transcript_wheel(&mut self, up: bool, lines: u16) {
        self.transcript_scroll = if up {
            self.transcript_scroll
                .saturating_add(lines)
                .min(self.transcript_scroll_max)
        } else {
            self.transcript_scroll.saturating_sub(lines)
        };
    }

    /// 打开全文检索（X8-04b）：只在全文会话视图内可用，打开即清空上次查询。
    pub fn open_search(&mut self) {
        if self.transcript_open {
            self.search.open();
        }
    }

    /// 关闭检索框（全文会话视图保持不动）。
    pub fn close_search(&mut self) {
        self.search.close();
    }

    /// 查询串追加一个字符并立即跳向首个命中。
    pub fn search_push(&mut self, c: char) {
        self.search.push(c);
        self.search_jump();
    }

    /// 查询串退格并跟随新的首个命中。
    pub fn search_pop(&mut self) {
        self.search.pop();
        self.search_jump();
    }

    /// 跳到下一条命中并滚到该行（循环）。
    pub fn search_next(&mut self) {
        self.search.next();
        self.search_jump();
    }

    /// 跳到上一条命中并滚到该行（循环）。
    pub fn search_prev(&mut self) {
        self.search.prev();
        self.search_jump();
    }

    /// 把当前命中行滚到视口顶端：命中行号自顶计数，距底偏移与之互补。
    /// `transcript_scroll_max` 由渲染每帧回写，超出即钳位到底。
    pub fn search_jump(&mut self) {
        if let Some(line) = self.search.anchor() {
            let line = line.min(self.transcript_scroll_max as u32) as u16;
            self.transcript_scroll = self.transcript_scroll_max.saturating_sub(line);
        }
    }

    /// F3 日志面板：向更早方向滚一条（距最新的条目偏移 +1）。
    pub fn logs_scroll_older(&mut self) {
        self.logs_scroll = self.logs_scroll.saturating_add(1);
    }

    /// F3 日志面板：向最新方向滚一条（偏移减 1，0 封底）。
    pub fn logs_scroll_newer(&mut self) {
        self.logs_scroll = self.logs_scroll.saturating_sub(1);
    }

    /// 思考链视图：向更早方向滚一行（正文首行下标 +1，顶部为上界由渲染钳位）。
    pub fn think_scroll_up(&mut self) {
        self.think_scroll = self.think_scroll.saturating_add(1);
    }

    /// 思考链视图：向更新方向滚一行（下标减 1，0 封底）。
    pub fn think_scroll_down(&mut self) {
        self.think_scroll = self.think_scroll.saturating_sub(1);
    }

    /// 思考链视图：上翻一页（与 chat 的 PgUp 步长一致，10 行）。
    pub fn think_page_up(&mut self) {
        self.think_scroll = self.think_scroll.saturating_add(10);
    }

    /// 思考链视图：下翻一页。
    pub fn think_page_down(&mut self) {
        self.think_scroll = self.think_scroll.saturating_sub(10);
    }

    /// Ctrl+C 退出确认（0.1.19 L7 U2）：首次按下只提示，窗口内二次按下
    /// 才返回 true；空闲态与 busy 态共用本入口。`now` 由调用方注入，
    /// 便于窗口边界的确定性单测。
    pub fn confirm_exit(&mut self, now: Instant) -> bool {
        if self.exit_guard.confirm(now) {
            return true;
        }
        self.add_message(MessageRole::System, "再按一次 Ctrl+C 退出".to_string());
        false
    }

    /// Shutdown gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.persist_tabs();
        if self.connected {
            self.add_message(MessageRole::System, "Shutting down...".to_string());
        }
        Ok(())
    }

    /// D-11：token 计数的界面唯一出口。usage.total 直接显示，不做换算；
    /// 有任一来源为估算值时加 `~` 前缀，避免把估算当成精确账。
    pub fn tok_text(&self) -> String {
        if self.tokens_est {
            format!("~{}", self.tokens)
        } else {
            self.tokens.to_string()
        }
    }

    /// 单轮 token 入账（D-11）：优先 usage.total（provider 同源）；旧引擎
    /// 无 usage 时回退 tokens_used（use_ticks），估算来源做粘滞标记。
    pub fn acc_tokens(&mut self, r: &crate::client::RunResponse) {
        match r.usage.as_ref() {
            Some(u) => {
                self.tokens += u.total;
                self.tokens_est |= u.estimated();
            }
            None => {
                if let Some(t) = r.tokens_used {
                    self.tokens += t;
                }
            }
        }
    }

    /// Get elapsed time as formatted string.
    pub fn elapsed_time(&self) -> String {
        let elapsed = self.session_start.elapsed();
        let seconds = elapsed.as_secs();
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let secs = seconds % 60;

        if hours > 0 {
            format!("{}h{:02}m{:02}s", hours, minutes, secs)
        } else {
            format!("{:02}m{:02}s", minutes, secs)
        }
    }
}
