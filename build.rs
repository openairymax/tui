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
    // 无论是否找到库都声明 mr_linked cfg，避免 rustc 1.80+ unexpected_cfgs 告警
    println!("cargo:rustc-check-cfg=cfg(mr_linked)");

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

/// 检测静态库是否携带 ASan 插桩（引用 __asan_* 符号）。
/// agentrt 默认 Release 构建（ENABLE_SANITIZERS=ON）产物无法被 TUI 独立
/// 链接；OSS 独立构建（products/memoryrovol/build_oss）无插桩，可直接链接。
#[cfg(feature = "memoryrovol")]
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
