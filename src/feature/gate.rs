// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 实验开关（0.1.21 X9-01a）：X 面渲染改动的统一放行闸门。
//
// 约定：开关名一律 snake_case，环境变量键为 `AIRY_TUI_EXP_<NAME>`（名转
// 大写），取值为 `1` / `true` / `on`（大小写不敏感）即开，其余（含缺失）
// 一律关。默认全关——未登记的改动不得假设开关存在；本模块只做放行判定，
// 不读配置文件、不热更新、不落盘，避免把「实验」长成第二套配置系统。
//
// fail-closed：名非法（空、超长、非 [a-z0-9_]）按未登记处理，直接关。
// 非法名不 panic、不日志刷屏——它不是运行期故障，而是调用方的编码错误，
// 由单测与评审拦截。

/// 环境变量前缀：完整键 = 前缀 + 大写开关名。
const PREFIX: &str = "AIRY_TUI_EXP_";

/// 开关名上界：超过此长度不可能是登记项，按未登记处理。
const NAME_MAX: usize = 32;

/// 开关是否放行；名非法或变量缺失时返回 false（fail-closed）。
pub(crate) fn enabled(name: &str) -> bool {
    match env_key(name) {
        Some(key) => truthy(std::env::var_os(key).as_deref()),
        None => false,
    }
}

/// 组装环境变量键；名非法时返回 None。
fn env_key(name: &str) -> Option<String> {
    let legal = !name.is_empty()
        && name.len() <= NAME_MAX
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    legal.then(|| format!("{PREFIX}{}", name.to_ascii_uppercase()))
}

/// 取值判定：`1` / `true` / `on` 为开，其余一律关。
fn truthy(val: Option<&std::ffi::OsStr>) -> bool {
    let Some(text) = val.and_then(|v| v.to_str()) else {
        return false;
    };
    let text = text.trim();
    text == "1" || text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("on")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;

    fn with(name: &str, val: &str) -> bool {
        let _guard = lock_env();
        let key = env_key(name).expect("测试名必须合法");
        std::env::set_var(&key, val);
        let got = enabled(name);
        std::env::remove_var(&key);
        got
    }

    #[test]
    fn missing_variable_is_off() {
        assert!(!with("x9_missing", ""));
        assert!(!enabled("x9_never_registered"));
    }

    #[test]
    fn truthy_values_turn_on() {
        for val in ["1", "true", "TRUE", "on", "On", " 1 "] {
            assert!(with("x9_truthy", val), "取值 {val:?} 必须放行");
        }
    }

    #[test]
    fn other_values_stay_off() {
        for val in ["0", "false", "off", "", "yes", "2"] {
            assert!(!with("x9_falsy", val), "取值 {val:?} 必须关闭");
        }
    }

    #[test]
    fn illegal_names_are_off() {
        for name in ["", "Upper", "with space", "dash-name", &"a".repeat(NAME_MAX + 1)] {
            assert!(!enabled(name), "非法名 {name:?} 必须关闭");
        }
        assert!(env_key("ok_9").is_some());
        assert_eq!(env_key("ok_9").as_deref(), Some("AIRY_TUI_EXP_OK_9"));
    }
}
