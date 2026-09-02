// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 提供商内置预设（单一来源）：选提供商即自动填充连接方式/接口格式/
// 请求地址/模型候选，非专业用户只需粘贴 API Key。

/// 单个提供商预设
pub(crate) struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub mode: &'static str,
    pub api_format: &'static str,
    pub base_url: &'static str,
    pub models: &'static [&'static str],
}

pub(crate) const PRESETS: &[Preset] = &[
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.deepseek.com",
        models: &["deepseek-chat", "deepseek-reasoner"],
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.openai.com/v1",
        models: &["gpt-4o", "gpt-4o-mini", "gpt-4.1", "o3-mini"],
    },
    Preset {
        id: "glm",
        label: "智谱 GLM",
        mode: "api",
        api_format: "openai",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["GLM-4.7-Flash", "GLM-4.7", "GLM-4-Plus"],
    },
    Preset {
        id: "qwen",
        label: "通义千问",
        mode: "api",
        api_format: "openai",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen-max", "qwen-plus", "qwen-turbo", "qwen3-flash"],
    },
    Preset {
        id: "kimi",
        label: "Kimi / Moonshot",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.moonshot.cn/v1",
        models: &["moonshot-v1-32k", "moonshot-v1-8k"],
    },
    Preset {
        id: "siliconflow",
        label: "硅基流动",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.siliconflow.cn/v1",
        models: &["deepseek-ai/DeepSeek-V3", "Qwen/Qwen3-235B-A22B"],
    },
    Preset {
        id: "spark",
        label: "讯飞星火",
        mode: "api",
        api_format: "openai",
        base_url: "https://spark-api-open.xf-yun.com/v1",
        models: &["spark-4.0", "spark-lite"],
    },
    Preset {
        id: "anthropic",
        label: "Anthropic",
        mode: "api",
        api_format: "anthropic",
        base_url: "https://api.anthropic.com",
        models: &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"],
    },
    Preset {
        id: "local",
        label: "本地模型 (Ollama/vLLM)",
        mode: "local",
        api_format: "openai",
        base_url: "http://localhost:11434/v1",
        models: &["llama3.1:8b", "qwen2.5:7b", "deepseek-r1:7b"],
    },
];

/// 预设 → 字段值的取值来源（由 `steps::FieldSpec::preset` 引用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresetSource {
    Mode,
    ApiFormat,
    Label,
    BaseUrl,
    Model,
}

/// 取预设中对应来源的文本
pub(crate) fn preset_text(p: &Preset, src: PresetSource) -> &'static str {
    match src {
        PresetSource::Mode => p.mode,
        PresetSource::ApiFormat => p.api_format,
        PresetSource::Label => p.label,
        PresetSource::BaseUrl => p.base_url,
        PresetSource::Model => p.models[0],
    }
}

/// 提供商文本 → 预设（id 精确或包含匹配；未匹配回落首个预设）
pub(crate) fn preset_by(text: &str) -> &'static Preset {
    let cur = text.trim().to_ascii_lowercase();
    PRESETS
        .iter()
        .find(|p| p.id == cur || cur.contains(p.id))
        .unwrap_or(&PRESETS[0])
}

/// 提供商文本 → 预设在表中的下标（Tab 循环用；未匹配 None）
pub(crate) fn preset_index(text: &str) -> Option<usize> {
    let cur = text.trim().to_ascii_lowercase();
    PRESETS
        .iter()
        .position(|p| p.id == cur || p.label.to_ascii_lowercase() == cur)
}

/// 请求地址 → 提供商 id（重开向导时回填提供商字段）
pub(crate) fn provider_of_url(base_url: &str) -> &'static str {
    let b = base_url.to_ascii_lowercase();
    if b.contains("deepseek") {
        "deepseek"
    } else if b.contains("openai") {
        "openai"
    } else if b.contains("bigmodel") || b.contains("zhipu") {
        "glm"
    } else if b.contains("dashscope") || b.contains("aliyuncs") {
        "qwen"
    } else if b.contains("moonshot") {
        "kimi"
    } else if b.contains("siliconflow") {
        "siliconflow"
    } else if b.contains("xf-yun") || b.contains("spark") {
        "spark"
    } else if b.contains("anthropic") {
        "anthropic"
    } else if b.contains("localhost") || b.contains("127.0.0.1") {
        "local"
    } else {
        "deepseek"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_matches_by_id_and_label() {
        assert_eq!(preset_by("qwen").id, "qwen");
        assert_eq!(preset_by("智谱 GLM").id, "glm");
        assert_eq!(preset_by("Anthropic").id, "anthropic");
        assert_eq!(preset_by("custom-gateway").id, "deepseek");
        assert_eq!(preset_by("local").mode, "local");
    }

    #[test]
    fn preset_index_drives_tab_cycle() {
        let last = PRESETS.len() - 1;
        assert_eq!(preset_index("deepseek"), Some(0));
        assert_eq!(preset_index("本地模型 (Ollama/vLLM)"), Some(last));
        assert_eq!(preset_index("未收录的提供商"), None);
        let next = preset_index("qwen").map(|i| PRESETS[(i + 1) % PRESETS.len()].id);
        assert_eq!(next, Some("kimi"));
    }

    #[test]
    fn preset_text_covers_all_sources() {
        let p = preset_by("openai");
        assert_eq!(preset_text(p, PresetSource::Mode), "api");
        assert_eq!(preset_text(p, PresetSource::ApiFormat), "openai");
        assert_eq!(preset_text(p, PresetSource::Label), "OpenAI");
        assert_eq!(preset_text(p, PresetSource::BaseUrl), "https://api.openai.com/v1");
        assert_eq!(preset_text(p, PresetSource::Model), "gpt-4o");
    }

    #[test]
    fn provider_of_url_infers_id() {
        assert_eq!(provider_of_url("https://open.bigmodel.cn/api/paas/v4"), "glm");
        assert_eq!(provider_of_url("http://127.0.0.1:8000/v1"), "local");
        assert_eq!(provider_of_url(""), "deepseek");
    }
}
