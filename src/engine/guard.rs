// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 进程守卫（0.1.19 TUI 建设方案 §5.5 #6）：终端死亡判定。
//
// 终端消失（SSH 断开、终端窗口关闭、控制台宿主退出）不会投递可捕获的
// 信号，只在后续 IO 上表现为写失败或输入 EOF。若把这类错误当作普通
// 故障重试，事件循环会在死终端上无限自旋——既耗尽 CPU，又让进程无法
// 被 shell 回收。本模块把散落的平台错误码统一归并为四个语义码
// `DEAD_TERMINAL_CODES = {EIO, EPIPE, ENOTCONN, EOF}`，由调用方据此
// 走终局退出而非重连。
//
// 来源枚举共 20 项：POSIX 10 + Win32 10。归并而非穷举判定——新增平台
// 或新 errno 只需在对应表登记，判定与退出路径不变（机制与策略分离）。
//
// 本模块零 IO、零时钟、零全局状态：输入是 `io::Error`，输出是纯枚举，
// 可直接在单测中构造任意平台码复现判定，无需真实断开终端。

use std::io;

/// 终端死亡语义（与 `DEAD_TERMINAL_CODES` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dead {
    /// EOF：输入读到 0 字节，会话已无来源。
    Eof,
    /// EIO：句柄/设备失效（写不进、读不出）。
    Io,
    /// EPIPE：管道对端已关闭。
    Pipe,
    /// ENOTCONN：连接已断开（ssh/串口/网络终端）。
    Unconn,
}

impl Dead {
    /// 语义码文本：日志与诊断字段的唯一拼写来源。
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Eof => "EOF",
            Self::Io => "EIO",
            Self::Pipe => "EPIPE",
            Self::Unconn => "ENOTCONN",
        }
    }
}

/// 来源枚举总数（§5.5 #6）：POSIX 10 + Win32 10。
#[cfg(test)]
const SOURCES: usize = 20;

/// POSIX 来源表。errno 数值随平台不同（如 ECONNRESET 在 Linux 为 104、
/// macOS 为 54），故一律用 libc 常量而非字面量。
#[cfg(unix)]
const TABLE: &[(i32, Dead)] = &[
    (libc::EIO, Dead::Io),
    (libc::ENXIO, Dead::Io),
    (libc::EBADF, Dead::Io),
    (libc::ENODEV, Dead::Io),
    (libc::EPIPE, Dead::Pipe),
    (libc::ESHUTDOWN, Dead::Pipe),
    (libc::ECONNRESET, Dead::Unconn),
    (libc::ECONNABORTED, Dead::Unconn),
    (libc::ECONNREFUSED, Dead::Unconn),
    (libc::ENOTCONN, Dead::Unconn),
];

/// Win32 来源表。数值取自 winerror.h（ABI 稳定，不随 SDK 版本变化）。
/// 不用 windows-sys 常量：这些码分散在多个 feature 门后，为一个判定
/// 打开整片绑定不划算。
#[cfg(windows)]
const TABLE: &[(i32, Dead)] = &[
    (6, Dead::Io),         // ERROR_INVALID_HANDLE
    (31, Dead::Io),        // ERROR_GEN_FAILURE
    (109, Dead::Pipe),     // ERROR_BROKEN_PIPE
    (232, Dead::Pipe),     // ERROR_NO_DATA
    (233, Dead::Unconn),   // ERROR_PIPE_NOT_CONNECTED
    (995, Dead::Io),       // ERROR_OPERATION_ABORTED
    (1167, Dead::Io),      // ERROR_DEVICE_NOT_CONNECTED
    (1236, Dead::Unconn),  // ERROR_CONNECTION_ABORTED
    (10054, Dead::Unconn), // ERROR_CONNECTION_RESET
    (10061, Dead::Unconn), // ERROR_CONNECTION_REFUSED
];

/// 其余平台无终端语义可判：一律不视为死亡，避免误杀。
#[cfg(not(any(unix, windows)))]
const TABLE: &[(i32, Dead)] = &[];

/// 判定一次 IO 失败是否意味着终端已消失。
///
/// 先查平台码表（精确），再按 `ErrorKind` 兜底（跨平台语义，覆盖表外
/// 的同类错误）。返回 `None` 才是可重试故障。
pub(crate) fn probe(err: &io::Error) -> Option<Dead> {
    if let Some(code) = err.raw_os_error() {
        if let Some((_, dead)) = TABLE.iter().find(|(known, _)| *known == code) {
            return Some(*dead);
        }
    }
    kind(err.kind())
}

/// `ErrorKind` 兜底映射（不依赖具体 OS 码）。
const fn kind(kind: io::ErrorKind) -> Option<Dead> {
    match kind {
        io::ErrorKind::BrokenPipe => Some(Dead::Pipe),
        io::ErrorKind::NotConnected
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionRefused => Some(Dead::Unconn),
        io::ErrorKind::UnexpectedEof => Some(Dead::Eof),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{kind, probe, Dead, SOURCES, TABLE};
    use std::io;

    /// 用平台码构造错误：raw_os_error 与 kind 同时具备，覆盖真实调用形态。
    fn raw(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    /// §5.5 #6：20 来源按平台分派（POSIX 10 + Win32 10），本平台只编译
    /// 其中一张；未知平台为空表，宁可不判死亡也不误杀会话。
    #[test]
    fn source_table_enumerates_twenty_codes() {
        assert_eq!(SOURCES, 20, "跨平台来源枚举总数");
        #[cfg(any(unix, windows))]
        {
            assert_eq!(TABLE.len(), SOURCES / 2, "本平台来源表恰好 10 项");
            for want in [Dead::Io, Dead::Pipe, Dead::Unconn] {
                assert!(
                    TABLE.iter().any(|(_, d)| *d == want),
                    "语义码 {:?} 必须有平台来源",
                    want
                );
            }
        }
        #[cfg(not(any(unix, windows)))]
        assert!(TABLE.is_empty(), "未知平台不得登记来源");
    }

    /// 四语义码的拼写是日志/诊断契约，不得漂移。
    #[test]
    fn dead_codes_match_the_declared_set() {
        assert_eq!(Dead::Eof.code(), "EOF");
        assert_eq!(Dead::Io.code(), "EIO");
        assert_eq!(Dead::Pipe.code(), "EPIPE");
        assert_eq!(Dead::Unconn.code(), "ENOTCONN");
    }

    /// 表内每个平台码都判为死亡，且归并语义正确。
    #[test]
    fn every_listed_code_is_dead() {
        for (code, dead) in TABLE {
            assert_eq!(probe(&raw(*code)), Some(*dead), "code={code}");
        }
    }

    /// 非致命错误必须可重试：误判为死亡会让瞬时抖动杀死会话。
    #[test]
    fn transient_errors_are_retryable() {
        assert_eq!(probe(&io::Error::from(io::ErrorKind::WouldBlock)), None);
        assert_eq!(probe(&io::Error::from(io::ErrorKind::Interrupted)), None);
        assert_eq!(probe(&io::Error::from(io::ErrorKind::TimedOut)), None);
        assert_eq!(
            probe(&io::Error::from(io::ErrorKind::PermissionDenied)),
            None
        );
        assert_eq!(probe(&io::Error::from(io::ErrorKind::InvalidInput)), None);
    }

    /// 表外平台码走 kind 兜底；表内码优先于 kind（同一错误两种来源）。
    #[test]
    fn kind_fallback_covers_unlisted_platform_codes() {
        let pipe = io::Error::new(io::ErrorKind::BrokenPipe, "注入断管");
        assert_eq!(probe(&pipe), Some(Dead::Pipe));
        let reset = io::Error::new(io::ErrorKind::ConnectionReset, "注入重置");
        assert_eq!(probe(&reset), Some(Dead::Unconn));
        let short = io::Error::new(io::ErrorKind::UnexpectedEof, "注入短读");
        assert_eq!(probe(&short), Some(Dead::Eof));
    }

    /// 兜底映射的纯函数形态可直接穷举，不需要构造 io::Error。
    #[test]
    fn kind_map_is_total() {
        assert_eq!(kind(io::ErrorKind::BrokenPipe), Some(Dead::Pipe));
        assert_eq!(kind(io::ErrorKind::NotConnected), Some(Dead::Unconn));
        assert_eq!(kind(io::ErrorKind::ConnectionAborted), Some(Dead::Unconn));
        assert_eq!(kind(io::ErrorKind::ConnectionRefused), Some(Dead::Unconn));
        assert_eq!(kind(io::ErrorKind::UnexpectedEof), Some(Dead::Eof));
        assert_eq!(kind(io::ErrorKind::OutOfMemory), None);
    }
}
