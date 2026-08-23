// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// Config panel rendering.
//
// 2.2.1.5 任务 3 强化：
//   (a) 模型 API Key 便捷配置节：列出 secrets.env 已知 Key（值脱敏显示后 4 位），
//       编辑入口为输入栏 /set-key <KEY> <VALUE>（写入 $AIRY_HOME/config/secrets.env，
//       chmod 600，llm_d 热加载）；
//   (b) 宿主机实时信息节：架构（uname）、操作系统、内存总量/可用、CPU 核数、
//       GPU/加速器（nvidia-smi / rocm-smi / /dev/dri 探测）、TUI 主题模式。
//       数据采集在 Rust 内完成（std::process::Command + /proc），失败显示占位；
//       探测结果经 OnceLock 缓存（面板每帧渲染，不反复拉起子进程）。

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::secrets;
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
    ];

    // (a) 模型 API Key 便捷配置节
    lines.extend(api_key_lines());

    // (b) 宿主机实时信息节
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ── 宿主机实时信息 ──",
        Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
    )));
    lines.extend(host_env_lines());

    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 模型 API Key 便捷配置节：列出 secrets.env 中已知 Key（值脱敏后 4 位），
/// 提供 /set-key 编辑入口提示（写入 secrets.env，chmod 600，llm_d 热加载）。
fn api_key_lines() -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ── 模型 API Key（secrets.env） ──",
        Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
    )));

    let path = secrets::secrets_path();
    let pairs = secrets::read_all();
    let configured = secrets::KNOWN_KEYS
        .iter()
        .filter(|(k, _)| pairs.iter().any(|(pk, v)| pk == k && !v.is_empty()))
        .count();

    lines.push(Line::from(vec![
        Span::styled("  文件  ", Style::default().fg(theme::faint())),
        Span::styled(
            path.display().to_string(),
            Style::default().fg(theme::text()),
        ),
        Span::styled(
            format!("   已配置 {}/{}", configured, secrets::KNOWN_KEYS.len()),
            Style::default()
                .fg(if configured > 0 { theme::SUCCESS } else { theme::dim() }),
        ),
    ]));

    for (key, label) in secrets::KNOWN_KEYS {
        let value = pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let (dot, dot_color, status) = if value.is_empty() {
            ("○", theme::faint(), "未配置".to_string())
        } else {
            ("●", theme::SUCCESS, format!("已配置 {}", secrets::mask(&value)))
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(dot, Style::default().fg(dot_color).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default()),
            Span::styled(key, Style::default().fg(theme::text())),
            Span::styled(format!("（{}）", label), Style::default().fg(theme::faint())),
            Span::styled("  ", Style::default()),
            Span::styled(status, Style::default().fg(if value.is_empty() { theme::faint() } else { theme::ACCENT })),
        ]));
    }

    // 未列入已知清单的自定义 Key（secrets.env 中存在但 KNOWN_KEYS 未覆盖）
    let extra: Vec<&str> = pairs
        .iter()
        .filter(|(k, v)| {
            !v.is_empty()
                && !secrets::KNOWN_KEYS.iter().any(|(nk, _)| nk == k)
                && k.to_ascii_uppercase().contains("API_KEY")
        })
        .map(|(k, _)| k.as_str())
        .collect();
    if !extra.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  其他  ", Style::default().fg(theme::faint())),
            Span::styled(extra.join(" / "), Style::default().fg(theme::dim())),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  编辑  ", Style::default().fg(theme::faint())),
        Span::styled(
            "/set-key <KEY> <VALUE>",
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   ·  写回 secrets.env（chmod 600，llm_d 热加载，无需重启）",
            Style::default().fg(theme::dim()),
        ),
    ]));
    lines
}

/// 宿主机硬件配置与环境展示（架构/OS/CPU/内存/GPU/主题）。
///
/// 数据采集在 Rust 内完成：`uname`（架构/内核）与 `nvidia-smi`/`rocm-smi`
/// 经 std::process::Command 拉起，内存/CPU 读 /proc；任一探测失败显示占位
/// （n/a / 无 / 未探测），绝不阻塞渲染。结果经 OnceLock 缓存——面板每帧
/// 渲染，避免反复拉起子进程造成卡顿。
fn host_env_lines() -> Vec<Line<'static>> {
    let h = host_info();
    let theme_mode = match theme::ThemeMode::current() {
        theme::ThemeMode::Dark => "深色（Dark）",
        theme::ThemeMode::Light => "浅色（Light）",
    };
    let home = std::env::var("AIRY_HOME").unwrap_or_else(|_| "未设置".to_string());

    vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  架构      ", Style::default().fg(theme::faint())),
            Span::styled(h.arch.clone(), Style::default().fg(theme::text())),
            Span::styled("    操作系统  ", Style::default().fg(theme::faint())),
            Span::styled(h.os.clone(), Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled("  CPU       ", Style::default().fg(theme::faint())),
            Span::styled(
                format!("{}（{} 逻辑核）", h.cpu_model, h.cpu_cores),
                Style::default().fg(theme::text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("  内存      ", Style::default().fg(theme::faint())),
            Span::styled(
                format!("总量 {} · 可用 {}", h.mem_total, h.mem_avail),
                Style::default().fg(theme::text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("  GPU/加速器", Style::default().fg(theme::faint())),
            Span::styled(h.gpu.clone(), Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled("  主题模式  ", Style::default().fg(theme::faint())),
            Span::styled(theme_mode, Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  AIRY_HOME ", Style::default().fg(theme::faint())),
            Span::styled(home, Style::default().fg(theme::text())),
        ]),
    ]
}

/// 宿主机信息快照（采集一次后缓存）。
#[derive(Debug)]
struct HostInfo {
    /// 架构（uname -m，失败回退编译期常量）
    arch: String,
    /// 操作系统（/etc/os-release PRETTY_NAME 优先，其次 uname -sr）
    os: String,
    /// 内存总量（GiB，1 位小数）
    mem_total: String,
    /// 内存可用（GiB，1 位小数）
    mem_avail: String,
    /// CPU 型号（/proc/cpuinfo model name）
    cpu_model: String,
    /// CPU 逻辑核数
    cpu_cores: usize,
    /// GPU / 加速器描述
    gpu: String,
}

fn host_info() -> &'static HostInfo {
    static INFO: std::sync::OnceLock<HostInfo> = std::sync::OnceLock::new();
    INFO.get_or_init(probe_host)
}

fn probe_host() -> HostInfo {
    let arch = cmd_first_line("uname", &["-m"])
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let os = os_release_pretty()
        .or_else(|| cmd_first_line("uname", &["-sr"]))
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let (mem_total, mem_avail) = meminfo_gb();
    let (cpu_model, cpu_cores) = cpuinfo();
    let gpu = probe_gpu();
    HostInfo {
        arch,
        os,
        mem_total,
        mem_avail,
        cpu_model,
        cpu_cores,
        gpu,
    }
}

/// 运行命令并取首行输出；进程拉起失败 / 非零退出 / 空输出返回 None。
fn cmd_first_line(bin: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// 操作系统发行版名称：/etc/os-release PRETTY_NAME。
fn os_release_pretty() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for l in content.lines() {
        if let Some(v) = l.strip_prefix("PRETTY_NAME=") {
            let v = v.trim().trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 内存总量 / 可用（GiB 字符串）：/proc/meminfo MemTotal / MemAvailable。
fn meminfo_gb() -> (String, String) {
    let mut total = 0.0f64;
    let mut avail = 0.0f64;
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for l in s.lines() {
            if total == 0.0 {
                if let Some(rest) = l.strip_prefix("MemTotal:") {
                    total = parse_kb(rest);
                }
            }
            if avail == 0.0 {
                if let Some(rest) = l.strip_prefix("MemAvailable:") {
                    avail = parse_kb(rest);
                }
            }
            if total > 0.0 && avail > 0.0 {
                break;
            }
        }
    }
    (fmt_gb(total), fmt_gb(avail))
}

/// 解析 /proc/meminfo 行值（"MemTotal:   16384 kB" → 16384.0）。
fn parse_kb(rest: &str) -> f64 {
    rest.split_whitespace()
        .next()
        .and_then(|n| n.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn fmt_gb(kb: f64) -> String {
    if kb > 0.0 {
        format!("{:.1} GB", kb / 1024.0 / 1024.0)
    } else {
        "n/a".to_string()
    }
}

/// CPU 型号 + 逻辑核数：/proc/cpuinfo（model name + processor 行数），
/// 文件不可读时回退 std::thread::available_parallelism。
fn cpuinfo() -> (String, usize) {
    let mut cpu_model = String::from("n/a");
    let mut cores = 0usize;
    if let Ok(s) = std::fs::read_to_string("/proc/cpuinfo") {
        for l in s.lines() {
            if let Some(v) = l.strip_prefix("model name") {
                cpu_model = v.trim().trim_start_matches(':').trim().to_string();
            }
        }
        let logical = s.lines().filter(|l| l.starts_with("processor")).count();
        if logical > 0 {
            cores = logical;
        }
    }
    if cores == 0 {
        cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    }
    (cpu_model, cores)
}

/// GPU / 加速器探测：nvidia-smi → rocm-smi → 设备节点兜底。
fn probe_gpu() -> String {
    if let Some(name) = cmd_first_line("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]) {
        return format!("NVIDIA {}", name);
    }
    if let Some(line) = cmd_first_line("rocm-smi", &["--showproductname"]) {
        let clean = line.split(':').next_back().unwrap_or(&line).trim();
        if !clean.is_empty() {
            return format!("AMD {}", clean);
        }
    }
    if std::path::Path::new("/dev/nvidia0").exists() {
        return "NVIDIA（/dev/nvidia0）".to_string();
    }
    if std::path::Path::new("/dev/kfd").exists() {
        return "AMD ROCm（/dev/kfd）".to_string();
    }
    if std::path::Path::new("/dev/dri").exists() {
        return "有显卡（/dev/dri）".to_string();
    }
    "无 / 未探测".to_string()
}

/// 内存快照（供聊天英雄区硬件快照复用）：(总量, 可用) GiB 字符串。
pub(crate) fn mem_snapshot() -> (String, String) {
    let h = host_info();
    (h.mem_total.clone(), h.mem_avail.clone())
}

/// 架构（供聊天英雄区硬件快照复用）。
pub(crate) fn arch_snapshot() -> String {
    host_info().arch.clone()
}

/// 加速器快照（供聊天英雄区硬件快照复用）。
pub(crate) fn accelerator_snapshot() -> String {
    host_info().gpu.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_gb_formats() {
        assert_eq!(fmt_gb(16.0 * 1024.0 * 1024.0), "16.0 GB");
        assert_eq!(fmt_gb(0.0), "n/a");
    }

    #[test]
    fn parse_kb_parses_meminfo_line() {
        assert_eq!(parse_kb("   16384 kB"), 16384.0);
        assert_eq!(parse_kb("nonsense"), 0.0);
    }

    #[test]
    fn probe_functions_never_panic() {
        // 探测失败必须返回占位而非 panic（无卡/无 /proc 环境）
        let _ = arch_snapshot();
        let _ = mem_snapshot();
        let _ = accelerator_snapshot();
    }
}
