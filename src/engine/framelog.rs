// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧日志落盘（0.1.21 X8-06b，§23.9 #10 帧预算口径）。
//
// `AIRY_TUI_FRAME_LOG=1` 时，每帧写一行 JSONL 到 AIRY_HOME/logs：
// `{"seq":1,"compose":120,"diff":null,...,"total":420}`。键名即六段名，
// 未采样段为 `null`（不是 0），整帧为已采样段之和。行与行之间没有会话头、
// 没有注释：文件按运行截断，每行就是一条独立样本，读侧逐行校验即可。
//
// 存在的理由：卡顿现场的帧样本必须在进程被强杀后仍能取回。因此每次记录后
// 不立即落盘（每 64 帧刷一次，崩溃最多丢约 1 秒），但也不做长期遥测——单次
// 运行上限 65,536 行，触顶即停，避免日志自己变成容量问题。
//
// 写失败（磁盘满、权限）只警告一次并停用本进程记录：诊断通道不得影响帧
// 节拍，更不得把失败升级成渲染错误。
//
// 开关解析、行编码、文件打开都是纯函数/单点副作用，测试可在临时 AIRY_HOME
// 下逐行校验并离线复算分位，与在线聚合对齐。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use super::frame::{Costs, Stage};

/// 帧日志文件名（置于 AIRY_HOME/logs）。
const FILE_NAME: &str = "agentrt-tui-frames.jsonl";

/// 每 64 帧刷一次盘：崩溃最多丢约 1 秒样本，又不让每帧都等磁盘。
const FLUSH_EVERY: u32 = 64;

/// 单次运行上限：30fps 下约 36 分钟、约 8MiB。帧日志是卡顿现场取证，
/// 不是长期遥测，触顶后停止写入。
const MAX_LINES: u64 = 65_536;

/// 帧日志写入器。
pub(crate) struct FrameLog {
    out: BufWriter<File>,
    seq: u64,
    pending: u32,
    broken: bool,
}

impl FrameLog {
    /// 按环境开关构造；未开启、路径不可用或 IO 失败时返回 `None`（fail-closed）。
    pub(crate) fn from_env() -> Option<Self> {
        let value = std::env::var("AIRY_TUI_FRAME_LOG").ok();
        if !enabled(value.as_deref()) {
            return None;
        }
        // 与文件日志同一条底线：HOME/AIRY_HOME 都缺失时不落到相对路径，
        // 否则样本会写进当前工作目录。
        if std::env::var_os("AIRY_HOME").is_none() && std::env::var_os("HOME").is_none() {
            log::warn!("帧日志未开启：AIRY_HOME/HOME 均为空");
            return None;
        }
        let path = crate::paths::airy_home_path(&["logs", FILE_NAME]);
        match open(&path) {
            Ok(file) => Some(Self {
                out: BufWriter::new(file),
                seq: 0,
                pending: 0,
                broken: false,
            }),
            Err(e) => {
                log::warn!("帧日志未开启：{}（{e}）", path.display());
                None
            }
        }
    }

    /// 记录一帧：编码一行并追写，按节拍刷盘；失败即停用。
    pub(crate) fn record(&mut self, costs: &Costs) {
        if self.broken || self.seq >= MAX_LINES {
            return;
        }
        self.seq = self.seq.saturating_add(1);
        if let Err(e) = self.write_line(&line(self.seq, costs)) {
            log::warn!("帧日志写入失败，本进程停止记录：{e}");
            self.broken = true;
            return;
        }
        self.pending += 1;
        if self.pending == FLUSH_EVERY {
            self.pending = 0;
            if let Err(e) = self.out.flush() {
                log::warn!("帧日志刷盘失败，本进程停止记录：{e}");
                self.broken = true;
            }
        }
    }

    fn write_line(&mut self, text: &str) -> std::io::Result<()> {
        self.out.write_all(text.as_bytes())?;
        self.out.write_all(b"\n")
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// 开关判定：1/true/on（大小写与首尾空白不敏感），其余一律关闭。
fn enabled(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on"
        )
    })
}

/// 建目录并打开日志文件；按运行截断，保证每次会话的 `seq` 从 1 起连续。
fn open(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

/// 编码一行样本：键序固定为 seq → 六段 → total，值为整数或 null。
fn line(seq: u64, costs: &Costs) -> String {
    let mut text = String::with_capacity(160);
    text.push_str("{\"seq\":");
    text.push_str(&seq.to_string());
    for stage in Stage::ALL {
        text.push_str(",\"");
        text.push_str(stage.name());
        text.push_str("\":");
        match costs.get(stage) {
            Some(us) => text.push_str(&us.to_string()),
            None => text.push_str("null"),
        }
    }
    text.push_str(",\"total\":");
    text.push_str(&costs.total().to_string());
    text.push('}');
    text
}

#[cfg(test)]
mod tests {
    use super::{enabled, line, FrameLog, FILE_NAME, FLUSH_EVERY};
    use crate::engine::frame::{Costs, Stage};
    use crate::engine::stats::aggregate;
    use crate::test_env::Home;

    /// 单帧：compose 与 present 有值，其余段未采样。
    fn frame(compose: u64, present: u64) -> Costs {
        let mut costs = Costs::new();
        costs.set(Stage::Compose, compose);
        costs.set(Stage::Present, present);
        costs
    }

    /// 开关只认肯定值，空串与其它字样一律关闭。
    #[test]
    fn switch_accepts_only_affirmative_values() {
        for on in ["1", "true", "TRUE", " on "] {
            assert!(enabled(Some(on)), "{on:?} 应开启");
        }
        for off in ["", "0", "false", "off", "yes"] {
            assert!(!enabled(Some(off)), "{off:?} 应关闭");
        }
        assert!(!enabled(None));
    }

    /// 行的键集合与取值：seq、六段（含 null）、total，且 total 只累已采样段。
    #[test]
    fn line_carries_all_stages_and_total() {
        let text = line(7, &frame(120, 300));
        assert!(!text.contains('\n'), "一行不得含换行");
        let value: serde_json::Value = serde_json::from_str(&text).expect("行必须是合法 JSON");
        let object = value.as_object().expect("行必须是对象");
        assert_eq!(object.len(), 8, "seq + 六段 + total");
        assert_eq!(object["seq"], 7);
        assert_eq!(object["compose"], 120);
        assert_eq!(object["present"], 300);
        assert_eq!(object["total"], 420);
        for stage in [Stage::Diff, Stage::Encode, Stage::Write, Stage::Flush] {
            assert!(object[stage.name()].is_null(), "{} 应为 null", stage.name());
        }
    }

    /// 全空样本仍出一行：六段全 null、total 为 0，便于发现采样缺口。
    #[test]
    fn line_of_unsampled_frame_is_all_null() {
        let value: serde_json::Value =
            serde_json::from_str(&line(1, &Costs::new())).expect("合法 JSON");
        for stage in Stage::ALL {
            assert!(value[stage.name()].is_null());
        }
        assert_eq!(value["total"], 0);
    }

    /// 逐行校验：每行恰好一个对象，seq 连续，整数值非负。
    #[test]
    fn every_line_is_one_well_formed_object() {
        for seq in 1..=64u64 {
            let text = line(seq, &frame(seq, seq * 2));
            let value: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
            assert_eq!(value["seq"], seq);
            for stage in Stage::ALL {
                let cell = &value[stage.name()];
                if !cell.is_null() {
                    assert!(cell.as_u64().is_some(), "{} 必须为非负整数", stage.name());
                }
            }
        }
    }

    /// 真实落盘：临时 AIRY_HOME 下记录一批帧，读回后逐行校验，并用文件内容
    /// 离线复算分位，结论须与在线聚合逐字段一致。
    #[test]
    fn written_samples_recompute_like_online_aggregation() {
        let home = Home::new("framelog");
        std::env::set_var("AIRY_TUI_FRAME_LOG", "1");
        let samples: Vec<Costs> = (1..=200).map(|i| frame(i, i * 3)).collect();
        {
            let mut log = FrameLog::from_env().expect("开关已开启且路径可建");
            for costs in &samples {
                log.record(costs);
            }
            assert!(samples.len() > FLUSH_EVERY as usize, "用例须跨过一次刷盘");
        }
        std::env::remove_var("AIRY_TUI_FRAME_LOG");

        let path = home.path().join("logs").join(FILE_NAME);
        let body = std::fs::read_to_string(&path).expect("日志文件已落盘");
        let mut parsed: Vec<Costs> = Vec::new();
        for (i, text) in body.lines().enumerate() {
            let value: serde_json::Value =
                serde_json::from_str(text).expect("每行必须是合法 JSON");
            assert_eq!(value["seq"], i as u64 + 1, "seq 须从 1 起连续");
            assert_eq!(value["total"], (i as u64 + 1) * 4, "整帧等于两段之和");
            parsed.push(samples[i]);
        }
        assert_eq!(parsed.len(), samples.len(), "帧数须与写入一致");

        let online = aggregate(samples.iter().copied());
        let offline = aggregate(parsed);
        assert_eq!(online, offline, "离线复算须与在线聚合一致");
    }
}
