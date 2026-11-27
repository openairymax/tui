// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 组件注册表（0.1.21 X3-05/X3-06）：命名空间约定 + 按名取件 + 启动期冲突
// 即失败。
//
// 约定：组件名形如 `域/名`（恰好一个 `/`，两段均非空，只允许小写字母、
// 数字与 `-`）。命名空间用于把同域组件聚在一起（`hall/board`），也避免
// 面板改名后旧名被静默复用。
//
// 冲突即失败（fail-closed）：重名注册返回错误而不是覆盖——覆盖会让先注册
// 的组件在生产环境静默消失，这类缺陷在运行时几乎无法定位；启动期直接
// 拒绝启动并给出重名，才能把问题暴露在最早时刻（与 X5-05 键位表同策）。

use std::collections::HashMap;

/// 组件类别（注册表只存元数据，实例由使用方按类别构造）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Table,
}

/// 按名索引的注册表。值类型由使用方决定，避免注册表反向依赖具体组件。
#[derive(Debug)]
pub(crate) struct Registry<V> {
    map: HashMap<&'static str, V>,
}

impl<V> Registry<V> {
    pub(crate) fn empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 注册一个组件；名称不合规或已存在时返回错误（不覆盖）。
    pub(crate) fn register(&mut self, name: &'static str, v: V) -> Result<(), String> {
        if !valid_name(name) {
            return Err(format!("组件名不合规：{name}（应为 域/名）"));
        }
        if self.map.contains_key(name) {
            return Err(format!("组件重名：{name}"));
        }
        self.map.insert(name, v);
        Ok(())
    }

    pub(crate) fn get(&self, name: &str) -> Option<&V> {
        self.map.get(name)
    }
}

/// 名称合规：恰好一个 `/`，两段均非空且只含 `[a-z0-9-]`。
fn valid_name(name: &str) -> bool {
    let mut parts = name.split('/');
    let ns = parts.next().unwrap_or("");
    let item = parts.next().unwrap_or("");
    parts.next().is_none() && seg_ok(ns) && seg_ok(item)
}

fn seg_ok(seg: &str) -> bool {
    !seg.is_empty()
        && seg
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 内置组件清单。新增内置组件必须登记在此，否则启动自检视为缺失。
pub(crate) fn builtin() -> Result<Registry<Kind>, String> {
    let mut reg = Registry::empty();
    reg.register("hall/board", Kind::Table)?;
    Ok(reg)
}

/// 启动期自检：内置清单可构建且必需组件存在（X3-06b）。
pub(crate) fn validate_builtin() -> Result<(), String> {
    let reg = builtin()?;
    reg.get("hall/board")
        .map(|_| ())
        .ok_or_else(|| "缺少内置组件：hall/board".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_gets_by_name() {
        let mut reg = Registry::empty();
        assert!(reg.register("hall/board", Kind::Table).is_ok());
        assert_eq!(reg.get("hall/board"), Some(&Kind::Table));
        assert_eq!(reg.get("hall/none"), None);
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut reg = Registry::empty();
        reg.register("hall/board", Kind::Table).unwrap();
        let err = reg.register("hall/board", Kind::Table).unwrap_err();
        assert!(err.contains("重名"), "{err}");
    }

    #[test]
    fn invalid_names_are_rejected() {
        let mut reg: Registry<Kind> = Registry::empty();
        for name in ["", "board", "hall/", "/board", "hall/board/x", "Hall/Board", "hall/bo ard"]
        {
            assert!(reg.register(name, Kind::Table).is_err(), "{name:?} 应被拒绝");
        }
        assert!(reg.register("hall/bo-ard2", Kind::Table).is_ok());
    }

    #[test]
    fn builtin_selfcheck_passes() {
        assert!(validate_builtin().is_ok());
    }
}
