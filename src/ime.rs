// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! 内置拼音输入法（IME）：Rust FFI 绑定 agentrt `libairy_common.a` 的
//! `airy_ime_*` 符号（commons/utils/ime）。
//!
//! feature `ime` 默认启用；是否真正链接由 build.rs 决定：定位到
//! libairy_common.a 时声明 cfg(ime_linked)，否则本模块的 FFI 不编译，
//! `ImeEngine::load()` 恒返回 None（IME 降级禁用，F10 无效果，英文输入
//! 不受影响——fail-closed）。
//!
//! 状态机（拼音态：切换/输入/选字/上屏）在 app.rs 实现，与 C 侧 CLI
//! （tools/airy_cli/src/cli_tui.c 的 tui_ime_* 系列）语义保持一致：
//!   - F10 切换 中/英；切回英文时拼音原文上屏
//!   - a-z 追加拼音并实时刷新候选；1-9 选字（选字后保持拼音态，连续词组
//!     输入不中断）；空格选第一个候选
//!   - Backspace 删拼音（空则退出拼音态）；Enter 提交拼音原文走正常提交
//!   - 其他可见字符：拼音原文上屏后按正常路径处理

use std::os::raw::c_int;
use std::path::PathBuf;

// ---- FFI 声明（与 commons/utils/ime/include/airy_ime.h 严格对齐） ----

/// C 侧类型定义无条件存在（Rust 结构体引用需要）；extern 函数声明仅在
/// build.rs 声明 cfg(ime_linked)（成功链接 libairy_common.a）时编译。
mod ffi {
    use std::os::raw::{c_char, c_int};

    #[repr(C)]
    pub struct airy_ime {
        _private: [u8; 0],
    }

    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    #[repr(C)]
    pub struct airy_ime_cand {
        pub text: *const c_char,
        pub freq: u32,
    }

    #[cfg(all(feature = "ime", ime_linked))]
    extern "C" {
        pub fn airy_ime_load(path: *const c_char) -> *mut airy_ime;
        pub fn airy_ime_destroy(ime: *mut airy_ime);
        pub fn airy_ime_query(
            ime: *const airy_ime,
            pinyin: *const c_char,
            out: *mut airy_ime_cand,
            out_cap: c_int,
        ) -> c_int;
    }
}

/// 拼音输入法引擎（词典句柄 RAII 包装）。
///
/// `handle` 在 !ime_linked 时恒为 null（load 返回 None，不进入构造）。
pub struct ImeEngine {
    handle: *mut ffi::airy_ime,
}

// 句柄仅在单线程 UI 事件循环使用（C 库无内部锁，勿跨线程）
unsafe impl Send for ImeEngine {}

impl ImeEngine {
    /// 按优先级加载词典（与 C 侧 tui_ime_load_dict 一致）：
    ///   AIRY_IME_DICT env → $AIRY_HOME/share/agentrt/ime/airy_ime.dat
    ///   → ./share/agentrt/ime/airy_ime.dat → agentrt 源码树 data/airy_ime.dat
    /// 全部缺失（或库未链接）返回 None（IME 禁用，fail-closed）。
    pub fn load() -> Option<Self> {
        #[cfg(all(feature = "ime", ime_linked))]
        {
            let path = Self::locate_dict()?;
            let cpath = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
            let handle = unsafe { ffi::airy_ime_load(cpath.as_ptr()) };
            if handle.is_null() {
                log::warn!("ime: dict load failed: {}", path.display());
                return None;
            }
            log::info!("ime: dict loaded: {}", path.display());
            return Some(Self { handle });
        }
        #[cfg(not(all(feature = "ime", ime_linked)))]
        {
            let _ = (); // libairy_common.a 未链接：IME 禁用
            None
        }
    }

    fn locate_dict() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var("AIRY_IME_DICT") {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(home) = std::env::var("AIRY_HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("share")
                    .join("agentrt")
                    .join("ime")
                    .join("airy_ime.dat"),
            );
        }
        // 二进制包布局（解压即用）：bin/agentrt-tui 与 share/ 平级，
        // 词典在 <exe>/../share/agentrt/ime/airy_ime.dat。避免用户
        // 直接从包解压运行（未安装、AIRY_HOME 未设）时 IME 因缺词典
        // 降级禁用（F10 无效果）。
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(
                    dir.join("..")
                        .join("share")
                        .join("agentrt")
                        .join("ime")
                        .join("airy_ime.dat"),
                );
            }
        }
        candidates.push(PathBuf::from("share/agentrt/ime/airy_ime.dat"));
        // 开发布局：agentrt 源码树内联词典（伞仓 sdk/tui → agent-workload/agentrt）
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../agentrt/commons/utils/ime/data/airy_ime.dat"),
        );
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 全拼前缀查询：pinyin 仅接受小写 [a-z]（ü 以 v 表示）。
    /// 返回候选文本（UTF-8，频次降序），空 = 无匹配或非法输入。
    pub fn query(&self, pinyin: &str) -> Vec<String> {
        #[cfg(all(feature = "ime", ime_linked))]
        {
            if pinyin.is_empty() {
                return Vec::new();
            }
            // C 侧仅接受小写字母，非 [a-z] 直接视为无候选
            if !pinyin.bytes().all(|b| b.is_ascii_lowercase()) {
                return Vec::new();
            }
            let cpy = match std::ffi::CString::new(pinyin.as_bytes()) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            const CAP: c_int = 9;
            let mut out = [ffi::airy_ime_cand {
                text: std::ptr::null(),
                freq: 0,
            }; CAP as usize];
            let n =
                unsafe { ffi::airy_ime_query(self.handle, cpy.as_ptr(), out.as_mut_ptr(), CAP) };
            if n <= 0 {
                return Vec::new();
            }
            let mut cands = Vec::with_capacity(n as usize);
            for i in 0..(n as usize).min(CAP as usize) {
                let ptr = out[i].text;
                if ptr.is_null() {
                    continue;
                }
                if let Ok(s) = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str() {
                    cands.push(s.to_string());
                }
            }
            return cands;
        }
        #[cfg(not(all(feature = "ime", ime_linked)))]
        {
            let _ = pinyin;
            Vec::new()
        }
    }
}

impl Drop for ImeEngine {
    fn drop(&mut self) {
        #[cfg(all(feature = "ime", ime_linked))]
        unsafe {
            ffi::airy_ime_destroy(self.handle)
        };
        #[cfg(not(all(feature = "ime", ime_linked)))]
        let _ = &self.handle;
    }
}

#[cfg(all(test, feature = "ime", ime_linked))]
mod tests {
    use super::*;

    #[test]
    fn ime_loads_dict_from_source_tree() {
        // 开发布局：词典从 agentrt 源码树定位（env!("CARGO_MANIFEST_DIR") 上溯）
        let eng = ImeEngine::load();
        assert!(eng.is_some(), "airy_ime.dat 应从 agentrt 源码树加载");
    }

    #[test]
    fn ime_query_returns_sorted_candidates() {
        let eng = ImeEngine::load().expect("dict loaded");
        // 全拼前缀："zhongguo" → 中国（词频最高，首位）
        let cands = eng.query("zhongguo");
        assert!(!cands.is_empty(), "zhongguo 应有候选");
        assert_eq!(cands[0], "中国", "首个候选应为词频最高的「中国」");
        // 短前缀 "ni" 应命中多候选（你好/你…）
        let cands2 = eng.query("ni");
        assert!(!cands2.is_empty(), "ni 应有候选");
        // 非法输入：大写/非字母 → 无候选
        assert!(eng.query("NI").is_empty(), "大写字母应被拒绝");
        assert!(eng.query("zhong1").is_empty(), "非字母应被拒绝");
    }
}
