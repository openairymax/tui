// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧信封（0.1.21 X1-05b，§23.1.5 事件入口的契约层）。
//
// D-15 信封固定为 `{"schema":1,"seq":N,"ts":ms,"type":T,"payload":{...}}`。
// 本模块只做两件事：把 JSON 收成字段确定的结构体，以及拒绝版本不明的帧。
//
// schema 只认 1。版本不同的帧一律拒绝而不是按现状尽力解读——新 CLI 的字段
// 语义可能与旧渲染层的假设冲突，误读比缺帧更难查；拒掉的帧由上层计数，
// 用户在状态栏看到「n 帧被丢弃」，而不是看到一份悄悄错位的画面。
//
// 未知的顶层字段照常接受：向前兼容靠的是「不认识的字段不参与判断」，
// 靠 `deny_unknown_fields` 反而会让一次无害的字段扩充中断整条链路。

#![cfg_attr(not(test), allow(dead_code))]

use serde::Deserialize;
use serde_json::Value;

/// 唯一支持的 schema 版本（D-15）。
pub(crate) const SCHEMA: u64 = 1;

/// 信封：字段确定、载荷原样保留。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Envelope {
    pub(crate) seq: u64,
    pub(crate) ts: u64,
    pub(crate) kind: String,
    pub(crate) payload: Value,
}

/// 拒收原因：形状不合契约，或版本不受支持。两类的处置相同（跳过并计数），
/// 分开是为了让诊断能指出「是我们的版本旧了」还是「对端发了杂散对象」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reject {
    /// 缺字段、字段类型不符或载荷不是对象。
    Shape,
    /// 版本不受支持（携带对端声明的 schema）。
    Schema(u64),
}

/// serde 侧的收件形状：只声明契约字段，其余顶层字段忽略。
#[derive(Deserialize)]
struct Raw {
    schema: u64,
    seq: u64,
    ts: u64,
    #[serde(rename = "type")]
    kind: String,
    payload: Value,
}

impl Envelope {
    /// 从一个 JSON 值收成信封。
    pub(crate) fn read(value: Value) -> Result<Self, Reject> {
        let raw = serde_json::from_value::<Raw>(value).map_err(|_| Reject::Shape)?;
        if raw.schema != SCHEMA {
            return Err(Reject::Schema(raw.schema));
        }
        if !raw.payload.is_object() {
            return Err(Reject::Shape);
        }
        Ok(Self {
            seq: raw.seq,
            ts: raw.ts,
            kind: raw.kind,
            payload: raw.payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Envelope, Reject, SCHEMA};
    use serde_json::{json, Value};

    fn frame() -> Value {
        json!({
            "schema": 1,
            "seq": 7,
            "ts": 1763000000123u64,
            "type": "run_start",
            "payload": {"run": "r1"}
        })
    }

    /// 合契约的帧原样收下，字段与载荷都不走样。
    #[test]
    fn reads_valid_envelope() {
        let env = Envelope::read(frame()).expect("合法帧可读");
        assert_eq!(env.seq, 7);
        assert_eq!(env.ts, 1_763_000_000_123);
        assert_eq!(env.kind, "run_start");
        assert_eq!(env.payload["run"], json!("r1"));
        assert_eq!(SCHEMA, 1);
    }

    /// 版本不符即拒收，并把对端声明的版本带出来。
    #[test]
    fn rejects_other_schema() {
        for declared in [0u64, 2, 99] {
            let mut value = frame();
            value["schema"] = json!(declared);
            assert_eq!(Envelope::read(value), Err(Reject::Schema(declared)));
        }
    }

    /// 缺字段或类型不符：按形状拒收，不 panic。
    #[test]
    fn rejects_wrong_shape() {
        let mut missing = frame();
        missing.as_object_mut().expect("对象").remove("seq");
        assert_eq!(Envelope::read(missing), Err(Reject::Shape));

        let mut textual = frame();
        textual["seq"] = json!("seven");
        assert_eq!(Envelope::read(textual), Err(Reject::Shape));

        assert_eq!(Envelope::read(json!([1, 2])), Err(Reject::Shape));
    }

    /// 载荷必须是对象：D-15 的 payload 承载服务端事件，数组/标量都不合契约。
    #[test]
    fn rejects_non_object_payload() {
        let mut value = frame();
        value["payload"] = json!([1, 2, 3]);
        assert_eq!(Envelope::read(value), Err(Reject::Shape));
    }

    /// 多出的顶层字段不阻断解析：字段扩充只增不改。
    #[test]
    fn accepts_extra_fields() {
        let mut value = frame();
        value["trace"] = json!({"span": "abc"});
        let env = Envelope::read(value).expect("未知字段不影响收件");
        assert_eq!(env.kind, "run_start");
    }
}
