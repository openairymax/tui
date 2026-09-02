// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导界面语言：环境检测 + 持久化码 + 选项展示名（单一来源）。

/// 界面语言（步骤 1 的选项集，见 `steps::LANG_CHOICES`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 自动检测（优先 LC_ALL，其次 LANG）
    Auto,
    /// English
    English,
    /// 简体中文
    Chinese,
}

impl Lang {
    /// 从环境检测界面语言：优先 LC_ALL，其次 LANG。
    pub fn detect() -> Lang {
        for var in ["LC_ALL", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                let l = v.to_ascii_lowercase();
                if l.contains("zh") {
                    return Lang::Chinese;
                }
                if l.contains("en") {
                    return Lang::English;
                }
            }
        }
        Lang::English
    }

    /// 选中后的实际生效语言（Auto → 环境检测）。
    pub(crate) fn resolve(self) -> Lang {
        match self {
            Lang::Auto => Lang::detect(),
            other => other,
        }
    }

    /// 选项展示名
    pub(crate) fn label(self) -> &'static str {
        match self {
            Lang::Auto => "自动检测 (Auto)",
            Lang::English => "English",
            Lang::Chinese => "简体中文",
        }
    }

    /// 持久化使用的语言码（zh / en）
    pub(crate) fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            _ => "zh",
        }
    }

    /// 文案选择器：true = 中文
    pub(crate) fn zh(self) -> bool {
        self == Lang::Chinese
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resolves_to_environment() {
        let _g = crate::test_env::lock_env();
        std::env::set_var("LC_ALL", "zh_CN.UTF-8");
        std::env::remove_var("LANG");
        assert_eq!(Lang::Auto.resolve(), Lang::Chinese);
        std::env::set_var("LC_ALL", "C");
        std::env::set_var("LANG", "en_US.UTF-8");
        assert_eq!(Lang::Auto.resolve(), Lang::English);
        std::env::remove_var("LC_ALL");
        std::env::remove_var("LANG");
        assert_eq!(Lang::Auto.resolve(), Lang::English);
    }

    #[test]
    fn explicit_lang_ignores_environment() {
        let _g = crate::test_env::lock_env();
        std::env::set_var("LC_ALL", "zh_CN.UTF-8");
        assert_eq!(Lang::English.resolve(), Lang::English);
        assert_eq!(Lang::Chinese.resolve(), Lang::Chinese);
        assert_eq!(Lang::English.code(), "en");
        assert_eq!(Lang::Chinese.code(), "zh");
        assert!(Lang::Chinese.zh());
        assert!(!Lang::English.zh());
        std::env::remove_var("LC_ALL");
    }
}
