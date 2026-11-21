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
// 数据真实性：探测只有两个真实来源——gateway ping 帧与 {ns}.health_check
// 查询帧（经事件管道下发，代表端到端可达）。探测失败按原样入日志与错误
// 聚合，不合成事件、不猜测成因。

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

/// 健康探测在途阶段：网关存活确认 → 各域并发探测。
pub(crate) enum HealthStage {
    /// 网关存活确认中；`full` 决定随后探测范围
    Ping { full: bool },
    /// 各域 health_check 待应答 + 已到结果
    Deps {
        pending: Vec<(String, String)>,
        probes: Vec<Probe>,
    },
}

/// 健康面板翻页步长（行）。
const HEALTH_PAGE: usize = 10;

impl App {
    /// 注入主循环单调时刻并刷新时间线（墙钟/单调钟配对，回拨按单调序修正）。
    /// 统计快照（§6.7 #5）在同一挂点按 DEFAULT_LIVE_PUBLISH_INTERVAL_MS 节拍
    /// 发布：帧节拍内不逐帧重算，跨节拍才取真实耗时（不逐 delta 刷屏）。
    pub fn set_clock(&mut self, now_ms: u64) {
        self.clock_ms = now_ms;
        self.stamp = self.timeline.observe(wall_ms(), now_ms);
        if now_ms.saturating_sub(self.stats_at_ms)
            >= crate::budget::DEFAULT_LIVE_PUBLISH_INTERVAL_MS
        {
            self.stats_at_ms = now_ms;
            self.busy_shown_secs = self.busy_started.elapsed().as_secs_f64();
        }
    }

    /// 启动一轮健康探测。`full` 为真时覆盖全部业务 daemon（/health 面板），
    /// 否则只探 LLM 后端（5s 节拍的轻量保活）。
    ///
    /// gateway 自身不可达时短路：后续 daemon 探测必然全部失败，逐项上报只会
    /// 制造一屏「不可达」噪声，真实根因只有一个——网关。
    pub fn start_health(&mut self, full: bool) {
        if self.health_q.is_some() {
            return;
        }
        // 网关存活先确认（ping 帧）：不可达时短路，逐项上报只制造噪声。
        match self.send_query("health.ping", "ping", &serde_json::json!({})) {
            Ok(()) => self.health_q = Some(HealthStage::Ping { full }),
            Err(e) => {
                self.note_dep("gateway", false, self.clock_ms, Some(&e));
                self.sync_banner(self.clock_ms);
            }
        }
    }

    /// 推进探测：网关确认 → 各域应答收齐后统一落表与同步一次横幅
    /// （批量探测只触发一次横幅裁决，避免同批内多次抖动计数）。
    pub fn poll_health(&mut self, now_ms: u64) {
        match self.health_q.take() {
            None => {}
            Some(HealthStage::Ping { full }) => match self.take_query("health.ping") {
                Some(Ok(_)) => self.start_health_deps(full),
                Some(Err(e)) => {
                    self.note_dep("gateway", false, now_ms, Some(&e));
                    self.sync_banner(now_ms);
                    self.mark_landed(Lane::Poll);
                }
                None if !self.pipe_alive() => {
                    self.note_dep("gateway", false, now_ms, Some("事件管道不可用"));
                    self.sync_banner(now_ms);
                    self.mark_landed(Lane::Poll);
                }
                None => self.health_q = Some(HealthStage::Ping { full }),
            },
            Some(st @ HealthStage::Deps { .. }) => self.poll_health_deps(st, now_ms),
        }
    }

    /// 各域并发探测：每域一条 `health.daemon.{i}`，应答由 poll_health 汇总。
    fn start_health_deps(&mut self, full: bool) {
        let names: Vec<&str> = if full {
            OPS_DAEMON_NS.to_vec()
        } else {
            vec!["llm"]
        };
        let mut pending = Vec::with_capacity(names.len());
        let mut probes = vec![Probe {
            name: "gateway".to_string(),
            ok: true,
            err: String::new(),
        }];
        for (i, ns) in names.iter().enumerate() {
            let id = format!("health.daemon.{i}");
            let method = format!("{ns}.health_check");
            match self.send_query(&id, &method, &serde_json::json!({})) {
                Ok(()) => pending.push((ns.to_string(), id)),
                Err(e) => probes.push(Probe {
                    name: ns.to_string(),
                    ok: false,
                    err: e,
                }),
            }
        }
        self.health_q = Some(HealthStage::Deps { pending, probes });
    }

    /// 收集各域应答：缺应答保留在途；管道退出则已到结果照常落表。
    fn poll_health_deps(&mut self, st: HealthStage, now_ms: u64) {
        let HealthStage::Deps {
            mut pending,
            mut probes,
        } = st
        else {
            return;
        };
        let mut i = 0;
        while i < pending.len() {
            if let Some(r) = self.take_query(&pending[i].1) {
                let (ns, _) = pending.remove(i);
                let (ok, err) = match r {
                    Ok(_) => (true, String::new()),
                    Err(e) => (false, e),
                };
                probes.push(Probe { name: ns, ok, err });
            } else {
                i += 1;
            }
        }
        if pending.is_empty() || !self.pipe_alive() {
            // 管道退出时未应答的域保持健康表原状——不用「失败」冒充未知。
            self.land_health(probes, now_ms);
            return;
        }
        self.health_q = Some(HealthStage::Deps { pending, probes });
    }

    /// 探测结果落地：逐项落健康表与错误聚合。
    fn land_health(&mut self, probes: Vec<Probe>, now_ms: u64) {
        for p in probes {
            self.note_dep(&p.name, p.ok, now_ms, Some(&p.err));
        }
        self.sync_banner(now_ms);
        self.mark_landed(Lane::Poll);
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
        self.start_health(true);
    }

    /// 可行动错误卡片（§6.12 #4）：人话成因 + 修复命令 + 原始错误折叠。
    ///
    /// 前 3 行固定为 cause / remedy / 原始错误头——折叠只保留前 3 行，
    /// 关键行动项必须在折叠后仍可见。
    pub(crate) fn fail_card(&mut self, e: &anyhow::Error) {
        let raw = e.to_string();
        self.add_log("ERROR", format!("LLM 调用失败：{raw}"));
        let cause = self.err_agg.record_wire(None, &raw, self.clock_ms);
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
