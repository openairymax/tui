// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AgentRT TUI 库入口：除 `entry` 外的模块全部在 crate 内可见，二进制入口
// 只做一行转发。拆库的唯一目的是让 golden 测试能实例化真实 `App` 并驱动
// 真实渲染路径——此前集成测试只能复制一份镜像结构，属桩式验证（与
// 0.1.19 方案 §7.1「对标 golden」及工程纪律「严禁桩实现」冲突）。

mod app;
mod budget;
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
mod present;
mod probe;
mod secrets;
mod skills;
mod style;
mod wizard;

pub mod entry;

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

/// golden 能力矩阵（0.1.19 TUI 方案 §6/§7.1）：仅在测试构建中编入，
/// 生产二进制零体积影响。
#[cfg(test)]
mod golden;
