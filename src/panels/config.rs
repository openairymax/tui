// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Config panel rendering.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::theme;

/// Render the config panel.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(
            " 配置 ",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ));

    let model_display = if app.model.is_empty() {
        "默认（由网关 / llm_d 自动回落）".to_string()
    } else {
        app.model.clone()
    };

    let mut lines: Vec<Line> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  当前模型  ", Style::default().fg(theme::faint())),
            Span::styled(model_display, Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  切换模型：/model <模型名>（持久化到 $AIRY_HOME/tui/config.toml）",
            Style::default().fg(theme::dim()),
        )),
        Line::from(Span::styled(
            "  查看当前：/model",
            Style::default().fg(theme::dim()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  模型默认配置（model.yaml 用户覆盖，可自由增删 provider 与模型）：",
            Style::default().fg(theme::faint()),
        )),
        Line::from(Span::styled(
            format!("    {}", app.config_file),
            Style::default().fg(theme::text()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  回落优先级：请求 model 参数 → env AIRY_AGENT_MODEL →",
            Style::default().fg(theme::faint()),
        )),
        Line::from(Span::styled(
            "  $AIRY_HOME/config/model.yaml global.default_model → 内置默认",
            Style::default().fg(theme::faint()),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  API Key 配置  ", Style::default().fg(theme::faint())),
            Span::styled("输入 /hiairy 重新打开配置向导（写回 $AIRY_HOME/config/secrets.env，",
                         Style::default().fg(theme::dim())),
        ]),
        Line::from(Span::styled(
            "  llm_d 热加载，无需重启；也可手动编辑 secrets.env（勿提交到仓库）",
            Style::default().fg(theme::dim()),
        )),
    ];

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ── 宿主机硬件与环境（实时） ──",
        Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
    )));
    lines.extend(host_env_lines());

    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 2.2.1.5.8：宿主机硬件配置与环境展示（架构/OS/CPU/内存/GPU/AIRY_HOME）。
/// 尽力而为探测：Linux 读取 /proc/cpuinfo 与 /proc/meminfo，其余平台回退
/// 到 Rust 编译期常量与线程数；探测失败显示 n/a，绝不阻塞渲染。
fn host_env_lines() -> Vec<Line<'static>> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    let mut cpu_model = String::from("n/a");
    let mut cpu_cores = 0usize;
    if let Ok(s) = std::fs::read_to_string("/proc/cpuinfo") {
        for l in s.lines() {
            if let Some(v) = l.strip_prefix("model name") {
                cpu_model = v.trim().trim_start_matches(':').trim().to_string();
                break;
            }
        }
        let logical = s.lines().filter(|l| l.starts_with("processor")).count();
        if logical > 0 {
            cpu_cores = logical;
        }
    }
    if cpu_cores == 0 {
        cpu_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    }

    let mut mem_gb = 0.0f64;
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for l in s.lines() {
            if let Some(rest) = l.strip_prefix("MemTotal:") {
                let kb: u64 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                mem_gb = kb as f64 / 1024.0 / 1024.0;
                break;
            }
        }
    }

    // GPU 尽力探测：NVIDIA 设备节点 / DRM 设备存在即视为有 GPU（不拉起
    // nvidia-smi，避免在无卡环境产生额外进程与延迟）。
    let gpu = if std::path::Path::new("/dev/nvidia0").exists()
        || std::path::Path::new("/dev/dri").exists()
    {
        "有（/dev/nvidia* 或 /dev/dri 存在）"
    } else {
        "无 / 未探测"
    };

    let home = std::env::var("AIRY_HOME").unwrap_or_else(|_| "未设置".to_string());

    vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  架构      ", Style::default().fg(theme::faint())),
            Span::styled(arch, Style::default().fg(theme::text())),
            Span::styled("    操作系统  ", Style::default().fg(theme::faint())),
            Span::styled(os, Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled("  CPU       ", Style::default().fg(theme::faint())),
            Span::styled(
                format!("{cpu_model}（{cpu_cores} 逻辑核）"),
                Style::default().fg(theme::text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("  内存      ", Style::default().fg(theme::faint())),
            Span::styled(
                if mem_gb > 0.0 { format!("{mem_gb:.1} GB") } else { "n/a".to_string() },
                Style::default().fg(theme::text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("  GPU       ", Style::default().fg(theme::faint())),
            Span::styled(gpu, Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled("  AIRY_HOME ", Style::default().fg(theme::faint())),
            Span::styled(home, Style::default().fg(theme::text())),
        ]),
    ]
}
