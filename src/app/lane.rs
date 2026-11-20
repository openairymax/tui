// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件管道运行轮：TUI 经 airy_cli 事件流下发运行、消费投影增量，终态并入
// 既有 pending 落定通道——落定后的行为与网关直连完全同一条 apply 路径，
// 不因事件源切换而分叉。

use serde_json::{json, Value};

use crate::client::event::EventClient;
use crate::client::RunResponse;
use crate::engine::event::pipe::Spec;
use crate::engine::project::{msgs::Kind, tools::Status, Phase};

use super::*;

/// 在途轮：起始水位 + 已消费游标（认知行 / 工具行状态）。
///
/// 投影跨轮累积，游标只记"本轮已上屏到哪里"；水位用于区分上一轮的终态
/// 与本轮的新终态——否则新轮下发后、run_start 到达前会把旧终态当成本轮结果。
pub(super) struct LaneTurn {
    base: u64,
    msgs: usize,
    tools: Vec<Status>,
    final_text: String,
}

impl App {
    /// 挂接事件管道（启动期一次）。失败不静默：保留原因供诊断，运行轮仍走
    /// 网关直连——事件源不可用不应让整机不可用。
    pub fn attach_lane(&mut self) -> bool {
        match EventClient::start(&Spec::cli()) {
            Ok(ev) => {
                log::info!("事件管道已挂接（pid={}）", ev.pid());
                self.ev = Some(ev);
                self.lane_err = None;
                true
            }
            Err(e) => {
                log::warn!("事件管道挂接失败：{e}");
                self.lane_err = Some(e.to_string());
                self.ev = None;
                false
            }
        }
    }

    /// 经事件管道发起一轮：`opts` 与流式入口的参数同构（goal/mode 由 CLI
    /// 赋值，调用方不重复给）。返回 false 表示本轮回退网关直连，原因已记录。
    pub(super) fn start_lane_turn(&mut self, kind: PendingKind, prompt: &str, opts: Value) -> bool {
        let Some(ev) = self.ev.as_ref() else {
            return false;
        };
        if !ev.alive() {
            self.lane_err = Some("事件管道已退出".to_string());
            self.ev = None;
            log::warn!("事件管道已退出，本轮回退网关直连");
            return false;
        }
        let start = ev.snapshot();
        let base = start.seq();
        let msgs = start.msgs().rows().len();
        let tools: Vec<Status> = start.tools().rows().iter().map(|r| r.status).collect();
        let sent = ev.prompt(prompt, Some(&opts));
        if let Err(e) = sent {
            self.lane_err = Some(e.to_string());
            self.ev = None;
            log::warn!("事件管道下发失败：{e}，本轮回退网关直连");
            return false;
        }
        let sid = opts
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending = Some(PendingTurn {
            rx,
            kind,
            task: None,
            session_id: sid,
            stream_rx: None,
            tool_rx: None,
        });
        self.lane = Some(LaneTurn {
            base,
            msgs,
            tools,
            final_text: String::new(),
        });
        self.lane_tx = Some(tx);
        self.begin_busy();
        self.set_task_control(TaskControl::Running);
        true
    }

    /// 消费事件投影：增量上屏、工具行收尾、终态并入 pending 落定通道。
    pub fn poll_lane(&mut self) {
        let Some(mut t) = self.lane.take() else {
            return;
        };
        let Some(ev) = self.ev.as_ref() else {
            self.settle_lane(Err(anyhow!("事件管道已断开")));
            return;
        };
        if !ev.alive() {
            self.lane_err = Some("事件管道已退出".to_string());
            self.ev = None;
            self.settle_lane(Err(anyhow!("事件管道已退出")));
            return;
        }
        let snap = ev.snapshot();
        self.absorb_rows(&mut t, &snap);
        self.absorb_tools(&mut t, &snap);
        // 水位未推进说明本轮还没开始：上一轮的终态不属于本轮，不得提前落定
        if snap.seq() <= t.base {
            self.lane = Some(t);
            return;
        }
        let res = match (snap.terminal(), snap.fault()) {
            (Some(raw), _) => lane_result(raw, &t.final_text),
            (None, Some(f)) => Err(anyhow!("{}", f.text)),
            (None, None) if snap.phase() == Phase::Closed => {
                let mut v = json!({});
                if !t.final_text.is_empty() {
                    v["response"] = json!(t.final_text);
                }
                lane_result(&v.to_string(), &t.final_text)
            }
            _ => {
                self.lane = Some(t);
                return;
            }
        };
        self.settle_lane(res);
    }

    /// 认知增量：正文/思考链按到达顺序追加，终局消息留作数据缺省时的回填。
    fn absorb_rows(&mut self, t: &mut LaneTurn, snap: &crate::engine::project::Projection) {
        let rows = snap.msgs().rows();
        for m in &rows[t.msgs..] {
            match m.kind {
                Kind::Delta => self.streaming_text.push_str(&m.text),
                Kind::Think => self.stream_reasoning.push_str(&m.text),
                Kind::Final => t.final_text = m.text.clone(),
                _ => {}
            }
        }
        t.msgs = rows.len();
    }

    /// 工具行：新行上屏调用行，状态迁移到收尾时补一行结果（与原 SSE 口径一致）。
    fn absorb_tools(&mut self, t: &mut LaneTurn, snap: &crate::engine::project::Projection) {
        let rows = snap.tools().rows();
        for (i, row) in rows.iter().enumerate() {
            let prev = t.tools.get(i).copied();
            if prev.is_none() {
                self.stream_tool_events
                    .push(crate::app::toolcat::event_line(&row.name, &row.args, true));
                t.tools.push(row.status);
                continue;
            }
            if prev != Some(row.status) && row.status != Status::Running {
                let label = crate::app::toolcat::label(&row.name);
                let line = if row.status == Status::Ok {
                    format!("{label} 完成")
                } else {
                    format!("{label}（失败）{}", first_err(&row.text))
                };
                self.stream_tool_events.push(line);
                t.tools[i] = row.status;
            }
        }
    }

    /// 终态落定：经 pending 通道交给既有 apply 路径，轮状态一并释放。
    fn settle_lane(&mut self, res: Result<RunResponse>) {
        self.lane = None;
        self.mark_landed(Lane::Stream);
        if let Some(tx) = self.lane_tx.take() {
            let _ = tx.send(PendingOutcome::Run(res));
        } else if let Some(p) = self.pending.take() {
            // 通道缺失（启动期异常）也得把本轮应用掉，不能让 loading 悬死
            self.apply_result(p.kind, PendingOutcome::Run(res));
        } else {
            self.loading = false;
        }
    }
}

/// 终态载荷 → 结构化结果：缺 response 时用流式终局文本回填，缺状态按完成计。
fn lane_result(raw: &str, final_text: &str) -> Result<RunResponse> {
    let mut v: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
    if v.get("response").is_none() && !final_text.is_empty() {
        v["response"] = json!(final_text);
    }
    serde_json::from_value(v).map_err(|e| anyhow!("终态载荷无法解析：{e}"))
}

/// 失败摘要首行（≤80 字符），与流式工具事件同一口径。
fn first_err(text: &str) -> String {
    let line: String = text.lines().next().unwrap_or("").chars().take(80).collect();
    if line.is_empty() {
        String::new()
    } else {
        format!(" · {line}")
    }
}
