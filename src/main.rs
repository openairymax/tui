// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT TUI - Main entry point
//
// Terminal-based user interface for AgentRT.
// Communicates with the gateway via HTTP API for all runtime operations.
//
// Logging:
//   TUI 使用全屏渲染，stderr 日志会直接污染画面（alt screen 共用终端）。
//   因此详细日志写入文件（$AIRY_HOME/logs/agentrt-tui.log，可 RUST_LOG 调级）；
//   用户可见的关键错误在 TUI 启动前/退出后用 eprintln 直接输出。
//     RUST_LOG=debug agentrt-tui          # verbose（写入日志文件）
//     AGENTRT_TUI_LOG=/tmp/tui.log agentrt-tui  # 自定义日志路径

mod app;
mod client;
mod gccp;
mod markdown;
mod memory;
mod panels;
mod skills;
mod theme;
mod ui;
mod wizard;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor::Hide,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use log::{debug, error, info, warn};
use ratatui::prelude::*;
use std::io;
use std::time::{Duration, Instant};

use crate::app::{ActivePanel, App};
use crate::client::GatewayClient;
use crate::gccp::TaskControl;

/// AgentRT Terminal User Interface
#[derive(Parser)]
#[command(
    name = "agentrt-tui",
    version = env!("CARGO_PKG_VERSION"),
    about = "AgentRT Terminal User Interface",
)]
struct Cli {
    /// Gateway API base URL
    #[arg(long, env = "AGENTRT_GATEWAY_URL", default_value = "http://localhost:8080")]
    gateway_url: String,

    /// Agent definition file
    #[arg(short, long, default_value = "agents/main.agent.yaml")]
    agent_file: String,
}

#[tokio::main]
async fn main() {
    // ── Phase 0: Initialize logging（写入文件，避免污染 TUI 画面）──
    if let Err(e) = init_file_logger() {
        // 日志文件不可用时退回 stderr（仅启动期，进入 TUI 前）
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("warn")
        )
        .format_timestamp_millis()
        .target(env_logger::Target::Stderr)
        .init();
        eprintln!("⚠ 日志文件不可用（{}），回退 stderr（warn 级）", e);
    }

    // 主题初始化（AIRY_TUI_THEME / COLORFGBG 自动适配浅色终端）
    theme::init_from_env();

    info!("══════════════════════════════════════════");
    info!("  AgentRT TUI v{} starting", env!("CARGO_PKG_VERSION"));
    info!("══════════════════════════════════════════");

    let cli = Cli::parse();
    info!("CLI args parsed:");
    info!("  gateway_url = {}", cli.gateway_url);
    info!("  agent_file  = {}", cli.agent_file);

    // ── Phase 1: Pre-flight checks ──
    let start_time = Instant::now();

    // Check if agent file exists
    if !std::path::Path::new(&cli.agent_file).exists() {
        warn!("Agent file '{}' not found on disk, will pass name to gateway",
              cli.agent_file);
    }

    // ── Phase 2: Gateway client ──
    // 连接探测在 run_tui 内统一完成（health_check 2s 快速失败），
    // 避免启动阶段重复检查、离线时阻塞 UI 首帧。
    info!("Connecting to gateway at {}...", cli.gateway_url);
    let gateway = match GatewayClient::new(&cli.gateway_url) {
        Ok(gw) => {
            info!("HTTP client initialized (base={})", cli.gateway_url);
            gw
        }
        Err(e) => {
            error!("Failed to create HTTP client: {}", e);
            error!("  → Check that gateway_url is valid: '{}'", cli.gateway_url);
            error!("  → Hint: is the gateway running? Try: docker compose up -d");
            eprintln!("\n❌ Cannot create HTTP client: {}\n", e);
            std::process::exit(1);
        }
    };

    // ── Phase 3: Start TUI ──
    info!("Setting up terminal (raw mode + alternate screen)...");

    let tui_result = run_tui(&cli, gateway).await;

    // ── Phase 4: Shutdown diagnostics ──
    let total_time = start_time.elapsed();
    match &tui_result {
        Ok(()) => info!("TUI exited normally after {:?}", total_time),
        Err(e) => {
            error!("TUI exited with error after {:?}: {}", total_time, e);
            error!("  → Full error chain: {:#}", e);
            eprintln!("\n❌ TUI error: {}\n", e);
        }
    }
    info!("AgentRT TUI shutdown complete");

    if let Err(_e) = tui_result {
        std::process::exit(1);
    }
}

/// 初始化文件日志（避免 stderr 污染全屏 TUI）。
///
/// 路径优先级：`AGENTRT_TUI_LOG` → `$AIRY_HOME/logs/agentrt-tui.log` →
/// `$HOME/.airymaxrt/logs/agentrt-tui.log`。
fn init_file_logger() -> Result<(), Box<dyn std::error::Error>> {
    let path = if let Ok(p) = std::env::var("AGENTRT_TUI_LOG") {
        p
    } else {
        let home = std::env::var("AIRY_HOME").or_else(|_| {
            std::env::var("HOME").map(|h| format!("{}/.airymaxrt", h))
        })?;
        format!("{}/logs/agentrt-tui.log", home)
    };

    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    )
    .format_timestamp_millis()
    .target(env_logger::Target::Pipe(Box::new(file)))
    .init();
    Ok(())
}

async fn run_tui(cli: &Cli, gateway: GatewayClient) -> Result<()> {
    // Setup terminal
    enable_raw_mode()
        .map_err(|e| {
            error!("Failed to enable raw mode: {}", e);
            error!("  → This usually means you're not in a real terminal.");
            error!("  → Try running in a terminal emulator, not an IDE panel.");
            e
        })?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)
        .map_err(|e| {
            error!("Failed to enter alternate screen: {}", e);
            e
        })?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| {
            error!("Failed to create terminal backend: {}", e);
            e
        })?;

    info!("Terminal initialized. Starting event loop.");

    // Create app state
    let mut app = App::new(&cli.agent_file, gateway);

    // ── Phase 3b: Deferred connection check in app ──
    if let Err(e) = app.check_connection().await {
        debug!("Initial connection check returned error (non-fatal): {}", e);
    }
    info!("App state initialized. connected={}, version={:?}",
          app.connected, app.gateway_version);

    // Main event loop
    let result = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    info!("Restoring terminal...");
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    info!("Terminal restored.");

    result?;
    Ok(())
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    // 空闲帧率：100ms 一帧。呼吸灯光标、宿主机时间、thinking 动效都依赖
    // 持续重绘——阻塞式 event::read() 会让画面在无按键时静止。
    let idle_frame = Duration::from_millis(100);

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // 无按键事件时定时重绘（保持动态视觉），有事件则立即处理
        if !event::poll(idle_frame)? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        info!("User pressed Ctrl+C, shutting down...");
                        app.shutdown().await?;
                        return Ok(());
                    }
                // Ctrl+X：人工中止当前后台请求（任务执行/对话等待）
                KeyCode::Char('x') | KeyCode::Char('X')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        if app.is_busy() {
                            info!("User pressed Ctrl+X, aborting pending request");
                            app.abort_task();
                        }
                    }
                // Ctrl+Z：暂停/恢复后台请求等待（请求继续在网关执行）
                KeyCode::Char('z') | KeyCode::Char('Z')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        if app.is_busy() {
                            info!("User pressed Ctrl+Z, toggling pause");
                            if app.task_control == TaskControl::Paused {
                                app.resume_task();
                            } else {
                                app.pause_task();
                            }
                        }
                    }
                // 首次启动向导激活：按键全部交给向导
                // （↑↓ 移动 · 1-3 直达 · Enter 确认 · Esc 跳过）
                _ if app.wizard.active => {
                    if app.wizard.handle_key(&key) {
                        // 向导完成：手动配置模型 → 打开配置面板；跳过 → 留在对话
                        if app.wizard.result.is_some_and(|r| r.configured) {
                            app.active_panel = ActivePanel::Config;
                            app.add_message(
                                app::MessageRole::System,
                                "已选择「手动配置模型」。请编辑模型配置（model.yaml）\
                                 并设置对应 API Key 环境变量，或按 F2 查看配置。"
                                    .to_string(),
                            );
                        } else {
                            app.add_message(
                                app::MessageRole::System,
                                "欢迎使用 AirymaxRT！已跳过模型配置，\
                                 输入 /hiairy 可随时重新打开首次启动向导。"
                                    .to_string(),
                            );
                        }
                    }
                    continue;
                }
                KeyCode::Esc
                    if app.active_panel != ActivePanel::Chat => {
                        debug!("Panel: Esc → return to Chat");
                        app.active_panel = ActivePanel::Chat;
                    }
                KeyCode::F(1) => {
                    debug!("Panel: toggle Help");
                    app.toggle_panel(ActivePanel::Help);
                }
                KeyCode::F(2) => {
                    debug!("Panel: toggle Config");
                    app.toggle_panel(ActivePanel::Config);
                }
                KeyCode::F(3) => {
                    debug!("Panel: toggle Logs");
                    app.toggle_panel(ActivePanel::Logs);
                }
                KeyCode::F(4) => {
                    debug!("Panel: toggle Memory");
                    app.toggle_panel(ActivePanel::Memory);
                }
                KeyCode::F(5) => {
                    debug!("Panel: toggle Plugins");
                    app.toggle_panel(ActivePanel::Plugins);
                }
                KeyCode::Enter => {
                    // Alt+Enter 换行（多行输入），Enter 发送
                    if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.input.push('\n');
                        continue;
                    }
                    let input = std::mem::take(&mut app.input);
                    debug!("User submitted input: '{}' ({} chars)",
                           truncate_str(&input, 80), input.len());
                    if let Err(e) = app.submit_input(&input) {
                        warn!("submit_input error: {}", e);
                        app.add_message(
                            app::MessageRole::System,
                            format!("Error: {}", e),
                        );
                    }
                    // 后台 LLM 请求进行中 → 每 50ms 渲染 + 轮询：
                    //   - thinking... 动效（chat.rs / ui.rs 按时间取帧，50ms 一帧更丝滑）
                    //   - 回复到达后自动上屏（add_message 自动回到底部）
                    //   - Ctrl+X 中止 / Ctrl+Z 暂停（等待期间可人工控制）
                    while app.is_busy() {
                        terminal.draw(|f| ui::render(f, app))?;
                        // 等待期间轮询按键：Ctrl+X 中止、Ctrl+Z 暂停/恢复
                        if event::poll(Duration::ZERO)? {
                            if let Event::Key(key) = event::read()? {
                                if key.kind == KeyEventKind::Press {
                                    if (key.code == KeyCode::Char('x')
                                        || key.code == KeyCode::Char('X'))
                                        && key.modifiers.contains(event::KeyModifiers::CONTROL)
                                    {
                                        app.abort_task();
                                    } else if (key.code == KeyCode::Char('z')
                                        || key.code == KeyCode::Char('Z'))
                                        && key.modifiers.contains(event::KeyModifiers::CONTROL)
                                    {
                                        if app.task_control == TaskControl::Paused {
                                            app.resume_task();
                                        } else {
                                            app.pause_task();
                                        }
                                    }
                                }
                            }
                        }
                        app.poll_pending();
                        if app.is_busy() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                    terminal.draw(|f| ui::render(f, app))?;
                }
                KeyCode::Backspace => {
                    app.input.pop();
                }
                KeyCode::Char(c) => {
                    app.input.push(c);
                }
                KeyCode::Up => {
                    // Alt+↑ 浏览输入历史；普通 ↑ 滚动对话
                    if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.history_prev();
                    } else {
                        app.scroll_up();
                    }
                }
                KeyCode::Down => {
                    // Alt+↓ 浏览输入历史（下一条）；普通 ↓ 滚动对话
                    if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.history_next();
                    } else {
                        app.scroll_down();
                    }
                }
                KeyCode::PageUp => {
                    app.scroll_page_up();
                }
                KeyCode::PageDown => {
                    app.scroll_page_down();
                }
                KeyCode::End => {
                    // 一键回到底部（最新消息）
                    app.scroll_offset = 0;
                }
                _ => {}
            }
        }
    }
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}