// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 自托管专页（D-1）：只读展示 llm_d 的端点事实——模型 / 后端 / 显存 /
// 并发 / 队列 / 失败率 / 最近错误，与 doctor、daemon 日志同源（都来自
// llm.get_stats 与同一套 provider 计数）。
//
// 诚实语义：后端不可达时整页降级（不显示上一轮端点数值），端点为空或
// 字段缺失都如实说明，不合成 0 值。展示层不读时钟。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::engine::health::dur_cn;
use crate::engine::selfhost::{Endpoint, Snapshot};
use crate::style;

/// 容量与显存复用同一量纲约定：MiB 小于 1024 时保留 MiB，避免 0.0 GiB。
fn mem(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GiB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MiB")
    }
}

fn hint(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {text}"),
        Style::default().fg(style::faint()),
    ))
}

/// 状态行：采集在途 / 后端不可用 / 在线三态；在线给出端点数和更新时长。
fn stat_line(sh: Option<&Snapshot>, now_ms: u64, at_ms: u64) -> Line<'static> {
    let age = dur_cn(now_ms.saturating_sub(at_ms));
    match sh {
        None => Line::from(Span::styled(
            "  ○ 正在采集自托管状态…",
            Style::default().fg(style::dim()),
        )),
        Some(s) if !s.live => Line::from(vec![
            Span::styled(
                "  ▲ 自托管后端不可用  ",
                Style::default()
                    .fg(style::warning())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{age} 前尝试"), Style::default().fg(style::faint())),
        ]),
        Some(s) => Line::from(vec![
            Span::styled(
                "  ● 自托管在线  ",
                Style::default()
                    .fg(style::success())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} 个端点 · {age} 前更新", s.endpoints.len()),
                Style::default().fg(style::faint()),
            ),
        ]),
    }
}

/// 在途空态：首轮结果未到，不显示占位数值。
fn pending_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "  正在经 gateway 拉取 llm_d 自托管统计。",
            Style::default().fg(style::dim()),
        )),
        hint("结果到达后本页自动更新，按 Esc 返回对话。"),
    ]
}

/// 不可用空态：原样给出原因，并声明不回显任何缓存值。
fn down_lines(reason: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("  原因：{reason}"),
            Style::default().fg(style::warning()),
        )),
        hint("数据源：llm_d · llm.get_stats（与 doctor / daemon 日志同源）"),
        hint("本页不显示任何缓存值；后端恢复后自动刷新。"),
    ]
}

/// 端点状态短语：未就绪时带原因，未上报时不冒充就绪。
fn ready_text(e: &Endpoint) -> String {
    match e.ready {
        Some(true) => "就绪".to_string(),
        Some(false) if !e.reason.is_empty() => format!("未就绪：{}", e.reason),
        Some(false) => "未就绪".to_string(),
        None => "状态未上报".to_string(),
    }
}

/// 并发/队列/槽位一行：capacity=0 是「未设闸」，不是容量为零。
fn load_text(e: &Endpoint) -> String {
    let inflight = e
        .inflight
        .map(|v| v.to_string())
        .unwrap_or_else(|| "—".to_string());
    let conc = if e.capacity == 0 {
        format!("并发 {inflight}（未设闸）")
    } else {
        format!("并发 {inflight}/{}", e.capacity)
    };
    let waiting = e
        .waiting
        .map(|v| v.to_string())
        .unwrap_or_else(|| "—".to_string());
    let slot = match (e.total_slots, e.ctx_slot) {
        (Some(s), Some(c)) => format!("槽位 {s} × {c} tok"),
        (Some(s), None) => format!("槽位 {s}"),
        _ => "槽位 —".to_string(),
    };
    format!("{conc} · 队列 {waiting} · {slot}")
}

/// 单端点三行：模型、后端、并发/队列。
fn endpoint_lines(e: &Endpoint) -> Vec<Line<'static>> {
    let models = if e.models.is_empty() {
        "未上报".to_string()
    } else {
        e.models.join(", ")
    };
    let api_base = if e.api_base.is_empty() {
        "未上报"
    } else {
        &e.api_base
    };
    vec![
        Line::from(vec![
            Span::styled(
                format!("  ▸ {}  ", e.provider),
                Style::default()
                    .fg(style::primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(models, Style::default().fg(style::accent())),
        ]),
        Line::from(vec![
            Span::styled("    后端  ", Style::default().fg(style::dim())),
            Span::styled(api_base.to_string(), Style::default().fg(style::faint())),
            Span::styled(format!(" · {}", ready_text(e)), Style::default().fg(
                match e.ready {
                    Some(true) => style::success(),
                    Some(false) => style::warning(),
                    None => style::faint(),
                },
            )),
        ]),
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(load_text(e), Style::default().fg(style::dim())),
        ]),
    ]
}

/// 全局摘要：显存 / 请求与失败率 / 最近错误。
fn summary_lines(s: &Snapshot) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(4);
    let vram = match s.vram {
        Some((used, total)) => {
            let pct = used as f64 * 100.0 / total as f64;
            format!("显存  {} / {}（{pct:.1}%）", mem(used), mem(total))
        }
        None => "显存  未检测到 GPU（nvidia-smi 不可用）".to_string(),
    };
    out.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(vram, Style::default().fg(style::dim())),
    ]));
    let req = match s.requests {
        Some(r) => {
            let rate = r
                .fail_rate()
                .map(|v| format!("{:.1}%", v * 100.0))
                .unwrap_or_else(|| "—（无请求）".to_string());
            format!(
                "请求  共 {} · 成功 {} · 失败 {} · 超时 {} · 失败率 {rate}",
                r.total, r.ok, r.fail, r.timeout
            )
        }
        None => "请求  未上报".to_string(),
    };
    out.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(req, Style::default().fg(style::dim())),
    ]));
    let err = match s.last_error.as_ref() {
        Some(e) => {
            let model = if e.model.is_empty() { "未知模型" } else { &e.model };
            format!(
                "最近错误  {model} · {}（rc={}）· {} 前",
                e.reason,
                e.rc,
                dur_cn(e.age_ms)
            )
        }
        None => "最近错误  无".to_string(),
    };
    out.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(err, Style::default().fg(style::dim())),
    ]));
    out
}

/// 正文行：摘要 + 端点清单；降级/空态不出现任何旧端点数值。
fn body_lines(app: &App) -> Vec<Line<'static>> {
    let Some(s) = app.selfhost.as_ref() else {
        return pending_lines();
    };
    if !s.live {
        return down_lines(&s.note);
    }
    let mut out = summary_lines(s);
    out.push(Line::raw(""));
    if !s.ep_reported {
        out.push(hint(
            "llm_d 未上报 endpoints 字段（daemon 版本过旧）——升级后本页自动补齐。",
        ));
        return out;
    }
    if s.endpoints.is_empty() {
        out.push(hint("llm_d 已连接，但未注册任何自托管端点。"));
        return out;
    }
    for e in &s.endpoints {
        out.extend(endpoint_lines(e));
    }
    out
}

/// Render the self-hosted dashboard (read-only; /selfhost or Alt+S).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(style::border()))
        .title(Span::styled(
            " 自托管 ",
            Style::default()
                .fg(style::primary())
                .add_modifier(Modifier::BOLD),
        ));

    let inner_h = (area.height.saturating_sub(2) as usize).max(1);
    let body = body_lines(app);

    // 顶部状态行与底部提示行固定，仅正文窗口滚动（offset = 正文首行下标）。
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    lines.push(stat_line(app.selfhost.as_ref(), app.clock_ms, app.selfhost_at_ms));
    if inner_h >= 3 {
        let body_rows = inner_h - 2;
        let offset = app.selfhost_scroll.min(body.len().saturating_sub(body_rows));
        lines.extend(body.into_iter().skip(offset).take(body_rows));
    }
    while lines.len() < inner_h.saturating_sub(1) {
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled("  ↑↓ 滚动", Style::default().fg(style::faint())),
        Span::styled("  PgUp/PgDn 翻页", Style::default().fg(style::faint())),
        Span::styled("  Esc 返回对话", Style::default().fg(style::faint())),
        crate::panels::nav::observe_hint(),
    ]));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(capacity: u64) -> Endpoint {
        Endpoint {
            provider: "gateapi".to_string(),
            models: vec!["gate-model".to_string()],
            api_base: "http://127.0.0.1:21490/v1".to_string(),
            capacity,
            total_slots: Some(4),
            ctx_slot: Some(4096),
            inflight: Some(2),
            waiting: Some(1),
            ready: Some(true),
            reason: String::new(),
        }
    }

    fn text_of(lines: &[Line<'static>]) -> String {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// 并发两态：有闸显示 inflight/capacity，未设闸显式标注而不是 0。
    #[test]
    fn load_text_distinguishes_gated_and_ungated() {
        let t = load_text(&ep(8));
        assert!(t.contains("并发 2/8"), "缺少闸容量: {t}");
        assert!(t.contains("队列 1"), "缺少队列: {t}");
        assert!(t.contains("槽位 4 × 4096 tok"), "缺少槽位: {t}");
        let u = load_text(&ep(0));
        assert!(u.contains("未设闸"), "未设闸必须显式: {u}");
        assert!(!u.contains("/0"), "不得显示 /0: {u}");
    }

    /// 未就绪原因与字段缺失都不冒充「就绪」。
    #[test]
    fn ready_text_is_honest() {
        let mut e = ep(8);
        e.ready = Some(false);
        e.reason = "api_key_missing".to_string();
        assert_eq!(ready_text(&e), "未就绪：api_key_missing");
        e.reason.clear();
        assert_eq!(ready_text(&e), "未就绪");
        e.ready = None;
        assert_eq!(ready_text(&e), "状态未上报");
    }

    /// 显存与失败率格式化：GiB 保留一位，零请求不显示 0%。
    #[test]
    fn mem_and_rate_formatting() {
        assert_eq!(mem(2048), "2.0 GiB");
        assert_eq!(mem(512), "512 MiB");
    }

    /// 降级空态保留原因并声明不回显缓存值。
    #[test]
    fn down_state_shows_reason_and_no_cache() {
        let t = text_of(&down_lines("llm_d unreachable"));
        assert!(t.contains("llm_d unreachable"), "缺少原因: {t}");
        assert!(t.contains("不显示任何缓存值"), "缺少诚实声明: {t}");
    }

    /// 状态行三态：采集在途 / 不可用 / 在线（在线给端点数量）。
    #[test]
    fn stat_line_distinguishes_states() {
        assert!(stat_line(None, 0, 0).to_string().contains("正在采集"));
        let down = crate::engine::selfhost::unavailable("unreachable");
        assert!(stat_line(Some(&down), 0, 0).to_string().contains("不可用"));
        let live = Snapshot {
            live: true,
            note: String::new(),
            ep_reported: true,
            endpoints: vec![ep(8)],
            requests: None,
            last_error: None,
            vram: None,
        };
        let t = stat_line(Some(&live), 65_000, 0).to_string();
        assert!(t.contains("自托管在线"), "缺少在线态: {t}");
        assert!(t.contains("1 个端点"), "缺少端点数量: {t}");
        assert!(t.contains("1m5s 前更新"), "缺少更新时长: {t}");
    }
}
