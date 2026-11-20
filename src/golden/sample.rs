// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// golden 事件样本校验（0.1.21 X1-31b）。
//
// `golden/samples` 下 20 份录制由 `scripts/gen-golden-samples.py` 生成，格式
// 即 D-15 帧（`schema/seq/ts/type/payload`），与 `AIRY_TUI_EVENT_RECORD` 的
// 产物同构：样本可由脚本造，也可由现场录制替换，下游无需区分来源。
//
// 本模块只守「样本本身不是坏数据」：能过 recorder::replay、序号从 1 连续、
// schema 固定为 1、时间不回退、类型词法合规、以终态帧收尾、透传帧的信封与
// 帧头一致。投影与快照断言归 X1-32a——若样本先坏，那边失败会归错因。
//
// 类型覆盖是样本集的承诺而非偶然：20 份场景必须覆盖 run-stream 的全部对外
// 类型，新增类型时这里先失败，提醒补场景而不是让投影悄悄失去样本。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::engine::recorder;

/// 样本数：X1-31b 的验收口径。
const COUNT: usize = 20;

/// 终态帧：成功 run 以 run_end 收尾；失败 run 之后还有一帧 error
/// （服务端可见时序为 message → run_end → error）。
const TERMINAL: [&str; 2] = ["run_end", "error"];

/// 样本集必须覆盖的帧类型。
const COVERED: [&str; 17] = [
    "run_start",
    "task_state",
    "token_delta",
    "message",
    "run_end",
    "tool_start",
    "tool_delta",
    "tool_end",
    "error",
    "plan",
    "step",
    "catalog",
    "plan_unavailable",
    "heartbeat",
    "notice",
    "snapshot",
    "control_request",
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("golden")
        .join("samples")
}

fn files() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir())
        .expect("golden/samples 目录存在")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    paths
}

/// 一帧的（类型，内容）二元组；内容剔除信封里逐次运行都不同的标识与时间，
/// 使同一场景换一段录制仍判为同一场景，而换文本必然判为不同。
fn shape(frame: &Value) -> (String, String) {
    let payload = &frame["payload"];
    let body = payload.get("data").unwrap_or(payload);
    let kind = frame["type"].as_str().unwrap_or("").to_string();
    (kind, serde_json::to_string(body).unwrap_or_default())
}

/// 校验一份样本；返回逐帧的（类型，内容）。
fn check(path: &Path) -> Vec<(String, String)> {
    let frames = recorder::replay(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        frames.len() >= 2,
        "{}: 至少一帧起点与一帧终态",
        path.display()
    );
    let mut last_ts = 0u64;
    let mut parts = Vec::with_capacity(frames.len());
    for (i, text) in frames.iter().enumerate() {
        let frame: Value = serde_json::from_str(text).expect("replay 已保证是 JSON");
        assert_eq!(
            frame["seq"],
            i as u64 + 1,
            "{}: seq 须从 1 连续",
            path.display()
        );
        assert_eq!(frame["schema"], 1, "{}: schema 固定为 1", path.display());
        let ts = frame["ts"].as_u64().expect("ts 为非负整数");
        assert!(ts >= last_ts, "{}: 时间不得回退", path.display());
        last_ts = ts;

        let kind = frame["type"].as_str().expect("type 为字符串");
        assert!(
            kind.chars().all(legal),
            "{}: 类型 {:?} 不合词法",
            path.display(),
            kind
        );
        let payload = frame["payload"].as_object().expect("payload 为对象");
        if payload.contains_key("v") {
            assert_eq!(payload["v"], 1, "{}: 信封版本固定为 1", path.display());
            assert_eq!(
                payload["type"],
                kind,
                "{}: 信封类型须等于帧头",
                path.display()
            );
            assert!(
                payload["data"].is_object(),
                "{}: data 为对象",
                path.display()
            );
        }
        parts.push(shape(&frame));
    }
    let last = parts.last().expect("非空").0.clone();
    assert!(
        TERMINAL.contains(&last.as_str()),
        "{}: 末帧须为终态",
        path.display()
    );
    parts
}

/// 类型词法：小写字母、数字、下划线与点（与 cli_ev 的帧头校验一致）。
fn legal(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'
}

#[test]
fn ships_twenty_replayable_samples() {
    let paths = files();
    assert_eq!(
        paths.len(),
        COUNT,
        "样本数须为 {COUNT}（实得 {}）",
        paths.len()
    );
    let mut seen = BTreeSet::new();
    for path in &paths {
        let body: Vec<String> = check(path)
            .into_iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect();
        assert!(
            seen.insert(body.join("\n")),
            "{}: 样本不得重复",
            path.display()
        );
    }
}

#[test]
fn samples_cover_run_stream_vocabulary() {
    let mut all = BTreeSet::new();
    for path in &files() {
        for (kind, _) in check(path) {
            all.insert(kind);
        }
    }
    for required in COVERED {
        assert!(all.contains(required), "样本集缺少 {} 场景", required);
    }
}
