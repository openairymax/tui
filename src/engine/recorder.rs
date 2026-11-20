// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件录制（0.1.21 X1-31a，§19.1 D-15 帧模型）。
//
// 事件流的每一帧形如 `{"schema":1,"seq":N,"ts":...,"type":...,"payload":...}`。
// 开启 `AIRY_TUI_EVENT_RECORD=1` 时，经本模块的帧原样追加到
// `AIRY_HOME/.airy-events.jsonl`：录制是纯旁路，不改变、不重排、不修补事件
// 流——被拒收的帧仍照常交给下游，返回值只说明「是否留存」。
//
// 可回放的前提是文件自洽：只接受单行 JSON 对象，且 `seq` 为非负整数并严格
// 递增。重复、回退、坏行、超长行（>1 MiB，对齐 X1-03 读取侧上界）一律拒收，
// 并计入 rejected。回放侧用同一判据复验整份文件：损坏的录制在读取时即报错
// 并指出行号，而不是把残帧喂给 golden 比较。
//
// 本模块只做留存与复验，不解析 payload、不关联业务语义；样本的 schema 头与
// 生成脚本归 X1-31b。消费者（X1-01a 子进程事件流）接线后删除此注记。

#![cfg_attr(not(test), allow(dead_code))]

use std::io;
use std::path::Path;

use serde_json::Value;

use super::flag;
use super::logfile::{LineLog, MAX_LINES};

/// 录制文件名（AIRY_HOME 下；点前缀表示内部诊断产物）。
pub(crate) const FILE_NAME: &str = ".airy-events.jsonl";

/// 录制开关。
pub(crate) const SWITCH: &str = "AIRY_TUI_EVENT_RECORD";

/// 单帧字节上限：与 D-15 编码侧及 X1-03 读取侧同一上界。
pub(crate) const MAX_FRAME_BYTES: usize = 1 << 20;

/// 事件录制器。
pub(crate) struct Recorder {
    sink: LineLog,
    last: Option<u64>,
    rejected: u64,
}

impl Recorder {
    /// 按环境开关构造；未开启或路径不可用时返回 `None`（fail-closed）。
    pub(crate) fn from_env() -> Option<Self> {
        if !flag::on(SWITCH) {
            return None;
        }
        if std::env::var_os("AIRY_HOME").is_none() && std::env::var_os("HOME").is_none() {
            log::warn!("事件录制未开启：AIRY_HOME/HOME 均为空");
            return None;
        }
        let path = crate::paths::airy_home_path(&[FILE_NAME]);
        match LineLog::open(&path, MAX_LINES) {
            Ok(sink) => Some(Self {
                sink,
                last: None,
                rejected: 0,
            }),
            Err(e) => {
                log::warn!("事件录制未开启：{}（{e}）", path.display());
                None
            }
        }
    }

    /// 留存一帧；返回是否写入。拒收不从事件流里摘除该帧。
    pub(crate) fn record(&mut self, text: &str) -> bool {
        let Some(seq) = scan(text) else {
            self.rejected = self.rejected.saturating_add(1);
            return false;
        };
        if self.last.is_some_and(|prev| seq <= prev) {
            self.rejected = self.rejected.saturating_add(1);
            return false;
        }
        if !self.sink.write(text) {
            return false;
        }
        self.last = Some(seq);
        true
    }

    /// 最近留存的帧序号。
    pub(crate) fn last_seq(&self) -> Option<u64> {
        self.last
    }

    /// 被拒收的帧数（坏行、超长、重复或回退序号）。
    pub(crate) fn rejected(&self) -> u64 {
        self.rejected
    }
}

/// 提取一帧的 `seq`：单行、JSON 对象、非负整数，缺一不可。
pub(crate) fn scan(text: &str) -> Option<u64> {
    if text.len() > MAX_FRAME_BYTES || text.contains('\n') {
        return None;
    }
    let value: Value = serde_json::from_str(text).ok()?;
    value.as_object()?.get("seq")?.as_u64()
}

/// 读取整份录制并复验；任一行不合格即报错并指出行号（1 起）。
pub(crate) fn replay(path: &Path) -> io::Result<Vec<String>> {
    let body = std::fs::read_to_string(path)?;
    let mut frames = Vec::new();
    let mut last = None;
    for (i, text) in body.lines().enumerate() {
        let seq = scan(text)
            .filter(|seq| last.is_none_or(|prev| *seq > prev))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, format!("第 {} 行帧不合规", i + 1))
            })?;
        last = Some(seq);
        frames.push(text.to_string());
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::{replay, scan, Recorder, FILE_NAME, MAX_FRAME_BYTES, SWITCH};
    use crate::test_env::Home;

    fn frame(seq: u64) -> String {
        format!(
            "{{\"schema\":1,\"seq\":{seq},\"ts\":1700000000,\
             \"type\":\"tool_result\",\"payload\":{{\"ok\":true}}}}"
        )
    }

    /// 只认单行 JSON 对象里的非负整数 seq。
    #[test]
    fn scan_requires_object_with_integer_seq() {
        assert_eq!(scan(&frame(7)), Some(7));
        assert_eq!(scan(r#"{"seq":0}"#), Some(0));
        for bad in [
            "",
            "   ",
            "[1,2]",
            r#"{"seq":"7"}"#,
            r#"{"seq":-1}"#,
            r#"{"seq":1.5}"#,
            r#"{"type":"x"}"#,
            "{\"seq\":1}\n{\"seq\":2}",
        ] {
            assert_eq!(scan(bad), None, "{bad:?} 不应通过");
        }
        assert_eq!(scan(&"x".repeat(MAX_FRAME_BYTES + 1)), None, "超长行须拒收");
    }

    /// 录制只留序号严格递增的帧；坏行、重复、回退均计入 rejected 且不落盘。
    #[test]
    fn record_keeps_only_monotonic_frames() {
        let home = Home::new("recorder");
        std::env::set_var(SWITCH, "1");
        let mut rec = Recorder::from_env().expect("开关已开启且路径可建");
        assert!(rec.record(&frame(1)));
        assert!(rec.record(&frame(2)));
        assert!(!rec.record(&frame(2)), "重复 seq 须拒收");
        assert!(!rec.record(&frame(1)), "回退 seq 须拒收");
        assert!(!rec.record("not json"), "坏行须拒收");
        assert_eq!(rec.rejected(), 3);
        assert_eq!(rec.last_seq(), Some(2));
        drop(rec);
        std::env::remove_var(SWITCH);

        let frames = replay(&home.path().join(FILE_NAME)).expect("录制文件自洽");
        assert_eq!(frames, vec![frame(1), frame(2)], "回放须逐字节还原");
    }

    /// 损坏的录制在回放时即报错，不把残帧交给下游。
    #[test]
    fn replay_rejects_corrupt_capture() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("cap.jsonl");

        std::fs::write(&path, format!("{}\nnot json\n", frame(1))).expect("预置坏文件");
        let err = replay(&path).expect_err("坏行须报错");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("第 2 行"), "须指出坏行位置");

        std::fs::write(&path, format!("{}\n{}\n", frame(2), frame(1))).expect("预置乱序文件");
        assert!(replay(&path).is_err(), "回退 seq 的文件不可回放");

        assert!(replay(&dir.path().join("missing.jsonl")).is_err(), "缺失文件须报错");
    }

    /// 开关未开启时不构造录制器，也不触碰磁盘。
    #[test]
    fn recorder_stays_off_without_switch() {
        let _home = Home::new("recorder-off");
        std::env::remove_var(SWITCH);
        assert!(Recorder::from_env().is_none());
    }
}
