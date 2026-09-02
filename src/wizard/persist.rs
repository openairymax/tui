// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导持久化：wizard.toml（lang + configured + 版本 + 提供商/模型）、
// secrets.env（API Key，llm_d 热加载）、首次运行探测与旧目录迁移。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 向导配置文件（$AIRY_HOME/data/agentrt/tui/wizard.toml），存在即非首次运行
const WIZARD_FILE: &str = "wizard.toml";

/// API Key 写入 secrets.env 使用的变量名（与 model.yaml api_key_env 对应）
pub(crate) const API_KEY_ENV: &str = "MODEL_1_API_KEY";

#[derive(Serialize, Deserialize)]
struct WizardConfig {
    lang: String,
    configured: String,
    version: String,
    provider: String,
    model: String,
}

/// 向导目录：$AIRY_HOME/data/agentrt/tui（与 TUI config.toml 同目录约定；
/// 旧版曾用 $AIRY_HOME/tui，读时自动迁移）
fn wizard_dir() -> PathBuf {
    let home = crate::paths::airy_home();
    let legacy = home.join("tui").join(WIZARD_FILE);
    let new_dir = home.join("data").join("agentrt").join("tui");
    if legacy.is_file()
        && !new_dir.join(WIZARD_FILE).exists()
        && std::fs::create_dir_all(&new_dir).is_ok()
    {
        let _ = std::fs::rename(&legacy, new_dir.join(WIZARD_FILE));
    }
    new_dir
}

/// 运行配置目录：$AIRY_HOME/config（secrets.env 所在目录）
fn config_dir() -> PathBuf {
    crate::paths::airy_home_path(&["config"])
}

fn config_path() -> PathBuf {
    wizard_dir().join(WIZARD_FILE)
}

/// 是否首次运行（wizard.toml 不存在）
pub(crate) fn is_first_run() -> bool {
    !config_path().exists()
}

/// 将 API Key 写回 $AIRY_HOME/config/secrets.env（llm_d 热加载，无需重启）。
///
/// 已有同名变量行 → 原位替换值；无则追加到文件末尾。失败返回 false（不阻断向导）。
pub(crate) fn write_secret(env_name: &str, value: &str) -> bool {
    if env_name.is_empty() || value.is_empty() {
        return false;
    }
    let path = config_dir().join("secrets.env");
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("wizard: create config dir failed: {}", e);
            return false;
        }
    }
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let marker = format!("{}=", env_name);
    let mut lines: Vec<String> = original.lines().map(|l| l.to_string()).collect();
    let line = format!("{}{}", marker, value);
    let mut replaced = false;
    for l in lines.iter_mut() {
        let trimmed = l.trim_start();
        if trimmed.starts_with(&marker) && !trimmed.starts_with('#') {
            *l = line.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        if !original.ends_with('\n') && !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(line);
    }
    let content = lines.join("\n") + "\n";
    if std::fs::write(&path, content).is_ok() {
        log::info!(
            "wizard: API Key written to secrets.env ({}={})",
            env_name,
            if value.len() > 4 {
                format!("sk-…{}", &value[value.len() - 4..])
            } else {
                "***".to_string()
            }
        );
        true
    } else {
        log::warn!("wizard: write secrets.env failed");
        false
    }
}

/// 将选择写回 wizard.toml（lang + configured + version + provider + model）
pub(crate) fn persist(lang: &str, configured: bool, provider: &str, model: &str) {
    let cfg = WizardConfig {
        lang: lang.to_string(),
        configured: if configured {
            "manual".to_string()
        } else {
            "skipped".to_string()
        },
        version: env!("AIRY_RT_VERSION").to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
    };
    if let Some(parent) = config_path().parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("wizard: create dir failed: {}", e);
            return;
        }
    }
    match toml::to_string(&cfg) {
        Ok(s) => {
            if let Err(e) = std::fs::write(config_path(), s) {
                log::warn!("wizard: persist failed: {}", e);
            } else {
                log::info!("wizard: config saved to {}", config_path().display());
            }
        }
        Err(e) => log::warn!("wizard: serialize failed: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_until_persisted() {
        let home = crate::test_env::Home::new("firstrun");
        assert!(is_first_run());
        persist("zh", true, "qwen", "qwen-max");
        assert!(!is_first_run());
        let saved =
            std::fs::read_to_string(home.path().join("data/agentrt/tui/wizard.toml")).expect("已写盘");
        assert!(saved.contains("lang = \"zh\""));
        assert!(saved.contains("configured = \"manual\""));
        assert!(saved.contains("model = \"qwen-max\""));
    }

    #[test]
    fn legacy_dir_migrated_on_read() {
        let home = crate::test_env::Home::new("migrate");
        let legacy = home.path().join("tui");
        std::fs::create_dir_all(&legacy).expect("建旧目录");
        std::fs::write(legacy.join(WIZARD_FILE), "lang = \"en\"\n").expect("写旧文件");
        assert!(!is_first_run(), "旧路径文件应被识别并迁移");
        assert!(!legacy.join(WIZARD_FILE).exists(), "旧文件已移走");
        assert!(home.path().join("data/agentrt/tui").join(WIZARD_FILE).is_file());
    }

    #[test]
    fn secret_replace_and_append() {
        let home = crate::test_env::Home::new("secret");
        let env_file = home.path().join("config").join("secrets.env");
        assert!(write_secret(API_KEY_ENV, "sk-new"));
        let content = std::fs::read_to_string(&env_file).expect("已写盘");
        assert!(content.contains("MODEL_1_API_KEY=sk-new"));
        assert!(write_secret(API_KEY_ENV, "sk-replaced"));
        let content = std::fs::read_to_string(&env_file).expect("已写盘");
        assert_eq!(content.matches("MODEL_1_API_KEY=").count(), 1, "原位替换不重复追加");
        assert!(content.contains("sk-replaced"));
        assert!(!write_secret(API_KEY_ENV, ""), "空值不写");
    }
}
