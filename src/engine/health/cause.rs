// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 错误成因：稳定枚举是唯一权威（机制层 failure.reason），关键词归类只作
// 旧引擎与未登记原因的回落路径（§6.13 #2）。

use agentrt_rs::run_stream::gen;

/// 错误成因（人话摘要 + 可行动修复命令）。字段全为静态串，聚合零分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cause {
    pub(crate) code: &'static str,
    pub(crate) label: &'static str,
    pub(crate) remedy: &'static str,
}

const QUOTA: Cause = Cause {
    code: "quota",
    label: "后端限流，请稍后重试",
    remedy: "airymaxrt doctor",
};
const AUTH: Cause = Cause {
    code: "auth",
    label: "API Key 无效或未授权",
    remedy: "airymaxrt doctor",
};
const MODEL: Cause = Cause {
    code: "model",
    label: "模型不存在或未安装",
    remedy: "airymaxrt doctor",
};
const CONFIG: Cause = Cause {
    code: "config",
    label: "未配置可用模型，请先完成模型配置",
    remedy: "airymaxrt doctor",
};
const REJECT: Cause = Cause {
    code: "rejected",
    label: "模型服务拒绝了请求（API Key / 模型名 / 配额）",
    remedy: "airymaxrt doctor",
};
const TIMEOUT: Cause = Cause {
    code: "timeout",
    label: "请求超时，后端未响应",
    remedy: "airymaxrt logs llm_d",
};
pub(crate) const UNREACH: Cause = Cause {
    code: "unreachable",
    label: "无法连接后端服务",
    remedy: "airymaxrt logs llm_d",
};
const BACKEND: Cause = Cause {
    code: "backend",
    label: "后端服务内部错误",
    remedy: "airymaxrt logs llm_d",
};
const UNKNOWN: Cause = Cause {
    code: "unknown",
    label: "未知错误，请查看日志",
    remedy: "airymaxrt doctor",
};

/// 失败载荷的稳定枚举 → 成因。
///
/// 机制层下发的 `failure.reason` 是归类的唯一权威：摘要文案可随措辞
/// 调整甚至本地化，按文本关键词判成因必然漂移。只有载荷缺失或原因未
/// 登记（旧引擎、未来的新原因）才回落到 `classify` 的关键词路径。
pub(crate) fn from_wire(reason: &str) -> Option<Cause> {
    match reason {
        gen::AIRY_RS_REASON_CONFIG => Some(CONFIG),
        gen::AIRY_RS_REASON_PROVIDER => Some(UNREACH),
        gen::AIRY_RS_REASON_PROVIDER_ERR => Some(REJECT),
        _ => None,
    }
}

/// 把原始错误串归类为人话成因。顺序即优先级：先端点侧语义，后传输侧语义。
/// 本函数只作回落路径——调用方持有结构化 `reason` 时必须先用 `from_wire`；
/// 否则中文摘要与英文原文的措辞差异会决定归类结果。
pub(crate) fn classify(raw: &str) -> Cause {
    let s = raw.to_ascii_lowercase();
    if has_any(
        &s,
        &[
            "未配置可用模型",
            "未配置模型",
            "config_missing",
            "no model configured",
        ],
    ) {
        return CONFIG;
    }
    if has_any(&s, &["拒绝了请求", "provider_error"]) {
        return REJECT;
    }
    if has_any(
        &s,
        &[
            "429",
            "rate limit",
            "quota",
            "too many requests",
            "限流",
            "配额",
        ],
    ) {
        return QUOTA;
    }
    if has_any(
        &s,
        &[
            "401",
            "unauthorized",
            "invalid api key",
            "api key",
            "authentication",
            "鉴权",
            "未授权",
        ],
    ) {
        return AUTH;
    }
    if has_any(
        &s,
        &[
            "model not found",
            "no such model",
            "unknown model",
            "target not found",
            "模型不存在",
            "模型未安装",
        ],
    ) {
        return MODEL;
    }
    if has_any(&s, &["timed out", "timeout", "deadline exceeded", "超时"]) {
        return TIMEOUT;
    }
    if has_any(
        &s,
        &[
            "connection refused",
            "connection reset",
            "connect error",
            "failed to connect",
            "broken pipe",
            "unexpected eof",
            "connection closed",
            "unreachable",
            "不可达",
            "无法连接",
        ],
    ) {
        return UNREACH;
    }
    if has_any(
        &s,
        &[
            " 500",
            " 502",
            " 503",
            " 504",
            "internal server error",
            "bad gateway",
            "service unavailable",
            "后端服务内部错误",
        ],
    ) {
        return BACKEND;
    }
    UNKNOWN
}

fn has_any(s: &str, pats: &[&str]) -> bool {
    pats.iter().any(|p| s.contains(p))
}
