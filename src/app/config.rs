// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// TUI 当前模型读写：统一落到配置权威源 $AIRY_HOME/config/model.yaml 的顶层
// `default_model:`（与 llm_d / think_d / gateway_d 同源，见 models_cfg）。
// 0.1.16 架构改造移除 TUI 独占的 $AIRY_HOME/data/agentrt/tui/config.toml，
// 当前模型不再单独持久化——它只是 model.yaml 的 default_model 的读写视图。

/// AIRY_HOME（用于展示统一配置文件路径）
pub(super) fn airy_home() -> String {
    crate::paths::airy_home().to_string_lossy().into_owned()
}

/// 加载当前模型名（model.yaml 的 default_model；缺省/为空时返回 None，
/// 表示由网关 / llm_d 自动回落默认）。
pub(super) fn load_saved_model() -> Option<String> {
    let m = crate::models_cfg::read_model_yaml();
    if m.default_model.trim().is_empty() {
        None
    } else {
        Some(m.default_model.trim().to_string())
    }
}

/// 持久化当前模型名到 model.yaml 的 default_model（保留文件其余内容）。
pub(super) fn persist_model(model: &str) {
    match crate::models_cfg::set_default_model(model) {
        Ok(()) => log::info!(
            "model config: default_model → {}（{}）",
            model.trim(),
            crate::models_cfg::model_yaml_path().display()
        ),
        Err(e) => log::warn!("model config: {}", e),
    }
}
