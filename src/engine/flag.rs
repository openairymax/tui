// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 诊断开关解析（0.1.21 X8-06b / X1-31a 共用）。
//
// 帧日志与事件录制这类旁路通道共用同一套开关语义：只有显式的肯定值
// （1 / true / on，大小写与首尾空白不敏感）才开启，其余一律视为关闭。
// 失败方向固定为关闭——诊断通道不得因环境变量拼写错误而偷偷写盘。

use std::env;

/// 读取环境开关：变量缺失或取值非肯定值时返回 `false`。
pub(crate) fn on(name: &str) -> bool {
    on_value(env::var(name).ok().as_deref())
}

/// 取值判定（与变量读取分离，便于无环境依赖地锁定语义）。
pub(crate) fn on_value(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on"))
}

#[cfg(test)]
mod tests {
    use super::{on, on_value};

    /// 只有肯定值开启，空串、否定值与其它字样一律关闭。
    #[test]
    fn only_affirmative_values_enable() {
        for value in ["1", "true", "TRUE", " on "] {
            assert!(on_value(Some(value)), "{value:?} 应开启");
        }
        for value in ["", "0", "false", "off", "yes", "2"] {
            assert!(!on_value(Some(value)), "{value:?} 应关闭");
        }
        assert!(!on_value(None), "变量缺失应关闭");
    }

    /// 变量读取路径与取值判定同源；读取后随即清理，不污染其它用例。
    #[test]
    fn env_lookup_follows_same_rule() {
        let _lock = crate::test_env::lock_env();
        std::env::set_var("AIRY_TUI_TEST_FLAG", "on");
        assert!(on("AIRY_TUI_TEST_FLAG"));
        std::env::set_var("AIRY_TUI_TEST_FLAG", "no");
        assert!(!on("AIRY_TUI_TEST_FLAG"));
        std::env::remove_var("AIRY_TUI_TEST_FLAG");
        assert!(!on("AIRY_TUI_TEST_FLAG"));
    }
}
