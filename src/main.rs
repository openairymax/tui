// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT TUI - Main entry point
//
// Terminal-based user interface for AgentRT.
// Communicates with the gateway via HTTP API for all runtime operations.
//
// 职责边界（0.1.18 拆分）：本文件只管启动编排（日志/主题/参数/网关连接、
// F8 exec 切 CLI）与主循环骨架（渲染节拍、鼠标捕获执行点、事件分派）；
// 键位分派在 keys，终端生命周期（日志初始化/panic 钩子/RAII 守卫）在 term。
//
// Logging:
//   TUI 使用全屏渲染，stderr 日志会直接污染画面（alt screen 共用终端）。
//   因此详细日志写入文件（$AIRY_HOME/logs/agentrt-tui.log，可 RUST_LOG 调级）；
//   用户可见的关键错误在 TUI 启动前/退出后用 eprintln 直接输出。
//     RUST_LOG=debug agentrt-tui          # verbose（写入日志文件）
//     AGENTRT_TUI_LOG=/tmp/tui.log agentrt-tui  # 自定义日志路径

mod app;
mod client;
mod engine;
mod gccp;
mod ime;
mod keys;
mod markdown;
mod memory;
mod models_cfg;
mod panels;
mod paths;
mod secrets;
mod skills;
mod term;
mod theme;
mod ui;
mod wizard;

/// `AIRY_HOME` 是进程级环境变量：所有改写它的测试（app / wizard）共享此锁
/// 串行化，否则并行时互相覆盖，写入与读取落到不同临时目录（偶发断言失败）。
#[cfg(test)]
pub(crate) mod test_env {
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 加锁（容忍前一个测试 panic 导致的毒化：锁内仅 set_var/文件读写，
    /// 毒化不影响后续测试正确性）。
    pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 独占的 AIRY_HOME 守卫：构造时持 ENV_LOCK 并建立临时目录，离开作用域
    /// （含断言 panic）时撤销环境变量、删目录、放锁。测试因此既不会并行
    /// 互相覆盖 AIRY_HOME，也不会在 /tmp 留下残留目录。
    pub(crate) struct Home {
        _lock: std::sync::MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
    }

    impl Home {
        /// `tag` 只用于临时目录前缀，便于人工排查残留。
        pub(crate) fn new(tag: &str) -> Self {
            let _lock = lock_env();
            let dir = tempfile::Builder::new()
                .prefix(&format!("airy-{}-", tag))
                .tempdir()
                .expect("临时 AIRY_HOME");
            std::env::set_var("AIRY_HOME", dir.path());
            Self { _lock, dir }
        }

        pub(crate) fn path(&self) -> &std::path::Path {
            self.dir.path()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            std::env::remove_var("AIRY_HOME");
        }
    }
}

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind},
    execute,
};
use log::{debug, error, info, warn};
use ratatui::prelude::*;
use std::io;
use std::time::{Duration, Instant};

use crate::app::{ActivePanel, App};
use crate::client::GatewayClient;
use crate::engine::sched::{Beat, Cfg, Lane, Sched};

/// AgentRT Terminal User Interface
#[derive(Parser)]
#[command(
    name = "agentrt-tui",
    version = env!("AIRY_RT_VERSION"),
    about = "AgentRT Terminal User Interface",
)]
struct Cli {
    /// Gateway API base URL（缺省：读取 $AIRY_HOME/run/gateway.port 实际端口，
    /// 再回退 http://127.0.0.1:8080）
    #[arg(long, env = "AGENTRT_GATEWAY_URL")]
    gateway_url: Option<String>,

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

/// 0.1.6h：gateway 实际端口兜底。完整启动器在端口漂移后把实际端口
/// 固化到 $AIRY_HOME/run/gateway.port；此处读取之（仅数字），缺省 8080。
/// 固定 127.0.0.1：localhost 可能解析到 ::1，而 gateway 只绑 IPv4，
/// 新安装"网关拉取不到"的常见根因。
fn default_gateway_url() -> String {
    let home = crate::paths::airy_home();
    let port = std::fs::read_to_string(home.join("run").join("gateway.port"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| "8080".to_string());
    format!("http://127.0.0.1:{port}")
}

#[tokio::main]
async fn main() {
    // ── Phase 0: Initialize logging（写入文件，避免污染 TUI 画面）──
    if let Err(e) = term::init_file_logger() {
        // 日志文件不可用时退回 stderr（仅启动期，进入 TUI 前）
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
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
    // 0.1.6h 修复：gateway 实际端口从 run/gateway.port 读取（完整启动器在
    // 端口漂移后固化；缺省 8080），并固定 127.0.0.1（localhost 可能解析
    // 到 ::1，而 gateway 只绑 IPv4 → 新安装"网关拉取不到"根因之一）。
    let gateway_url = cli.gateway_url.clone().unwrap_or_else(default_gateway_url);
    info!("CLI args parsed:");
    info!("  gateway_url = {}", gateway_url);
    info!("  agent_file  = {}", cli.agent_file);
    info!("  resume      = {}", cli.resume);
    info!("  project     = {:?}", cli.project);

    // ── Phase 1: Pre-flight checks ──
    let start_time = Instant::now();

    // Check if agent file exists
    if !std::path::Path::new(&cli.agent_file).exists() {
        warn!(
            "Agent file '{}' not found on disk, will pass name to gateway",
            cli.agent_file
        );
    }

    // ── Phase 2: Gateway client ──
    // 连接探测在 run_tui 内统一完成（health_check 2s 快速失败），
    // 避免启动阶段重复检查、离线时阻塞 UI 首帧。
    info!("Connecting to gateway at {}...", gateway_url);
    let gateway = match GatewayClient::new(&gateway_url) {
        Ok(gw) => {
            info!("HTTP client initialized (base={})", gateway_url);
            gw
        }
        Err(e) => {
            error!("Failed to create HTTP client: {}", e);
            error!("  → Check that gateway_url is valid: '{}'", gateway_url);
            error!("  → Hint: is the gateway running? Try: airymaxrt start");
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

async fn run_tui(cli: &Cli, gateway: GatewayClient) -> Result<()> {
    // T-03（P0-8）：崩溃瞬间先还原终端再打印 panic 报文，用户保住 shell。
    term::install_panic_hook();

    // Setup terminal —— RAII 守卫：acquire 失败/任何后续路径（run_app Err、
    // panic、exec）退出作用域时 Drop 完整还原，终端状态永不滞留。
    let mut guard = term::TerminalGuard::acquire()?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| {
        error!("Failed to create terminal backend: {}", e);
        e
    })?;
    // 此处起 guard 已在作用域：Terminal::new 失败经 `?` 返回时 Drop 还原
    // （修复原代码 raw mode 已开但还原链不可达的泄漏路径）。

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
    info!(
        "App state initialized. connected={}, version={:?}",
        app.connected, app.gateway_version
    );

    // Main event loop
    let result = run_app(&mut terminal, &mut app).await;

    // Restore terminal —— T-03（P0-8）：每步独立容错，绝不 `?` 短路
    // （原还原链首步 disable_raw_mode 失败即跳过后续全部步骤，终端
    // 永久滞留备用屏/raw mode）；guard Drop 兜底，此为显式提前还原，
    // 保证 switch_to_cli exec 前终端已交还。
    info!("Restoring terminal...");
    guard.restore();
    info!("Terminal restored.");

    // 2026-08-17：F8 切换到 CLI——终端已恢复，用 airy_cli 替换当前进程
    // （exec 语义，同一终端由 CLI 接管；与 CLI 的 /tui 命令构成双向互切，
    // 无进程嵌套）。exec 失败（CLI 缺失等）时保留错误提示正常退出。
    if app.switch_to_cli {
        // 原语义：AIRY_HOME/HOME 均缺失时按 PATH 查找 airy_cli（不能落到
        // paths::airy_home() 的相对回退，否则会去 ./.airymaxrt/bin 找）。
        let env_present =
            std::env::var_os("AIRY_HOME").is_some() || std::env::var_os("HOME").is_some();
        let cli_bin = if env_present {
            crate::paths::airy_home()
                .join("bin")
                .join("airy_cli")
                .to_string_lossy()
                .into_owned()
        } else {
            "airy_cli".to_string()
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

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    // 0.1.18 A 轨 §5A.3 W4：L4 调度层持有者，唯一节拍权威。时刻一律由本基准
    // 的单调毫秒注入（调度层禁止自取时钟），故节拍判定可被虚拟时钟直接驱动。
    let base = Instant::now();
    let mut sched = Sched::new(Cfg::from_env());

    // 0.1.18 A 轨 §5A.3 W3：L5 合成层持有者。全仓唯一成帧出口在此收敛，
    // 不再直调 terminal.draw。失效时刻由调度层给出——输入事件、节拍到期或
    // 面板数据落地才成帧，静止界面零绘制（"无脏跳帧"为运行期真实路径）。
    let mut compose = engine::compose::Compositor::new();

    // 0.1.18 B11（V11.1）：鼠标捕获终端序列的唯一执行点。Ctrl+M 只翻转
    // app.mouse_capture 状态，循环头检测到与当前终端态不一致时才发送
    // Enable/DisableMouseCapture——状态与序列不散落（SSoT）。
    let mut mouse_on = app.mouse_capture;

    loop {
        let now_ms = base.elapsed().as_millis() as u64;
        let beats = sched.advance(now_ms);

        if app.mouse_capture != mouse_on {
            let res = if app.mouse_capture {
                execute!(io::stdout(), EnableMouseCapture)
            } else {
                execute!(io::stdout(), DisableMouseCapture)
            };
            if let Err(e) = res {
                warn!("mouse capture switch failed: {}", e);
            }
            mouse_on = app.mouse_capture;
        }

        // 节拍启用态随界面状态同步。相位来源：chat 输入行的呼吸光标（Blink）、
        // 思考动效与 GCCP 节点旋转（Anim，仅回合进行中可见）、状态条时钟（Clock）、
        // 看板/事件流拉取（Hall，与 poll_hall 的面板判定一致）、审批轮询（Approvals）。
        sched.beat_set(Beat::Blink, app.active_panel == ActivePanel::Chat);
        sched.beat_set(Beat::Anim, app.loading || app.is_busy());
        sched.beat_set(Beat::Clock, true);
        sched.beat_set(
            Beat::Hall,
            matches!(app.active_panel, ActivePanel::Board | ActivePanel::Events),
        );
        sched.beat_set(Beat::Approvals, app.is_busy());

        // 看板/事件流面板数据拉取 + /chain、运维命令异步结果消费。上一帧超预算
        // 时抑制这类最低优先级工作：降级不阻塞渲染，也不引入第二套差异实现。
        if !sched.degraded() {
            app.poll_hall(beats.has(Beat::Hall));
            app.poll_chain();
            app.poll_ops();
        }
        // 在途请求结果消费（流式增量 / 结果落定 / 审批待决议）。空闲期内部早退
        // 无开销；busy 期它是唯一的流式与落定入口——等待期不再有独立内层循环。
        app.poll_pending(beats.has(Beat::Approvals));
        // 消费到新内容（流式增量/工具事件/结果落定/面板数据）才排队：档位即优先级，
        // 非输入档搭最近一帧，窗口内的多次落地合并为一帧最终态。
        if let Some(lane) = app.take_landed() {
            sched.push(lane);
        }
        // hall 显式刷新（F6/F7、/board、/events、SSE 推送）：把 Hall 节拍提前到
        // 下一帧，拉取时刻仍由调度器裁决。
        if app.take_hall_force() {
            sched.beat_now(Beat::Hall);
        }

        // 失效裁决（§5A.3 W4）：节拍到期或调度器清算出待办帧即置脏。此处只报
        // 「何时失效」，不预判「是否成帧」——成帧/零绘制的判据唯一落在合成层。
        let pending = sched.take_frame();
        if beats.any() || pending {
            compose.invalidate();
        }
        // 唯一成帧出口（§3.6）：每轮无条件请求；未置脏即零绘制帧（不进渲染
        // 回调、不触碰后端）。零绘制帧无耗时可言，故帧耗时仅在成帧时上报。
        // 渲染故障（§5A.3 W10）在合成层收敛，此处**不冒泡**：事件循环与 daemon
        // 会话照常推进，降级态回写 App 由状态条示警（自愈后自动消隐）。
        let started = Instant::now();
        let mut layout_us = 0_u64;
        let outcome = compose.frame(terminal, |f| {
            let laid = Instant::now();
            ui::render(f, app);
            layout_us = laid.elapsed().as_micros() as u64;
        });
        if outcome == engine::compose::FrameOutcome::Drawn {
            let total_us = started.elapsed().as_micros() as u64;
            sched.note_frame(layout_us, total_us.saturating_sub(layout_us));
        }
        app.render_degraded = compose.degraded();

        // 插入对话推进：请求完成后取一条排队输入提交（单 pending 槽，逐条消费）。
        // 计入输入档——用户消息即时成帧回显，不受合帧窗口约束。
        if app.step_insert_queue() {
            sched.push(Lane::Input);
        }

        // 事件等待：无事件时按调度器给出的最大等待挂起（空闲降频），有事件则
        // 立即唤醒并处理。终端无"失焦"语义，故不做失焦降帧。
        let wait = Duration::from_millis(sched.wait_ms());
        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Paste(text) => {
                // 输入档立即成帧：回显延迟不劣化（不受合帧窗口约束）
                sched.push(Lane::Input);
                // bracketed paste（2026-08-26）：向导编辑态插入字段；
                // 对话态在光标处插入输入框（API Key / 长文本粘贴可用）
                if app.wizard.active {
                    app.wizard.handle_paste(&text);
                } else {
                    app.input_insert_text(&text);
                }
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                sched.push(Lane::Input);
                // 键位分派 SSoT 在 keys（空闲态全量 match + busy 等待态受限
                // match，入口按 is_busy 自选）。Exit 信号即退出主循环（终端
                // 守卫 Drop 兜底还原）。
                if matches!(keys::handle(app, key).await?, keys::Flow::Exit) {
                    return Ok(());
                }
            }
            // 0.1.18 B11（V11.1）：滚轮滚动对话。捕获未开启时多数终端不
            // 投递鼠标事件；收到也安全（滚动契约钳位兜底）。修饰键语义：
            // Shift 翻页 / Ctrl 单行 / 默认 3 行（wheel_lines）。
            Event::Mouse(m) => {
                sched.push(Lane::Input);
                let shift = m.modifiers.contains(event::KeyModifiers::SHIFT);
                let ctrl = m.modifiers.contains(event::KeyModifiers::CONTROL);
                match m.kind {
                    MouseEventKind::ScrollUp => {
                        if app.active_panel == ActivePanel::Focus {
                            app.focus_scroll_up();
                        } else {
                            app.wheel_up(app.wheel_lines(shift, ctrl));
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        if app.active_panel == ActivePanel::Focus {
                            app.focus_scroll_down();
                        } else {
                            app.wheel_down(app.wheel_lines(shift, ctrl));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
