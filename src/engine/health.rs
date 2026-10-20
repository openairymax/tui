// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 降级态与可观测性投影（0.1.19 U12：TUI 建设方案 §6.12 六条 / §6.13 五条）。
//
// 分层：本文件属 L1 视图模型层——纯数据与纯函数，禁止 IO、加锁与时钟读取；
// 所有时刻由调用方注入（单调毫秒），状态机因此可在单测里确定性重放。
//
// 数据真实性：机制层当前不外发 degraded/restored 事件，健康表只由真实端到端
// 探测回填（gateway GET /health 与 {ns}.health_check RPC），不合成事件源；
// 错误聚合只吃真实错误串，分类不中一律落 unknown，不猜测成因。
//
// 三面职责：
//   1. HealthTable：逐依赖可达性与最后成功时刻（§6.12 #3、§6.13 #3）。
//   2. Banner：单行降级横幅、恢复冷却与抖动抑制（§6.12 #1/#2/#6）。
//   3. ErrAgg：错误码去重聚合与有界淘汰（§6.13 #2/#4）。
// Timeline 负责时钟回拨检测与单调序修正（§6.13 #1），diag_report 负责
// 脱敏导出（§6.13 #5）：报告结构只含依赖名、状态、计数、时长与错误码，
// 不读取会话正文，也不携带原始错误串。

use crate::budget::{DEGRADED_BANNER_COOLDOWN_MS, MAX_ERROR_AGG_ENTRIES, MAX_HEALTH_PANEL_ITEMS};

/// 判定时钟回拨的容差：小于该幅度的抖动视为正常调度误差。
const CLOCK_SKEW_TOL_MS: u64 = 1_000;
const MS_PER_S: u64 = 1_000;
const S_PER_MIN: u64 = 60;
const MIN_PER_H: u64 = 60;

/// 依赖状态迁移（由 observe 返回，调用方据此决定是否记录事件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// 状态未变（含首次即正常的登记）。
    Same,
    /// 由可达转为不可达（含首次探测即失败）。
    Down,
    /// 由不可达恢复为可达。
    Up,
}

/// 单个依赖的探测状态。
#[derive(Debug, Clone)]
pub(crate) struct Dep {
    pub(crate) name: String,
    pub(crate) up: bool,
    pub(crate) last_ok_ms: Option<u64>,
    pub(crate) last_fail_ms: Option<u64>,
    pub(crate) fails: u32,
    pub(crate) flaps: u32,
}

impl Dep {
    fn new(name: &str, ok: bool, now_ms: u64) -> Self {
        Self {
            name: name.to_string(),
            up: ok,
            last_ok_ms: ok.then_some(now_ms),
            last_fail_ms: (!ok).then_some(now_ms),
            fails: u32::from(!ok),
            flaps: 0,
        }
    }

    /// 吸收一次探测结果，返回状态迁移。
    fn update(&mut self, ok: bool, now_ms: u64) -> Change {
        if ok {
            self.last_ok_ms = Some(now_ms);
            if self.up {
                return Change::Same;
            }
            self.up = true;
            self.flaps = self.flaps.saturating_add(1);
            return Change::Up;
        }
        self.last_fail_ms = Some(now_ms);
        self.fails = self.fails.saturating_add(1);
        if self.up {
            self.up = false;
            return Change::Down;
        }
        Change::Same
    }

    /// 人话状态（面板与诊断共用，不暴露内部通道名）。
    pub(crate) fn state(&self) -> &'static str {
        if self.up {
            "正常"
        } else {
            "不可达"
        }
    }

    /// 距上次成功的时长（单调毫秒）；从未成功则无值。
    pub(crate) fn ok_age(&self, now_ms: u64) -> Option<u64> {
        self.last_ok_ms.map(|t| now_ms.saturating_sub(t))
    }

    /// 距本次不可达起点的时长；可达时无值。
    pub(crate) fn down_age(&self, now_ms: u64) -> Option<u64> {
        if self.up {
            return None;
        }
        self.last_fail_ms.map(|t| now_ms.saturating_sub(t))
    }
}

/// 依赖健康表（条目有界，超限拒新增：依赖集合由机制层枚举，不随会话增长）。
#[derive(Debug, Default)]
pub(crate) struct HealthTable {
    deps: Vec<Dep>,
}

impl HealthTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 吸收一次端到端探测结果。
    pub(crate) fn observe(&mut self, name: &str, ok: bool, now_ms: u64) -> Change {
        if let Some(dep) = self.deps.iter_mut().find(|d| d.name == name) {
            return dep.update(ok, now_ms);
        }
        if self.deps.len() >= MAX_HEALTH_PANEL_ITEMS {
            return Change::Same;
        }
        self.deps.push(Dep::new(name, ok, now_ms));
        if ok {
            Change::Same
        } else {
            Change::Down
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Dep> {
        self.deps.iter().find(|d| d.name == name)
    }

    pub(crate) fn deps(&self) -> &[Dep] {
        &self.deps
    }

    /// 该依赖是否已知且不可达。未探测过的依赖不冒充降级。
    pub(crate) fn is_down(&self, name: &str) -> bool {
        self.get(name).is_some_and(|d| !d.up)
    }

    /// 是否存在任一降级依赖。
    pub(crate) fn degraded(&self) -> bool {
        self.deps.iter().any(|d| !d.up)
    }

    /// 最重降级依赖：取失败起点最早者（持续不可达时间最长）。
    /// 状态条与横幅只显示这一项，全量在 /health 面板。
    pub(crate) fn worst(&self) -> Option<&str> {
        self.deps
            .iter()
            .filter(|d| !d.up)
            .min_by_key(|d| d.last_fail_ms.unwrap_or(0))
            .map(|d| d.name.as_str())
    }
}

/// 降级横幅的一次同步结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerAct {
    /// 无变化。
    None,
    /// 横幅出现或切换目标（调用方仅重绘）。
    Show,
    /// 降级解除（调用方提示恢复并撤横幅）。
    Restored,
}

/// 单行降级横幅：常显最重依赖，恢复后进入冷却期抑制抖动重弹。
#[derive(Debug, Default)]
pub(crate) struct Banner {
    dep: Option<String>,
    text: String,
    remedy: String,
    since_ms: u64,
    cool_until_ms: u64,
    flaps: u32,
}

impl Banner {
    /// 按健康表最重依赖同步横幅状态。调用方在每轮探测落地后调用一次。
    pub(crate) fn sync(&mut self, worst: Option<&str>, now_ms: u64) -> BannerAct {
        match worst {
            Some(name) => {
                if self.dep.as_deref() == Some(name) {
                    return BannerAct::None;
                }
                // 已有横幅时直接切换目标：严重度排序变化不应留下旧文案。
                let switching = self.dep.is_some();
                if !switching && now_ms < self.cool_until_ms {
                    self.flaps = self.flaps.saturating_add(1);
                    return BannerAct::None;
                }
                self.show(name, now_ms);
                BannerAct::Show
            }
            None => {
                if self.dep.is_none() {
                    return BannerAct::None;
                }
                self.dep = None;
                self.text.clear();
                self.remedy.clear();
                self.cool_until_ms = now_ms.saturating_add(DEGRADED_BANNER_COOLDOWN_MS);
                BannerAct::Restored
            }
        }
    }

    fn show(&mut self, name: &str, now_ms: u64) {
        self.dep = Some(name.to_string());
        self.text = banner_text(name);
        self.remedy = remedy(name);
        self.since_ms = now_ms;
    }

    pub(crate) fn visible(&self) -> bool {
        self.dep.is_some()
    }

    pub(crate) fn dep(&self) -> Option<&str> {
        self.dep.as_deref()
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 可一键复制的修复命令（真实存在的 airymaxrt 子命令）。
    pub(crate) fn remedy(&self) -> &str {
        &self.remedy
    }

    pub(crate) fn since_ms(&self) -> u64 {
        self.since_ms
    }

    /// 冷却期内被抑制的重弹次数（§6.12 #6，进健康面板）。
    pub(crate) fn flaps(&self) -> u32 {
        self.flaps
    }
}

fn banner_text(name: &str) -> String {
    match name {
        "llm" => "LLM 后端未连接 · 已切换本地降级路径".to_string(),
        "gateway" => "网关不可达 · 正在重连".to_string(),
        other => format!("{other} 不可达 · 功能降级"),
    }
}

fn remedy(name: &str) -> String {
    match name {
        "llm" => "airymaxrt logs llm_d".to_string(),
        _ => "airymaxrt doctor".to_string(),
    }
}

/// 错误成因（人话摘要 + 可行动修复命令）。字段全为静态串，聚合零分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cause {
    pub(crate) code: &'static str,
    pub(crate) label: &'static str,
    pub(crate) remedy: &'static str,
}

const QUOTA: Cause = Cause {
    code: "quota",
    label: "后端限流，请稍后重试",
    remedy: "airymaxrt doctor",
};
const AUTH: Cause = Cause {
    code: "auth",
    label: "API Key 无效或未授权",
    remedy: "airymaxrt doctor",
};
const MODEL: Cause = Cause {
    code: "model",
    label: "模型不存在或未安装",
    remedy: "airymaxrt doctor",
};
const TIMEOUT: Cause = Cause {
    code: "timeout",
    label: "请求超时，后端未响应",
    remedy: "airymaxrt logs llm_d",
};
pub(crate) const UNREACH: Cause = Cause {
    code: "unreachable",
    label: "无法连接后端服务",
    remedy: "airymaxrt logs llm_d",
};
const BACKEND: Cause = Cause {
    code: "backend",
    label: "后端服务内部错误",
    remedy: "airymaxrt logs llm_d",
};
const UNKNOWN: Cause = Cause {
    code: "unknown",
    label: "未知错误，请查看日志",
    remedy: "airymaxrt doctor",
};

/// 把原始错误串归类为人话成因。顺序即优先级：先端点侧语义，后传输侧语义。
pub(crate) fn classify(raw: &str) -> Cause {
    let s = raw.to_ascii_lowercase();
    if has_any(&s, &["429", "rate limit", "quota", "too many requests"]) {
        return QUOTA;
    }
    if has_any(
        &s,
        &[
            "401",
            "unauthorized",
            "invalid api key",
            "api key",
            "authentication",
        ],
    ) {
        return AUTH;
    }
    if has_any(
        &s,
        &[
            "model not found",
            "no such model",
            "unknown model",
            "target not found",
        ],
    ) {
        return MODEL;
    }
    if has_any(&s, &["timed out", "timeout", "deadline exceeded"]) {
        return TIMEOUT;
    }
    if has_any(
        &s,
        &[
            "connection refused",
            "connection reset",
            "connect error",
            "failed to connect",
            "broken pipe",
            "unexpected eof",
            "connection closed",
            "unreachable",
        ],
    ) {
        return UNREACH;
    }
    if has_any(
        &s,
        &[
            " 500",
            " 502",
            " 503",
            " 504",
            "internal server error",
            "bad gateway",
            "service unavailable",
        ],
    ) {
        return BACKEND;
    }
    UNKNOWN
}

fn has_any(s: &str, pats: &[&str]) -> bool {
    pats.iter().any(|p| s.contains(p))
}

/// 聚合后的单类错误。
#[derive(Debug, Clone)]
pub(crate) struct ErrEntry {
    pub(crate) code: &'static str,
    pub(crate) label: &'static str,
    pub(crate) remedy: &'static str,
    pub(crate) count: u32,
    pub(crate) first_ms: u64,
    pub(crate) last_ms: u64,
}

/// 错误聚合表：同一错误码连续重复聚合为 ×N；条目有界，最旧条目并入溢出桶。
#[derive(Debug, Default)]
pub(crate) struct ErrAgg {
    items: Vec<ErrEntry>,
    overflow: u32,
}

impl ErrAgg {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 记录一条原始错误，返回其归类（调用方用于错误卡片）。
    pub(crate) fn record(&mut self, raw: &str, now_ms: u64) -> Cause {
        let cause = classify(raw);
        self.record_cause(cause, now_ms);
        cause
    }

    pub(crate) fn record_cause(&mut self, cause: Cause, now_ms: u64) {
        if let Some(e) = self.items.iter_mut().find(|e| e.code == cause.code) {
            e.count = e.count.saturating_add(1);
            e.last_ms = now_ms;
            return;
        }
        if self.items.len() >= MAX_ERROR_AGG_ENTRIES.saturating_sub(1) {
            if let Some(idx) = self.oldest() {
                let gone = self.items.remove(idx);
                self.overflow = self.overflow.saturating_add(gone.count);
            }
        }
        self.items.push(ErrEntry {
            code: cause.code,
            label: cause.label,
            remedy: cause.remedy,
            count: 1,
            first_ms: now_ms,
            last_ms: now_ms,
        });
    }

    fn oldest(&self) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.last_ms)
            .map(|(i, _)| i)
    }

    pub(crate) fn entries(&self) -> &[ErrEntry] {
        &self.items
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// 并入溢出桶的历史错误次数（条目被淘汰时累加）。
    pub(crate) fn overflow(&self) -> u32 {
        self.overflow
    }

    /// 全部错误次数（命名条目 + 溢出桶）。
    pub(crate) fn total(&self) -> u32 {
        let named: u32 = self.items.iter().map(|e| e.count).sum();
        named.saturating_add(self.overflow)
    }
}

/// 一次时间注入的处理结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stamp {
    /// 单调序时刻（用于所有时序展示）。
    pub(crate) order_ms: u64,
    /// 本次注入检测到墙钟回拨并已修正。
    pub(crate) corrected: bool,
}

/// 时间线：墙钟与单调钟配对，回拨时按单调序修正并累计异常量。
#[derive(Debug, Default)]
pub(crate) struct Timeline {
    last_wall: Option<u64>,
    order: u64,
    skew_ms: u64,
    corrections: u32,
}

impl Timeline {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 注入一对时刻（墙钟毫秒、单调毫秒），返回单调序时刻与修正标记。
    pub(crate) fn observe(&mut self, wall_ms: u64, mono_ms: u64) -> Stamp {
        let corrected = self
            .last_wall
            .is_some_and(|prev| wall_ms.saturating_add(CLOCK_SKEW_TOL_MS) < prev);
        if let Some(prev) = self.last_wall {
            if corrected {
                self.skew_ms = self.skew_ms.saturating_add(prev.saturating_sub(wall_ms));
                self.corrections = self.corrections.saturating_add(1);
            }
        }
        self.last_wall = Some(wall_ms);
        self.order = self.order.max(mono_ms);
        Stamp {
            order_ms: self.order,
            corrected,
        }
    }

    /// 回拨异常的人话标注；从未回拨则无值。
    pub(crate) fn note(&self) -> Option<String> {
        if self.corrections == 0 {
            return None;
        }
        let secs = self.skew_ms.div_ceil(MS_PER_S);
        Some(format!("时钟已校准（回拨 ≈{secs}s），时序已按单调序修正"))
    }
}

/// 时长格式化：10s / 3m12s / 2h5m。
pub(crate) fn dur_cn(ms: u64) -> String {
    let secs = ms / MS_PER_S;
    if secs < S_PER_MIN {
        return format!("{secs}s");
    }
    let mins = secs / S_PER_MIN;
    if mins < MIN_PER_H {
        return format!("{}m{}s", mins, secs % S_PER_MIN);
    }
    format!("{}h{}m", mins / MIN_PER_H, mins % MIN_PER_H)
}

/// 诊断快照（§6.13 #5）：只含时间线异常、依赖健康与错误聚合，
/// 结构上不引用会话正文与密钥；原始错误串亦不入报告。
pub(crate) fn diag_report(
    stamp: &Stamp,
    timeline: &Timeline,
    health: &HealthTable,
    errs: &ErrAgg,
    wall_text: &str,
    now_ms: u64,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("AirymaxRT 诊断快照\n");
    out.push_str(&format!("采集时刻：{wall_text}\n"));
    out.push_str(&format!("单调序：{}ms\n", stamp.order_ms));
    match timeline.note() {
        Some(note) => out.push_str(&format!("时间线：{note}\n")),
        None => out.push_str("时间线：正常\n"),
    }

    let down = health.deps().iter().filter(|d| !d.up).count();
    out.push_str(&format!(
        "依赖健康（{} 项，{} 项降级）\n",
        health.deps().len(),
        down
    ));
    for dep in health.deps() {
        let mark = if dep.up { "✓" } else { "✗" };
        let span = if dep.up {
            match dep.ok_age(now_ms) {
                Some(age) => format!("正常 · {} 前", dur_cn(age)),
                None => "正常".to_string(),
            }
        } else {
            match dep.down_age(now_ms) {
                Some(age) => format!("不可达 · {} 前起", dur_cn(age)),
                None => "不可达".to_string(),
            }
        };
        out.push_str(&format!(
            "  {mark} {} · {span}（失败 {} 次 / 抖动 {} 次）\n",
            dep.name, dep.fails, dep.flaps
        ));
    }

    out.push_str(&format!(
        "错误聚合（{} 类，共 {} 次）\n",
        errs.len(),
        errs.total()
    ));
    for e in errs.entries() {
        out.push_str(&format!(
            "  ×{} {} · {} · 最近 {} 前\n",
            e.count,
            e.code,
            e.label,
            dur_cn(now_ms.saturating_sub(e.last_ms))
        ));
    }
    if errs.overflow() > 0 {
        out.push_str(&format!("  其他合并 {} 次\n", errs.overflow()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §6.12 #3/§6.13 #3 状态机侧：last_ok 单调时刻随探测更新，
    /// ok_age/down_age 按注入时刻投影，不下发墙钟。
    #[test]
    fn table_reports_transitions() {
        let mut t = HealthTable::new();
        assert_eq!(t.observe("llm", true, 1_000), Change::Same);
        assert_eq!(t.observe("llm", false, 2_000), Change::Down);
        assert_eq!(t.observe("llm", false, 3_000), Change::Same);
        assert_eq!(t.observe("llm", true, 4_000), Change::Up);
        let Some(dep) = t.get("llm") else {
            panic!("llm 未登记");
        };
        assert!(dep.up);
        assert_eq!(dep.flaps, 1);
        assert_eq!(dep.fails, 2);
        assert_eq!(dep.ok_age(7_000), Some(3_000));
        assert_eq!(dep.down_age(7_000), None);
    }

    #[test]
    fn worst_picks_earliest_failure() {
        let mut t = HealthTable::new();
        t.observe("gateway", false, 5_000);
        t.observe("llm", false, 3_000);
        t.observe("tool", true, 2_000);
        assert_eq!(t.worst(), Some("llm"));
        assert!(t.degraded());
        assert!(t.is_down("gateway"));
        assert!(!t.is_down("tool"));
        assert!(!t.is_down("never-probed"));
    }

    #[test]
    fn table_is_bounded() {
        let mut t = HealthTable::new();
        for i in 0..MAX_HEALTH_PANEL_ITEMS {
            t.observe(&format!("d{i}"), false, i as u64);
        }
        assert_eq!(t.observe("extra", false, 99), Change::Same);
        assert_eq!(t.deps().len(), MAX_HEALTH_PANEL_ITEMS);
        assert!(t.get("extra").is_none());
    }

    #[test]
    fn banner_shows_restores_and_cools() {
        let mut h = HealthTable::new();
        let mut b = Banner::default();
        h.observe("llm", false, 1_000);
        assert_eq!(b.sync(h.worst(), 1_000), BannerAct::Show);
        assert!(b.visible());
        assert!(b.text().contains("LLM"));
        assert_eq!(b.remedy(), "airymaxrt logs llm_d");

        assert_eq!(b.sync(h.worst(), 1_500), BannerAct::None);

        h.observe("llm", true, 2_000);
        assert_eq!(b.sync(h.worst(), 2_000), BannerAct::Restored);
        assert!(!b.visible());
        assert!(b.text().is_empty());

        // 冷却期（2s + 5s）内重弹被抑制，抖动计数递增。
        h.observe("llm", false, 3_000);
        assert_eq!(b.sync(h.worst(), 3_000), BannerAct::None);
        assert_eq!(b.flaps(), 1);

        h.observe("llm", true, 3_500);
        assert_eq!(b.sync(h.worst(), 3_500), BannerAct::None);
        h.observe("llm", false, 9_000);
        assert_eq!(b.sync(h.worst(), 9_000), BannerAct::Show);
    }

    #[test]
    fn banner_switches_to_new_worst() {
        let mut h = HealthTable::new();
        let mut b = Banner::default();
        h.observe("llm", false, 1_000);
        assert_eq!(b.sync(h.worst(), 1_000), BannerAct::Show);
        assert_eq!(b.dep(), Some("llm"));
        h.observe("gateway", false, 500);
        assert_eq!(b.sync(h.worst(), 2_000), BannerAct::Show);
        assert_eq!(b.dep(), Some("gateway"));
        assert!(b.text().contains("网关"));
    }

    #[test]
    fn err_agg_counts_same_code() {
        let mut a = ErrAgg::new();
        a.record("HTTP 429 Too Many Requests", 1_000);
        a.record("rate limit exceeded", 2_000);
        assert_eq!(a.len(), 1);
        assert_eq!(a.total(), 2);
        let Some(e) = a.entries().first() else {
            panic!("聚合条目缺失");
        };
        assert_eq!(e.code, "quota");
        assert_eq!(e.count, 2);
        assert_eq!(e.first_ms, 1_000);
        assert_eq!(e.last_ms, 2_000);
    }

    #[test]
    fn err_agg_evicts_oldest_into_overflow() {
        let mut a = ErrAgg::new();
        let rounds = MAX_ERROR_AGG_ENTRIES + 8;
        for i in 0..rounds {
            let code: &'static str = Box::leak(format!("c{i}").into_boxed_str());
            a.record_cause(
                Cause {
                    code,
                    label: "x",
                    remedy: "y",
                },
                i as u64,
            );
        }
        assert_eq!(a.len(), MAX_ERROR_AGG_ENTRIES - 1);
        assert_eq!(a.overflow() as usize, rounds - (MAX_ERROR_AGG_ENTRIES - 1));
        assert_eq!(a.total() as usize, rounds);
    }

    #[test]
    fn classify_covers_known_causes() {
        assert_eq!(classify("HTTP 429").code, "quota");
        assert_eq!(classify("Unauthorized: invalid api key").code, "auth");
        assert_eq!(classify("model not found").code, "model");
        assert_eq!(classify("request timed out").code, "timeout");
        assert_eq!(classify("connection refused").code, "unreachable");
        assert_eq!(classify("HTTP 503 service unavailable").code, "backend");
        assert_eq!(classify("weird failure").code, "unknown");
    }

    #[test]
    fn timeline_detects_backstep() {
        let mut t = Timeline::new();
        assert!(!t.observe(10_000, 1_000).corrected);
        let s = t.observe(8_000, 2_000);
        assert!(s.corrected);
        assert_eq!(s.order_ms, 2_000);
        let s = t.observe(8_500, 2_500);
        assert!(!s.corrected);
        assert_eq!(s.order_ms, 2_500);
        let Some(note) = t.note() else {
            panic!("回拨未记录");
        };
        assert!(note.contains("回拨"));
        assert!(note.contains("2s"));
    }

    #[test]
    fn dur_cn_formats_scales() {
        assert_eq!(dur_cn(900), "0s");
        assert_eq!(dur_cn(5_000), "5s");
        assert_eq!(dur_cn(65_000), "1m5s");
        assert_eq!(dur_cn(3_725_000), "1h2m");
    }

    #[test]
    fn diag_report_is_redacted() {
        let mut h = HealthTable::new();
        h.observe("llm", false, 10_000);
        h.observe("tool", true, 10_000);
        let mut a = ErrAgg::new();
        a.record("HTTP 503 upstream body=secret-token", 10_500);
        let tl = Timeline::new();
        let stamp = Stamp {
            order_ms: 11_000,
            corrected: false,
        };
        let out = diag_report(&stamp, &tl, &h, &a, "12:00:00", 11_000);
        assert!(out.contains("llm"));
        assert!(out.contains("tool"));
        assert!(out.contains("×1 backend"));
        assert!(out.contains("12:00:00"));
        assert!(!out.contains("secret-token"));
    }
}
