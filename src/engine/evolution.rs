// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 演化指标快照（W8）：把 gateway `airy_sys_telemetry_metrics` 回执里的
// `evolution` 段解析为纯数据。只有两种如实状态——可用（含指标清单）与
// 不可用（含原始原因）；缺值不以 0 冒充，未知键也照实保留。
//
// 本层不做 IO、不读时钟：解析与量纲译名全是纯函数，便于确定性单测。

use serde_json::Value;

/// 单条演化指标。`value` 缺失或非数值时为 NaN（面板显示「未上报」）。
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub key: String,
    pub label: String,
    pub value: f64,
    pub unit: String,
}

/// 演化指标快照：available=false 时 items 恒为空、reason 为原始原因。
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    pub reason: String,
    pub items: Vec<Metric>,
}

/// 已知六项演化指标（键、中文名、量纲），顺序即面板展示顺序。
const KNOWN: [(&str, &str, &str); 6] = [
    ("patterns", "演化模式", "个"),
    ("detections_total", "检出次数", "次"),
    ("preemptions_total", "抢占次数", "次"),
    ("effectiveness", "有效性", "比例"),
    ("records", "演化记录", "条"),
    ("evolve_noop_total", "空转次数", "次"),
];

/// 指标类型中文名：计数器按总量累加，瞬时值按当前快照。未知键返回空串。
pub fn kind(key: &str) -> &'static str {
    if key.ends_with("_total") {
        "计数器"
    } else if KNOWN
        .iter()
        .any(|(k, _, _)| *k == key && !k.ends_with("_total"))
    {
        "瞬时值"
    } else {
        ""
    }
}

/// 解析 `evolution` 段。缺字段、不可用、指标为空三种情况都不伪造数据。
pub fn snapshot(v: &Value) -> Snapshot {
    let Some(evo) = v.get("evolution") else {
        return unavailable("网关响应缺少 evolution 字段");
    };
    if !evo.get("available").and_then(Value::as_bool).unwrap_or(false) {
        let reason = evo
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("演化指标不可用");
        return unavailable(reason);
    }
    Snapshot {
        available: true,
        reason: String::new(),
        items: collect(evo.get("metrics")),
    }
}

/// 收集指标：已知六项按固定顺序在前，其余键按回执顺序追加（标签用键名）。
fn collect(map: Option<&Value>) -> Vec<Metric> {
    let mut items = Vec::new();
    for (key, label, unit) in KNOWN {
        if let Some(v) = map.and_then(|m| m.get(key)) {
            items.push(Metric {
                key: key.to_string(),
                label: label.to_string(),
                value: number(v),
                unit: unit.to_string(),
            });
        }
    }
    if let Some(obj) = map.and_then(Value::as_object) {
        for (key, v) in obj {
            if KNOWN.iter().any(|(k, _, _)| k == key) {
                continue;
            }
            items.push(Metric {
                key: key.clone(),
                label: key.clone(),
                value: number(v),
                unit: String::new(),
            });
        }
    }
    items
}

fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn unavailable(reason: &str) -> Snapshot {
    Snapshot {
        available: false,
        reason: reason.to_string(),
        items: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_available_metrics_in_fixed_order() {
        let v = json!({
            "evolution": {
                "available": true,
                "source": "monit",
                "metrics": {
                    "evolve_noop_total": 4.0,
                    "patterns": 12.0,
                    "effectiveness": 0.85
                }
            }
        });
        let s = snapshot(&v);
        assert!(s.available);
        assert_eq!(s.reason, "");
        let keys: Vec<&str> = s.items.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["patterns", "effectiveness", "evolve_noop_total"]);
        assert_eq!(s.items[0].label, "演化模式");
        assert_eq!(s.items[0].value, 12.0);
        assert_eq!(s.items[1].unit, "比例");
    }

    #[test]
    fn unavailable_keeps_reason_and_no_items() {
        let v = json!({
            "evolution": { "available": false, "reason": "monit unreachable" }
        });
        let s = snapshot(&v);
        assert!(!s.available);
        assert_eq!(s.reason, "monit unreachable");
        assert!(s.items.is_empty());
    }

    #[test]
    fn empty_metrics_is_available_not_fake_zeros() {
        let v = json!({
            "evolution": { "available": true, "source": "monit", "metrics": {} }
        });
        let s = snapshot(&v);
        assert!(s.available);
        assert!(s.items.is_empty(), "空指标不得合成 0 值: {:?}", s.items);
    }

    #[test]
    fn missing_evolution_field_is_unavailable() {
        let s = snapshot(&json!({ "runtime": {} }));
        assert!(!s.available);
        assert!(s.reason.contains("evolution"));
    }

    #[test]
    fn missing_value_is_nan_not_zero() {
        let v = json!({
            "evolution": { "available": true, "metrics": { "patterns": null } }
        });
        let s = snapshot(&v);
        assert_eq!(s.items.len(), 1);
        assert!(s.items[0].value.is_nan(), "缺值不得当作 0");
    }

    #[test]
    fn unknown_key_is_kept_with_raw_label() {
        let v = json!({
            "evolution": { "available": true, "metrics": { "patterns": 1.0, "future_metric": 7.0 } }
        });
        let s = snapshot(&v);
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.items[1].key, "future_metric");
        assert_eq!(s.items[1].label, "future_metric");
    }

    #[test]
    fn kind_tells_counter_from_gauge() {
        assert_eq!(kind("patterns"), "瞬时值");
        assert_eq!(kind("detections_total"), "计数器");
        assert_eq!(kind("future_metric"), "");
    }
}
