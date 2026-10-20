// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 降级态编排（0.1.19 U12：TUI 建设方案 §6.12 / §6.13 的接线层）。
//
// 分层：本文件属应用编排层——只做真实探测的调度与结果转交，投影与状态机
// 全部在 engine::health（L1 纯数据）。时刻一律由主循环注入，避免应用层
// 自读时钟导致单测无法确定性重放。
//
// 数据真实性：探测只有两个真实来源——gateway GET /health 与
// {ns}.health_check RPC（经 gateway FWD，代表端到端可达）。探测失败按原样
// 入日志与错误聚合，不合成事件、不猜测成因。

use std::time::Duration;

use crate::engine::health::{diag_report, BannerAct, Change, UNREACH};
use crate::engine::sched::Lane;
use crate::paths::airy_home_path;

use super::*;

/// 单依赖探测结果。`err` 为原始错误串，只进日志，不上屏。
pub(crate) struct Probe {
    pub(crate) name: String,
    pub(crate) ok: bool,
    pub(crate) err: String,
}

/// 单依赖探测超时：与 gateway health_check 的 2s 口径一致。
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 健康面板翻页步长（行）。
const HEALTH_PAGE: usize = 10;

impl App {
    /// 注入主循环单调时刻并刷新时间线（墙钟/单调钟配对，回拨按单调序修正）。
    pub fn set_clock(&mut self, now_ms: u64) {
        self.clock_ms = now_ms;
        self.stamp = self.timeline.observe(wall_ms(), now_ms);
    }

    /// 启动一轮健康探测。`full` 为真时覆盖全部业务 daemon（/health 面板），
    /// 否则只探 LLM 后端（5s 节拍的轻量保活）。
    ///
    /// gateway 自身不可达时短路：后续 daemon 探测必然全部失败，逐项上报只会
    /// 制造一屏「不可达」噪声，真实根因只有一个——网关。
    pub fn start_health(&mut self, full: bool) {
        if self.health_rx.is_some() {
            return;
        }
        let gw = self.gateway.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if let Err(e) = gw.health_check().await {
                let _ = tx.send(vec![Probe {
                    name: "gateway".to_string(),
                    ok: false,
                    err: e.to_string(),
                }]);
                return;
            }
            let names: Vec<&str> = if full {
                OPS_DAEMON_NS.to_vec()
            } else {
                vec!["llm"]
            };
            // 并发探测：总耗时约一个超时周期，不随依赖数线性放大。
            let mut tasks = Vec::with_capacity(names.len());
            for ns in names {
                let gw = gw.clone();
                tasks.push(tokio::spawn(async move {
                    let r = tokio::time::timeout(
                        PROBE_TIMEOUT,
                        gw.rpc_call(&format!("{ns}.health_check"), serde_json::json!({})),
                    )
                    .await;
                    let (ok, err) = match r {
                        Ok(Ok(_)) => (true, String::new()),
                        Ok(Err(e)) => (false, e.to_string()),
                        Err(_) => (false, format!("探测超时（{}s）", PROBE_TIMEOUT.as_secs())),
                    };
                    Probe {
                        name: ns.to_string(),
                        ok,
                        err,
                    }
                }));
            }
            let mut out = Vec::with_capacity(tasks.len() + 1);
            out.push(Probe {
                name: "gateway".to_string(),
                ok: true,
                err: String::new(),
            });
            for t in tasks {
                // JoinError 说明任务被取消：其结果未知，保持健康表原状，
                // 不用「失败」冒充未知。
                if let Ok(p) = t.await {
                    out.push(p);
                }
            }
            let _ = tx.send(out);
        });
        self.health_rx = Some(rx);
    }

    /// 消费探测结果：逐项落健康表与错误聚合，最后统一同步一次横幅
    /// （批量探测只触发一次横幅裁决，避免同批内多次抖动计数）。
    pub fn poll_health(&mut self, now_ms: u64) {
        let Some(mut rx) = self.health_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(probes) => {
                for p in probes {
                    self.note_dep(&p.name, p.ok, now_ms, Some(&p.err));
                }
                self.sync_banner(now_ms);
                self.mark_landed(Lane::Poll);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.health_rx = Some(rx);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    /// 吸收一次真实探测：状态迁移与错误聚合的唯一落点。
    ///
    /// 只在「由可达转为不可达」时记错误与原始日志——持续不可达由同一份
    /// 状态持续呈现，5s 重复刷屏没有信息增量。
    pub(crate) fn note_dep(&mut self, name: &str, ok: bool, now_ms: u64, err: Option<&str>) {
        if self.health.observe(name, ok, now_ms) != Change::Down {
            return;
        }
        self.err_agg.record_cause(UNREACH, now_ms);
        let detail = err.unwrap_or("").trim();
        let label = dep_cn(name);
        if detail.is_empty() {
            self.add_log("WARN", format!("{label}（{name}）不可达"));
        } else {
            self.add_log("WARN", format!("{label}（{name}）不可达：{detail}"));
        }
    }

    /// 按健康表最重依赖同步降级横幅（§6.12 #1/#6）。
    pub(crate) fn sync_banner(&mut self, now_ms: u64) {
        let prev = self.banner.dep().map(str::to_string);
        match self.banner.sync(self.health.worst(), now_ms) {
            BannerAct::Show => {
                let (text, remedy) = (
                    self.banner.text().to_string(),
                    self.banner.remedy().to_string(),
                );
                self.add_log("WARN", format!("降级：{text}（修复：{remedy}）"));
            }
            BannerAct::Restored => {
                let label = prev.as_deref().map(dep_cn).unwrap_or("依赖");
                self.add_log("INFO", format!("{label}已恢复，降级横幅撤销"));
                self.toast(format!("{label}已恢复"));
            }
            BannerAct::None => {}
        }
        self.mark_landed(Lane::Poll);
    }

    /// 打开健康面板（/health）：切面板、归零滚动、全量探测。
    pub fn open_health(&mut self) {
        self.active_panel = ActivePanel::Health;
        self.health_scroll = 0;
        self.stop_hall_watch();
        self.start_health(true);
    }

    /// 可行动错误卡片（§6.12 #4）：人话成因 + 修复命令 + 原始错误折叠。
    ///
    /// 前 3 行固定为 cause / remedy / 原始错误头——折叠只保留前 3 行，
    /// 关键行动项必须在折叠后仍可见。
    pub(crate) fn fail_card(&mut self, e: &anyhow::Error) {
        let raw = e.to_string();
        self.add_log("ERROR", format!("LLM 调用失败：{raw}"));
        let cause = self.err_agg.record(&raw, self.clock_ms);
        let count = self
            .err_agg
            .entries()
            .iter()
            .find(|x| x.code == cause.code)
            .map(|x| x.count)
            .unwrap_or(1);
        // 卡片须逐行成块（§6.6 #1）：markdown 会把仅以单 \n 相邻的行软换行
        // 合并为同一段——摘要与「原始错误」正文因此糊成一段，块行数不足以
        // 触发折叠，原始错误泄漏进折叠前的预览。这里用行尾双空格的**硬换行**
        // 让每行独立成行（不引入空段，否则空行会白占折叠保留额度）。
        let mut card = String::new();
        card.push_str(&format!("请求失败：{}  \n", cause.label));
        card.push_str(&format!("修复：{}  \n", cause.remedy));
        card.push_str("原始错误（Alt+O 展开）：");
        for line in raw.lines().take(6) {
            card.push_str("  \n  ");
            card.push_str(&crate::engine::grid::clip(line, 120));
        }
        // 计数行随末段并入（软换行），避免自身占用折叠保留额度。
        if count > 1 {
            card.push_str(&format!(
                "\n· 同类错误已出现 {count} 次（/health 查看聚合）"
            ));
        }
        self.add_message(MessageRole::System, card);
    }

    /// `/diag export`：导出脱敏诊断快照到日志目录（§6.13 #5）。
    pub(super) fn cmd_diag_export(&mut self) {
        let text = diag_report(
            &self.stamp,
            &self.timeline,
            &self.health,
            &self.err_agg,
            &wall_text(),
            self.clock_ms,
        );
        let dir = airy_home_path(&["logs"]);
        let name = format!("diag-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let path = dir.join(name);
        let res = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text));
        let shown = path.display().to_string();
        match res {
            Ok(()) => {
                self.add_log("INFO", format!("诊断快照已导出：{shown}"));
                self.add_message(MessageRole::System, format!("诊断快照已写入：{shown}"));
                self.toast("诊断快照已导出");
            }
            Err(e) => {
                self.add_log("ERROR", format!("诊断快照导出失败：{e}"));
                self.add_message(MessageRole::System, format!("诊断快照导出失败：{e}"));
            }
        }
    }

    pub fn health_scroll_up(&mut self) {
        self.health_scroll = self.health_scroll.saturating_sub(1);
    }

    pub fn health_scroll_down(&mut self) {
        self.health_scroll = self.health_scroll.saturating_add(1);
    }

    pub fn health_page_up(&mut self) {
        self.health_scroll = self.health_scroll.saturating_sub(HEALTH_PAGE);
    }

    pub fn health_page_down(&mut self) {
        self.health_scroll = self.health_scroll.saturating_add(HEALTH_PAGE);
    }
}

/// 依赖中文名（用户面不显示内部命名空间标识）。
pub(crate) fn dep_cn(name: &str) -> &str {
    match name {
        "gateway" => "网关",
        "llm" => "LLM 后端",
        "agent" => "智能体",
        "tool" => "工具",
        "think" => "思考",
        "monit" => "监控",
        "sched" => "调度",
        "channel" => "通道",
        "market" => "市场",
        "cupolas" => "穹顶",
        "mem" => "记忆",
        "notify" => "通知",
        "hook" => "钩子",
        "a2a" => "协作",
        "maths" => "数学",
        other => other,
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn wall_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
