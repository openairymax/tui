// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 进程存活探针（0.1.20 P2-8：W8 骨架，0.1.21 演化指标视图的前置）。
//
// 承诺边界只有三件：可启动、可退出、可被 health probe 观察到（0.1.20
// 方案 Q16）。不渲染、不交互、不读网关数据、不碰演化指标；夹带任何功能
// 即退回。唯一输出是 stdout 上的一行 JSON，因此 CI、容器与串口日志都能
// 直接判定二进制可执行——这正是全屏路径做不到的（非 TTY 拒进，见 entry）。
//
// 与 engine::probe 的分工：后者解析终端能力回包（OSC11/DEC2031），属渲染
// 前置；本模块是进程级自检，不读终端、不产生任何副作用。

use std::io::Write;

use serde_json::json;

use crate::engine::exit::Code;
use crate::paths;

/// 探针输出的公开契约标识：字段只增不改，改名即破坏健康检查方。
pub(crate) const SCHEMA: &str = "airy.tui.probe/1";

/// 组装探针帧（纯函数，不做 IO，便于单测断言）。
pub(crate) fn frame(gateway: &str) -> String {
    json!({
        "schema": SCHEMA,
        "status": "ok",
        "version": env!("AIRY_RT_VERSION"),
        "pid": std::process::id(),
        "airy_home": paths::airy_home().display().to_string(),
        "gateway": gateway,
        "scope": "probe",
    })
    .to_string()
}

/// 输出一行探针帧并给出退出码；stdout 已关闭时按冻结表返回 Pipe。
pub(crate) fn run(gateway: &str) -> i32 {
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{}", frame(gateway)).and_then(|()| out.flush()) {
        Ok(()) => Code::Ok.code(),
        Err(_) => Code::Pipe.code(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_carries_contract_fields() {
        let home = crate::test_env::Home::new("probe");
        let text = frame("http://127.0.0.1:8123");
        let got: serde_json::Value = serde_json::from_str(&text).expect("探针帧必须是合法 JSON");
        assert_eq!(got["schema"], SCHEMA);
        assert_eq!(got["status"], "ok");
        assert_eq!(got["scope"], "probe");
        assert_eq!(got["version"], env!("AIRY_RT_VERSION"));
        assert_eq!(got["gateway"], "http://127.0.0.1:8123");
        assert_eq!(got["airy_home"], home.path().display().to_string());
        assert!(got["pid"].as_u64().unwrap_or(0) > 0);
        assert_eq!(text.lines().count(), 1, "探针帧必须单行");
    }
}
