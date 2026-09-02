// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导步骤与字段注册表（Unify Design SSoT）：步骤序号、标题、副标题、
// 动作按钮、字段标签/提示/交互类型/默认值/预设来源/校验严格度全部在此
// 单处声明。渲染、按键、校验、写回 model.yaml 一律按 key 取值，不再有
// 散落的字段下标魔数与并行的默认值回落链。

use crate::models_cfg::ModelYaml;

use super::lang::Lang;
use super::presets::provider_of_url;
use super::presets::PresetSource;

/// 选项型步骤数（语言、开始方式）
const CHOICE_STEPS: u8 = 2;

/// 字段交互类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    /// 自由文本：Enter 进入编辑态
    Text,
    /// 候选集循环（Enter/Tab）；strict = 取值必须落在候选集内
    Options {
        opts: &'static [&'static str],
        strict: bool,
    },
    /// 提供商：可编辑，Tab 循环内置预设，编辑结束后回填关联字段
    Provider,
    /// 模型 ID：Tab 循环当前提供商的候选模型
    PresetModels,
}

impl FieldKind {
    /// Enter 是否进入文本编辑态（Options 为 Enter 循环，不进编辑）
    pub(crate) fn editable(self) -> bool {
        !matches!(self, FieldKind::Options { .. })
    }
}

/// 字段标识（跨步骤唯一定位一个配置项）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKey {
    Provider,
    Name,
    Mode,
    ApiFormat,
    BaseUrl,
    ModelId,
    ApiKey,
    CtxWindow,
    MaxOutput,
    ToolRounds,
    Vision,
    Thinking,
    ThinkEnabled,
    SlowModel,
    FastModel,
    ProfModel,
}

/// 单个字段的完整规格
pub(crate) struct FieldSpec {
    pub key: FieldKey,
    /// (中文, English)
    pub label: (&'static str, &'static str),
    pub hint: (&'static str, &'static str),
    pub kind: FieldKind,
    /// model.yaml 无现值时的默认值
    pub fallback: &'static str,
    /// 提供商预设自动填充来源
    pub preset: Option<PresetSource>,
    /// 非编辑态掩码展示（API Key）
    pub masked: bool,
    /// mode = local 时不展示（本地模型无需 API Key）
    pub hidden_on_local: bool,
}

/// 单个表单步骤的完整规格
pub(crate) struct StepSpec {
    pub id: u8,
    pub title: (&'static str, &'static str),
    pub subtitle: Option<(&'static str, &'static str)>,
    /// 底部动作按钮文案
    pub action: (&'static str, &'static str),
    pub fields: &'static [FieldSpec],
}

impl StepSpec {
    /// 字段在表单中的位置
    pub(crate) fn field_idx(&self, key: FieldKey) -> Option<usize> {
        self.fields.iter().position(|f| f.key == key)
    }

    /// 该位置是否展示（越界 = 完成按钮行，恒展示）
    pub(crate) fn shown(&self, idx: usize, mode_local: bool) -> bool {
        match self.fields.get(idx) {
            Some(f) => !(f.hidden_on_local && mode_local),
            None => true,
        }
    }

    /// 可见字段位置列表（光标按可见位置计数）
    pub(crate) fn visible(&self, mode_local: bool) -> Vec<usize> {
        (0..self.fields.len())
            .filter(|i| self.shown(*i, mode_local))
            .collect()
    }
}

/// 表单步骤注册表（步骤 3/4/5）
pub(crate) const FORM_STEPS: &[StepSpec] = &[
    StepSpec {
        id: 3,
        title: ("模型基本配置", "Model Setup"),
        subtitle: Some((
            "选择提供商后地址/格式/模型自动填充，只需粘贴 API Key",
            "Pick a provider — URL, format & models fill in automatically",
        )),
        action: ("下一步：高级选项", "Next: Advanced options"),
        fields: &[
            FieldSpec {
                key: FieldKey::Provider,
                label: ("提供商", "Provider"),
                hint: (
                    "deepseek / openai / glm / qwen / kimi / local …（Tab 循环预设）",
                    "deepseek / openai / glm / qwen / kimi / local … (Tab cycles presets)",
                ),
                kind: FieldKind::Provider,
                fallback: "deepseek",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::Name,
                label: ("名称", "Name"),
                hint: ("展示名称（默认取提供商名，可改）", "Display name (defaults to the provider name)"),
                kind: FieldKind::Text,
                fallback: "",
                preset: Some(PresetSource::Label),
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::Mode,
                label: ("连接方式", "Mode"),
                hint: (
                    "api = 云端 API · local = 本地模型（Enter 切换）",
                    "api = cloud API · local = local model (Enter toggles)",
                ),
                kind: FieldKind::Options {
                    opts: &["api", "local"],
                    strict: true,
                },
                fallback: "api",
                preset: Some(PresetSource::Mode),
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::ApiFormat,
                label: ("接口格式", "API Format"),
                hint: (
                    "openai = OpenAI 兼容 · anthropic = Anthropic Messages（Enter 切换）",
                    "openai = OpenAI-compatible · anthropic = Anthropic Messages (Enter toggles)",
                ),
                kind: FieldKind::Options {
                    opts: &["openai", "anthropic"],
                    strict: true,
                },
                fallback: "openai",
                preset: Some(PresetSource::ApiFormat),
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::BaseUrl,
                label: ("请求地址", "Base URL"),
                hint: (
                    "完整请求地址（不含 /chat/completions 后缀）",
                    "Full base URL (without /chat/completions suffix)",
                ),
                kind: FieldKind::Text,
                fallback: "",
                preset: Some(PresetSource::BaseUrl),
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::ModelId,
                label: ("模型 ID", "Model ID"),
                hint: ("实际调用的模型名（Tab 循环候选）", "Model id actually used (Tab cycles candidates)"),
                kind: FieldKind::PresetModels,
                fallback: "",
                preset: Some(PresetSource::Model),
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::ApiKey,
                label: ("API Key", "API Key"),
                hint: ("本地模型无需填写", "Leave empty for local models"),
                kind: FieldKind::Text,
                fallback: "",
                preset: None,
                masked: true,
                hidden_on_local: true,
            },
        ],
    },
    StepSpec {
        id: 4,
        title: ("高级选项（可全部保持默认）", "Advanced Options"),
        subtitle: None,
        action: ("下一步：双思考系统", "Next: Dual Thinking"),
        fields: &[
            FieldSpec {
                key: FieldKey::CtxWindow,
                label: ("上下文窗口", "Context window"),
                hint: ("128k / 256k / 512k / 1M / 2M（Tab 循环）", "128k / 256k / 512k / 1M / 2M (Tab cycles)"),
                kind: FieldKind::Options {
                    opts: &["128k", "256k", "512k", "1M", "2M"],
                    strict: false,
                },
                fallback: "128k",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::MaxOutput,
                label: ("最大输出", "Max output"),
                hint: ("4k / 16k / 32k / 128k / 256k（Tab 循环）", "4k / 16k / 32k / 128k / 256k (Tab cycles)"),
                kind: FieldKind::Options {
                    opts: &["4k", "16k", "32k", "128k", "256k"],
                    strict: false,
                },
                fallback: "16k",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::ToolRounds,
                label: ("工具轮数", "Tool rounds"),
                hint: ("工具调用轮数上限，默认 1000", "Max tool call rounds, default 1000"),
                kind: FieldKind::Text,
                fallback: "1000",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::Vision,
                label: ("图片输入", "Vision"),
                hint: ("true / false（Enter 切换）", "true / false (Enter toggles)"),
                kind: FieldKind::Options {
                    opts: &["true", "false"],
                    strict: true,
                },
                fallback: "false",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::Thinking,
                label: ("思考模式", "Thinking"),
                hint: (
                    "auto = 跟随模型默认 · on / off（Tab 循环）",
                    "auto = follow model default · on / off (Tab cycles)",
                ),
                kind: FieldKind::Options {
                    opts: &["auto", "on", "off"],
                    strict: false,
                },
                fallback: "auto",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
        ],
    },
    StepSpec {
        id: 5,
        title: ("双思考系统", "Dual Thinking System"),
        subtitle: Some((
            "慢思考组织逻辑 → 快思考快速校验 → 专业思考专家终裁",
            "Slow think plans → Fast think verifies → Professional think finalizes",
        )),
        action: ("完成配置，开始对话", "Finish & start chatting"),
        fields: &[
            FieldSpec {
                key: FieldKey::ThinkEnabled,
                label: ("启用双思考", "Enable"),
                hint: (
                    "true / false（Enter 切换；关闭则退化为单轮普通计划）",
                    "true / false (Enter toggles; off degrades to single-pass planning)",
                ),
                kind: FieldKind::Options {
                    opts: &["true", "false"],
                    strict: true,
                },
                fallback: "true",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::SlowModel,
                label: ("慢思考模型", "Slow model (t2)"),
                hint: ("组织逻辑、制定任务图纸（建议用最强模型）", "Plans logic & blueprints (use your strongest model)"),
                kind: FieldKind::Text,
                fallback: "",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::FastModel,
                label: ("快思考模型", "Fast model (t1-f)"),
                hint: ("对计划快速终裁（建议用轻快模型）", "Quickly finalizes the plan (use a fast model)"),
                kind: FieldKind::Text,
                fallback: "",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
            FieldSpec {
                key: FieldKey::ProfModel,
                label: ("专业思考模型", "Pro model (t1-p)"),
                hint: ("专家校验，留空 = 使用默认模型", "Expert review; empty = default model"),
                kind: FieldKind::Text,
                fallback: "",
                preset: None,
                masked: false,
                hidden_on_local: false,
            },
        ],
    },
];

/// 步骤总数
pub(crate) const TOTAL_STEPS: u8 = CHOICE_STEPS + FORM_STEPS.len() as u8;

/// 步骤 1 语言选项
pub(crate) const LANG_CHOICES: &[Lang] = &[Lang::Auto, Lang::English, Lang::Chinese];

/// 开始方式选项
pub(crate) struct Choice {
    pub label: (&'static str, &'static str),
    pub desc: (&'static str, &'static str),
}

/// 步骤 2 选项
pub(crate) const START_CHOICES: &[Choice] = &[
    Choice {
        label: ("快速配置模型", "Quick model setup"),
        desc: (
            "选提供商填 Key，自动匹配请求地址与模型，立即开始对话",
            "Pick a provider & paste a key; URL and models fill in automatically",
        ),
    },
    Choice {
        label: ("跳过，先进 TUI 探索", "Skip — explore the TUI first"),
        desc: (
            "直接进入对话，随时可用 /hiairy 重开向导",
            "Jump into chat; reopen anytime with /hiairy",
        ),
    },
];

/// 表单步骤规格（选项步骤返回 None）
pub(crate) fn form_step(id: u8) -> Option<&'static StepSpec> {
    FORM_STEPS.iter().find(|s| s.id == id)
}

/// 是否表单步骤
pub(crate) fn is_form_step(id: u8) -> bool {
    form_step(id).is_some()
}

/// 选项步骤的选项数（表单步骤返回 0）
pub(crate) fn choice_len(step: u8) -> usize {
    match step {
        1 => LANG_CHOICES.len(),
        2 => START_CHOICES.len(),
        _ => 0,
    }
}

/// 字段规格（key 全局唯一）
pub(crate) fn field_spec(key: FieldKey) -> &'static FieldSpec {
    FORM_STEPS
        .iter()
        .find_map(|s| s.fields.iter().find(|f| f.key == key))
        .expect("FieldKey 必须在注册表中声明")
}

/// 候选集循环：命中 → 下一项；空 → 首项；未命中 → strict ? 首项 : 保留自定义
pub(crate) fn cycle_value(cur: &str, opts: &[&str], strict: bool) -> String {
    let cur = cur.trim();
    if cur.is_empty() {
        return opts[0].to_string();
    }
    if let Some(pos) = opts.iter().position(|o| *o == cur) {
        return opts[(pos + 1) % opts.len()].to_string();
    }
    if strict {
        opts[0].to_string()
    } else {
        cur.to_string()
    }
}

/// 初值：model.yaml 现值 → 回落链 → 注册表默认值。
///
/// `hint` = 步骤 3 已选模型 ID（model.yaml 无 default_model 时供思考角色回落）。
pub(crate) fn seed_value(key: FieldKey, m: &ModelYaml, hint: &str) -> String {
    let row = m.rows.first();
    let think = m.think.as_ref();
    let raw = match key {
        FieldKey::Provider => match row {
            Some(r) => provider_of_url(&r.base_url).to_string(),
            None => std::env::var("AIRY_LLM_PROVIDER").unwrap_or_default(),
        },
        FieldKey::Name => row.map(|r| r.name.clone()).unwrap_or_default(),
        FieldKey::Mode => row.map(|r| r.mode.clone()).unwrap_or_default(),
        FieldKey::ApiFormat => row.map(|r| r.api_format.clone()).unwrap_or_default(),
        FieldKey::BaseUrl => row.map(|r| r.base_url.clone()).unwrap_or_default(),
        FieldKey::ModelId => row.map(|r| r.model_id.clone()).unwrap_or_default(),
        FieldKey::ApiKey => String::new(),
        FieldKey::CtxWindow => row.map(|r| r.context_window.clone()).unwrap_or_default(),
        FieldKey::MaxOutput => row.map(|r| r.max_output.clone()).unwrap_or_default(),
        FieldKey::ToolRounds => row.map(|r| r.tool_rounds.clone()).unwrap_or_default(),
        FieldKey::Vision => row.map(|r| r.vision.clone()).unwrap_or_default(),
        FieldKey::Thinking => row.map(|r| r.thinking.clone()).unwrap_or_default(),
        FieldKey::ThinkEnabled => think
            .and_then(|t| t.enabled)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        FieldKey::SlowModel | FieldKey::FastModel => {
            let cur = match key {
                FieldKey::SlowModel => think.map(|t| t.slow_model.clone()).unwrap_or_default(),
                _ => think.map(|t| t.fast_model.clone()).unwrap_or_default(),
            };
            if cur.is_empty() {
                if m.default_model.is_empty() {
                    hint.to_string()
                } else {
                    m.default_model.clone()
                }
            } else {
                cur
            }
        }
        FieldKey::ProfModel => think.map(|t| t.prof_model.clone()).unwrap_or_default(),
    };
    if raw.trim().is_empty() {
        field_spec(key).fallback.to_string()
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models_cfg::{ModelRow, ThinkCfg};

    #[test]
    fn registry_is_consistent() {
        assert_eq!(TOTAL_STEPS, 5);
        assert_eq!(FORM_STEPS.len(), 3);
        for (i, s) in FORM_STEPS.iter().enumerate() {
            assert_eq!(s.id, (i as u8) + 3, "表单步骤序号连续");
            assert_eq!(choice_len(s.id), 0);
            for f in s.fields {
                let owners = FORM_STEPS
                    .iter()
                    .filter(|st| st.fields.iter().any(|ff| ff.key == f.key))
                    .count();
                assert_eq!(owners, 1, "字段标识跨步骤唯一：{:?}", f.key);
                assert!(!f.label.0.is_empty() && !f.label.1.is_empty());
                assert!(!f.hint.0.is_empty() && !f.hint.1.is_empty());
                if let FieldKind::Options { opts, .. } = f.kind {
                    assert!(opts.contains(&f.fallback), "默认值必须是候选之一：{:?}", f.key);
                }
            }
        }
        assert_eq!(choice_len(1), LANG_CHOICES.len());
        assert_eq!(choice_len(2), START_CHOICES.len());
    }

    #[test]
    fn local_mode_hides_api_key() {
        let step = form_step(3).expect("步骤 3");
        assert_eq!(step.visible(false).len(), 7);
        assert_eq!(step.visible(true).len(), 6, "本地模型不展示 API Key");
        assert_eq!(step.field_idx(FieldKey::ApiKey), Some(6));
        assert_eq!(step.field_idx(FieldKey::ModelId), Some(5));
    }

    #[test]
    fn cycle_value_semantics() {
        assert_eq!(cycle_value("", &["a", "b"], true), "a");
        assert_eq!(cycle_value("a", &["a", "b"], true), "b");
        assert_eq!(cycle_value("b", &["a", "b"], false), "a");
        assert_eq!(cycle_value("custom", &["a", "b"], false), "custom");
        assert_eq!(cycle_value("custom", &["a", "b"], true), "a");
    }

    #[test]
    fn seed_prefers_yaml_then_fallback() {
        let m = ModelYaml {
            rows: vec![ModelRow {
                name: "我的模型".into(),
                mode: "api".into(),
                api_format: "anthropic".into(),
                base_url: "https://api.anthropic.com".into(),
                model_id: "claude-haiku-4-5-20251001".into(),
                api_key_env: String::new(),
                context_window: String::new(),
                max_output: "4k".into(),
                tool_rounds: String::new(),
                vision: "true".into(),
                thinking: String::new(),
            }],
            default_model: "claude-haiku-4-5-20251001".into(),
            think: Some(ThinkCfg {
                enabled: Some(false),
                slow_model: String::new(),
                fast_model: "fast-one".into(),
                prof_model: String::new(),
                timeout_ms: String::new(),
            }),
        };
        let seed = |key| seed_value(key, &m, "hint-model");
        assert_eq!(seed(FieldKey::Provider), "anthropic");
        assert_eq!(seed(FieldKey::Name), "我的模型");
        assert_eq!(seed(FieldKey::ApiKey), "");
        assert_eq!(seed(FieldKey::CtxWindow), "128k", "空值回落注册表默认");
        assert_eq!(seed(FieldKey::MaxOutput), "4k");
        assert_eq!(seed(FieldKey::ToolRounds), "1000");
        assert_eq!(seed(FieldKey::ThinkEnabled), "false");
        assert_eq!(seed(FieldKey::SlowModel), "claude-haiku-4-5-20251001", "回落 default_model");
        assert_eq!(seed(FieldKey::FastModel), "fast-one");
        assert_eq!(seed(FieldKey::ProfModel), "", "专业模型允许留空");

        let empty = ModelYaml::default();
        assert_eq!(
            seed_value(FieldKey::SlowModel, &empty, "hint-model"),
            "hint-model",
            "无 default_model 时回落步骤 3 模型"
        );
        assert_eq!(seed_value(FieldKey::Mode, &empty, ""), "api");
    }
}
