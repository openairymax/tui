// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! 单实例守卫（0.1.21 手册 E）：同一 `AIRY_HOME` 只允许一个 TUI 进程。
//!
//! 双实例会各自向网关注册会话并争抢同一条事件管道：第二个实例长期空白、
//! 任务静默悬死，极端情况下把整机拖入僵死。锁用
//! `flock(LOCK_EX | LOCK_NB)` 持有 `run/agentrt-tui.pid`，进程退出（含
//! 崩溃）时由内核自动释放，不存在需要人工清理的陈旧锁；文件内容只用于
//! 冲突时告知占用者 pid。

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    use anyhow::{bail, Context, Result};

    /// 持有 flock 的守卫；Drop 关闭 fd 即释放锁。
    pub(crate) struct Guard {
        _file: File,
    }

    pub(crate) fn acquire() -> Result<Guard> {
        let path = crate::paths::airy_home_path(&["run", "agentrt-tui.pid"]);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("无法创建运行目录 {}", dir.display()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("无法打开单实例锁 {}", path.display()))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            let owner = pid.trim();
            bail!(
                "已有另一个 TUI 实例占用 {}（pid={}）。同一 AIRY_HOME 只允许一个 TUI：\
                 双实例会争抢网关会话与事件管道。请先关闭另一个窗口。",
                path.display(),
                if owner.is_empty() { "未知" } else { owner }
            );
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Guard { _file: file })
    }
}

#[cfg(not(unix))]
mod imp {
    use anyhow::Result;

    /// 非 Unix 平台暂不引入文件锁（TUI 生产目标为 Linux）。
    pub(crate) struct Guard;

    pub(crate) fn acquire() -> Result<Guard> {
        Ok(Guard)
    }
}

pub(crate) use imp::Guard;

/// 获取单实例锁；冲突时返回带占用者 pid 的错误。
pub(crate) fn acquire() -> anyhow::Result<Guard> {
    imp::acquire()
}
