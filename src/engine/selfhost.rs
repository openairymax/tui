// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 自托管快照（D-1）：把 llm_d `llm.get_stats` 回执里的 `endpoints` /
// `requests` / `last_error` 三段解析为纯数据，面板据此展示模型、后端、
// 并发、队列、失败率与最近错误。显存由采集任务另填（本层不做 IO）。
//
// 诚实语义：不可用时 endpoints 恒为空且保留原始原因；缺字段不补 0，
// 面板显示「未上报」。本层不读时钟，`age_ms` 是 llm_d 采集时刻的差值。

use serde_json::Value;

/// 单个自托管端点：`capacity == 0` 表示未设并发闸（不是「容量为零」）。
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub provider: String,
    pub models: Vec<String>,
    pub api_base: String,
    pub capacity: u64,
    pub total_slots: Option<u64>,
    pub ctx_slot: Option<u64>,
    pub inflight: Option<u64>,
    pub waiting: Option<u64>,
    pub ready: Option<bool>,
    pub reason: String,
}

/// 请求计数四元组（D-7 口径：ok/fail/timeout 互斥，三者之和为 total）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requests {
    pub total: u64,
    pub ok: u64,
    pub fail: u64,
    pub timeout: u64,
}

impl Requests {
    /// 失败率 =（失败 + 超时）/ 总量；无请求时无定义（返回 None，不显示 0%）。
    pub fn fail_rate(&self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some((self.fail + self.timeout) as f64 / self.total as f64)
        }
    }
}

/// 最近一次失败：模型 + 错误码 + 权威描述 + 采集时刻距今毫秒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastError {
    pub model: String,
    pub rc: i64,
    pub reason: String,
    pub age_ms: u64,
}

/// 自托管观测快照。live=false 时其余字段不承载任何旧值。
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub live: bool,
    pub note: String,
    /// llm_d 是否上报了 endpoints 字段（缺失说明 daemon 版本过旧）。
    pub ep_reported: bool,
    pub endpoints: Vec<Endpoint>,
    pub requests: Option<Requests>,
    pub last_error: Option<LastError>,
    /// 本机显存（已用 MiB，总量 MiB）；采集失败为 None。
    pub vram: Option<(u64, u64)>,
}

/// 不可用快照：清空端点等旧值，只保留原始原因。
pub fn unavailable(reason: impl Into<String>) -> Snapshot {
    Snapshot {
        live: false,
        note: reason.into(),
        ep_reported: false,
        endpoints: Vec::new(),
        requests: None,
        last_error: None,
        vram: None,
    }
}

/// 解析 `llm.get_stats` 回执。缺字段按「未上报」处理，不合成数值。
pub fn snapshot(v: &Value) -> Snapshot {
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return unavailable(err);
    }
    let ep_reported = v.get("endpoints").is_some();
    let endpoints = v
        .get("endpoints")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(parse_ep).collect())
        .unwrap_or_default();
    Snapshot {
        live: true,
        note: String::new(),
        ep_reported,
        endpoints,
        requests: parse_requests(v.get("requests")),
        last_error: parse_last_error(v.get("last_error")),
        vram: None,
    }
}

fn parse_ep(v: &Value) -> Option<Endpoint> {
    let provider = v.get("provider").and_then(Value::as_str)?.to_string();
    Some(Endpoint {
        provider,
        models: v
            .get("models")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        api_base: text(v, "api_base"),
        capacity: num(v, "capacity").unwrap_or(0),
        total_slots: num(v, "total_slots"),
        ctx_slot: num(v, "n_ctx_slot"),
        inflight: num(v, "inflight"),
        waiting: num(v, "waiting"),
        ready: v.get("backend_ready").and_then(Value::as_bool),
        reason: text(v, "backend_reason"),
    })
}

fn parse_requests(v: Option<&Value>) -> Option<Requests> {
    let o = v?;
    Some(Requests {
        total: num(o, "total")?,
        ok: num(o, "ok").unwrap_or(0),
        fail: num(o, "fail").unwrap_or(0),
        timeout: num(o, "timeout").unwrap_or(0),
    })
}

fn parse_last_error(v: Option<&Value>) -> Option<LastError> {
    let o = v?;
    Some(LastError {
        model: text(o, "model"),
        rc: o.get("rc").and_then(Value::as_i64).unwrap_or(0),
        reason: text(o, "reason"),
        age_ms: num(o, "age_ms").unwrap_or(0),
    })
}

fn num(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn full() -> Value {
        json!({
            "requests": {"total": 40, "ok": 35, "fail": 3, "timeout": 2, "success_rate": 0.875},
            "endpoints": [{
                "provider": "gateapi",
                "models": ["gate-model", "gate-fast"],
                "capacity": 8,
                "api_base": "http://127.0.0.1:21490/v1",
                "props_state": 1,
                "total_slots": 4,
                "n_ctx_slot": 4096,
                "inflight": 2,
                "waiting": 1,
                "backend_ready": true,
                "backend_reason": ""
            }],
            "last_error": {"model": "gate-model", "rc": -12, "reason": "upstream_error", "age_ms": 42000}
        })
    }

    #[test]
    fn parses_full_snapshot() {
        let s = snapshot(&full());
        assert!(s.live);
        assert!(s.ep_reported);
        assert_eq!(s.endpoints.len(), 1);
        let e = &s.endpoints[0];
        assert_eq!(e.provider, "gateapi");
        assert_eq!(e.models, ["gate-model", "gate-fast"]);
        assert_eq!(e.api_base, "http://127.0.0.1:21490/v1");
        assert_eq!(e.capacity, 8);
        assert_eq!(e.total_slots, Some(4));
        assert_eq!(e.ctx_slot, Some(4096));
        assert_eq!(e.inflight, Some(2));
        assert_eq!(e.waiting, Some(1));
        assert_eq!(e.ready, Some(true));
        let r = s.requests.expect("requests");
        assert_eq!((r.total, r.ok, r.fail, r.timeout), (40, 35, 3, 2));
        let le = s.last_error.expect("last_error");
        assert_eq!(le.model, "gate-model");
        assert_eq!(le.rc, -12);
        assert_eq!(le.reason, "upstream_error");
        assert_eq!(le.age_ms, 42000);
    }

    #[test]
    fn missing_endpoints_is_reported_not_faked() {
        let s = snapshot(&json!({"requests": {"total": 1, "ok": 1}}));
        assert!(s.live);
        assert!(!s.ep_reported, "缺失必须如实标记为未上报");
        assert!(s.endpoints.is_empty());
        assert_eq!(s.requests.map(|r| r.total), Some(1));
    }

    #[test]
    fn endpoint_without_provider_is_skipped() {
        let s = snapshot(&json!({"endpoints": [{"models": ["x"]}, {"provider": "ok"}]}));
        assert_eq!(s.endpoints.len(), 1);
        assert_eq!(s.endpoints[0].provider, "ok");
        assert_eq!(s.endpoints[0].capacity, 0, "未上报容量按未设闸处理");
        assert_eq!(s.endpoints[0].ready, None, "缺值不得当作就绪");
    }

    #[test]
    fn missing_counters_stay_none() {
        let s = snapshot(&json!({"endpoints": [{"provider": "p"}]}));
        let e = &s.endpoints[0];
        assert_eq!(e.total_slots, None);
        assert_eq!(e.inflight, None);
        assert_eq!(e.waiting, None);
    }

    #[test]
    fn fail_rate_counts_timeouts_and_is_none_without_requests() {
        let r = Requests { total: 40, ok: 35, fail: 3, timeout: 2 };
        assert!((r.fail_rate().expect("rate") - 0.125).abs() < 1e-9);
        let none = Requests { total: 0, ok: 0, fail: 0, timeout: 0 };
        assert_eq!(none.fail_rate(), None, "零请求不得显示 0%");
    }

    #[test]
    fn unavailable_clears_old_values() {
        let s = unavailable("llm_d unreachable");
        assert!(!s.live);
        assert_eq!(s.note, "llm_d unreachable");
        assert!(s.endpoints.is_empty());
        assert!(s.requests.is_none());
        assert!(s.last_error.is_none());
        assert!(s.vram.is_none());
    }

    #[test]
    fn error_field_is_unavailable() {
        let s = snapshot(&json!({"error": "gateway timeout"}));
        assert!(!s.live);
        assert_eq!(s.note, "gateway timeout");
    }
}
