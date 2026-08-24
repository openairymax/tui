// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// MemoryRovol 静态库定位（仅 memoryrovol feature 启用时执行）。
//
// 设计（IRON-8）：MemoryRovol 为默认记忆提供商，但 libagentrt_memoryrovol.a
// 是 agentrt C 侧构建产物，TUI 构建环境不一定可用。因此 build.rs 负责：
//   1. 按优先级查找静态库：
//        MEMORYROVOL_LIB env → $AIRY_HOME/lib/libagentrt_memoryrovol.a
//        → 伞仓 products/memoryrovol/build_oss/src/libagentrt_memoryrovol.a
//      （build_oss 为 OSS 模式 L1+L2 构建，不依赖 agentrt 运行时符号，
//        可被 TUI 独立二进制直接链接；PRO 全功能库依赖 agentrt 平台/
//        provider/LLM 符号，不适合独立链接，故不作为自动候选）
//   2. 找到且未携带 ASan 插桩（agentrt sanitizer 构建的库无法独立链接）
//      → 输出链接参数并声明 cfg(mr_linked)，FFI 模块编译、启用 MemoryRovol
//   3. 未找到 / 不可链接 → 仅输出 warning；FFI 代码不编译（不产生桩），
//      build_memory() 优雅降级为 JsonlMemory（真实可用后备）

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // 无论是否找到库都声明 mr_linked / ime_linked cfg，避免 rustc 1.80+ unexpected_cfgs 告警
    println!("cargo:rustc-check-cfg=cfg(mr_linked)");
    println!("cargo:rustc-check-cfg=cfg(ime_linked)");

    // 版本号 SSoT（2.6.2 Unify Design）：单一来源为 agentrt/VERSION 文件
    // （本 crate 位于 sdk/tui，上溯两级到 agent-workload 后进入 agentrt/）。
    // 读取失败时降级为 CARGO_PKG_VERSION（独立发布/源码缺失场景）。
    let version = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agentrt/VERSION"),
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rerun-if-changed=../../agentrt/VERSION");
    println!("cargo:rustc-env=AIRY_RT_VERSION={version}");

    #[cfg(feature = "memoryrovol")]
    {
        println!("cargo:rerun-if-env-changed=MEMORYROVOL_LIB");
        println!("cargo:rerun-if-env-changed=AIRY_HOME");
        println!("cargo:rerun-if-changed=build.rs");

        if let Some(lib) = locate_lib() {
            if is_asan_instrumented(&lib) {
                println!(
                    "cargo:warning=memoryrovol: {} carries ASan instrumentation and cannot be linked into the TUI; falling back to JsonlMemory",
                    lib.display()
                );
                return;
            }
            let dir = lib.parent().expect("lib path has parent dir");
            // 库名从文件名推导（支持 libagentrt_memoryrovol.a / *_oss.a 两种
            // 命名）：file_stem 去扩展名 → 去 lib 前缀 → rustc-link-lib 名。
            let stem = lib
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("agentrt_memoryrovol");
            let libname = stem.strip_prefix("lib").unwrap_or(stem);
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static={}", libname);
            // OSS 库（L1+L2）的外部依赖：cJSON/curl/yaml/sqlite3/zlib/OpenSSL/pthread
            // （PRO 模式需要的 agentrt 运行时符号不在此列，OSS 无此依赖）。
            // 平台差异：macOS 的 pthread/dl 已并入 libSystem（-lpthread/-ldl
            // 会触发 "library not found"），仅 Linux 显式链接。
            let mut sys_libs: Vec<&str> =
                vec!["cjson", "curl", "yaml", "sqlite3", "z", "ssl", "crypto", "m"];
            #[cfg(target_os = "linux")]
            sys_libs.extend(["pthread", "dl"]);
            for sys in sys_libs {
                println!("cargo:rustc-link-lib={}", sys);
            }
            println!("cargo:rustc-cfg=mr_linked");
            println!(
                "cargo:warning=memoryrovol: linked libagentrt_memoryrovol.a ({})",
                lib.display()
            );
        } else {
            println!("cargo:warning=memoryrovol: libagentrt_memoryrovol.a not found, TUI memory falls back to JsonlMemory");
        }
    }

    #[cfg(feature = "ime")]
    {
        println!("cargo:rerun-if-env-changed=AIRY_COMMON_LIB");
        println!("cargo:rerun-if-env-changed=AIRY_HOME");
        println!("cargo:rerun-if-changed=build.rs");

        if let Some(lib) = locate_common_lib() {
            if is_asan_instrumented(&lib) {
                println!(
                    "cargo:warning=ime: {} carries ASan instrumentation and cannot be linked into the TUI; IME disabled",
                    lib.display()
                );
                return;
            }
            let dir = lib.parent().expect("lib path has parent dir");
            // rust-lld 对静态库按出现位置惰性提取：-lairy_common 位于所有
            // rlib 之前且带 --as-needed，airy_ime.o 会被整库跳过（undefined
            // symbol: airy_ime_*）。修复：把 airy_ime 成员抽出为独立
            // libairy_ime.a，用 +whole-archive 强制提取；airy_common 紧随
            // 其后作为辅助符号（memory_alloc 等）的兜底来源。
            if let Some(ime_a) = extract_ime_member(&lib) {
                let ime_dir = ime_a.parent().expect("libairy_ime.a has parent dir");
                println!("cargo:rustc-link-search=native={}", ime_dir.display());
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-lib=static:+whole-archive=airy_ime");
                println!("cargo:rustc-link-lib=static=airy_common");
                println!("cargo:rustc-cfg=ime_linked");
                println!(
                    "cargo:warning=ime: linked airy_ime.o from {} via libairy_ime.a ({})",
                    lib.display(),
                    ime_a.display()
                );
            } else {
                println!("cargo:warning=ime: libairy_common.a has no extractable airy_ime member; IME disabled (F10 unavailable)");
            }
        } else {
            println!("cargo:warning=ime: libairy_common.a not found, builtin pinyin IME disabled (F10 unavailable)");
        }
    }
}

/// 从 libairy_common.a 中抽出 airy_ime 成员并打包为独立 libairy_ime.a
/// （OUT_DIR 下），供 IME FFI 以 +whole-archive 方式强制链接。
/// 返回 libairy_ime.a 路径；失败返回 None（IME 降级禁用）。
#[cfg(feature = "ime")]
fn extract_ime_member(lib: &Path) -> Option<PathBuf> {
    let out = PathBuf::from(env::var("OUT_DIR").ok()?);
    let ar = env::var("AR").unwrap_or_else(|_| "ar".to_owned());
    // 1. 列出归档成员，找 airy_ime 成员（cmake 命名为 airy_ime.c.o）
    let list = Command::new(&ar).arg("t").arg(lib).output().ok()?;
    let member = String::from_utf8_lossy(&list.stdout)
        .lines()
        .find(|l| l.contains("airy_ime"))?
        .to_owned();
    // 2. 解压到 OUT_DIR（成员名不含路径分隔符时落在 OUT_DIR 下）
    let status = Command::new(&ar)
        .current_dir(&out)
        .args(["x", lib.to_str()?, &member])
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    // 3. 重新打包为 libairy_ime.a（+whole-archive 按库名链接）
    let ime_a = out.join("libairy_ime.a");
    let _ = std::fs::remove_file(&ime_a);
    let status = Command::new(&ar)
        .current_dir(&out)
        .args(["rcs", ime_a.to_str()?, &member])
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    Some(ime_a)
}

/// 按优先级定位 libagentrt_memoryrovol.a：
///   1. MEMORYROVOL_OSS_LIB env（显式指定 OSS 独立可链库，TUI 首选）
///   2. $AIRY_HOME/lib/libagentrt_memoryrovol_oss.a（OSS 部署位置，2.6）
///   3. MEMORYROVOL_LIB env（兼容旧用法）
///   4. $AIRY_HOME/lib/libagentrt_memoryrovol.a（安装前缀）
///   5. 伞仓 OSS 构建产物 products/memoryrovol/build_oss/src/libagentrt_memoryrovol.a
/// 注意：PRO 全功能库（4）依赖 agentrt 运行时符号，TUI 独立二进制无法
/// 链接；build.sh/install.sh 会先构建 OSS 库部署为 *_oss.a（2），保证
/// TUI memoryrovol 全功能可用（L1+L2），PRO 留给 agentrt C 侧。
#[cfg(feature = "memoryrovol")]
fn locate_lib() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = env::var("MEMORYROVOL_OSS_LIB") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = env::var("AIRY_HOME") {
        candidates.push(PathBuf::from(home).join("lib").join("libagentrt_memoryrovol_oss.a"));
    }
    if let Ok(p) = env::var("MEMORYROVOL_LIB") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = env::var("AIRY_HOME") {
        candidates.push(PathBuf::from(home).join("lib").join("libagentrt_memoryrovol.a"));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(
        manifest
            .join("../../products/memoryrovol/build_oss/src/libagentrt_memoryrovol.a"),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// 按优先级定位 libairy_common.a（airy_ime 符号所在，agentrt commons 静态库）：
///   1. AIRY_COMMON_LIB env（显式指定，交叉构建首选）
///   2. $AIRY_HOME/lib/libairy_common.a（安装前缀）
///   3. agentrt 源码树标准构建产物 agentrt/build/commons/libairy_common.a
///   4. AIRYRT_HOME/agentrt/build/commons/libairy_common.a（伞仓环境变量）
/// 注意：libairy_common.a 必须来自无 sanitizer 构建（ENABLE_SANITIZERS=OFF），
/// 否则 __asan_* 符号无法被 Rust 独立链接（is_asan_instrumented 兜底拒绝）。
#[cfg(feature = "ime")]
fn locate_common_lib() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = env::var("AIRY_COMMON_LIB") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = env::var("AIRY_HOME") {
        candidates.push(PathBuf::from(home).join("lib").join("libairy_common.a"));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(
        manifest
            .join("../../agentrt/build/commons/libairy_common.a"),
    );
    if let Ok(rt_home) = env::var("AIRYRT_HOME") {
        candidates.push(
            PathBuf::from(rt_home)
                .join("agentrt/build/commons/libairy_common.a"),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 检测静态库是否携带 ASan 插桩（引用 __asan_* 符号）。
/// agentrt 默认 Release 构建（ENABLE_SANITIZERS=ON）产物无法被 TUI 独立
/// 链接；OSS 独立构建（products/memoryrovol/build_oss）无插桩，可直接链接。
fn is_asan_instrumented(lib: &Path) -> bool {
    Command::new("nm")
        .arg("-u")
        .arg(lib)
        .output()
        .map(|out| {
            let s = String::from_utf8_lossy(&out.stdout);
            s.lines().any(|l| l.contains(" U __asan_"))
        })
        .unwrap_or(false)
}
