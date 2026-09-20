// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端生命周期：日志初始化、panic 还原钩子、RAII 终端守卫。
// 自 main.rs 拆出（0.1.18：单一职责——main 只管启动编排）。
//
// panic 还原的两条互斥路径（§5A.3 W10）：默认路径（致命 panic）还原终端，
// 保证报文可读；渲染故障隔离窗口内的 panic 由合成层接管，本模块不得还原。

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use log::error;
use std::cell::Cell;
use std::io;

thread_local! {
    /// 渲染故障隔离窗口（§5A.3 W10）：L5 合成层在 `catch_unwind` 区间置位。
    /// 按线程隔离——panic 钩子运行于 panic 发生的线程，故窗口内的 panic 能被
    /// 精确识别，非渲染路径（含其他线程的后台任务）的 panic 不受影响。
    static RENDER_GUARD: Cell<bool> = const { Cell::new(false) };
}

/// 进入/退出渲染故障隔离窗口。只应由 L5 合成层调用（窗口语义归合成层所有）。
pub(crate) fn set_render_guard(on: bool) {
    RENDER_GUARD.with(|guard| guard.set(on));
}

/// 初始化文件日志（避免 stderr 污染全屏 TUI）。
///
/// 路径优先级：`AGENTRT_TUI_LOG` → `$AIRY_HOME/logs/agentrt-tui.log` →
/// `$HOME/.airymaxrt/logs/agentrt-tui.log`。
pub(crate) fn init_file_logger() -> Result<(), Box<dyn std::error::Error>> {
    let path = if let Ok(p) = std::env::var("AGENTRT_TUI_LOG") {
        p
    } else {
        // 原语义：AIRY_HOME/HOME 均缺失（极端环境）→ Err → 调用方回退
        // stderr。不能落到 paths::airy_home() 的相对回退，否则日志会写进
        // 当前工作目录。
        if std::env::var_os("AIRY_HOME").is_none() && std::env::var_os("HOME").is_none() {
            return Err("AIRY_HOME/HOME unset".into());
        }
        crate::paths::airy_home()
            .join("logs")
            .join("agentrt-tui.log")
            .to_string_lossy()
            .into_owned()
    };

    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(file)))
        .init();
    Ok(())
}

/// T-03（P0-8）：崩溃路径终端还原（panic hook 专用）。只还原 raw mode /
/// 备用屏 / 光标，**不清屏**——保住 panic 报文可见。所有步骤独立容错。
fn restore_on_panic() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
    let _ = execute!(io::stdout(), Show);
}

/// T-03（P0-8）：panic hook——先还原终端（否则 raw mode 下报文不可读、
/// 备用屏吞掉全部输出，用户终端看似死机），再走默认报文输出。
/// 与 `TerminalGuard` 幂等协作（重复还原无害）。
///
/// §5A.3 W10：处于渲染故障隔离窗口内的 panic 由合成层接管——此处**不得**
/// 还原终端（离开备用屏即丢帧，隔离失效），报文改入日志（默认钩子写 stderr
/// 会直接污染画面）。
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if RENDER_GUARD.with(Cell::get) {
            log::error!("term: 渲染 panic 已交由合成层隔离: {info}");
            return;
        }
        restore_on_panic();
        default_hook(info);
    }));
}

/// T-03（P0-8）：RAII 终端守卫。构造即进入 TUI 终端态（raw mode、备用屏、
/// 括号粘贴、隐藏光标）；`restore`/Drop 完整还原且每步独立容错——
/// 修复原 `run_tui` 还原链 `?` 短路（首步失败即跳过后续全部步骤）与
/// `Terminal::new` 失败时 raw mode 泄漏两条缺陷路径。
///
/// WS-1 出口 DoD：panic/Err/exec/正常退出四条路径终端状态均还原。
pub(crate) struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub(crate) fn acquire() -> Result<Self> {
        enable_raw_mode().map_err(|e| {
            error!("Failed to enable raw mode: {}", e);
            error!("  → This usually means you're not in a real terminal.");
            error!("  → Try running in a terminal emulator, not an IDE panel.");
            e
        })?;
        let res = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        );
        if let Err(e) = res {
            error!("Failed to enter alternate screen: {}", e);
            // 已进入 raw mode：必须先退出再返回错误（acquire 自身不泄漏）
            let _ = disable_raw_mode();
            return Err(e.into());
        }
        Ok(Self { active: true })
    }

    /// 完整还原。幂等：重复调用安全（守卫 restore + Drop 兜底协作）。
    /// 每步独立容错，绝不短路。
    pub(crate) fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let _ = disable_raw_mode();
        // 0.1.18 B11：鼠标捕获兜底关闭（Ctrl+M 开启后异常退出也还原终端
        // 原生文本选择；未开启时幂等无害）。
        let _ = execute!(io::stdout(), DisableMouseCapture);
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
        let _ = execute!(io::stdout(), Show);
        /* 2.2.1.2 修复沿用：F8 切 CLI / 正常退出前清空主屏，CLI 从干净
         * 画布开始（panic 路径不走此处，见 restore_on_panic）。 */
        let _ = execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0));
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
