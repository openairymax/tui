// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终止信号收敛（TERM/HUP）：全屏 TUI 的 raw mode 与备用屏是终端级副作用，
// 进程被外部停止（启动器退出、SSH 会话关闭、kill）时默认处置会直接消失，
// 留下无法回显的 shell。本模块只做两件事：信号处置改为「置原子标志」，
// 由主循环在下一拍自行退出（终端还原交给 RAII 守卫）；还原完成后按原信号
// 自绝，父进程仍以 WIFSIGNALED 观察真实死因（不伪造 128+signum 退出码）。
//
// 信号处理函数内只调用原子存储——其余一切（日志、文件写入、终端 IO）
// 都在主循环上下文完成，避免异步信号不安全。

#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(unix)]
static HIT: AtomicI32 = AtomicI32::new(0);

/// 接管 TERM/HUP（可重入：重复安装无副作用）。非 unix 平台为空实现——
/// Windows 的控制台关闭事件无法用 POSIX 信号表达，仍由 RAII 守卫兜底还原。
#[cfg(unix)]
pub(crate) fn install() {
    extern "C" fn on_term(sig: libc::c_int) {
        HIT.store(sig, Ordering::Relaxed);
    }
    for sig in [libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            libc::signal(sig, on_term as libc::sighandler_t);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn install() {}

/// 已到达的终止信号（不清标志）；主循环每拍据此收敛退出。
#[cfg(unix)]
pub(crate) fn pending() -> Option<i32> {
    match HIT.load(Ordering::Relaxed) {
        0 => None,
        sig => Some(sig),
    }
}

#[cfg(not(unix))]
pub(crate) fn pending() -> Option<i32> {
    None
}

/// 取走终止信号（一次性）：只在进程退出路径使用，避免二次裁决。
#[cfg(unix)]
pub(crate) fn take() -> Option<i32> {
    match HIT.swap(0, Ordering::Relaxed) {
        0 => None,
        sig => Some(sig),
    }
}

#[cfg(not(unix))]
pub(crate) fn take() -> Option<i32> {
    None
}

/// 信号短名（日志/退出记录用）：只覆盖本模块接管的两种与用户中断。
pub(crate) const fn name(sig: i32) -> &'static str {
    match sig {
        1 => "HUP",
        2 => "INT",
        15 => "TERM",
        _ => "SIG",
    }
}

/// 还原为默认处置后重发原信号，让进程以真实信号死亡。
#[cfg(unix)]
pub(crate) fn reraise(sig: i32) -> ! {
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
    // raise 返回说明信号被忽略（例如被上层屏蔽）；退回内部错误码，不静默成功。
    std::process::exit(crate::engine::exit::Code::Internal.code())
}

#[cfg(not(unix))]
pub(crate) fn reraise(_sig: i32) -> ! {
    std::process::exit(crate::engine::exit::Code::Internal.code())
}

#[cfg(test)]
mod tests {
    use super::{name, pending, take};

    #[test]
    fn flag_starts_clear_and_take_drains() {
        assert_eq!(pending(), None);
        assert_eq!(take(), None);
    }

    #[test]
    fn names_cover_handled_signals() {
        assert_eq!(name(1), "HUP");
        assert_eq!(name(15), "TERM");
        assert_eq!(name(2), "INT");
        assert_eq!(name(9), "SIG");
    }
}
