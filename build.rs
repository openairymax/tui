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

/// W6 构建期门禁：TUI 只允许链接下列库——
///   agentrt_memoryrovol(_oss)：memoryrovol 独立 OSS 库（L1+L2）；
///   airy_common / airy_ime：commons 宿主机地基（IME 词典符号）。
/// 严禁链入任何 daemon 内部库 / coreloopthree / cognition 等 agentrt
/// 运行时库（TUI 只许 gateway HTTP/SSE 客户端 + 渲染栈）。新增链接时
/// 若不在白名单内直接 panic 阻断（fail-closed）。
const ALLOWED_STATIC_LIBS: &[&str] = &[
    "agentrt_memoryrovol",
    "agentrt_memoryrovol_oss",
    "airy_common",
    "airy_ime",
];

fn assert_allowed_lib(name: &str) {
    if !ALLOWED_STATIC_LIBS.contains(&name) {
        panic!(
            "agentrt-tui 构建门禁（W6）：禁止链接非白名单静态库 `{}`。\n\
             允许集：{:?}。TUI 只许 gateway HTTP/SSE 客户端 + 渲染栈，\n\
             不得链入 agentrt daemon/coreloopthree 等运行时内部库。",
            name, ALLOWED_STATIC_LIBS
        );
    }
}

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

    // 0.1.9 M5 W1：agent.run_stream v1 事件协议常量生成（SSoT）。
    // 唯一权威为 agentrt commons/include/airy_run_stream.h，本 crate 经
    // include!(OUT_DIR/run_stream_gen.rs) 共引，禁止手写 wire 键名字面量
    // （与 sdk-rust/src/run_stream.rs 同一机制，见方案 §2.4.4）。
    println!("cargo:rerun-if-env-changed=AGENTRT_TUI_RUN_STREAM_H");
    let rs_header = locate_run_stream_header();
    println!("cargo:rerun-if-changed={}", rs_header.display());
    let rs_content = std::fs::read_to_string(&rs_header).unwrap_or_else(|e| {
        panic!(
            "run_stream schema header unreadable: {} ({})",
            rs_header.display(),
            e
        )
    });
    let rs_gen = generate_run_stream_consts(&rs_content);
    if rs_gen.is_empty() {
        panic!(
            "run_stream schema header parsed zero AIRY_RS_* macros: {}",
            rs_header.display()
        );
    }
    let rs_out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    std::fs::write(rs_out.join("run_stream_gen.rs"), rs_gen)
        .expect("failed to write run_stream_gen.rs");

    #[cfg(feature = "memoryrovol")]
    {
        println!("cargo:rerun-if-env-changed=MEMORYROVOL_LIB");
        println!("cargo:rerun-if-env-changed=MEMORYROVOL_OSS_LIB");
        println!("cargo:rerun-if-env-changed=AIRY_HOME");
        println!("cargo:rerun-if-changed=build.rs");
        // 库文件出现/变化必须重跑本脚本：首次构建时 OSS 库尚未部署（仅
        // PRO 库存在）会选择 PRO 库，之后 OSS 库补建时若无此指令，cargo
        // 沿用缓存链接 PRO 库 → airy_thread_* 未定义（TUI 独立二进制
        // 无法链接依赖 agentrt 运行时符号的 PRO 库）。
        if let Ok(home) = env::var("AIRY_HOME") {
            for name in ["libagentrt_memoryrovol_oss.a", "libagentrt_memoryrovol.a"] {
                println!(
                    "cargo:rerun-if-changed={}",
                    Path::new(&home).join("lib").join(name).display()
                );
            }
        }

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
            assert_allowed_lib(libname);
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
                assert_allowed_lib("airy_common");
                assert_allowed_lib("airy_ime");
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

/// 定位 airy_run_stream.h（v1 协议 schema SSoT）：优先 env
/// AGENTRT_TUI_RUN_STREAM_H，否则 monorepo 默认布局（tui 上溯两级到
/// agent-workload 后进入 agentrt/）。缺失即构建失败（fail-closed）。
fn locate_run_stream_header() -> PathBuf {
    if let Ok(path) = env::var("AGENTRT_TUI_RUN_STREAM_H") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return p;
        }
        panic!("AGENTRT_TUI_RUN_STREAM_H set but not a file: {path}");
    }
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let candidate = Path::new(&manifest).join("../../agentrt/commons/include/airy_run_stream.h");
    if candidate.is_file() {
        return candidate;
    }
    panic!(
        "run_stream schema header not found (tried {}). \
         Set AGENTRT_TUI_RUN_STREAM_H to the airy_run_stream.h path.",
        candidate.display()
    );
}

/// 解析 #define AIRY_RS_* 宏并生成 Rust 常量（统一收纳于 gen 子模块）。
/// 与 sdk-rust/build.rs 逻辑对齐，保证编码端/解码端字段一致。
fn generate_run_stream_consts(content: &str) -> String {
    let mut out = String::new();
    out.push_str("// @generated by tui/build.rs from agentrt commons/include/airy_run_stream.h\n");
    out.push_str("// DO NOT EDIT. 字段键/事件类型常量以 C 侧 schema 为唯一权威（SSoT）。\n\n");
    out.push_str("pub mod gen {\n");
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("#define ") else {
            continue;
        };
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("").trim();
        if !name.starts_with("AIRY_RS_") || value.is_empty() {
            continue;
        }
        if value.starts_with('"') && value.ends_with('"') {
            out.push_str(&format!("    pub const {name}: &str = {value};\n"));
        } else if value.chars().all(|c| c.is_ascii_digit()) {
            out.push_str(&format!("    pub const {name}: i64 = {value};\n"));
        }
    }
    out.push_str("}\n");
    out
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
