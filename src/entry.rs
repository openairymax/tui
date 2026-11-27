// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT TUI - Main entry point
//
// Terminal-based user interface for AgentRT.
// Communicates with the gateway through the event pipe (airy_cli) for all
// runtime operations and observation reads.
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

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind},
    execute,
};
use log::{debug, error, info, warn};
use ratatui::prelude::*;
use ratatui::TerminalOptions;
use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

use crate::app::{ActivePanel, App};
use crate::budget::FIRST_FRAME_BUDGET_MS;
use crate::engine::boot::Boot;
use crate::engine::sched::{Beat, Cfg, Lane, Sched};
// 本模块从 main.rs 迁入库后，原先在 crate 根可直接命名的模块需显式引入；
// 显式 use 优先于 ratatui 前奏里的同名 `style` 模块。
use crate::engine::backend::{SyncCaps, WriteLevel};
use crate::{engine, keys, present, style};

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

    /// Agent 定义文件路径或裸 agent id（缺省：$AIRY_HOME/agents/main.agent.yaml，
    /// 两者都不存在时交由 gateway 用内置默认 spec）
    #[arg(short, long, env = "AIRY_AGENT_FILE")]
    agent_file: Option<String>,

    /// 会话恢复：加载上次会话历史（对标 Codex sessions / Claude /resume）
    #[arg(long, default_value_t = false)]
    resume: bool,

    /// 项目根目录（查找 AGENTS.md/CLAUDE.md 项目上下文；默认当前工作目录）
    #[arg(long)]
    project: Option<String>,

    /// 进程存活探针（0.1.20 P2-8）：输出一行自检帧后退出，不进入 TUI。
    /// 仅承诺可启动、可退出、可被 health probe 观察到；无渲染、无交互。
    #[arg(long, default_value_t = false)]
    probe: bool,
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

/// A-9 H4：父进程（airy_cli）死亡联动。启动器被强杀时 CLI 经 PDEATHSIG
/// 退出，TUI 若不做同样绑定就会在重启窗口内残留为孤儿并抢占终端。Linux 下
/// 绑定 PR_SET_PDEATHSIG(SIGTERM)，并复查 getppid 处理「设置前父已死」竞态。
#[cfg(target_os = "linux")]
fn bind_parent_death() {
    unsafe {
        let ppid = libc::getppid();
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
            warn!(
                "PR_SET_PDEATHSIG 设置失败: {}（父死不自退，退化为手动关闭）",
                io::Error::last_os_error()
            );
            return;
        }
        if libc::getppid() != ppid {
            eprintln!("agentrt-tui: 启动器已退出，停止进入交互界面");
            std::process::exit(0);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_parent_death() {}

/// D-11（0.1.21）：显式给出的 agent_file 解析不到时必须失败——静默降级会让
/// 调用方以为自己指定了角色，实际跑的是默认角色。缺省（未给 flag/env）时
/// 依次尝试安装态 `$AIRY_HOME/agents/main.agent.yaml`、裸 id `main`，都不
/// 存在则不下发 agent_file，由 gateway 用内置默认 spec（S-P0-01：发布包
/// 不再依赖 cwd 相对的 `agents/main.agent.yaml` 幽灵路径）。
fn resolve_agent_file(arg: Option<&str>) -> Option<String> {
    let home = crate::paths::airy_home();
    if let Some(arg) = arg.map(str::trim).filter(|s| !s.is_empty()) {
        let p = std::path::Path::new(arg);
        if let Ok(abs) = p.canonicalize() {
            return Some(abs.to_string_lossy().to_string());
        }
        let alt = if arg.contains('/') {
            home.join(arg)
        } else {
            home.join("agents").join(format!("{arg}.json"))
        };
        if let Ok(abs) = alt.canonicalize() {
            return Some(abs.to_string_lossy().to_string());
        }
        let code = engine::exit::Code::Usage;
        engine::exit::record("agent", code);
        error!("agent_file 无法解析：{}（AIRY_HOME={}）", arg, home.display());
        eprintln!("❌ 找不到 agent 文件：{}", arg);
        eprintln!("   已尝试：{}", p.display());
        eprintln!("   已尝试：{}", alt.display());
        eprintln!("   请传入存在的绝对路径，或改用 bare agent id。");
        eprintln!("❌ {}（退出码 {}）", code.human(), code.code());
        std::process::exit(code.code());
    }
    let dflt = home.join("agents").join("main.agent.yaml");
    if let Ok(abs) = dflt.canonicalize() {
        return Some(abs.to_string_lossy().to_string());
    }
    let dflt_json = home.join("agents").join("main.json");
    if let Ok(abs) = dflt_json.canonicalize() {
        return Some(abs.to_string_lossy().to_string());
    }
    info!("未找到 agent 定义（{}），交由 gateway 按默认 spec 运行", dflt.display());
    None
}

/// 网关地址解析（S-P1-01）：`--gateway-url` > `AGENTRT_GATEWAY_URL` >
/// `AIRY_GATEWAY_URL`（启动器/CLI 的既有变量名）> run/gateway.port 兜底。
fn pick_gateway_url(cli_val: Option<&str>) -> String {
    if let Some(v) = cli_val.map(str::trim).filter(|s| !s.is_empty()) {
        return v.to_string();
    }
    for key in ["AGENTRT_GATEWAY_URL", "AIRY_GATEWAY_URL"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    default_gateway_url()
}

#[tokio::main]
pub async fn run() {
    bind_parent_death();
    // 首帧预算的计时基准：进程进入 main 的第一时刻（§6.1 #1）。
    let boot = Boot::mark();

    // 参数解析先于一切门禁：探针必须在 TTY 检查之前裁决——它存在的理由
    // 正是「非 TTY 环境（CI／容器／串口）也要能判定二进制存活」，排在门禁
    // 之后则永远走不到（0.1.20 P2-8）。
    let mut cli = Cli::parse();
    if cli.probe {
        let url = pick_gateway_url(cli.gateway_url.as_deref());
        std::process::exit(crate::probe::run(&url));
    }

    // ── 0.1.21 手册 E：同一 AIRY_HOME 单实例 ──
    // 锁在参数解析/探针之后、任何终端副作用（日志文件、替代屏）之前获取：
    // 探针必须随时可用，而第二个交互实例要在进入替代屏前得到确定失败，
    // 而不是注册不上网关后静默悬死。
    let _single = match engine::single::acquire() {
        Ok(guard) => guard,
        Err(e) => {
            let code = engine::exit::Code::Soft;
            engine::exit::record("single", code);
            eprintln!("❌ {e}");
            eprintln!("❌ {}（退出码 {}）", code.human(), code.code());
            std::process::exit(code.code());
        }
    };

    // §6.1 #3：非 TTY 拒进。全屏渲染依赖可读写的交互终端；管道或重定向下
    // 强行进入只会把控制序列写进日志、并在死终端上反复失败。这里在任何
    // 副作用（日志文件、替代屏）之前裁决，并给出可行动的替代路径。
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let code = engine::exit::Code::Usage;
        engine::exit::record("gate", code);
        eprintln!("❌ AirymaxRT TUI 需要交互式终端（stdin/stdout 均为 TTY）。");
        eprintln!("   当前为标准输入/输出被重定向或接入管道。");
        eprintln!("   改用行式对话：airy_cli（-p \"问题\"）");
        eprintln!("   或经启动器进入：airymaxrt");
        eprintln!("❌ {}（退出码 {}）", code.human(), code.code());
        std::process::exit(code.code());
    }

    // ── Phase 0: Initialize logging（写入文件，避免污染 TUI 画面）──
    if let Err(e) = engine::term::init_file_logger() {
        // 日志文件不可用时退回 stderr（仅启动期，进入 TUI 前）
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .format_timestamp_millis()
            .target(env_logger::Target::Stderr)
            .init();
        eprintln!("⚠ 日志文件不可用（{}），回退 stderr（warn 级）", e);
    }

    // 主题初始化（AIRY_TUI_THEME / COLORFGBG 自动适配浅色终端）
    style::init_from_env();

    info!("══════════════════════════════════════════");
    info!("  AgentRT TUI v{} starting", env!("AIRY_RT_VERSION"));
    info!("══════════════════════════════════════════");

    // 0.1.6h 修复：gateway 实际端口从 run/gateway.port 读取（完整启动器在
    // 端口漂移后固化；缺省 8080），并固定 127.0.0.1（localhost 可能解析
    // 到 ::1，而 gateway 只绑 IPv4 → 新安装"网关拉取不到"根因之一）。
    let gateway_url = pick_gateway_url(cli.gateway_url.as_deref());
    info!("CLI args parsed:");
    info!("  gateway_url = {}", gateway_url);
    info!("  agent_file  = {:?}", cli.agent_file);
    info!("  resume      = {}", cli.resume);
    info!("  project     = {:?}", cli.project);

    // ── Phase 1: Pre-flight checks ──
    let start_time = Instant::now();

    // Check if agent file exists（D-11：显式值解析不到即失败；缺省可缺省）
    cli.agent_file = resolve_agent_file(cli.agent_file.as_deref());
    info!("  agent_file resolved = {:?}", cli.agent_file);

    // ── Phase 2: Gateway 连接 ──
    // 传输全部由事件管道承载（D-15）：启动期探测在 run_tui 内经 ping 查询帧
    // 完成，离线时同样不阻塞首帧。
    info!("Using gateway at {} via event pipe", gateway_url);

    // ── Phase 3: Start TUI ──
    info!("Setting up terminal (raw mode + renderer)...");

    let tui_result = run_tui(&cli, &boot).await;

    // TERM/HUP 收敛（N-1 取证）：run_app 只在标志到达后正常返回，终端已由
    // RAII 守卫还原；此处落盘死因并按原信号自绝，父进程仍见 WIFSIGNALED。
    if let Some(sig) = engine::signal::take() {
        error!(
            "收到终止信号 SIG{}，终端已还原，按原信号退出",
            engine::signal::name(sig)
        );
        engine::exit::record_sig(sig);
        engine::signal::reraise(sig);
    }

    // ── Phase 4: Shutdown diagnostics ──
    let total_time = start_time.elapsed();
    match &tui_result {
        Ok(code) => info!("TUI exited normally ({}) after {:?}", code.human(), total_time),
        Err(e) => {
            error!("TUI exited with error after {:?}: {}", total_time, e);
            error!("  → Full error chain: {:#}", e);
            eprintln!("\n❌ TUI error: {}\n", e);
        }
    }
    info!("AgentRT TUI shutdown complete");

    match tui_result {
        Ok(code) => {
            engine::exit::record("run", code);
            if code.code() != 0 {
                std::process::exit(code.code());
            }
        }
        Err(_e) => {
            let code = engine::exit::Code::Internal;
            engine::exit::record("run", code);
            eprintln!("❌ {}（退出码 {}）\n", code.human(), code.code());
            std::process::exit(code.code());
        }
    }
}

async fn run_tui(cli: &Cli, boot: &Boot) -> Result<engine::exit::Code> {
    // T-03（P0-8）：崩溃瞬间先还原终端再打印 panic 报文，用户保住 shell。
    engine::term::install_panic_hook();
    // N-1：TERM/HUP 只置标志，主循环收敛退出后由 run() 还原并按原信号自绝。
    engine::signal::install();

    // Setup terminal —— RAII 守卫：acquire 失败/任何后续路径（run_app Err、
    // panic、exec）退出作用域时 Drop 完整还原，终端状态永不滞留。
    // §5.5 #7：请求形态来自 AIRY_TUI_SCREEN；备用屏不可用时守卫内部已
    // 回滚内联，mode 为实际生效形态，视口据此构造。
    let requested = engine::screen::Mode::from_env();
    let (mut guard, mode) = engine::term::TerminalGuard::acquire(requested)?;
    // §5.5 #1：能力探测与首帧并发；结论在首帧后回收（晚到热修正）。
    // 声明在 guard 之后，Drop 逆序保证探测线程先于终端还原退出。
    let mut probe = engine::term::Probe::spawn();

    // 出帧写入统一经帧编码后端（X2-12a）：一帧在内存中编码完成后一次性
    // 写出；分片在转义序列边界处切（frame_io），不再按任意字节数截断。
    let backend = engine::backend::BufferedBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: mode.viewport(),
        },
    )
    .map_err(|e| {
        error!("Failed to create terminal backend: {}", e);
        e
    })?;
    info!(
        "engine/screen: 渲染器 {}（请求 {}）",
        mode.label(),
        requested.label()
    );
    // 此处起 guard 已在作用域：Terminal 构造失败经 `?` 返回时 Drop 还原
    // （修复原代码 raw mode 已开但还原链不可达的泄漏路径）。

    info!("Terminal initialized. Starting event loop.");

    // Create app state
    let mut app = App::new(cli.agent_file.as_deref().unwrap_or(""));
    // 事件管道（X1-26c）：运行轮与观察面读均经 airy_cli 事件流下发。挂接
    // 失败不拦启动，记录原因（诊断面板可读）；此后交互按管道不可用降级。
    if !app.attach_lane() {
        info!("Event pipe unavailable; interactive requests stay offline");
    }

    // 会话布局恢复（0.1.21 D-14）：sessions.json 只存 tab 结构与标题，
    // 消息正文的唯一权威在 mem_d；历史由主循环在水合就绪后按 conv 回填。
    // 文件缺失/损坏保持单 tab（对话仍可从 mem_d 还原）。
    app.restore_tabs();
    // D-13-04：任务态从 task_state.json 独立恢复（阻断态才提示三选）。
    app.restore_task_state();

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

    // ── Phase 3b: 启动期连接探测与首帧并发（§6.1 #1）──
    // 健康探针异步落地，结果由主循环 poll_boot 热修正连接态；首绘不等网络。
    app.probe_connection();
    // C-19：槽位 effective 快照同批并发（徽标判据，静默落地）。
    app.probe_slots();
    info!("App state initialized. boot probe dispatched");

    // Main event loop
    let result = run_app(&mut terminal, &mut app, boot, probe.as_mut()).await;

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
        // 解析规则与事件源共用一处（engine::event::pipe::cli_bin）：AIRY_HOME/
        // HOME 均缺失时按 PATH 查找，不落 paths::airy_home() 的相对回退。
        let cli_bin = engine::event::pipe::cli_bin();
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
        return Ok(engine::exit::Code::Ok);
    }

    result
}

/// 终端 IO 失败的统一出口（§5.5 #6）：死亡即静默退出（管道码），其余照常冒泡。
fn leave_on_dead(err: io::Error) -> Result<engine::exit::Code> {
    match engine::guard::probe(&err) {
        Some(dead) => {
            warn!("engine/guard: 终端已消失（{}），结束主循环", dead.code());
            Ok(engine::exit::Code::Pipe)
        }
        None => Err(err.into()),
    }
}

async fn run_app<B: Backend + SyncCaps + WriteLevel>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    boot: &Boot,
    mut probe: Option<&mut engine::term::Probe>,
) -> Result<engine::exit::Code> {
    // 0.1.18 A 轨 §5A.3 W4：L4 调度层持有者，唯一节拍权威。时刻一律由本基准
    // 的单调毫秒注入（调度层禁止自取时钟），故节拍判定可被虚拟时钟直接驱动。
    let base = Instant::now();
    let mut sched = Sched::new(Cfg::from_env());
    // X8-06b：帧日志是可选诊断通道，开关关闭或路径不可用时整个对象不存在，
    // 对帧节拍与常驻内存均无影响。
    let mut frame_log = engine::framelog::FrameLog::from_env();
    // §6.1 #1：首帧耗时只上报一次；超预算告警，不阻塞、不重试。
    let mut first_drawn = false;

    // 0.1.18 A 轨 §5A.3 W3：L5 合成层持有者。全仓唯一成帧出口在此收敛，
    // 不再直调 terminal.draw。失效时刻由调度层给出——输入事件、节拍到期或
    // 面板数据落地才成帧，静止界面零绘制（"无脏跳帧"为运行期真实路径）。
    let mut compose = engine::compose::Compositor::new();

    // 0.1.18 B11（V11.1）：鼠标捕获终端序列的唯一执行点。Ctrl+M 只翻转
    // app.mouse_capture 状态，循环头检测到与当前终端态不一致时才发送
    // Enable/DisableMouseCapture——状态与序列不散落（SSoT）。
    let mut mouse_on = app.mouse_capture;

    loop {
        // N-1：TERM/HUP 到达即收敛退出；终端还原由 run_tui 返回后的 RAII 守卫
        // 完成，真实死因由 run() 按原信号重发。
        if engine::signal::pending().is_some() {
            return Ok(engine::exit::Code::Ok);
        }
        let now_ms = base.elapsed().as_millis() as u64;
        // 单调时刻注入（U12）：应用层不自行读时钟，健康时长/时间线/诊断
        // 快照全部以主循环时刻为准。
        app.set_clock(now_ms);
        // 会话恢复回放（§6.1 #2）：100ms 节流逐条上屏，排空后落「已补账」。
        app.pump_replay(now_ms);
        // 历史还原（D-14）：mem_d 水合就绪后一次性按 conv 回填各 tab。
        if app.poll_history_restore() {
            sched.push(Lane::Panel);
        }
        let beats = sched.advance(now_ms);

        // 瞬态提示到期清退（0.1.19 L7 U7，TUI 方案 §6.3 #4）：队列只按
        // 注入时刻裁决，有提示消隐即补一帧，页面不停在过期内容上。
        let now = Instant::now();
        if app.toast_expire(now) {
            sched.push(Lane::Panel);
        }

        // leader 提示浮层到期（0.1.19 L7 U8，TUI 方案 §6.4 #7）：前缀键静置
        // 满 500ms 才浮出，浮出即补一帧。
        if app.leader_hint_due(now) {
            sched.push(Lane::Panel);
        }

        if app.mouse_capture != mouse_on {
            let res = if app.mouse_capture {
                execute!(io::stdout(), EnableMouseCapture)
            } else {
                execute!(io::stdout(), DisableMouseCapture)
            };
            if let Err(e) = res {
                if engine::guard::probe(&e).is_some() {
                    return leave_on_dead(e);
                }
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
        // 依赖健康保活（U12 §6.12 #1/#3）：5s 一次的轻量端到端探测。
        // 与渲染降级解耦——降级期更需要知道依赖何时恢复。
        sched.beat_set(Beat::Health, true);

        // 看板/事件流面板数据拉取 + /chain、运维命令异步结果消费。上一帧超预算
        // 时抑制这类最低优先级工作：降级不阻塞渲染，也不引入第二套差异实现。
        if !sched.degraded() {
            app.poll_hall(beats.has(Beat::Hall));
            app.poll_chain();
            app.poll_ops();
        }
        // 健康探测：节拍到期发起一轮，结果按就绪消费（不阻塞主循环）。
        if beats.has(Beat::Health) {
            app.start_health(false);
        }
        app.poll_health(now_ms);
        // 演化指标拉取结果消费（/evolution 打开期间续拉，空闲期早退无开销）。
        app.poll_evo(now_ms);
        // 自托管快照消费（/selfhost 或 Alt+S 打开期间续拉）。
        app.poll_selfhost(now_ms);
        // 批次五：/mem 检索结果与后端日志尾读快照消费（各自在途通道早退无开销）。
        app.poll_mem_mirror();
        app.poll_toolcat();
        app.poll_mem();
        app.poll_dlogs(now_ms);
        // 启动期连接探测结果消费（§6.1 #1）：晚到即热修正连接态，不阻塞首绘。
        app.poll_boot();
        app.poll_slots();
        // 在途请求结果消费（流式增量 / 结果落定 / 审批待决议）。空闲期内部早退
        // 无开销；busy 期它是唯一的流式与落定入口——等待期不再有独立内层循环。
        // 事件管道投影消费（X1-26c）：增量上屏与终态落定都经此，空闲期早退。
        app.poll_lane();
        app.poll_pending(beats.has(Beat::Approvals));
        // 会话完成通知（§5.5 #5、§6.7 #4）：App 只生产语义，写出归终端层。
        // 仅支持 BEL 或写入失败（非 TTY）时用状态条兜底，保证完成反馈
        // 始终可见——通知是增强，不是唯一通道。
        if let Some(n) = app.take_notice() {
            match engine::term::notify(&n.title, &n.body) {
                Some(engine::notify::Method::Bell) => {
                    app.toast(n.body);
                    sched.push(Lane::Panel);
                }
                Some(method) => info!("engine/notify: 回合通知经 {} 投递", method.label()),
                None => {
                    app.toast(n.body);
                    sched.push(Lane::Panel);
                }
            }
        }
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
        let mut timer = engine::timer::Timer::new(Instant::now);
        timer.begin();
        let outcome = compose.frame(terminal, |f| {
            present::render(f, app);
            timer.lap(engine::frame::Stage::Compose);
        });
        // §5.5 #6：终端已消失（SSH 断开/窗口关闭）时立即退出，不在死终端
        // 上重试或补帧；终端已不可写，故走静默正常退出而非错误上报。
        if let engine::compose::FrameOutcome::Dead(dead) = outcome {
            warn!("engine/guard: 终端已消失（{}），结束主循环", dead.code());
            return Ok(engine::exit::Code::Pipe);
        }
        if outcome == engine::compose::FrameOutcome::Drawn {
            if !first_drawn {
                first_drawn = true;
                let ms = boot.elapsed_ms();
                if engine::boot::over_budget(ms) {
                    warn!(
                        "engine/boot: FIRST-FRAME {ms}ms 超预算 {FIRST_FRAME_BUDGET_MS}ms（告警不阻塞，后续帧照常）"
                    );
                } else {
                    info!("engine/boot: FIRST-FRAME {ms}ms（预算 {FIRST_FRAME_BUDGET_MS}ms）");
                }
            }
            timer.lap(engine::frame::Stage::Present);
            // X2-30b：写出档位随帧进入指标环，降级帧在采样里可辨，
            // 不会与优化后的好帧混为一谈。
            let mut costs = timer.finish();
            costs.set_ladder(terminal.backend().write_level().code());
            if let Some(sink) = frame_log.as_mut() {
                sink.record(&costs);
            }
            sched.note_frame(&costs);
        }
        app.render_degraded = compose.degraded();

        // 终端能力探测结论消费（§5.5 #1、§6.8 #3）：首帧后一次性回收并
        // join 探测线程，之后才允许进入事件轮询（stdin 单一读者）。结论
        // 与启动期 COLORFGBG 推断不同则热修正主题并补一帧，不重建终端。
        if let Some(probe) = probe.take() {
            let caps = probe.collect();
            // X2-13a：DECRQM 2026 有肯定回执（无论当前开关）即启用同步包裹，
            // 由后端在每片自包 `\e[?2026h/l`；未回包或明确不支持则保持关闭。
            if caps.sync_usable() {
                terminal.backend_mut().apply_sync(true);
                info!("engine/probe: 同步输出可用（{:?}），启用 2026 包裹", caps.sync_output);
                sched.push(Lane::Panel);
                compose.invalidate();
            }
            match caps.scheme {
                Some((scheme, src)) => {
                    let light = matches!(scheme, engine::probe::Scheme::Light);
                    if style::apply_scheme(light) {
                        info!("engine/probe: 明暗 {scheme:?}（来源 {src:?}），主题已热修正");
                        sched.push(Lane::Panel);
                        compose.invalidate();
                    }
                }
                None => debug!("engine/probe: 窗口内无明暗应答，沿用环境推断"),
            }
        }

        // 插入对话推进：请求完成后取一条排队输入提交（单 pending 槽，逐条消费）。
        // 计入输入档——用户消息即时成帧回显，不受合帧窗口约束。
        if app.step_insert_queue() {
            sched.push(Lane::Input);
        }

        // 事件等待：无事件时按调度器给出的最大等待挂起（空闲降频），有事件则
        // 立即唤醒并处理。终端无"失焦"语义，故不做失焦降帧。有瞬态提示在屏
        // 时按最近到期时刻收窄等待，避免空闲节拍（250ms）拖长消隐延迟。
        let mut wait = Duration::from_millis(sched.wait_ms());
        if let Some(left) = app.toast_wait(Instant::now()) {
            wait = wait.min(left);
        }
        if let Some(left) = app.leader_wait(Instant::now()) {
            wait = wait.min(left);
        }
        // §5.5 #6：输入通道死亡（EOF/EIO/连接断开）与成帧失败同一判据，
        // 一律按终局静默退出——终端消失后 poll 会持续失败，冒泡成 Err
        // 只会在死终端上打印错误，且进程无法被 shell 及时回收。
        let ready = match event::poll(wait) {
            Ok(ready) => ready,
            Err(e) => return leave_on_dead(e),
        };
        if !ready {
            continue;
        }

        let ev = match event::read() {
            Ok(ev) => ev,
            Err(e) => return leave_on_dead(e),
        };
        match ev {
            Event::Paste(text) => {
                // 输入档立即成帧：回显延迟不劣化（不受合帧窗口约束）
                sched.push(Lane::Input);
                // bracketed paste（2026-08-26）：向导编辑态插入字段；
                // 对话态在光标处插入输入框（API Key / 长文本粘贴可用）
                if app.wizard.active {
                    app.wizard.handle_paste(&text);
                } else {
                    app.input_paste(&text);
                }
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                sched.push(Lane::Input);
                // 键位分派 SSoT 在 keys（空闲态全量 match + busy 等待态受限
                // match，入口按 is_busy 自选）。退出信号区分 Ctrl+C 与 F8：
                // 前者是用户中断（130），后者是形态切换后的正常交还（0）。
                match keys::handle(app, key).await? {
                    keys::Flow::Exit => return Ok(engine::exit::Code::Ok),
                    keys::Flow::SigInt => return Ok(engine::exit::Code::SigInt),
                    keys::Flow::Continue => {}
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
