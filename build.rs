// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// T-18 分级（见 clippy.toml）：build.rs 的 expect 属构建期环境契约
// （OUT_DIR/CARGO_MANIFEST_DIR 由 cargo 保证，缺失即构建环境损坏），
// 非运行时生产路径，故整体放行；src/** 生产路径仍由 [lints.clippy] 强制。
#![allow(clippy::expect_used)]

// T-09 铁律（0.1.15 裁决 2026-09-10，方案 §4.7）：一切客户端功能走
// gateway，绝对禁止 TUI 直连 daemon / 运行时库。本脚本不再定位、链接
// 任何 agentrt C 侧静态库（memoryrovol FFI 已收回，记忆读写一律走
// gateway mem.* RPC；内置拼音 IME 直连一并收回，输入交回终端/OS 输入法）。
// TUI 运行时依赖仅剩：gateway HTTP/SSE 客户端 + 终端渲染栈。

use std::env;
use std::path::Path;

/// W6 构建期门禁（T-10 推广）：TUI 允许静态链接的库集合。
///
/// T-09 收紧后为空集——任何 `assert_allowed_lib` 调用都会 panic，
/// 即"TUI 禁止链接任何运行时库"由机器强制而非约定保证。本闸门保留
/// 为铁律的机械化守卫：未来若确有新增链接需求，必须先经显式架构评审
/// 扩充此集合（见 0.1.15 方案 §4.6/§4.7 T-09/T-10），不允许绕过。
const ALLOWED_STATIC_LIBS: &[&str] = &[];

#[allow(dead_code)] // T-09 后无链接者；空集即任何调用都阻断（fail-closed 守卫）
fn assert_allowed_lib(name: &str) {
    if !ALLOWED_STATIC_LIBS.contains(&name) {
        panic!(
            "agentrt-tui 构建门禁（W6/T-09）：禁止链接非白名单静态库 `{}`。\n\
             允许集：{:?}。TUI 只许 gateway HTTP/SSE 客户端 + 渲染栈，\n\
             不得链入 agentrt daemon/runtime 任何内部库。",
            name, ALLOWED_STATIC_LIBS
        );
    }
}

fn main() {
    // ime_linked cfg 声明保留（src/ime.rs 等处以 all(feature, ime_linked)
    // 双门控 fail-closed 休眠）；mr_linked 随 T-09 FFI 收回一并移除。
    println!("cargo:rustc-check-cfg=cfg(ime_linked)");

    // 版本号 SSoT（2.6.2 Unify Design）：单一来源为 agentrt/VERSION 文件。
    // CI 布局（release 兄弟仓克隆：agent-workload/sdk/tui 与 agentrt/ 非
    // 邻接）下伞仓相对路径 ../../agentrt/VERSION 不存在，故支持
    // AIRY_RT_VERSION_FILE env 显式注入（build-tui.sh / release.yml 导出）；
    // 读取失败时降级 CARGO_PKG_VERSION。
    let version = std::env::var("AIRY_RT_VERSION_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .or_else(|| {
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "../../agentrt/VERSION",
            ))
            .ok()
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    if let Ok(vf) = std::env::var("AIRY_RT_VERSION_FILE") {
        println!("cargo:rerun-if-changed={vf}");
    }
    println!("cargo:rerun-if-changed=../../agentrt/VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=AIRY_RT_VERSION={version}");
}
