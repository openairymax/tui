// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 后端日志读取器（0.1.20 批次五）：/dlogs 面板的数据通路。
//
// daemon 日志是文件落盘事实源（AIRY_LOG_DIR 或 $AIRY_HOME/data/agentrt/logs，
// 与 commons/platform 日志目录解析一致）。本层只做真实文件尾读与行解析；
// 目录缺失或暂无日志文件返回空表，由面板显式说明路径，不伪造内容。

use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::LogEntry;
use crate::budget::{BACKEND_TAIL_BYTES, BACKEND_TAIL_LINES};

/// 解析 daemon 日志目录（与 C 侧口径一致：AIRY_LOG_DIR 环境变量优先，
/// 否则 $AIRY_HOME/data/agentrt/logs）。
pub fn log_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("AIRY_LOG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    crate::paths::airy_home()
        .join("data")
        .join("agentrt")
        .join("logs")
}

/// 单文件尾读：从文件末尾向前取一个字节窗，返回窗口内至多 TAIL 行。
/// 首行若被窗口截断（start>0）则丢弃，避免半行误导。
async fn tail_lines(path: &Path) -> Vec<String> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let start = meta.len().saturating_sub(BACKEND_TAIL_BYTES);
    let mut f = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return Vec::new();
    }
    let mut raw = Vec::new();
    if f.read_to_end(&mut raw).await.is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&raw);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(BACKEND_TAIL_LINES);
    lines.into_iter().skip(skip).collect()
}

/// 解析一行 daemon 日志：`TS [LEVEL] daemon: message`（logger 固定格式）。
/// 非该形态的行整行入 message、不拆字段，daemon 归文件名——解析失败
/// 不丢行，诚实呈现原文。
pub fn parse_line(line: &str, daemon: &str) -> LogEntry {
    let b = line.as_bytes();
    // 时间戳定长 23 字节（如 2026-10-30T12:54:12.340），随后是 " [LEVEL]"
    if b.len() > 25 && b[23] == b' ' && b[24] == b'[' {
        if let Some(close) = line[24..].find(']') {
            // close 为相对 line[24..] 的下标，']' 绝对位置 = 24 + close。
            let level = &line[25..24 + close];
            let rest = line[25 + close + 1..].trim_start();
            if let Some(colon) = rest.find(':') {
                let d = &rest[..colon];
                if !d.is_empty() && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    let msg = rest[colon + 1..].trim_start();
                    return LogEntry {
                        timestamp: line[..23].to_string(),
                        level: level.to_string(),
                        message: msg.to_string(),
                        daemon: Some(d.to_string()),
                    };
                }
            }
        }
    }
    LogEntry {
        timestamp: String::new(),
        level: String::new(),
        message: line.to_string(),
        daemon: Some(daemon.to_string()),
    }
}

/// 目录尾读汇总：枚举顶层 *.log 按文件名排序，逐文件取尾部拼接。
/// 文件内保持原有顺序（越靠后越新），文件间不跨文件混排。
pub async fn tail_all(dir: &Path) -> Vec<LogEntry> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut files: Vec<PathBuf> = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("log") {
            files.push(p);
        }
    }
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let daemon = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        for l in tail_lines(&f).await {
            out.push(parse_line(&l, &daemon));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准行四字段齐备：时间戳、级别、来源 daemon、消息。
    #[test]
    fn parse_line_splits_standard_format() {
        let e = parse_line(
            "2026-10-30T12:54:12.340 [INFO] supervisor_d: spawn cupolas worker",
            "supervisor_d",
        );
        assert_eq!(e.timestamp, "2026-10-30T12:54:12.340");
        assert_eq!(e.level, "INFO");
        assert_eq!(e.daemon.as_deref(), Some("supervisor_d"));
        assert_eq!(e.message, "spawn cupolas worker");
    }

    /// 消息内再含冒号不误切：只按首个「daemon: 」边界拆分。
    #[test]
    fn parse_line_keeps_colons_in_message() {
        let e = parse_line(
            "2026-10-30T12:54:12.340 [WARN] llm_d: CHAIN-SEAT: ok in 12ms",
            "llm_d",
        );
        assert_eq!(e.daemon.as_deref(), Some("llm_d"));
        assert_eq!(e.message, "CHAIN-SEAT: ok in 12ms");
    }

    /// 非标准行整行保留在 message，不丢行也不臆造字段。
    #[test]
    fn parse_line_fallback_keeps_raw_text() {
        let e = parse_line("  multi-line continuation", "tool_d");
        assert_eq!(e.timestamp, "");
        assert_eq!(e.level, "");
        assert_eq!(e.daemon.as_deref(), Some("tool_d"));
        assert_eq!(e.message, "  multi-line continuation");
    }
}
