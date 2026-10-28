// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// P2-8 验收（0.1.20 方案 Q16）：探针只承诺「可启动、可退出、可被 health
// probe 观察到」。本测试的 stdout 本身就是管道（非 TTY），因此它同时证明
// 探针排在 TTY 门禁之前裁决——否则进程会以 Usage(2) 退出。

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_agentrt-tui");
const SCHEMA: &str = "airy.tui.probe/1";

fn probe(args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.arg("--probe").args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("探针必须可被启动");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn probe_exits_zero_with_single_json_line() {
    let (code, stdout, stderr) = probe(&["--gateway-url", "http://127.0.0.1:65535"], &[]);
    assert_eq!(code, 0, "探针退出码必须是 0，stderr: {stderr}");
    assert_eq!(stdout.lines().count(), 1, "探针只输出一行，实际: {stdout:?}");

    let got: serde_json::Value = serde_json::from_str(stdout.trim()).expect("探针帧必须是合法 JSON");
    assert_eq!(got["schema"], SCHEMA);
    assert_eq!(got["status"], "ok");
    assert_eq!(got["scope"], "probe");
    assert_eq!(got["gateway"], "http://127.0.0.1:65535");
    assert!(got["pid"].as_u64().unwrap_or(0) > 0);
    assert!(!got["version"].as_str().unwrap_or_default().is_empty());
}

#[test]
fn probe_reports_resolved_airy_home() {
    let home = std::env::temp_dir().join(format!("airy-probe-{}", std::process::id()));
    let (code, stdout, stderr) = probe(&[], &[("AIRY_HOME", &home.to_string_lossy())]);
    assert_eq!(code, 0, "探针退出码必须是 0，stderr: {stderr}");

    let got: serde_json::Value = serde_json::from_str(stdout.trim()).expect("探针帧必须是合法 JSON");
    assert_eq!(got["airy_home"], home.display().to_string());
    // 未显式给 --gateway-url 时回落到 gateway.port 缺失的默认端口。
    assert_eq!(got["gateway"], "http://127.0.0.1:8080");
}
