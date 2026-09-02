// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// secrets.env 便捷读写（2.2.1.5 任务 3：F2 配置面板 API Key 配置节）。
//
// 存储位置：$AIRY_HOME/config/secrets.env（默认 ~/.airymaxrt/config/secrets.env），
// 与 wizard.rs 的写回路径一致（llm_d 热加载，无需重启）。
// 安全约定：读入仅用于脱敏展示（后 4 位）；写入后文件权限收紧为 600（Unix）。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// secrets.env 文件路径：$AIRY_HOME/config/secrets.env（默认 ~/.airymaxrt/config/）。
pub fn secrets_path() -> PathBuf {
    crate::paths::airy_home_path(&["config", "secrets.env"])
}

/// 已知模型 API Key 变量名（v2 表格形式：统一 MODEL_N_API_KEY，与 model.yaml
/// models 表 / wizard 对齐）。展示顺序即 F2 配置面板列表顺序。
pub const KNOWN_KEYS: [(&str, &str); 3] = [
    ("MODEL_1_API_KEY", "模型 1（默认）"),
    ("MODEL_2_API_KEY", "模型 2"),
    ("MODEL_3_API_KEY", "模型 3"),
];

/// 读取 secrets.env 全部键值对（KEY=VALUE；跳过空行与 # 注释；容忍 export 前缀与引号）。
pub fn read_all() -> Vec<(String, String)> {
    read_all_at(&secrets_path())
}

fn read_all_at(path: &Path) -> Vec<(String, String)> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = l.strip_prefix("export ").unwrap_or(l).trim();
        let Some(eq) = l.find('=') else { continue };
        let k = l[..eq].trim();
        let v = l[eq + 1..].trim().trim_matches('"').trim_matches('\'');
        if !k.is_empty() {
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

/// 值脱敏：仅保留后 4 位（如 `sk-…abcd`）；空值返回空；短值（≤4）返回 `***`。
pub fn mask(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        return "***".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("…{}", tail)
}

/// 设置 / 更新一个 API Key：写回 secrets.env（文件权限收紧为 600，Unix）。
/// 已有同名变量原位替换值（保留注释与其他行）；无则追加到文件末尾。
pub fn set_key(key: &str, value: &str) -> std::io::Result<()> {
    set_key_at(&secrets_path(), key, value)
}

fn set_key_at(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty key name",
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let original = fs::read_to_string(path).unwrap_or_default();
    let marker = format!("{}=", key);
    let line = format!("{}{}", marker, value);
    let mut lines: Vec<String> = original.lines().map(|s| s.to_string()).collect();
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
        // 追加到文件末尾（保留注释与已有内容）
        if !original.ends_with('\n') && !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(line);
    }
    let content = lines.join("\n") + "\n";
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    // Unix：文件权限收紧为 600（仅属主可读写；密钥文件不随仓库/组读扩散）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 校验 Key 名合法性：非空且仅含大写字母、数字与下划线（如 DEEPSEEK_API_KEY）。
pub fn valid_key_name(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.env");
        (dir, path)
    }

    #[test]
    fn set_key_creates_file_with_0600() {
        let (_d, path) = temp_file();
        set_key_at(&path, "DEEPSEEK_API_KEY", "sk-abc12345").expect("set");
        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("DEEPSEEK_API_KEY=sk-abc12345"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secrets.env 权限应收紧为 600");
        }
    }

    #[test]
    fn set_key_replaces_in_place_keeps_others() {
        let (_d, path) = temp_file();
        set_key_at(&path, "OPENAI_API_KEY", "old-value").expect("set1");
        set_key_at(&path, "DEEPSEEK_API_KEY", "deep-value").expect("set2");
        set_key_at(&path, "OPENAI_API_KEY", "new-value").expect("set3");
        let pairs = read_all_at(&path);
        let openai = pairs.iter().find(|(k, _)| k == "OPENAI_API_KEY").expect("openai");
        assert_eq!(openai.1, "new-value", "同名变量原位替换");
        assert_eq!(pairs.len(), 2, "不产生重复行");
        assert!(pairs.iter().any(|(k, v)| k == "DEEPSEEK_API_KEY" && v == "deep-value"));
    }

    #[test]
    fn read_skips_comments_and_export_prefix() {
        let (_d, path) = temp_file();
        fs::write(
            &path,
            "# 注释行\n\n  export ANTHROPIC_API_KEY=\"sk-an-1111\"\nGLM_API_KEY='sk-glm-2222'\n",
        )
        .expect("write");
        let pairs = read_all_at(&path);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "ANTHROPIC_API_KEY");
        assert_eq!(pairs[0].1, "sk-an-1111", "容忍 export 前缀与双引号");
        assert_eq!(pairs[1].0, "GLM_API_KEY");
        assert_eq!(pairs[1].1, "sk-glm-2222", "容忍单引号");
    }

    #[test]
    fn mask_keeps_last_four() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("sk-abc12345"), "…2345");
        assert_eq!(mask("abcd"), "***", "短值不泄露");
        assert_eq!(mask("a"), "***");
    }

    #[test]
    fn key_name_validation() {
        assert!(valid_key_name("DEEPSEEK_API_KEY"));
        assert!(valid_key_name("K1_ABC"));
        assert!(!valid_key_name(""));
        assert!(!valid_key_name("deepseek_api_key"), "小写非法");
        assert!(!valid_key_name("A B"), "空格非法");
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let (_d, path) = temp_file();
        let pairs = read_all_at(&path);
        assert!(pairs.is_empty(), "文件不存在时返回空列表而非报错");
    }
}
