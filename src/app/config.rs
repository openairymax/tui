// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// TUI 本地配置：$AIRY_HOME 配置目录定位与当前模型名持久化（config.toml）。

/// 用户配置目录：$AIRY_HOME/data/agentrt/tui（AIRY_HOME 路径体系收敛，2026-08-19）
pub(super) fn tui_config_dir() -> std::path::PathBuf {
    crate::paths::airy_home_path(&["data", "agentrt", "tui"])
}

/// AIRY_HOME（用于展示 model.yaml 用户覆盖配置路径）
pub(super) fn airy_home() -> String {
    crate::paths::airy_home().to_string_lossy().into_owned()
}

/// TUI 本地配置（config.toml）：目前持久化当前模型名。
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct TuiConfig {
    model: String,
    version: String,
}

pub(super) fn config_path() -> std::path::PathBuf {
    tui_config_dir().join("config.toml")
}

/// 加载上次保存的模型名（config.toml 不存在或损坏时返回 None）。
pub(super) fn load_saved_model() -> Option<String> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    let cfg: TuiConfig = toml::from_str(&raw).ok()?;
    if cfg.model.is_empty() {
        None
    } else {
        Some(cfg.model)
    }
}

/// 持久化当前模型名到 config.toml（保留版本字段，未来可扩展）。
pub(super) fn persist_model(model: &str) {
    let cfg = TuiConfig {
        model: model.to_string(),
        version: env!("AIRY_RT_VERSION").to_string(),
    };
    if let Some(parent) = config_path().parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("model config: create dir failed: {}", e);
            return;
        }
    }
    match toml::to_string(&cfg) {
        Ok(s) => {
            if let Err(e) = std::fs::write(config_path(), s) {
                log::warn!("model config: persist failed: {}", e);
            } else {
                log::info!("model config saved to {}", config_path().display());
            }
        }
        Err(e) => log::warn!("model config: serialize failed: {}", e),
    }
}
