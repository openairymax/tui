// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 代码语言探测（0.1.21 X4-04a）：把围栏标注与文件扩展名归一到受支持
// 语言的规范 id。本模块只做标记判定，不触碰分词与样式——词法接入见
// X4-05，着色映射见 X4-06。
//
// 判定纪律：只认白名单内的 8 种语言及其别名；未知标记返回 None，由
// 调用方降级为纯文本。不做「按内容猜语言」：猜错的着色比不着色更难读，
// 还会让同一段文本在两次渲染之间换色。

/// 受支持语言：`id` 为规范名（徽章展示用），`token` 为 syntect 查找令牌。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lang {
    pub(crate) id: &'static str,
    pub(crate) token: &'static str,
}

/// 白名单（8 种）：`token` 取 syntect 默认语法包内的名称或扩展名。
const LANGS: &[Lang] = &[
    Lang {
        id: "rust",
        token: "rust",
    },
    Lang {
        id: "python",
        token: "python",
    },
    Lang {
        id: "javascript",
        token: "javascript",
    },
    Lang {
        id: "typescript",
        token: "typescript",
    },
    Lang {
        id: "go",
        token: "go",
    },
    Lang {
        id: "c",
        token: "c",
    },
    Lang {
        id: "shell",
        token: "sh",
    },
    Lang {
        id: "json",
        token: "json",
    },
];

/// 别名与扩展名 → 规范 id。同一语言的不同写法必须收敛到同一 id，
/// 否则徽章与缓存键会随作者习惯漂移。
const ALIASES: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("rust", "rust"),
    ("py", "python"),
    ("py2", "python"),
    ("py3", "python"),
    ("python", "python"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("javascript", "javascript"),
    ("node", "javascript"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("typescript", "typescript"),
    ("go", "go"),
    ("golang", "go"),
    ("c", "c"),
    ("h", "c"),
    ("sh", "shell"),
    ("bash", "shell"),
    ("zsh", "shell"),
    ("ksh", "shell"),
    ("shell", "shell"),
    ("console", "shell"),
    ("json", "json"),
];

/// 探测语言：接受围栏标注（`rs`、`bash`）或文件名/扩展名（`main.go`、`.py`）。
///
/// 文件名取最后一个点之后的部分当扩展名；围栏标注可能带选项（```rust title=x），
/// 只取第一个空白分隔的词。大小写不敏感。未知标记返回 `None`。
pub(crate) fn detect(tag: &str) -> Option<Lang> {
    let head = tag.split_whitespace().next()?;
    let low = head.to_ascii_lowercase();
    let key = match low.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => low.as_str(),
    };
    let id = ALIASES.iter().find(|(a, _)| *a == key).map(|(_, id)| *id)?;
    LANGS.iter().find(|l| l.id == id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases_and_extensions() {
        assert_eq!(detect("rs").map(|l| l.id), Some("rust"));
        assert_eq!(detect(".py").map(|l| l.id), Some("python"));
        assert_eq!(detect("src/main.go").map(|l| l.id), Some("go"));
        assert_eq!(detect("  BASH ").map(|l| l.id), Some("shell"));
        assert_eq!(detect("rust title=demo").map(|l| l.id), Some("rust"));
    }

    #[test]
    fn unknown_and_empty_are_none() {
        assert!(detect("").is_none());
        assert!(detect("   ").is_none());
        assert!(detect("no-such-lang-xyz").is_none());
        assert!(detect("main.unknown").is_none());
        assert!(detect(".").is_none());
    }
}
