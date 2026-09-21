// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! 内置拼音输入法（IME）：纯 Rust 词典引擎，直接消费 agentrt 生成的
//! `airy_ime.dat`（commons/utils/ime，布局见 tools/gen_ime_dict.py），
//! 零 FFI、零 C 构建依赖——x86/ARM/RISC-V 与 Windows/Linux/macOS 一致可用。
//!
//! `ImeEngine::load()` 一次性把词典读入内存并做 magic/version/边界/CRC32
//! 校验（fail-closed：任一不符即拒绝加载）；`query()` 以全拼前缀二分定位
//! 命中条目、合并候选并按词频降序返回。词典缺失或损坏时 `load()` 返回
//! None（IME 降级禁用，Ctrl+1 无效果，英文输入不受影响）。
//!
//! 格式（全部小端，与 C 侧 airy_ime.c 二进制兼容）：
//!   Header(24B) | Entries[N*12B] | 候选区 | 字符串池
//!   Header = "AIRYIME1"(8) | version u32 | count u32 | crc32 u32 | pool_off u32
//!   Entry  = pinyin_off u32 | cand_off u32 | cand_count u16 | pad u16
//!   Cand   = text_off u32 | freq u32
//! 其中 pinyin_off/text_off 相对字符串池，cand_off 相对候选区起点。
//!
//! 状态机（拼音态：切换/输入/选字/上屏）在 app/input.rs 实现：
//!   - Ctrl+1 切换 中/英；切回英文时拼音原文上屏
//!   - a-z 追加拼音并实时刷新候选；1-9 选字（选字后保持拼音态，连续词组
//!     输入不中断）；空格选第一个候选
//!   - Backspace 删拼音（空则退出拼音态）；Enter 提交拼音原文走正常提交
//!   - 其他可见字符：拼音原文上屏后按正常路径处理

use std::path::PathBuf;

const MAGIC: &[u8; 8] = b"AIRYIME1";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 24;
const ENTRY_SIZE: usize = 12;
const CAND_SIZE: usize = 8;
/// 单次查询收集候选上限：防极端短前缀（如 "a"）扫出过量条目。
const SCAN_CAP: usize = 4096;
/// 一次取足 3 页（0.1.3 微信式分页 9×3），翻页不再回查词典。
const OUT_CAP: usize = 27;

/// 小端 u32 读取（越界返回 None，跨端序一致）。
fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    let s = buf.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 小端 u16 读取（越界返回 None）。
fn rd_u16(buf: &[u8], off: usize) -> Option<u16> {
    let s = buf.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

/// 从 `off` 起读取 '\0' 结尾字节串（不含结尾符；越界/无终结符返回 None）。
fn cstr_at(buf: &[u8], off: usize) -> Option<&[u8]> {
    let rest = buf.get(off..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    rest.get(..end)
}

/// CRC32（IEEE 802.3，逐位展开）：与 C 侧 airy_ime_crc32 查表实现结果等价。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// 拼音输入法引擎（词典内存视图 + 二分前缀查询）。
///
/// 自持文件内容所有权（`Vec<u8>`），无裸指针、无 Drop 时机依赖，
/// 天然 `Send + Sync`（与 UI 单线程事件循环无关）。
pub struct ImeEngine {
    buf: Vec<u8>,
    entry_count: u32,
    pool_off: u32,
}

impl ImeEngine {
    /// 按优先级加载词典（与 C 侧 tui_ime_load_dict 一致）：
    ///   AIRY_IME_DICT env → $AIRY_HOME/share/agentrt/ime/airy_ime.dat
    ///   → <exe>/../share/agentrt/ime/airy_ime.dat
    ///   → ./share/agentrt/ime/airy_ime.dat → agentrt 源码树 data/airy_ime.dat
    /// 全部缺失或内容校验失败返回 None（IME 禁用，fail-closed）。
    pub fn load() -> Option<Self> {
        let path = Self::locate_dict()?;
        let buf = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("ime: dict unreadable: {} ({})", path.display(), e);
                return None;
            }
        };
        match Self::parse(buf) {
            Some(eng) => {
                log::info!("ime: builtin dict ready: {}", path.display());
                Some(eng)
            }
            None => {
                log::warn!("ime: dict rejected as corrupt: {}", path.display());
                None
            }
        }
    }

    /// 校验并接管词典内容（fail-closed：magic/version/边界/CRC 逐项核对）。
    fn parse(buf: Vec<u8>) -> Option<Self> {
        let size = buf.len();
        if size < HEADER_SIZE || buf.get(..8) != Some(&MAGIC[..]) {
            return None;
        }
        if rd_u32(&buf, 8)? != VERSION {
            return None;
        }
        let count = rd_u32(&buf, 12)? as usize;
        let crc = rd_u32(&buf, 16)?;
        let pool_off = rd_u32(&buf, 20)? as usize;
        // 条目区不越界：count*12 必须容纳于 header 之后
        if count > (size - HEADER_SIZE) / ENTRY_SIZE {
            return None;
        }
        let entries_end = HEADER_SIZE + count * ENTRY_SIZE;
        if pool_off < entries_end || pool_off > size {
            return None;
        }
        if crc32(buf.get(HEADER_SIZE..)?) != crc {
            return None;
        }
        Some(Self {
            buf,
            entry_count: count as u32,
            pool_off: pool_off as u32,
        })
    }

    /// 第 `idx` 条 entry 的拼音串（字典序有序）。
    fn entry_py(&self, idx: usize) -> Option<&[u8]> {
        let off = HEADER_SIZE.checked_add(idx.checked_mul(ENTRY_SIZE)?)?;
        self.pool_str(rd_u32(&self.buf, off)?)
    }

    /// 第 `idx` 条 entry 的候选区（绝对起点, 候选数）。
    fn entry_cands(&self, idx: usize) -> Option<(usize, u16)> {
        let off = HEADER_SIZE.checked_add(idx.checked_mul(ENTRY_SIZE)?)?;
        let cand_off = rd_u32(&self.buf, off.checked_add(4)?)? as usize;
        let n = rd_u16(&self.buf, off.checked_add(8)?)?;
        let base = HEADER_SIZE
            .checked_add((self.entry_count as usize).checked_mul(ENTRY_SIZE)?)?
            .checked_add(cand_off)?;
        Some((base, n))
    }

    /// 候选区第 `k` 项（文本, 词频）。
    fn cand_at(&self, base: usize, k: usize) -> Option<(&[u8], u32)> {
        let off = base.checked_add(k.checked_mul(CAND_SIZE)?)?;
        let text = self.pool_str(rd_u32(&self.buf, off)?)?;
        let freq = rd_u32(&self.buf, off.checked_add(4)?)?;
        Some((text, freq))
    }

    /// 字符串池内相对偏移处的 '\0' 结尾字节串。
    fn pool_str(&self, rel: u32) -> Option<&[u8]> {
        let off = self.pool_off.checked_add(rel)? as usize;
        cstr_at(&self.buf, off)
    }

    /// 词典候选路径探测（环境变量 → 安装布局 → 包布局 → 开发布局）。
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
        // 词典在 <exe>/../share/agentrt/ime/airy_ime.dat。避免用户直接从
        // 包解压运行（未安装、AIRY_HOME 未设）时 IME 因缺词典降级禁用。
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
    /// 返回候选文本（UTF-8，词频降序，同频按文本升序），
    /// 空 = 无匹配或非法输入。上限 OUT_CAP（27 = 3 页 × 9）。
    pub fn query(&self, pinyin: &str) -> Vec<String> {
        let key = pinyin.as_bytes();
        if key.is_empty() || !key.iter().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }

        // 二分下界：首个拼音 >= key 的条目（与 C 侧 lower_bound 同语义）
        let (mut lo, mut hi) = (0usize, self.entry_count as usize);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.entry_py(mid) {
                Some(py) if py < key => lo = mid + 1,
                _ => hi = mid,
            }
        }

        // 顺序扫描前缀命中条目（字典序：前缀区间结束即停），合并候选
        let mut hits: Vec<(&[u8], u32)> = Vec::new();
        let mut idx = lo;
        while idx < self.entry_count as usize && hits.len() < SCAN_CAP {
            let py = match self.entry_py(idx) {
                Some(p) => p,
                None => break,
            };
            if !py.starts_with(key) {
                break;
            }
            if let Some((base, n)) = self.entry_cands(idx) {
                for k in 0..n as usize {
                    if hits.len() >= SCAN_CAP {
                        break;
                    }
                    if let Some(hit) = self.cand_at(base, k) {
                        hits.push(hit);
                    }
                }
            }
            idx += 1;
        }

        // 词频降序稳定排序，同频按文本升序（与 C 侧稳定插排结果一致）
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        hits.truncate(OUT_CAP);
        hits.into_iter()
            .map(|(text, _)| String::from_utf8_lossy(text).into_owned())
            .collect()
    }
}

#[cfg(test)]
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

    #[test]
    fn ime_rejects_corrupt_dict() {
        // 空/过短 → 拒绝
        assert!(ImeEngine::parse(Vec::new()).is_none());
        assert!(ImeEngine::parse(b"AIRYIME1".to_vec()).is_none());
        // magic 不符（长度足够）→ 拒绝
        let mut bad_magic = vec![0u8; HEADER_SIZE];
        bad_magic[..8].copy_from_slice(b"AIRYIME0");
        assert!(ImeEngine::parse(bad_magic).is_none());

        // 合法头（count=0、pool_off=24、crc=crc32(空)=0）→ 接受
        let mut ok = Vec::new();
        ok.extend_from_slice(MAGIC);
        ok.extend_from_slice(&VERSION.to_le_bytes());
        ok.extend_from_slice(&0u32.to_le_bytes());
        ok.extend_from_slice(&crc32(&[]).to_le_bytes());
        ok.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        let eng = ImeEngine::parse(ok.clone()).expect("合法空词典应可加载");
        assert!(eng.query("ni").is_empty(), "空词典无任何候选");

        // CRC 不符 → 拒绝（fail-closed）
        let mut bad_crc = ok.clone();
        bad_crc[16..20].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        assert!(ImeEngine::parse(bad_crc).is_none(), "CRC 不符应拒绝");

        // pool_off 落在条目区之前 → 拒绝
        let mut bad_pool = ok;
        bad_pool[20..24].copy_from_slice(&8u32.to_le_bytes());
        assert!(ImeEngine::parse(bad_pool).is_none(), "pool_off 越界应拒绝");
    }
}
