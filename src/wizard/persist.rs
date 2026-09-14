// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导落盘与首启探测：全部收敛到统一配置面（0.1.16 架构改造，用户决策
// 2026-09-13「CLI 是核心，TUI 是可选的增强」）——
//   - $AIRY_HOME/config/model.yaml（模型表 + default_model + think 段，
//     llm_d / think_d / gateway_d 共同读取的热加载权威源）；
//   - $AIRY_HOME/config/secrets.env（API Key，llm_d 热加载、权限 600）。
// TUI 不持有任何独占配置文件（wizard.toml / config.toml 已随本轮移除），
// 引导状态由上述两文件的真实内容推导，无额外状态文件。

use std::path::PathBuf;

/// API Key 写入 secrets.env 使用的变量名（与 model.yaml api_key_env 对应）
pub(crate) const API_KEY_ENV: &str = "MODEL_1_API_KEY";

/// 运行配置目录：$AIRY_HOME/config（model.yaml 与 secrets.env 所在目录）
fn config_dir() -> PathBuf {
    crate::paths::airy_home_path(&["config"])
}

/// 是否尚未完成模型配置（首启向导自动激活判据）。
///
/// 判据只来自统一配置面，不含任何 TUI 独占状态文件。满足任一即视为
/// "尚不可用"，入场时引导用户完成配置：
///   - `model.yaml` 缺失，或未设置顶层 `default_model`；
///   - `default_model` 指向的模型为 api 模式，但其 `api_key_env`
///     （缺省 `MODEL_1_API_KEY`）在 secrets.env 中没有非空值。
///
/// `local` 模式模型无需 Key，视为已就绪；`default_model` 不在 models 表中
/// （用户自定义或由网关回落）亦视为已配置，不再打扰。
pub(crate) fn is_first_run() -> bool {
    let yaml = crate::models_cfg::read_model_yaml();
    let default_model = yaml.default_model.trim();
    if default_model.is_empty() {
        return true;
    }
    let row = match yaml.rows.iter().find(|r| r.model_id == default_model) {
        Some(r) => r,
        None => return false,
    };
    if row.mode.trim() == "local" {
        return false;
    }
    let env = if row.api_key_env.trim().is_empty() {
        API_KEY_ENV
    } else {
        row.api_key_env.trim()
    };
    !crate::secrets::read_all()
        .iter()
        .any(|(k, v)| k == env && !v.trim().is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_model_yaml(body: &str) {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).expect("config 目录");
        std::fs::write(dir.join("model.yaml"), body).expect("写 model.yaml");
    }

    const API_ROW: &str = "models:\n  - name: DeepSeek\n    mode: api\n    api_format: openai\n    base_url: https://api.deepseek.com\n    model_id: deepseek-chat\n    api_key_env: MODEL_1_API_KEY\ndefault_model: deepseek-chat\n";

    #[test]
    fn first_run_when_model_yaml_missing() {
        let _home = crate::test_env::Home::new("firstrun-missing");
        assert!(is_first_run(), "无 model.yaml 应首启");
    }

    #[test]
    fn api_default_without_key_still_first_run() {
        let _home = crate::test_env::Home::new("firstrun-api");
        write_model_yaml(API_ROW);
        assert!(is_first_run(), "api 模型无 Key → 仍需引导");
        assert!(write_secret(API_KEY_ENV, "sk-abcdef"));
        assert!(!is_first_run(), "写入 Key 后视为已就绪");
    }

    #[test]
    fn local_default_model_ready_without_key() {
        let _home = crate::test_env::Home::new("firstrun-local");
        write_model_yaml(
            "models:\n  - name: Local\n    mode: local\n    base_url: http://localhost:11434/v1\n    model_id: llama3\n    api_key_env: \"\"\ndefault_model: llama3\n",
        );
        assert!(!is_first_run(), "本地模型无需 Key，视为已就绪");
    }

    #[test]
    fn unknown_default_model_is_ready() {
        let _home = crate::test_env::Home::new("firstrun-unknown");
        write_model_yaml("default_model: custom-remote\n");
        assert!(!is_first_run(), "默认模型不在表中（网关回落）不再打扰");
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
