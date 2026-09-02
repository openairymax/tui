// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// AIRY_HOME 路径解析单一来源（0.1.8 硬编码审计）。
//
// 此前 secrets / app / skills / memory / models_cfg / main / wizard 各自
// 实现「读 AIRY_HOME → 回退 $HOME/.airymaxrt」的解析，目录名字面量散落
// 7 处。一旦目录名变更或某处回退逻辑写偏，写入与读取落到不同目录（隐蔽
// 的数据丢失）。现收敛至此：`DEFAULT_DIR_NAME` 为目录名唯一权威源，
// `airy_home()` / `airy_home_path()` 为解析唯一入口。

use std::path::PathBuf;

/// agentrt 安装目录名：`AIRY_HOME` 未设置时 `$HOME` 下的默认目录。
pub const DEFAULT_DIR_NAME: &str = ".airymaxrt";

/// 解析 AIRY_HOME 根目录（三级回退）：
/// `$AIRY_HOME` → `$HOME/.airymaxrt` → 相对 `.airymaxrt`。
///
/// 空字符串视同未设置（与 agentrt-env.sh 条件赋值语义一致），避免
/// `AIRY_HOME=""` 使读写落到 CWD 造成数据分散。
pub fn airy_home() -> PathBuf {
    if let Ok(h) = std::env::var("AIRY_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join(DEFAULT_DIR_NAME);
        }
    }
    PathBuf::from(DEFAULT_DIR_NAME)
}

/// `airy_home()` 下的子路径（如 `&["config", "secrets.env"]`）。
pub fn airy_home_path(sub: &[&str]) -> PathBuf {
    let mut p = airy_home();
    for s in sub {
        p.push(s);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock_env;

    #[test]
    fn airy_home_prefers_env_var() {
        let _g = lock_env();
        let saved = std::env::var("AIRY_HOME").ok();
        std::env::set_var("AIRY_HOME", "/tmp/airy-test-home");
        assert_eq!(airy_home(), PathBuf::from("/tmp/airy-test-home"));
        assert_eq!(
            airy_home_path(&["config", "secrets.env"]),
            PathBuf::from("/tmp/airy-test-home/config/secrets.env")
        );
        match saved {
            Some(v) => std::env::set_var("AIRY_HOME", v),
            None => std::env::remove_var("AIRY_HOME"),
        }
    }

    #[test]
    fn airy_home_falls_back_to_home_with_dir_name() {
        let _g = lock_env();
        let saved_home = std::env::var("AIRY_HOME").ok();
        let saved_user_home = std::env::var("HOME").ok();
        std::env::remove_var("AIRY_HOME");
        std::env::set_var("HOME", "/tmp/airy-test");
        assert_eq!(airy_home(), PathBuf::from("/tmp/airy-test/.airymaxrt"));
        match saved_home {
            Some(v) => std::env::set_var("AIRY_HOME", v),
            None => std::env::remove_var("AIRY_HOME"),
        }
        match saved_user_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn default_dir_name_is_dot_airymaxrt() {
        assert_eq!(DEFAULT_DIR_NAME, ".airymaxrt");
    }

    #[test]
    fn airy_home_falls_back_to_relative_when_no_env() {
        let _g = lock_env();
        let saved_home = std::env::var("AIRY_HOME").ok();
        let saved_user_home = std::env::var("HOME").ok();
        std::env::remove_var("AIRY_HOME");
        std::env::remove_var("HOME");
        assert_eq!(airy_home(), PathBuf::from(DEFAULT_DIR_NAME));
        assert_eq!(
            airy_home_path(&["bin", "gateway_d"]),
            PathBuf::from(DEFAULT_DIR_NAME).join("bin").join("gateway_d")
        );
        match saved_home {
            Some(v) => std::env::set_var("AIRY_HOME", v),
            None => std::env::remove_var("AIRY_HOME"),
        }
        match saved_user_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn empty_airy_home_falls_through_to_home() {
        let _g = lock_env();
        let saved_home = std::env::var("AIRY_HOME").ok();
        let saved_user_home = std::env::var("HOME").ok();
        std::env::set_var("AIRY_HOME", "");
        std::env::set_var("HOME", "/tmp/airy-empty-home");
        assert_eq!(airy_home(), PathBuf::from("/tmp/airy-empty-home/.airymaxrt"));
        match saved_home {
            Some(v) => std::env::set_var("AIRY_HOME", v),
            None => std::env::remove_var("AIRY_HOME"),
        }
        match saved_user_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn empty_home_falls_through_to_relative() {
        let _g = lock_env();
        let saved_home = std::env::var("AIRY_HOME").ok();
        let saved_user_home = std::env::var("HOME").ok();
        std::env::remove_var("AIRY_HOME");
        std::env::set_var("HOME", "");
        assert_eq!(airy_home(), PathBuf::from(DEFAULT_DIR_NAME));
        match saved_home {
            Some(v) => std::env::set_var("AIRY_HOME", v),
            None => std::env::remove_var("AIRY_HOME"),
        }
        match saved_user_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// H10 防复发 grep 门禁：目录名 `.airymaxrt` 字面量只允许出现在
    /// paths.rs（DEFAULT_DIR_NAME 单一权威源）与注释/文档中。其余源码若
    /// 出现裸字面量说明又绕开了 airy_home() 单一入口，写入与读取将可能
    /// 分叉到不同目录。
    #[test]
    fn no_airymaxrt_literal_in_code_outside_paths() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut rs_files = Vec::new();
        collect_rs_files(&root, &mut rs_files);
        let mut offenders = Vec::new();
        for p in &rs_files {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if name == "paths.rs" {
                continue;
            }
            let content = std::fs::read_to_string(p).unwrap();
            for (i, line) in content.lines().enumerate() {
                if !line.contains(".airymaxrt") {
                    continue;
                }
                let t = line.trim_start();
                let in_comment = t.starts_with("//") || t.starts_with('*') || t.starts_with("/*");
                if !in_comment {
                    let rel = p.strip_prefix(&root).unwrap().display();
                    offenders.push(format!("{}:{}: {}", rel, i + 1, t));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "AIRY_HOME 目录名 `.airymaxrt` 代码字面量禁止出现在 paths.rs 之外（注释/文档除外）。\n\
             请改用 paths::DEFAULT_DIR_NAME / airy_home() 单一权威源。命中：\n{}",
            offenders.join("\n")
        );
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rs_files(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
}
