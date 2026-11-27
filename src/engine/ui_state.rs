// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 启动形态失败计数（X8-07/X8-08）：`ui_state.json` 记录连续备用屏进入失败次数。
//
// 备用屏进入失败常见于限制型终端、复用会话与 IDE 面板：每次都先清屏再回滚，
// 用户看到的是「每次启动都闪一下」。连续失败达到阈值后，自动形态不再重试
// 备用屏，直接内联；一次全屏成功即清零，显式 `AIRY_TUI_SCREEN` 始终优先。
//
// 落盘用「临时文件 + rename」：写中途被杀不会留下半截 JSON。读取侧对缺失、
// 不可读、损坏一律按「无记录」处理——启动路径绝不因状态文件失败而失败。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths;

/// 状态文件名（与机制侧数据目录同级，随 AIRY_HOME 迁移）。
const FILE: &str = "ui_state.json";

/// 连续失败阈值：达到即放弃自动全屏。
pub(crate) const FAIL_LIMIT: u32 = 3;

/// 持久化内容；未知字段忽略、缺字段取默认，便于后续追加而不破旧文件。
#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    screen_fails: u32,
}

/// 状态文件路径：`<home>/data/agentrt/ui_state.json`。
fn path() -> PathBuf {
    paths::airy_home_path(&["data", "agentrt", FILE])
}

/// 读取状态；任何读取/解析失败都退化为默认值。
fn load() -> State {
    let p = path();
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return State::default();
    };
    match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("ui_state: {} 解析失败（{e}），按无记录处理", p.display());
            State::default()
        }
    }
}

/// 原子写入：目录按需创建，先写临时文件再替换。
fn save(state: &State) {
    let p = path();
    let Some(dir) = p.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("ui_state: 建目录 {} 失败（{e}）", dir.display());
        return;
    }
    let Ok(raw) = serde_json::to_vec_pretty(state) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &raw) {
        log::warn!("ui_state: 写 {} 失败（{e}）", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &p) {
        log::warn!("ui_state: 替换 {} 失败（{e}）", p.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 当前连续失败次数。
pub(crate) fn fails() -> u32 {
    load().screen_fails
}

/// 记录一次备用屏进入失败，返回累计值（封顶阈值，避免无界增长）。
pub(crate) fn note_fail() -> u32 {
    let mut state = load();
    state.screen_fails = state.screen_fails.saturating_add(1).min(FAIL_LIMIT);
    save(&state);
    state.screen_fails
}

/// 备用屏成功进入：清零（已为零则不写盘，避免每次启动都改文件）。
pub(crate) fn reset() {
    if fails() != 0 {
        save(&State::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;
    use std::sync::MutexGuard;

    /// 隔离 AIRY_HOME 的测试夹具；Drop 恢复环境并清目录。
    struct Scratch {
        _g: MutexGuard<'static, ()>,
        dir: PathBuf,
        saved: Option<String>,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let g = lock_env();
            let dir = std::env::temp_dir().join(format!(
                "airy-ui-state-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let saved = std::env::var("AIRY_HOME").ok();
            std::env::set_var("AIRY_HOME", &dir);
            Self {
                _g: g,
                dir,
                saved,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            match &self.saved {
                Some(v) => std::env::set_var("AIRY_HOME", v),
                None => std::env::remove_var("AIRY_HOME"),
            }
        }
    }

    /// X8-07b #1：文件缺失时计数为 0，且不因此触发回退。
    #[test]
    fn missing_file_reads_zero() {
        let _s = Scratch::new("missing");
        assert_eq!(fails(), 0);
        assert!(fails() < FAIL_LIMIT);
    }

    /// X8-07a：失败计数跨进程持久——写入后重新读取仍然可见。
    #[test]
    fn fail_count_persists() {
        let _s = Scratch::new("persist");
        assert_eq!(note_fail(), 1);
        assert_eq!(note_fail(), 2);
        assert_eq!(fails(), 2, "计数值必须落盘可重读");
        assert!(path().exists(), "状态文件应在 data/agentrt 下生成");
    }

    /// X8-07b #2：损坏文件按无记录处理，不 panic 也不阻断启动。
    #[test]
    fn corrupt_file_is_ignored() {
        let _s = Scratch::new("corrupt");
        let p = path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{ not json").unwrap();
        assert_eq!(fails(), 0);
        // 损坏后仍可正常写入并恢复计数能力。
        assert_eq!(note_fail(), 1);
        assert_eq!(fails(), 1);
    }

    /// X8-07a：计数封顶阈值，长期失败不会让状态文件无界增长。
    #[test]
    fn fail_count_is_capped() {
        let _s = Scratch::new("capped");
        for _ in 0..(FAIL_LIMIT + 5) {
            note_fail();
        }
        assert_eq!(fails(), FAIL_LIMIT);
    }

    /// 全屏成功后计数清零，下次启动重新尝试备用屏。
    #[test]
    fn reset_clears_failures() {
        let _s = Scratch::new("reset");
        note_fail();
        note_fail();
        reset();
        assert_eq!(fails(), 0);
    }
}
