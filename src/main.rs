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
mod ime;
mod markdown;
mod memory;
mod panels;
mod secrets;
mod skills;
mod theme;
mod ui;
mod wizard;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor::{Hide, MoveTo},
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{Clear, disable_raw_mode, enable_raw_mode, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
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
    version = env!("AIRY_RT_VERSION"),
    about = "AgentRT Terminal User Interface",
)]
struct Cli {
    /// Gateway API base URL
    #[arg(long, env = "AGENTRT_GATEWAY_URL", default_value = "http://localhost:8080")]
    gateway_url: String,

    /// Agent definition file
    #[arg(short, long, default_value = "agents/main.agent.yaml")]
    agent_file: String,

    /// 会话恢复：加载上次会话历史（对标 Codex sessions / Claude /resume）
    #[arg(long, default_value_t = false)]
    resume: bool,

    /// 项目根目录（查找 AGENTS.md/CLAUDE.md 项目上下文；默认当前工作目录）
    #[arg(long)]
    project: Option<String>,
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
    info!("  AgentRT TUI v{} starting", env!("AIRY_RT_VERSION"));
    info!("══════════════════════════════════════════");

    let cli = Cli::parse();
    info!("CLI args parsed:");
    info!("  gateway_url = {}", cli.gateway_url);
    info!("  agent_file  = {}", cli.agent_file);
    info!("  resume      = {}", cli.resume);
    info!("  project     = {:?}", cli.project);

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

    // ── Phase 3c: 项目上下文文件机制（AGENTS.md / CLAUDE.md 等价物，P1）──
    let project_dir = cli.project.as_ref().map(std::path::PathBuf::from);
    if app.load_project_context(project_dir.as_deref()) {
        info!("Project context loaded (AGENTS.md equivalent)");
    } else {
        debug!("No project context file found (AGENTS.md/CLAUDE.md)");
    }

    // ── Phase 3d: 会话恢复 --resume（Codex sessions / Claude /resume，P0）──
    if cli.resume {
        let n = app.resume_session();
        info!("Session resume: restored {} messages", n);
    }

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
    /* 2.2.1.2 修复：F8 切 CLI 前必须清空主屏。退出 alt screen 后主屏
     * 仍是进入 TUI 前的画面，直接 exec airy_cli 会让 CLI 的新输出与残留
     * 内容重叠（用户反馈"界面重叠/英雄区混乱"）。清屏 + 光标归零，保证
     * CLI 从干净画布开始（正常退出路径同样受益，退出后终端无残留）。 */
    execute!(terminal.backend_mut(), Clear(ClearType::All), MoveTo(0, 0))?;
    info!("Terminal restored.");

    // 2026-08-17：F8 切换到 CLI——终端已恢复，用 airy_cli 替换当前进程
    // （exec 语义，同一终端由 CLI 接管；与 CLI 的 /tui 命令构成双向互切，
    // 无进程嵌套）。exec 失败（CLI 缺失等）时保留错误提示正常退出。
    if app.switch_to_cli {
        let home = std::env::var("AIRY_HOME").or_else(|_| {
            std::env::var("HOME").map(|h| format!("{}/.airymaxrt", h))
        });
        let cli_bin = match &home {
            Ok(h) => format!("{}/bin/airy_cli", h),
            Err(_) => "airy_cli".to_string(),
        };
        info!("Switching to CLI: {}", cli_bin);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&cli_bin).exec();
            error!("exec airy_cli failed: {}", err);
            eprintln!("\n⚠ 无法切换到 CLI（{}）\n", err);
        }
        #[cfg(windows)]
        {
            let err = std::process::Command::new(&cli_bin).spawn();
            match err {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(e) => {
                    error!("spawn airy_cli failed: {}", e);
                    eprintln!("\n⚠ 无法切换到 CLI（{}）\n", e);
                }
            }
        }
        return Ok(());
    }

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

        // 看板/事件流面板数据拉取 + /chain 异步结果消费（空闲节拍轮询）
        app.poll_hall();
        app.poll_chain();
        // 运维命令（/daemons /agents /tools /models /mem /rpc）异步结果消费
        app.poll_ops();

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
                // （↑↓ 移动 · 1-3 直达 · Enter 确认/编辑 · Esc 跳过/返回）
                _ if app.wizard.active => {
                    if app.wizard.handle_key(&key) {
                        // 向导完成：快速配置 → 应用配置并打开配置面板；跳过 → 留在对话
                        if let Some(r) = app.wizard.result.take() {
                            if r.configured {
                                app.apply_wizard_result(&r);
                                app.active_panel = ActivePanel::Config;
                                let model_txt = if r.model.is_empty() {
                                    "默认模型（网关自动回落）".to_string()
                                } else {
                                    r.model.clone()
                                };
                                let key_txt = if r.api_key_set {
                                    "API Key 已写入 secrets.env，可直接开始对话。".to_string()
                                } else {
                                    "未填写 API Key：请编辑模型配置（model.yaml）\
                                     或设置对应环境变量后开始对话。"
                                        .to_string()
                                };
                                app.add_message(
                                    app::MessageRole::System,
                                    format!(
                                        "模型配置完成：{}（{}）。{}",
                                        model_txt, r.provider, key_txt
                                    ),
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
                    }
                    continue;
                }
                KeyCode::Esc
                    if app.active_panel != ActivePanel::Chat => {
                        debug!("Panel: Esc → return to Chat");
                        app.active_panel = ActivePanel::Chat;
                    }
                // IME 拼音态：Esc 取消拼音（微信语义：清空缓冲，放弃组合）
                KeyCode::Esc if app.ime_visible() => {
                    app.ime_cancel();
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
                KeyCode::F(6) => {
                    debug!("Panel: toggle Board");
                    // 进入看板：强制立即刷新 + 订阅 hall.watch SSE 推送
                    app.active_panel = ActivePanel::Board;
                    app.force_hall_refresh();
                    app.start_hall_watch();
                }
                KeyCode::F(7) => {
                    debug!("Panel: toggle Events");
                    app.active_panel = ActivePanel::Events;
                    app.force_hall_refresh();
                    app.start_hall_watch();
                }
                // F8：切换到 CLI（airy_cli）——恢复终端后 exec 替换进程
                KeyCode::F(8) => {
                    debug!("F8: switching to CLI (airy_cli)");
                    app.switch_to_cli = true;
                    return Ok(());
                }
                // F10：内置拼音输入法 中/英 切换（词典缺失时无效果）
                KeyCode::F(10) => {
                    app.ime_toggle();
                }
                KeyCode::Enter => {
                    // 面板激活（Board/Events）：Enter = 查看选中条目详情
                    if app.active_panel == ActivePanel::Board {
                        debug!("Board: Enter → view selected decision chain");
                        app.board_view_selected();
                        continue;
                    }
                    if app.active_panel == ActivePanel::Events {
                        debug!("Events: Enter → view selected event detail");
                        app.events_view_selected();
                        continue;
                    }
                    // Alt+Enter 换行（多行输入，光标处插入），Enter 发送
                    if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.input_insert_text("\n");
                        continue;
                    }
                    // 拼音态：先提交拼音原文（随后提交整行）
                    app.ime_commit_enter();
                    let input = std::mem::take(&mut app.input);
                    app.cursor = 0;
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
                    //   - 工具权限审批：a=允许本次 · A=始终允许 · n=拒绝（Claude Code 风格）
                    // 后台请求进行中 → 每 50ms 渲染 + 轮询（等待期间可插入对话）。
                    // 任务完成后若有插入对话队列，逐条 pop 处理（单 pending 槽，
                    // 每条等其完成再处理下一条，逻辑链连续不割裂）。
                    loop {
                        // ── 等待当前请求完成（期间可输入插入对话）──
                        while app.is_busy() {
                            terminal.draw(|f| ui::render(f, app))?;
                            // 等待期间轮询按键：Ctrl+X 中止、Ctrl+Z 暂停/恢复、审批决议、
                            // 任务执行中输入文本（Enter 提交 → 插入对话队列，任务不打断）
                            if event::poll(Duration::ZERO)? {
                                if let Event::Key(key) = event::read()? {
                                    if key.kind == KeyEventKind::Press {
                                        match key.code {
                                            KeyCode::Char('x') | KeyCode::Char('X')
                                                if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                                            {
                                                app.abort_task();
                                            }
                                            KeyCode::Char('z') | KeyCode::Char('Z')
                                                if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                                            {
                                                if app.task_control == TaskControl::Paused {
                                                    app.resume_task();
                                                } else {
                                                    app.pause_task();
                                                }
                                            }
                                            // 工具级权限审批（Claude Code 风格 permission prompt）
                                            KeyCode::Char('a') | KeyCode::Char('y') => {
                                                app.approve_request("allow");
                                            }
                                            KeyCode::Char('A') => {
                                                app.approve_request("always");
                                            }
                                            KeyCode::Char('n') | KeyCode::Char('N')
                                                | KeyCode::Char('d') | KeyCode::Char('D') => {
                                                app.approve_request("deny");
                                            }
                                            // ── 2.3.13：等待回复（busy）期间 F6/F7 可切换看板/事件流 ──
                                            // 此前 F 键仅在非 busy 主循环处理，LLM 请求进行中（可能
                                            // 数十秒）按键落入 _ => {} 被吞，用户感知"看板不可操作"。
                                            // busy 中切换面板只改视图，不打断正在进行的请求。
                                            KeyCode::F(6) => {
                                                app.active_panel = ActivePanel::Board;
                                                app.force_hall_refresh();
                                                app.start_hall_watch();
                                            }
                                            KeyCode::F(7) => {
                                                app.active_panel = ActivePanel::Events;
                                                app.force_hall_refresh();
                                                app.start_hall_watch();
                                            }
                                            // F10：内置拼音输入法切换（busy 插入对话场景同样可用）
                                            KeyCode::F(10) => {
                                                app.ime_toggle();
                                            }
                                            // IME 拼音态：Esc 取消拼音（微信语义）
                                            KeyCode::Esc if app.ime_visible() => {
                                                app.ime_cancel();
                                            }
                                            // ── 插入对话（2.3.7）：任务执行中输入文本 ──
                                            KeyCode::Enter => {
                                                // 拼音态：先提交拼音原文（随后提交整行）
                                                app.ime_commit_enter();
                                                let input =
                                                    std::mem::take(&mut app.input);
                                                app.cursor = 0;
                                                if !input.trim().is_empty() {
                                                    app.queue_insert_chat(&input);
                                                }
                                            }
                                            KeyCode::Backspace => {
                                                if !app.ime_backspace() {
                                                    app.input_backspace();
                                                }
                                            }
                                            KeyCode::Delete => {
                                                app.input_delete_after();
                                            }
                                            KeyCode::Left => {
                                                // ←：IME 拼音态移动候选高亮（微信式）
                                                if app.ime_visible() {
                                                    app.ime_move_sel(-1);
                                                } else {
                                                    app.cursor_left();
                                                }
                                            }
                                            KeyCode::Right => {
                                                if app.ime_visible() {
                                                    app.ime_move_sel(1);
                                                } else {
                                                    app.cursor_right();
                                                }
                                            }
                                            KeyCode::Home => {
                                                app.cursor_home();
                                            }
                                            KeyCode::End => {
                                                app.cursor_end();
                                            }
                                            KeyCode::Char(c) => {
                                                if !app.ime_input_char(c) {
                                                    // 普通字符插入输入框（光标感知；IME 拼音态已消费时跳过）
                                                    app.input_insert_char(c);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            app.poll_pending();
                            if app.is_busy() {
                                tokio::time::sleep(Duration::from_millis(50)).await;
                            }
                        }
                        // 队列空：结束处理；否则取一条提交（submit_input 会置 busy）
                        let Some(msg) = app.insert_queue.pop_front() else {
                            break;
                        };
                        if let Err(e) = app.submit_input(&msg) {
                            log::warn!("insert queue submit failed: {}", e);
                            app.add_message(
                                app::MessageRole::System,
                                format!("插入对话处理失败：{}", e),
                            );
                        }
                    }
                    terminal.draw(|f| ui::render(f, app))?;
                }
                // ── 输入编辑：光标感知（readline 风格）──
                KeyCode::Tab => {
                    // Tab 补全：/ 命令 + 技能名（仅对话面板）
                    if app.active_panel == ActivePanel::Chat {
                        app.tab_complete();
                    }
                }
                KeyCode::Backspace => {
                    // 删除光标前一个字符（IME 拼音态：删拼音缓冲）
                    if !app.ime_backspace() {
                        app.input_backspace();
                    }
                }
                KeyCode::Delete => {
                    // 删除光标后一个字符
                    app.input_delete_after();
                }
                KeyCode::Left => {
                    // ←：IME 拼音态移动候选高亮；否则光标左移（微信式）
                    if app.ime_visible() {
                        app.ime_move_sel(-1);
                    } else {
                        app.cursor_left();
                    }
                }
                KeyCode::Right => {
                    // →：IME 拼音态移动候选高亮；否则光标右移（微信式）
                    if app.ime_visible() {
                        app.ime_move_sel(1);
                    } else {
                        app.cursor_right();
                    }
                }
                KeyCode::Home => {
                    // Home：光标到输入开头
                    app.cursor_home();
                }
                KeyCode::End => {
                    // End：输入非空 → 光标到末尾；空输入 → 回到底部（最新消息）
                    if app.input.is_empty() {
                        app.scroll_offset = 0;
                    } else {
                        app.cursor_end();
                    }
                }
                KeyCode::Char('a') | KeyCode::Char('A')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    // Ctrl+A：光标到输入开头
                    app.cursor_home();
                }
                KeyCode::Char('e') | KeyCode::Char('E')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    // Ctrl+E：光标到输入末尾
                    app.cursor_end();
                }
                KeyCode::Char('w') | KeyCode::Char('W')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    // Ctrl+W：删除光标前一个词
                    app.input_delete_word_before();
                }
                KeyCode::Char('u') | KeyCode::Char('U')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    // Ctrl+U：删除光标前全部内容
                    app.input_delete_to_start();
                }
                // Ctrl+T：新建会话 tab（多会话；请求进行中不可用，见 app 守卫）
                KeyCode::Char('t') | KeyCode::Char('T')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    app.new_session_tab();
                }
                // Alt+1..9：切换会话 tab（Alt+1 = 主会话；优先于面板数字过滤）
                KeyCode::Char(c)
                    if key.modifiers.contains(event::KeyModifiers::ALT)
                        && c.is_ascii_digit()
                        && c != '0' =>
                {
                    app.switch_tab((c as u8 - b'0') as usize);
                }
                KeyCode::Char(c) if app.active_panel == ActivePanel::Board => {
                    // F6 看板：0=全部 · 1-6=状态过滤（completed/running/pending/scheduled/failed/canceled）
                    match c {
                        '0' => app.board_set_filter(""),
                        '1' => app.board_set_filter("completed"),
                        '2' => app.board_set_filter("running"),
                        '3' => app.board_set_filter("pending"),
                        '4' => app.board_set_filter("scheduled"),
                        '5' => app.board_set_filter("failed"),
                        '6' => app.board_set_filter("canceled"),
                        _ => {}
                    }
                }
                KeyCode::Char(c) if app.active_panel == ActivePanel::Events => {
                    // F7 事件流：0=全部 · 1-7=类别过滤（blueprint/command/progress/result/issue/verify/chain）
                    match c {
                        '0' => app.events_set_filter(""),
                        '1' => app.events_set_filter("blueprint"),
                        '2' => app.events_set_filter("command"),
                        '3' => app.events_set_filter("progress"),
                        '4' => app.events_set_filter("result"),
                        '5' => app.events_set_filter("issue"),
                        '6' => app.events_set_filter("verify"),
                        '7' => app.events_set_filter("chain"),
                        _ => {}
                    }
                }
                KeyCode::Char(c) => {
                    // 普通字符插入到光标位置（IME 拼音态：先经拼音输入法）
                    if !app.ime_input_char(c) {
                        app.input_insert_char(c);
                    }
                }
                KeyCode::Up => {
                    // F6/F7 面板：↑ 移动选中光标（循环）；其余场景滚对话/浏览历史
                    if app.active_panel == ActivePanel::Board {
                        app.board_cursor_up();
                    } else if app.active_panel == ActivePanel::Events {
                        app.events_cursor_up();
                    } else if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.history_prev();
                    } else {
                        app.scroll_up();
                    }
                }
                KeyCode::Down => {
                    if app.active_panel == ActivePanel::Board {
                        app.board_cursor_down();
                    } else if app.active_panel == ActivePanel::Events {
                        app.events_cursor_down();
                    } else if key.modifiers.contains(event::KeyModifiers::ALT) {
                        app.history_next();
                    } else {
                        app.scroll_down();
                    }
                }
                KeyCode::PageUp => {
                    // PgUp：IME 拼音态翻上一页（微信式）；否则滚动上翻
                    if app.ime_visible() {
                        app.ime_page_flip(-1);
                    } else {
                        app.scroll_page_up();
                    }
                }
                KeyCode::PageDown => {
                    // PgDn：IME 拼音态翻下一页（微信式）；否则滚动下翻
                    if app.ime_visible() {
                        app.ime_page_flip(1);
                    } else {
                        app.scroll_page_down();
                    }
                }
                _ => {}
            }
        }
    }
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}