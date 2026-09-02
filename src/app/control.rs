// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 运行控制与视图滚动：审批、暂停/恢复/中止、连接检查、退出。

use super::*;

impl App {
    /// 决议一个审批请求（Claude Code 风格权限确认）。
    ///
    /// `decision` ∈ {"allow", "always", "deny"}。决议成功后从待办列表移除
    /// 并回显结果；失败（已超时/已决议）提示用户。
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
        let gw = self.gateway.clone();
        let decision = decision.to_string();
        let mut approvals = std::mem::take(&mut self.approvals);
        approvals.remove(0);
        self.approvals = approvals;
        let label = match decision.as_str() {
            "always" => "始终允许",
            "allow" => "允许本次",
            _ => "拒绝",
        };
        self.add_message(MessageRole::System, format!("已{label}工具「{}」", tool));
        self.add_log("INFO", format!("权限决议: {} → {} ({})", request_id, label, tool));
        tokio::spawn(async move {
            if let Err(e) = gw.resolve_approval(&request_id, &decision).await {
                log::warn!("tool.approve 请求失败 ({}): {}", request_id, e);
            }
        });
    }

    /// 人工中止当前后台请求（Ctrl+X）：取消 tokio 任务并清理待办状态。
    ///
    /// 客户端侧取消 tokio 任务并恢复 UI（阶段按请求类型回退，交互不挂死）；
    /// 服务端通过 gateway agent.cancel 中止运行中的 agent.run（资源释放）。
    pub fn abort_task(&mut self) {
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
            .unwrap_or(PendingKind::ChatRound { input: String::new() });
        self.loading = false;
        // 中止后保持中止态（状态徽章显示「已中止」）；新交互发起时复位 Running
        self.set_task_control(TaskControl::Aborted);
        log::info!("abort_task: 人工中止（session={}）", session_id);

        // 服务端取消：凭预分配 session_id 中止 gateway 运行中请求（尽力而为）
        if !session_id.is_empty() {
            let gw = self.gateway.clone();
            tokio::spawn(async move {
                if let Err(e) = gw.cancel_session(&session_id).await {
                    log::debug!("agent.cancel request failed (session={}): {}", session_id, e);
                }
            });
        }

        // 按请求类型回退阶段，避免停留在等待状态
        match kind {
            PendingKind::GradPlan => {
                // GRAD 生成被中止 → 回到五问齐备后的等待（用户可重发）
                self.set_flow_phase(FlowPhase::GradConfirm);
                self.add_message(
                    MessageRole::System,
                    "流程图生成已中止。输入任意内容可重新生成，或输入「退出」放弃任务。".to_string(),
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
                self.streaming_reveal = 0;
                self.stream_reasoning.clear();
                self.add_message(MessageRole::System, "已中止流式回复。".to_string());
            }
            PendingKind::AskGccp { .. } => {
                self.set_flow_phase(FlowPhase::Chat);
                self.add_message(MessageRole::System, "任务事实确认已中止，任务放弃。".to_string());
                self.task_mode = false;
            }
            PendingKind::Distill => {
                self.finish_task(false);
            }
            PendingKind::CheckConnect { .. } => {
                self.add_message(MessageRole::System, "已中止连接等待。".to_string());
            }
        }
        self.add_log("INFO", "任务已人工中止（Ctrl+X）".to_string());
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
        self.gccp.dag = None;
        self.gccp.grad_plan.clear();
        self.add_message(
            MessageRole::System,
            "已退出任务集，回到普通对话。".to_string(),
        );
        self.add_log("INFO", "已退出任务集（Ctrl+X 空闲态）".to_string());
    }

    /// 暂停后台请求轮询（Ctrl+Z）：冻结等待渲染，请求继续在网关执行。
    pub fn pause_task(&mut self) {
        if self.pending.is_some() {
            self.set_task_control(TaskControl::Paused);
            log::info!("pause_task: 已暂停（Ctrl+Z，请求仍在后台执行）");
            self.add_message(
                MessageRole::System,
                "⏸ 已暂停等待（Ctrl+Z 恢复，Ctrl+X 中止）。请求仍在后台执行。".to_string(),
            );
            self.add_log("INFO", "任务已暂停（Ctrl+Z）".to_string());
        }
    }

    /// 恢复暂停（Ctrl+Z）。
    pub fn resume_task(&mut self) {
        if self.task_control == TaskControl::Paused {
            self.set_task_control(TaskControl::Running);
            log::info!("resume_task: 已恢复等待（Ctrl+Z）");
            self.add_message(
                MessageRole::System,
                "▶ 已恢复，继续等待回复。".to_string(),
            );
            self.add_log("INFO", "任务已恢复（Ctrl+Z）".to_string());
        }
    }

    /// 是否有进行中的后台请求（主循环据此进入渲染等待）。
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Check gateway connection.
    pub async fn check_connection(&mut self) -> Result<()> {
        match self.gateway.health_check().await {
            Ok(health) => {
                self.connected = true;
                self.gateway_version = health.version.clone();
                let ver = health.version.as_deref().unwrap_or(env!("AIRY_RT_VERSION"));
                self.status_message = format!("Connected to AgentRT v{ver}");
                self.add_log("INFO", format!("已连接网关 v{ver}"));
            }
            Err(e) => {
                self.connected = false;
                self.status_message = format!("Gateway unreachable: {}", e);
                self.add_log("ERROR", format!("网关不可达：{}", e));
            }
        }
        Ok(())
    }

    /// Scroll up in chat.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll down in chat.
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll up one page.
    pub fn scroll_page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(10);
    }

    /// Scroll down one page.
    pub fn scroll_page_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(10);
    }

    /// Shutdown gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        if self.connected {
            self.add_message(MessageRole::System, "Shutting down...".to_string());
        }
        Ok(())
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
