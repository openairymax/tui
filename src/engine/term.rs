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
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::probe::{self, Caps, Src};
use crate::budget::CAP_PROBE_TIMEOUT_MS;

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

/// 最小 base64 编码（RFC 4648 标准字母表，含填充）。
///
/// 自实现而不引入依赖：仅 OSC52 一处消费，离线构建不新增第三方包。
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(T[((n >> 18) & 63) as usize]));
        out.push(char::from(T[((n >> 12) & 63) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(T[((n >> 6) & 63) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(T[(n & 63) as usize])
        } else {
            '='
        });
    }
    out
}

/// OSC52 剪贴板序列；编码后超界返回 None（改走平台命令链）。
fn osc52(text: &str) -> Option<String> {
    let encoded = b64(text.as_bytes());
    if encoded.len() > crate::budget::MAX_OSC52_ENCODED_LENGTH {
        return None;
    }
    Some(format!("\x1b]52;c;{encoded}\x07"))
}

/// 写入终端控制序列。write/flush 任一失败即视为不可用。
fn emit_seq(seq: &str) -> bool {
    let mut out = io::stdout();
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
}

/// 经平台命令写入剪贴板：逐个尝试，前一个不可用或失败才试下一个。
fn platform_copy(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    const CMDS: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(all(unix, not(target_os = "macos")))]
    const CMDS: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    #[cfg(target_os = "windows")]
    const CMDS: &[(&str, &[&str])] = &[
        (
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "[Console]::InputEncoding=[Text.Encoding]::UTF8; $input | Set-Clipboard",
            ],
        ),
        ("clip", &[]),
    ];
    #[cfg(not(any(unix, windows)))]
    const CMDS: &[(&str, &[&str])] = &[];

    CMDS.iter().any(|(cmd, args)| pipe_to(cmd, args, text))
}

/// 把文本经标准输入交给子命令；子命令缺失、写入失败或非零退出均返回假。
fn pipe_to(cmd: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut sin| sin.write_all(text.as_bytes()).is_ok());
    wrote && child.wait().is_ok_and(|s| s.success())
}

/// 终端能力探测句柄（§5.5 #1）：首帧并发、晚到热修正。
///
/// 线程在 crossterm 接手 stdin 之前独占读取最多 `CAP_PROBE_TIMEOUT_MS`；
/// `collect` 必然 stop + join 后才返回，保证主循环进入事件轮询时不存在
/// 第二个读者（stdin 竞态会导致按键被探测线程吞掉）。OSC11 应答先到即
/// 提前结束（分级：OSC11 > DEC2031），否则等满窗口收集 DEC2031。
pub(crate) struct Probe {
    rx: mpsc::Receiver<Caps>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    deadline: Instant,
}

impl Probe {
    /// 显式主题或非 TTY 时不起探测：前者结论不会被采用，后者读不到应答。
    pub(crate) fn spawn() -> Option<Self> {
        if explicit_theme() || !io::stdin().is_terminal() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("caps-probe".into())
            .spawn(move || probe_loop(&flag, &tx))
            .ok()?;
        Some(Self {
            rx,
            stop,
            handle: Some(handle),
            deadline: Instant::now() + Duration::from_millis(CAP_PROBE_TIMEOUT_MS),
        })
    }

    /// 等待探测结论并回收线程；窗口内无应答时返回空结论。
    pub(crate) fn collect(&mut self) -> Caps {
        let left = self.deadline.saturating_duration_since(Instant::now());
        let caps = self.rx.recv_timeout(left).unwrap_or_default();
        self.shutdown();
        caps
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 显式主题（AIRY_TUI_THEME）优先于一切探测结论。
fn explicit_theme() -> bool {
    std::env::var("AIRY_TUI_THEME")
        .is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "light" | "dark"))
}

/// 探测线程主体：先发 OSC11 与 DEC2031 查询，再按短片轮询原始输入。
fn probe_loop(stop: &AtomicBool, tx: &mpsc::Sender<Caps>) {
    if !emit_seq("\x1b]11;?\x1b\\\x1b[?996n") {
        let _ = tx.send(Caps::default());
        return;
    }
    let end = Instant::now() + Duration::from_millis(CAP_PROBE_TIMEOUT_MS);
    let mut buf = Vec::with_capacity(256);
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        if !read_some(&mut buf) {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if buf.len() > 1024 {
            buf.drain(..512);
        }
        let caps = probe::caps(&buf);
        if matches!(caps.scheme, Some((_, Src::Osc11))) {
            let _ = tx.send(caps);
            return;
        }
    }
    let _ = tx.send(probe::caps(&buf));
}

/// 非阻塞读取 stdin 原始字节；无数据返回假（unix: poll + read）。
#[cfg(unix)]
fn read_some(buf: &mut Vec<u8>) -> bool {
    use std::os::unix::io::AsRawFd;

    let mut pfd = libc::pollfd {
        fd: io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pfd 为单个有效 pollfd，nfds=1，超时 10ms 内不写越界。
    if unsafe { libc::poll(&mut pfd, 1, 10) } <= 0 {
        return false;
    }
    let mut chunk = [0u8; 256];
    // SAFETY: chunk 为 256 字节可写缓冲，fd 来自标准输入且长度如实传入。
    let n = unsafe { libc::read(pfd.fd, chunk.as_mut_ptr().cast(), chunk.len()) };
    if n <= 0 {
        return false;
    }
    buf.extend_from_slice(&chunk[..n as usize]);
    true
}

/// 非阻塞读取控制台宽字符；无数据返回假（windows）。
#[cfg(windows)]
fn read_some(buf: &mut Vec<u8>) -> bool {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Console::{GetStdHandle, ReadConsoleW, STD_INPUT_HANDLE};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // SAFETY: 句柄来自 GetStdHandle；缓冲区为本栈上 128 个 u16，
    // 读入长度由 n 回写且不以 n 以外内容构造字符串。
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if handle.is_null() || WaitForSingleObject(handle, 10) != WAIT_OBJECT_0 {
            return false;
        }
        let mut wide = [0u16; 128];
        let mut n = 0u32;
        if ReadConsoleW(
            handle,
            wide.as_mut_ptr().cast(),
            wide.len() as u32,
            &mut n,
            std::ptr::null(),
        ) == 0
        {
            return false;
        }
        let text: String = char::decode_utf16(wide[..n as usize].iter().copied())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        buf.extend_from_slice(text.as_bytes());
    }
    true
}

#[cfg(not(any(unix, windows)))]
fn read_some(_buf: &mut Vec<u8>) -> bool {
    false
}

/// 复制文本到系统剪贴板（TUI 方案 §6.3 #3）。
///
/// 双通道：先发 OSC52（终端原生，跨 ssh/多路复用同样可用），编码长度受
/// `MAX_OSC52_ENCODED_LENGTH` 约束；序列超长或写入失败时退平台命令链。
/// 返回假表示两条通道均不可用，调用方据此给出提示而非静默。
pub(crate) fn copy_text(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    if osc52(text).is_some_and(|seq| emit_seq(&seq)) {
        return true;
    }
    platform_copy(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_matches_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (raw, want) in cases {
            assert_eq!(b64(raw.as_bytes()), want, "raw={raw:?}");
        }
    }

    #[test]
    fn b64_keeps_utf8_bytes() {
        assert_eq!(b64("中".as_bytes()), "5Lit");
    }

    #[test]
    fn osc52_enforces_encoded_budget() {
        let max_src = crate::budget::MAX_OSC52_ENCODED_LENGTH / 4 * 3;
        assert!(osc52(&"a".repeat(max_src)).is_some(), "界内可用");
        assert!(osc52(&"a".repeat(max_src + 3)).is_none(), "超界改走平台链");
    }

    #[test]
    fn empty_text_is_not_copied() {
        assert!(!copy_text(""));
    }
}
