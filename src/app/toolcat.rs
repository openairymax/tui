// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 工具目录（0.1.21 D-11）：对话里的工具可见性以 tool.list 的真实目录驱动，
// 客户端不再维护白名单猜测。目录未加载或工具未登记时仍显示原始 tool_id
// 与参数摘要——把未知工具藏起来会让「工具明明跑了、界面却空白」比显示内部
// 标识符更难诊断，也让新增工具在旧客户端上完全不可见。

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

use serde_json::Value;

static CATALOG: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn cat() -> &'static RwLock<HashSet<String>> {
    CATALOG.get_or_init(|| RwLock::new(HashSet::new()))
}

/// 用 tool.list 结果覆盖目录。元素形如 {id, name, ...}；展示名一律取
/// `id`——`name` 当前是英文描述或内部标识，直接上屏会把英文文案带进
/// 中文界面，且与 CLI 的动作短语语义不一致。
pub fn load(items: &[Value]) {
    let ids: HashSet<String> = items
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if let Ok(mut guard) = cat().write() {
        *guard = ids;
    }
}

/// 工具是否在 daemon 目录内。未加载时恒 false——调用方只可把它当作
/// 补充信息（如子智能体标签），不得据此隐藏事件行。
pub fn known(tool: &str) -> bool {
    cat().read().map(|g| g.contains(tool)).unwrap_or(false)
}

/// 目录是否已加载。未加载时不对工具做任何"未登记"标注——离线/降级期间
/// 目录为空是常态，误标会把正常工具说成异常。
pub fn loaded() -> bool {
    cat().read().map(|g| !g.is_empty()).unwrap_or(false)
}

/// 上屏用的工具名：目录已加载且工具未登记时追加"未登记"标记，事件本身
/// 照常显示（D-11 禁止隐藏）。未加载时原样返回 tool_id。
pub fn label(tool: &str) -> String {
    if !loaded() || known(tool) {
        tool.to_string()
    } else {
        format!("{tool}（未登记）")
    }
}

/// 工具事件单行文案（对话区唯一装配点）：`<id>（<参数摘要>）…`，失败追加
/// 「（失败）」。参数摘要压成一行并截断，避免多行参数把预览挤爆。
pub fn event_line(tool: &str, args: &str, ok: bool) -> String {
    let name = label(tool);
    let brief: String = args.replace(['\n', '\r'], " ").chars().take(80).collect();
    let head = if brief.is_empty() {
        format!("{name}…")
    } else {
        format!("{name}（{brief}）…")
    };
    if ok {
        head
    } else {
        format!("{head}（失败）")
    }
}

/// 用 tool.list 应答装载目录。gateway 可能回数组，也可能回 C-16 目录对象
/// `{tools:[...]}`；两者都认。空/异形应答按「未加载」处理：目录是增强
/// 信息，不能把空表当成「所有工具都未登记」。
pub fn absorb(v: &Value) {
    let items = v
        .get("tools")
        .and_then(Value::as_array)
        .or_else(|| v.as_array());
    let Some(items) = items else {
        log::warn!("toolcat: tool.list 应答无目录字段");
        return;
    };
    log::info!("toolcat: tool.list -> {} entries", items.len());
    load(items);
}
