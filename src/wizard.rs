// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 首次启动向导（5 步，蓝晶风格）。
//
// 流程：
//   步骤 1/5：欢迎 + 版本 + 界面语言选择（自动检测 LC_ALL/LANG / English / 简体中文）
//   步骤 2/5：想怎么开始？（手动配置模型 / 跳过，先进 TUI 探索）
//   步骤 3/5：模型基本配置（v2 表格第 1 行：提供商/名称/连接方式/接口格式/
//             请求地址/模型 ID/API Key；提供商带内置预设，Tab 循环）
//   步骤 4/5：高级选项（上下文窗口/最大输出/工具轮数/图片输入/思考模式）
//   步骤 5/5：双思考系统（启用开关 + 慢/快/专业三个思考角色模型选择）
//
// 触发：
//   - 首次运行（$AIRY_HOME/tui/wizard.toml 不存在）自动弹出；
//   - 对话中输入 /hiairy 随时重开。
//
// 完成后的选择写回：
//   - $AIRY_HOME/tui/wizard.toml（lang + configured）
//   - $AIRY_HOME/config/secrets.env（MODEL_1_API_KEY，llm_d 热加载）
//   - $AIRY_HOME/config/model.yaml（models[0] 行 + think 段，llm_d/think_d 热加载）
//
// 2.3.15 重设计（2026-08-26）：从旧 3 字段表单升级为 v2 表格形式全字段，
// 新增提供商预设（非专业用户选提供商即可自动填充地址/格式/模型候选）、
// 高级选项与双思考系统独立步骤。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::models_cfg::{self, ModelRow, ThinkCfg};
use crate::theme;

/// 向导配置文件（$AIRY_HOME/tui/wizard.toml），存在即非首次运行。
const WIZARD_FILE: &str = "wizard.toml";

/// 步骤总数（语言 → 开始方式 → 模型基本 → 高级 → 双思考）。
const TOTAL_STEPS: u8 = 5;

/// 界面语言（步骤 1 的三选项）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 自动检测（优先 LC_ALL，其次 LANG）
    Auto,
    /// English
    English,
    /// 简体中文
    Chinese,
}

impl Lang {
    /// 从环境检测界面语言：优先 LC_ALL，其次 LANG。
    pub fn detect() -> Lang {
        for var in ["LC_ALL", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                let l = v.to_ascii_lowercase();
                if l.contains("zh") {
                    return Lang::Chinese;
                }
                if l.contains("en") {
                    return Lang::English;
                }
            }
        }
        Lang::English
    }

    /// 持久化使用的语言码（zh / en）
    fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            _ => "zh",
        }
    }

    /// 选项展示名
    fn label(self) -> &'static str {
        match self {
            Lang::Auto => "自动检测 (Auto)",
            Lang::English => "English",
            Lang::Chinese => "简体中文",
        }
    }
}

/// 提供商内置预设：非专业用户选提供商 → 自动填充连接方式/接口格式/
/// 请求地址/模型候选，只需粘贴 API Key 即可开始。
struct ProviderPreset {
    id: &'static str,
    label: &'static str,
    mode: &'static str,
    api_format: &'static str,
    base_url: &'static str,
    models: &'static [&'static str],
}

const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.deepseek.com",
        models: &["deepseek-chat", "deepseek-reasoner"],
    },
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.openai.com/v1",
        models: &["gpt-4o", "gpt-4o-mini", "gpt-4.1", "o3-mini"],
    },
    ProviderPreset {
        id: "glm",
        label: "智谱 GLM",
        mode: "api",
        api_format: "openai",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["GLM-4.7-Flash", "GLM-4.7", "GLM-4-Plus"],
    },
    ProviderPreset {
        id: "qwen",
        label: "通义千问",
        mode: "api",
        api_format: "openai",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen-max", "qwen-plus", "qwen-turbo", "qwen3-flash"],
    },
    ProviderPreset {
        id: "kimi",
        label: "Kimi / Moonshot",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.moonshot.cn/v1",
        models: &["moonshot-v1-32k", "moonshot-v1-8k"],
    },
    ProviderPreset {
        id: "siliconflow",
        label: "硅基流动",
        mode: "api",
        api_format: "openai",
        base_url: "https://api.siliconflow.cn/v1",
        models: &["deepseek-ai/DeepSeek-V3", "Qwen/Qwen3-235B-A22B"],
    },
    ProviderPreset {
        id: "spark",
        label: "讯飞星火",
        mode: "api",
        api_format: "openai",
        base_url: "https://spark-api-open.xf-yun.com/v1",
        models: &["spark-4.0", "spark-lite"],
    },
    ProviderPreset {
        id: "anthropic",
        label: "Anthropic",
        mode: "api",
        api_format: "anthropic",
        base_url: "https://api.anthropic.com",
        models: &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"],
    },
    ProviderPreset {
        id: "local",
        label: "本地模型 (Ollama/vLLM)",
        mode: "local",
        api_format: "openai",
        base_url: "http://localhost:11434/v1",
        models: &["llama3.1:8b", "qwen2.5:7b", "deepseek-r1:7b"],
    },
];

/// 模型基本配置字段类型：文本（编辑态插入）/ 循环（Enter/Tab 切换选项）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Text,
    Cycle,
}

/// 向导完成结果（供 App 使用）
#[derive(Debug, Clone)]
pub struct WizardResult {
    /// true = 完成快速配置；false = 跳过先探索
    pub configured: bool,
    /// 设置的模型 ID（空 = 未设置）
    pub model: String,
    /// 选择的提供商（空 = 未设置）
    pub provider: String,
    /// API Key 是否已写入 secrets.env
    pub api_key_set: bool,
    /// 双思考系统是否启用
    pub think_enabled: bool,
}

/// 首次启动向导状态
pub struct WizardState {
    /// 是否激活（首次运行自动激活；/hiairy 可随时重开）
    pub active: bool,
    /// 当前步骤（1..=5）
    pub step: u8,
    /// 步骤 1 语言选项光标（0=自动检测, 1=English, 2=简体中文）
    pub lang_cursor: usize,
    /// 步骤 2 配置选项光标（0=快速配置模型, 1=跳过）
    pub config_cursor: usize,
    /// 步骤 1 确认后的实际语言（驱动后续步骤文案）
    pub effective_lang: Lang,
    /// 当前表单字段值（按步骤切换：3=模型基本 7 项 / 4=高级 5 项 / 5=双思考 4 项）
    pub cfg_fields: Vec<String>,
    /// 步骤 3 模型基本字段快照（进入步骤 4 时保存，finish 写 model.yaml 用；
    /// 步骤 5 时 cfg_fields 已被双思考字段覆盖，不能丢失模型连接配置）
    model_fields: Vec<String>,
    /// 字段光标 / 编辑态
    pub cfg_cursor: usize,
    pub editing: bool,
    /// 编辑态插入点（字符索引）：支持 ← → 移动，在长文本（API Key /
    /// base_url）中精确定位修改，渲染时窗口跟随插入点滚动。
    edit_pos: usize,
    /// 文本字段是否被用户手动编辑过（预设自动填充不置位；防预设覆盖手改）
    touched: Vec<bool>,
    /// 向导完成结果（None = 未完成）
    pub result: Option<WizardResult>,
}

impl WizardState {
    /// 新建向导状态：首次运行（wizard.toml 不存在）时自动激活。
    pub fn new() -> Self {
        let active = is_first_run();
        if active {
            log::info!("wizard: first run detected, auto-activating");
        }
        Self {
            active,
            step: 1,
            lang_cursor: 0,
            config_cursor: 0,
            effective_lang: Lang::detect(),
            cfg_fields: default_model_fields(),
            model_fields: Vec::new(),
            cfg_cursor: 0,
            editing: false,
            edit_pos: 0,
            touched: vec![false; 7],
            result: None,
        }
    }

    /// 重新打开向导（/hiairy 调用；即使已完成也允许重开）。
    /// 预读现有 model.yaml，展示当前配置（不覆盖手写内容）。
    pub fn reopen(&mut self) {
        let m = models_cfg::read_model_yaml();
        self.active = true;
        self.step = 1;
        self.lang_cursor = 0;
        self.config_cursor = 0;
        self.effective_lang = Lang::detect();
        self.cfg_fields = if let Some(r) = m.rows.first() {
            vec![
                provider_from_base_url(&r.base_url).to_string(),
                r.name.clone(),
                r.mode.clone(),
                r.api_format.clone(),
                r.base_url.clone(),
                r.model_id.clone(),
                String::new(),
            ]
        } else {
            default_model_fields()
        };
        self.cfg_cursor = 0;
        self.editing = false;
        self.edit_pos = 0;
        self.touched = vec![false; 7];
        self.model_fields = Vec::new();
        self.result = None;
        log::info!("wizard: reopened via /hiairy");
    }

    // ─────────── 按键处理 ───────────

    /// 处理向导按键；返回 true 表示向导已关闭（完成或跳过）。
    ///
    /// 键位：↑↓ 移动光标 · Enter 确认/编辑/循环 · Tab 循环预设/选项 ·
    /// Esc 跳过/返回 · 编辑态所有可打印字符（含数字 1/2/3）插入字段。
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if !self.active {
            return false;
        }
        match key.code {
            // 编辑态：字符插入优先于一切快捷键（含数字 1/2/3），
            // 插入位置跟随 edit_pos（支持 ← → 移动后继续输入）
            KeyCode::Char(c) if self.editing => {
                let f = self.cfg_cursor;
                if f < self.cfg_fields.len() {
                    let pos = self.edit_pos.min(self.cfg_fields[f].len());
                    self.cfg_fields[f].insert(pos, c);
                    self.edit_pos = pos + 1;
                    self.mark_touched(f);
                }
            }
            KeyCode::Backspace if self.editing => {
                let f = self.cfg_cursor;
                if f < self.cfg_fields.len() && self.edit_pos > 0 {
                    self.edit_pos -= 1;
                    self.cfg_fields[f].remove(self.edit_pos);
                    self.mark_touched(f);
                }
            }
            KeyCode::Left if self.editing => {
                self.edit_pos = self.edit_pos.saturating_sub(1);
            }
            KeyCode::Right if self.editing => {
                let f = self.cfg_cursor;
                if f < self.cfg_fields.len() {
                    let max = self.cfg_fields[f].len();
                    self.edit_pos = (self.edit_pos + 1).min(max);
                }
            }
            KeyCode::Home if self.editing => self.edit_pos = 0,
            KeyCode::End if self.editing => {
                let f = self.cfg_cursor;
                if f < self.cfg_fields.len() {
                    self.edit_pos = self.cfg_fields[f].len();
                }
            }
            KeyCode::Enter if self.editing => {
                // 结束编辑 → 光标下移（文本字段）；提供商字段结束编辑后应用预设
                self.editing = false;
                self.edit_pos = 0;
                let f = self.cfg_cursor;
                self.step_after_field_edit(f);
            }
            KeyCode::Up => self.cursor_move(-1),
            KeyCode::Down => self.cursor_move(1),
            KeyCode::Enter => return self.confirm(),
            KeyCode::Tab => self.cycle_field(),
            KeyCode::Esc => {
                if self.step >= 3 && self.step <= 5 {
                    if self.editing {
                        self.editing = false;
                        self.edit_pos = 0;
                    } else if self.step > 3 {
                        self.step -= 1;
                        self.enter_step_form(self.step);
                    } else {
                        self.step = 2;
                    }
                    return false;
                }
                // Esc 随时跳过整个向导（不持久化，下次启动仍会自动弹出）
                log::info!("wizard: skipped via Esc");
                self.active = false;
                return true;
            }
            _ => {}
        }
        false
    }

    /// 粘贴文本（bracketed paste）：编辑态插入当前字段（含换行剥离）。
    pub fn handle_paste(&mut self, text: &str) {
        if !self.active || !self.editing {
            return;
        }
        let cleaned: String = text
            .chars()
            .filter(|c| !matches!(c, '\n' | '\r'))
            .collect();
        let f = self.cfg_cursor;
        if f >= self.cfg_fields.len() {
            return;
        }
        let pos = self.edit_pos.min(self.cfg_fields[f].len());
        self.cfg_fields[f].insert_str(pos, &cleaned);
        self.edit_pos = pos + cleaned.chars().count();
        self.mark_touched(f);
    }

    /// 文本字段结束编辑后的动作：提供商字段应用预设；其余下移光标。
    fn step_after_field_edit(&mut self, field: usize) {
        if self.step == 3 && field == 0 {
            self.apply_provider_preset();
        }
        if field < self.cfg_fields.len() {
            self.cfg_cursor = (field + 1).min(self.cfg_fields.len());
        }
    }

    /// 标记文本字段被手动编辑（预设自动填充不经过此路径）。
    fn mark_touched(&mut self, field: usize) {
        if field < self.touched.len() {
            self.touched[field] = true;
        }
    }

    /// 移动光标：步骤 1 语言（0..=2）、步骤 2 选项（0..=1）、表单字段。
    fn cursor_move(&mut self, delta: i8) {
        if self.step >= 3 {
            if self.editing {
                return;
            }
            let max = self.cfg_fields.len();
            self.cfg_cursor = if delta < 0 {
                self.cfg_cursor.saturating_sub(1)
            } else {
                (self.cfg_cursor + 1).min(max)
            };
            return;
        }
        let (cur, max) = match self.step {
            1 => (&mut self.lang_cursor, 2),
            _ => (&mut self.config_cursor, 1),
        };
        *cur = if delta < 0 {
            cur.saturating_sub(1)
        } else {
            (*cur + 1).min(max)
        };
    }

    /// 确认当前步骤 / 当前字段。
    fn confirm(&mut self) -> bool {
        match self.step {
            1 => {
                self.effective_lang = match self.lang_cursor {
                    1 => Lang::English,
                    2 => Lang::Chinese,
                    _ => Lang::detect(),
                };
                log::info!("wizard: step1 confirmed, lang={:?}", self.effective_lang);
                self.step = 2;
                false
            }
            2 => {
                if self.config_cursor == 0 {
                    // 快速配置模型 → 步骤 3
                    self.step = 3;
                    self.enter_step_form(3);
                    false
                } else {
                    self.finish(false)
                }
            }
            _ => {
                // 表单步骤：光标在字段上 → 进入编辑或循环切换；在「完成」→ 下一步/完成
                if self.cfg_cursor < self.cfg_fields.len() {
                    if self.field_kind(self.cfg_cursor) == FieldKind::Cycle {
                        self.cycle_field();
                    } else {
                        self.editing = true;
                        // 进入编辑：插入点默认在末尾（长 Key / URL 输入可见尾部）
                        self.edit_pos = self.cfg_fields[self.cfg_cursor].len();
                        self.mark_touched(self.cfg_cursor);
                    }
                    false
                } else {
                    // 完成按钮
                    if self.step < TOTAL_STEPS {
                        if self.step == 3 {
                            // 离开步骤 3 前快照模型基本字段（finish 写 model.yaml 用）
                            self.model_fields = self.cfg_fields.clone();
                        }
                        self.step += 1;
                        self.enter_step_form(self.step);
                        false
                    } else {
                        self.finish(true)
                    }
                }
            }
        }
    }

    /// 进入表单步骤时初始化字段集（预读现有配置填充）。
    fn enter_step_form(&mut self, step: u8) {
        self.cfg_cursor = 0;
        self.editing = false;
        self.edit_pos = 0;
        let m = models_cfg::read_model_yaml();
        match step {
            4 => {
                self.cfg_fields = if let Some(r) = m.rows.first() {
                    vec![
                        if r.context_window.is_empty() { "128k".into() } else { r.context_window.clone() },
                        if r.max_output.is_empty() { "16k".into() } else { r.max_output.clone() },
                        if r.tool_rounds.is_empty() { "1000".into() } else { r.tool_rounds.clone() },
                        if r.vision.is_empty() { "false".into() } else { r.vision.clone() },
                        if r.thinking.is_empty() { "auto".into() } else { r.thinking.clone() },
                    ]
                } else {
                    default_adv_fields()
                };
                self.touched = vec![false; 5];
            }
            5 => {
                let default_model = m.default_model.clone();
                self.cfg_fields = if let Some(t) = &m.think {
                    vec![
                        format!("{}", t.enabled.unwrap_or(true)),
                        if t.slow_model.is_empty() { default_model.clone() } else { t.slow_model.clone() },
                        if t.fast_model.is_empty() { default_model.clone() } else { t.fast_model.clone() },
                        t.prof_model.clone(),
                    ]
                } else {
                    vec![
                        "true".to_string(),
                        default_model.clone(),
                        default_model,
                        String::new(),
                    ]
                };
                self.touched = vec![false; 4];
            }
            _ => {}
        }
    }

    /// 当前字段类型（文本 / 循环）。
    fn field_kind(&self, field: usize) -> FieldKind {
        match self.step {
            3 => match field {
                2 | 3 => FieldKind::Cycle, // mode / api_format
                _ => FieldKind::Text,
            },
            4 => match field {
                0 | 1 | 3 | 4 => FieldKind::Cycle, // context / output / vision / thinking
                _ => FieldKind::Text,
            },
            5 => match field {
                0 => FieldKind::Cycle, // enabled
                _ => FieldKind::Text,
            },
            _ => FieldKind::Text,
        }
    }

    /// 循环切换当前字段（Cycle 字段 Enter/Tab 共用；文本字段 Tab 用预设）。
    fn cycle_field(&mut self) {
        if self.editing {
            return;
        }
        let f = self.cfg_cursor;
        if f >= self.cfg_fields.len() {
            return;
        }
        match self.step {
            3 => match f {
                0 => self.cycle_provider(),
                2 => self.cfg_fields[f] = toggle(&self.cfg_fields[f], &["api", "local"]),
                3 => self.cfg_fields[f] = toggle(&self.cfg_fields[f], &["openai", "anthropic"]),
                5 => self.cycle_model_id(),
                _ => {}
            },
            4 => match f {
                0 => self.cfg_fields[f] = cycle_list(&self.cfg_fields[f], &["128k", "256k", "512k", "1M", "2M"]),
                1 => self.cfg_fields[f] = cycle_list(&self.cfg_fields[f], &["4k", "16k", "32k", "128k", "256k"]),
                3 => self.cfg_fields[f] = toggle(&self.cfg_fields[f], &["true", "false"]),
                4 => self.cfg_fields[f] = cycle_list(&self.cfg_fields[f], &["auto", "on", "off"]),
                _ => {}
            },
            5 => match f {
                0 => self.cfg_fields[f] = toggle(&self.cfg_fields[f], &["true", "false"]),
                _ => {}
            },
            _ => {}
        }
    }

    /// 提供商 Tab 循环（deepseek → openai → … → local → 回到首项）并自动填充。
    fn cycle_provider(&mut self) {
        let cur = self.cfg_fields[0].to_ascii_lowercase();
        let idx = PRESETS
            .iter()
            .position(|p| p.id == cur || p.label.to_ascii_lowercase() == cur);
        let next = match idx {
            Some(i) => PRESETS[(i + 1) % PRESETS.len()].id,
            None => PRESETS[0].id,
        };
        self.cfg_fields[0] = next.to_string();
        self.apply_provider_preset();
    }

    /// 模型 ID Tab 循环（当前提供商预设模型候选，第二项起替换）。
    fn cycle_model_id(&mut self) {
        let p = self.provider_preset();
        let cur = self.cfg_fields[5].trim().to_string();
        if let Some(pos) = p.models.iter().position(|m| *m == cur.as_str()) {
            self.cfg_fields[5] = p.models[(pos + 1) % p.models.len()].to_string();
        } else if cur.is_empty() {
            self.cfg_fields[5] = p.models[0].to_string();
        } else if p.models.contains(&cur.as_str()) {
            // 命中候选但不在当前（不会发生，防御）
        }
    }

    /// 提供商文本 → 预设；未匹配时仅设置 mode/format 默认。
    fn apply_provider_preset(&mut self) {
        let p = self.provider_preset();
        // 连接方式 / 接口格式恒跟随预设（循环字段，易改回）
        self.cfg_fields[2] = p.mode.to_string();
        self.cfg_fields[3] = p.api_format.to_string();
        // 文本字段仅在用户未手改时自动填充（touched 防覆盖）
        if !self.touched[1] {
            self.cfg_fields[1] = p.label.to_string();
        }
        if !self.touched[4] {
            self.cfg_fields[4] = p.base_url.to_string();
        }
        if !self.touched[5] {
            self.cfg_fields[5] = p.models[0].to_string();
        }
    }

    /// 解析当前提供商文本对应的预设（未匹配 → 本地模型兜底？不，用默认 api）。
    fn provider_preset(&self) -> &'static ProviderPreset {
        let cur = self.cfg_fields[0].to_ascii_lowercase();
        PRESETS
            .iter()
            .find(|p| p.id == cur || cur.contains(p.id))
            .unwrap_or(&PRESETS[0])
    }

    // ─────────── 完成与写回 ───────────

    /// 完成向导：跳过（configured=false）或快速配置（configured=true，
    /// 写回 secrets.env + model.yaml + wizard.toml）。
    fn finish(&mut self, configured: bool) -> bool {
        let lang_code = self.effective_lang.code();
        if configured {
            // 模型基本字段：步骤 3 快照优先（步骤 4/5 后 cfg_fields 已被覆盖）；
            // 直接跳到 finish 未经过步骤 4 的场景回落当前字段。
            let mf: &Vec<String> = if self.model_fields.len() == 7 {
                &self.model_fields
            } else {
                &self.cfg_fields
            };
            let provider = mf[0].trim().to_string();
            let name = mf[1].trim().to_string();
            let mode = mf[2].trim().to_string();
            let api_format = mf[3].trim().to_string();
            let base_url = mf[4].trim().to_string();
            let model_id = mf[5].trim().to_string();
            let api_key = mf[6].trim().to_string();
            // 高级选项（步骤 4 已填，若从未进入则保持默认）
            let (context_window, max_output, tool_rounds, vision, thinking) =
                self.adv_values();
            // 双思考（步骤 5 已填，若从未进入则保持模型.yaml 现值或默认）
            let (think_enabled, slow_model, fast_model, prof_model) = self.think_values();

            let api_key_env = if mode == "local" {
                String::new()
            } else {
                "MODEL_1_API_KEY".to_string()
            };
            let api_key_set = if api_key_env.is_empty() || api_key.is_empty() {
                false
            } else {
                write_secret(&api_key_env, &api_key)
            };

            let row = ModelRow {
                name: if name.is_empty() { model_id.clone() } else { name },
                mode,
                api_format,
                base_url,
                model_id: model_id.clone(),
                api_key_env,
                context_window,
                max_output,
                tool_rounds,
                vision,
                thinking,
            };
            let think = ThinkCfg {
                enabled: Some(think_enabled),
                slow_model,
                fast_model,
                prof_model,
                timeout_ms: String::new(),
            };
            if let Err(e) = models_cfg::patch_model_yaml(0, &row, &think) {
                log::warn!("wizard: model.yaml 写回失败: {}", e);
            }

            persist(lang_code, true, &provider, &model_id);
            self.result = Some(WizardResult {
                configured: true,
                model: model_id,
                provider,
                api_key_set,
                think_enabled,
            });
            log::info!(
                "wizard: finished (lang={}, configured=true, api_key_set={}, think={})",
                lang_code,
                api_key_set,
                think_enabled
            );
        } else {
            persist(lang_code, false, "", "");
            self.result = Some(WizardResult {
                configured: false,
                model: String::new(),
                provider: String::new(),
                api_key_set: false,
                think_enabled: false,
            });
            log::info!("wizard: finished (lang={}, configured=false)", lang_code);
        }
        self.active = false;
        true
    }

    /// 步骤 4 高级选项值（未进入步骤 4 时用默认）。
    fn adv_values(&self) -> (String, String, String, String, String) {
        let d = default_adv_fields();
        if self.step >= 4 && self.cfg_fields.len() == 5 {
            (
                self.cfg_fields[0].clone(),
                self.cfg_fields[1].clone(),
                self.cfg_fields[2].clone(),
                self.cfg_fields[3].clone(),
                self.cfg_fields[4].clone(),
            )
        } else {
            (d[0].clone(), d[1].clone(), d[2].clone(), d[3].clone(), d[4].clone())
        }
    }

    /// 步骤 5 双思考值（未进入步骤 5 时读取模型.yaml 现值或默认）。
    fn think_values(&self) -> (bool, String, String, String) {
        if self.step >= 5 && self.cfg_fields.len() == 4 {
            return (
                self.cfg_fields[0] == "true",
                self.cfg_fields[1].trim().to_string(),
                self.cfg_fields[2].trim().to_string(),
                self.cfg_fields[3].trim().to_string(),
            );
        }
        let m = models_cfg::read_model_yaml();
        if let Some(t) = &m.think {
            let d = if m.default_model.is_empty() {
                self.cfg_fields.get(5).cloned().unwrap_or_default()
            } else {
                m.default_model.clone()
            };
            (
                t.enabled.unwrap_or(true),
                if t.slow_model.is_empty() { d.clone() } else { t.slow_model.clone() },
                if t.fast_model.is_empty() { d.clone() } else { t.fast_model.clone() },
                t.prof_model.clone(),
            )
        } else {
            (true, String::new(), String::new(), String::new())
        }
    }
}

// ─────────────────────────── 预设辅助 ───────────────────────────

/// 根据请求地址推断提供商 id（用于 /hiairy 重开时回填提供商字段）。
fn provider_from_base_url(base_url: &str) -> &'static str {
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

/// 循环列表切换（空值 → 首项；命中 → 下一项；未命中 → 保持自定义）。
fn cycle_list(cur: &str, opts: &[&str]) -> String {
    let cur = cur.trim();
    if cur.is_empty() {
        return opts[0].to_string();
    }
    if let Some(pos) = opts.iter().position(|o| *o == cur) {
        opts[(pos + 1) % opts.len()].to_string()
    } else {
        cur.to_string()
    }
}

/// 二值切换（空值 → 首项；命中 → 另一项；未命中 → 保持）。
fn toggle(cur: &str, opts: &[&str]) -> String {
    let cur = cur.trim();
    if let Some(pos) = opts.iter().position(|o| *o == cur) {
        opts[(pos + 1) % opts.len()].to_string()
    } else {
        opts[0].to_string()
    }
}

/// 步骤 3 模型基本字段默认值。
fn default_model_fields() -> Vec<String> {
    vec![
        std::env::var("AIRY_LLM_PROVIDER").unwrap_or_else(|_| "deepseek".to_string()),
        String::new(), // name（由预设填充）
        "api".to_string(),
        "openai".to_string(),
        String::new(), // base_url（由预设填充）
        String::new(), // model_id（由预设填充）
        String::new(), // api_key
    ]
}

/// 步骤 4 高级选项默认值。
fn default_adv_fields() -> Vec<String> {
    vec![
        "128k".to_string(),
        "16k".to_string(),
        "1000".to_string(),
        "false".to_string(),
        "auto".to_string(),
    ]
}

// ─────────────────────────── 持久化 ───────────────────────────

#[derive(Serialize, Deserialize)]
struct WizardConfig {
    lang: String,
    configured: String,
    version: String,
    provider: String,
    model: String,
}

/// 向导目录：$AIRY_HOME/data/agentrt/tui（与 TUI config.toml 同目录约定，
/// 2.3.15 统一运行时数据布局——旧版曾用 $AIRY_HOME/tui，读时自动迁移）。
fn wizard_dir() -> PathBuf {
    let home = if let Ok(h) = std::env::var("AIRY_HOME") {
        h
    } else if let Ok(h) = std::env::var("HOME") {
        format!("{}/.airymaxrt", h)
    } else {
        ".airymaxrt".to_string()
    };
    // 迁移：旧路径 $AIRY_HOME/tui/wizard.toml → 新路径 data/agentrt/tui/
    let legacy = PathBuf::from(&home).join("tui").join(WIZARD_FILE);
    let new_dir = PathBuf::from(&home).join("data").join("agentrt").join("tui");
    if legacy.is_file() && !new_dir.join(WIZARD_FILE).exists() {
        if std::fs::create_dir_all(&new_dir).is_ok() {
            let _ = std::fs::rename(&legacy, new_dir.join(WIZARD_FILE));
        }
    }
    new_dir
}

/// 运行配置目录：$AIRY_HOME/config（secrets.env 所在目录）
fn config_dir() -> PathBuf {
    if let Ok(home) = std::env::var("AIRY_HOME") {
        return PathBuf::from(home).join("config");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".airymaxrt").join("config");
    }
    PathBuf::from(".airymaxrt").join("config")
}

fn config_path() -> PathBuf {
    wizard_dir().join(WIZARD_FILE)
}

fn is_first_run() -> bool {
    !config_path().exists()
}

/// 将 API Key 写回 $AIRY_HOME/config/secrets.env（llm_d 热加载，无需重启）。
///
/// 已有同名变量行 → 原位替换值；无则追加到文件末尾。失败返回 false（不阻断向导）。
fn write_secret(env_name: &str, value: &str) -> bool {
    if env_name.is_empty() || value.is_empty() {
        return false;
    }
    let path = config_dir().join("secrets.env");
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("wizard: create config dir failed: {}", e);
            return false;
        }
    }
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let marker = format!("{}=", env_name);
    let mut lines: Vec<String> = original.lines().map(|l| l.to_string()).collect();
    let line = format!("{}{}", marker, value);
    let mut replaced = false;
    for l in lines.iter_mut() {
        let trimmed = l.trim_start();
        if trimmed.starts_with(&marker) && !trimmed.starts_with('#') {
            *l = line.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        if !original.ends_with('\n') && !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(line);
    }
    let content = lines.join("\n") + "\n";
    if std::fs::write(&path, content).is_ok() {
        log::info!("wizard: API Key written to secrets.env ({}={})", env_name,
                   if value.len() > 4 { format!("sk-…{}", &value[value.len() - 4..]) } else { "***".to_string() });
        true
    } else {
        log::warn!("wizard: write secrets.env failed");
        false
    }
}

/// 将选择写回 wizard.toml（lang + configured + version + provider + model）。
fn persist(lang: &str, configured: bool, provider: &str, model: &str) {
    let cfg = WizardConfig {
        lang: lang.to_string(),
        configured: if configured {
            "manual".to_string()
        } else {
            "skipped".to_string()
        },
        version: env!("AIRY_RT_VERSION").to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
    };
    if let Some(parent) = config_path().parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("wizard: create dir failed: {}", e);
            return;
        }
    }
    match toml::to_string(&cfg) {
        Ok(s) => {
            if let Err(e) = std::fs::write(config_path(), s) {
                log::warn!("wizard: persist failed: {}", e);
            } else {
                log::info!("wizard: config saved to {}", config_path().display());
            }
        }
        Err(e) => log::warn!("wizard: serialize failed: {}", e),
    }
}

// ─────────────────────────── 渲染 ───────────────────────────

/// 全屏渲染向导（首次运行 / /hiairy 时由 ui.rs 接管整个终端）。
pub fn render(f: &mut Frame, area: Rect, w: &WizardState) {
    // 全屏填充背景：确保居中视图外无任何残留（视觉干净，居中明确）
    f.render_widget(
        Paragraph::new(Text::raw("")).style(Style::default().bg(theme::bg())),
        area,
    );

    let width = (area.width as usize).min(78).max(24);
    let mut lines: Vec<Line> = Vec::new();

    match w.step {
        1 => build_step1(&mut lines, w, width),
        3 => build_step3(&mut lines, w, width),
        4 => build_step4(&mut lines, w, width),
        5 => build_step5(&mut lines, w, width),
        _ => build_step2(&mut lines, w, width),
    }
    push_footer(&mut lines, w.step, width);

    // 垂直居中；水平居中裁剪到 width 列
    let pad_top = area.height.saturating_sub(lines.len() as u16) / 2;
    let body = Rect {
        x: area.x + (area.width.saturating_sub(width as u16)) / 2,
        y: area.y.saturating_add(pad_top),
        width: width as u16,
        height: area.height.saturating_sub(pad_top),
    };
    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(theme::bg())),
        body,
    );
}

/// 步骤 1：欢迎 + 版本 + 界面语言选择（中英双语展示）。
fn build_step1(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let detected = Lang::detect();
    let detected_hint = match detected {
        Lang::Chinese => "简体中文",
        _ => "English",
    };

    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered("首次启动向导 · First-run Setup", width));
    lines.push(centered(&format!("步骤 1/{} · Step 1 of {}", TOTAL_STEPS, TOTAL_STEPS), width));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered(
        &format!("欢迎使用 AirymaxRT v{}", env!("AIRY_RT_VERSION")),
        width,
    ));
    lines.push(centered("AI Agent 运行时平台 · AI Agent Runtime Platform", width));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered("请选择界面语言 · Choose language:", width));
    lines.push(centered(&format!("当前检测到 · Detected: {}", detected_hint), width));
    lines.push(Line::raw(""));

    let options = [Lang::Auto, Lang::English, Lang::Chinese];
    for (i, lang) in options.iter().enumerate() {
        let selected = i == w.lang_cursor;
        let label = match lang {
            Lang::Auto => format!("{}  [推荐]", lang.label()),
            _ => lang.label().to_string(),
        };
        lines.push(option_line(selected, &label));
    }
}

/// 步骤 2：想怎么开始？（文案随步骤 1 所选语言切换）
fn build_step2(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang == Lang::Chinese;

    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered(if zh { "首次启动向导" } else { "First-run Setup" }, width));
    lines.push(centered(
        &format!("步骤 2/{} · Step 2 of {}", TOTAL_STEPS, TOTAL_STEPS),
        width,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered(
        if zh { "想怎么开始？" } else { "How would you like to start?" },
        width,
    ));
    lines.push(Line::raw(""));

    let opts: [(&str, &str); 2] = if zh {
        [
            ("快速配置模型", "选提供商填 Key，自动匹配请求地址与模型，立即开始对话"),
            ("跳过，先进 TUI 探索", "直接进入对话，随时可用 /hiairy 重开向导"),
        ]
    } else {
        [
            ("Quick model setup", "Pick a provider & paste a key; URL and models fill in automatically"),
            ("Skip — explore the TUI first", "Jump into chat; reopen anytime with /hiairy"),
        ]
    };

    for (i, (label, desc_text)) in opts.iter().enumerate() {
        let selected = i == w.config_cursor;
        lines.push(option_line(selected, label));
        for dl in desc_lines(desc_text, width) {
            lines.push(dl);
        }
        lines.push(Line::raw(""));
    }
}

/// 步骤 3：模型基本配置（v2 表格第 1 行）。
fn build_step3(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang == Lang::Chinese;
    step_header(lines, w, 3, if zh { "模型基本配置" } else { "Model Setup" }, width);
    lines.push(centered(
        if zh { "选择提供商后地址/格式/模型自动填充，只需粘贴 API Key" }
        else { "Pick a provider — URL, format & models fill in automatically" },
        width,
    ));
    lines.push(Line::raw(""));

    let labels: [&str; 7] = if zh {
        ["提供商", "名称", "连接方式", "接口格式", "请求地址", "模型 ID", "API Key"]
    } else {
        ["Provider", "Name", "Mode", "API Format", "Base URL", "Model ID", "API Key"]
    };
    let hints: [&str; 7] = if zh {
        [
            "deepseek / openai / glm / qwen / kimi / local …（Tab 循环预设）",
            "展示名称（默认取提供商名，可改）",
            "api = 云端 API · local = 本地模型（Enter 切换）",
            "openai = OpenAI 兼容 · anthropic = Anthropic Messages（Enter 切换）",
            "完整请求地址（不含 /chat/completions 后缀）",
            "实际调用的模型名（Tab 循环候选）",
            "本地模型无需填写",
        ]
    } else {
        [
            "deepseek / openai / glm / qwen / kimi / local … (Tab cycles presets)",
            "Display name (defaults to the provider name)",
            "api = cloud API · local = local model (Enter toggles)",
            "openai = OpenAI-compatible · anthropic = Anthropic Messages (Enter toggles)",
            "Full base URL (without /chat/completions suffix)",
            "Model id actually used (Tab cycles candidates)",
            "Leave empty for local models",
        ]
    };

    let mode_local = w.cfg_fields[2].trim() == "local";
    for i in 0..7 {
        if i == 6 && mode_local {
            continue; // 本地模型不展示 API Key 字段
        }
        form_field_line(lines, w, i, labels[i], hints[i], zh, w.cfg_cursor == i && w.editing);
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(option_line(
        w.cfg_cursor >= w.cfg_fields.len(),
        if zh { "下一步：高级选项" } else { "Next: Advanced options" },
    ));
}

/// 步骤 4：高级选项。
fn build_step4(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang == Lang::Chinese;
    step_header(lines, w, 4, if zh { "高级选项（可全部保持默认）" } else { "Advanced Options" }, width);
    lines.push(Line::raw(""));

    let labels: [&str; 5] = if zh {
        ["上下文窗口", "最大输出", "工具轮数", "图片输入", "思考模式"]
    } else {
        ["Context window", "Max output", "Tool rounds", "Vision", "Thinking"]
    };
    let hints: [&str; 5] = if zh {
        [
            "128k / 256k / 512k / 1M / 2M（Tab 循环）",
            "4k / 16k / 32k / 128k / 256k（Tab 循环）",
            "工具调用轮数上限，默认 1000",
            "true / false（Enter 切换）",
            "auto = 跟随模型默认 · on / off（Tab 循环）",
        ]
    } else {
        [
            "128k / 256k / 512k / 1M / 2M (Tab cycles)",
            "4k / 16k / 32k / 128k / 256k (Tab cycles)",
            "Max tool call rounds, default 1000",
            "true / false (Enter toggles)",
            "auto = follow model default · on / off (Tab cycles)",
        ]
    };

    for i in 0..5 {
        form_field_line(lines, w, i, labels[i], hints[i], zh, w.cfg_cursor == i && w.editing);
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(option_line(
        w.cfg_cursor >= w.cfg_fields.len(),
        if zh { "下一步：双思考系统" } else { "Next: Dual Thinking" },
    ));
}

/// 步骤 5：双思考系统。
fn build_step5(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang == Lang::Chinese;
    step_header(lines, w, 5, if zh { "双思考系统" } else { "Dual Thinking System" }, width);
    lines.push(centered(
        if zh {
            "慢思考组织逻辑 → 快思考快速校验 → 专业思考专家终裁"
        } else {
            "Slow think plans → Fast think verifies → Professional think finalizes"
        },
        width,
    ));
    lines.push(Line::raw(""));

    let labels: [&str; 4] = if zh {
        ["启用双思考", "慢思考模型", "快思考模型", "专业思考模型"]
    } else {
        ["Enable", "Slow model (t2)", "Fast model (t1-f)", "Pro model (t1-p)"]
    };
    let hints: [&str; 4] = if zh {
        [
            "true / false（Enter 切换；关闭则退化为单轮普通计划）",
            "组织逻辑、制定任务图纸（建议用最强模型）",
            "对计划快速终裁（建议用轻快模型）",
            "专家校验，留空 = 使用默认模型",
        ]
    } else {
        [
            "true / false (Enter toggles; off degrades to single-pass planning)",
            "Plans logic & blueprints (use your strongest model)",
            "Quickly finalizes the plan (use a fast model)",
            "Expert review; empty = default model",
        ]
    };

    for i in 0..4 {
        form_field_line(lines, w, i, labels[i], hints[i], zh, w.cfg_cursor == i && w.editing);
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(option_line(
        w.cfg_cursor >= w.cfg_fields.len(),
        if zh { "完成配置，开始对话" } else { "Finish & start chatting" },
    ));
}

/// 表单步骤标题（◈ 标志 + 标题 + 步骤计数）。
fn step_header(lines: &mut Vec<Line>, _w: &WizardState, step: u8, title: &str, width: usize) {
    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered(
        &format!("首次启动向导 · {}（步骤 {}/{}）", title, step, TOTAL_STEPS),
        width,
    ));
    lines.push(Line::raw(""));
}

/// 单个表单字段行：`▸ 标签[值]` + 编辑光标 + 说明。
fn form_field_line(
    lines: &mut Vec<Line>,
    w: &WizardState,
    idx: usize,
    label: &str,
    hint: &str,
    zh: bool,
    editing: bool,
) {
    let selected = w.cfg_cursor == idx;
    let (marker, mstyle) = if selected {
        ("▸", Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD))
    } else {
        (" ", Style::default().fg(theme::dim()))
    };
    let raw = w.cfg_fields.get(idx).cloned().unwrap_or_default();
    let value = if editing {
        // 编辑态：窗口跟随插入点滚动——长文本（API Key / base_url）不截断，
        // 始终可见光标附近内容；窗口内以 ▍ 标示插入点。
        let max_vis = 48usize;
        let len = raw.chars().count();
        let pos = w.edit_pos.min(len);
        let mut prefix_ell = false;
        let mut start = pos.saturating_sub(max_vis / 2);
        if start > 0 {
            start = pos.saturating_sub(max_vis - 3);
            prefix_ell = true;
        }
        let vis: String = raw.chars().skip(start).take(max_vis).collect();
        let inner_pos = pos.saturating_sub(start);
        let head = &vis[..char_boundary(&vis, inner_pos)];
        let tail = &vis[char_boundary(&vis, inner_pos)..];
        if prefix_ell {
            format!("…{}▍{}", head, tail)
        } else {
            format!("{}▍{}", head, tail)
        }
    } else if idx == 6 && !raw.is_empty() {
        // API Key 掩码：非编辑态仅显示尾 4 位
        if raw.len() > 4 {
            format!("{}…{}", "•".repeat(12), &raw[raw.len() - 4..])
        } else {
            "•".repeat(raw.len())
        }
    } else {
        raw
    };
    let value_style = if editing {
        Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD)
    } else if value.is_empty() {
        Style::default().fg(theme::faint())
    } else {
        Style::default().fg(theme::text())
    };
    let label_w = if zh { 10 } else { 12 };
    lines.push(Line::from(vec![
        Span::styled(format!("    {} ", marker), mstyle.clone()),
        Span::styled(
            format!("{:<w$}", label, w = label_w),
            Style::default().fg(if selected { theme::primary() } else { theme::dim() }),
        ),
        Span::styled(format!("[{}]", value), value_style),
    ]));
    if !editing {
        for dl in desc_lines(hint, 64) {
            lines.push(dl);
        }
    }
}

fn push_footer(lines: &mut Vec<Line>, step: u8, width: usize) {
    lines.push(Line::raw(""));
    if step >= 3 {
        lines.push(centered(
            "↑↓ 选择字段 · Enter 编辑/切换 · ←→ 移动插入点 · Tab 循环 · 支持粘贴 · Esc 返回",
            width,
        ));
        lines.push(centered(
            "↑↓ Select · Enter Edit/Toggle · ←→ Move cursor · Tab Cycles · Paste ok · Esc Back",
            width,
        ));
    } else {
        lines.push(centered("↑↓ 移动 · 1-3 直达 · Enter 确认 · Esc 跳过", width));
        lines.push(centered("↑↓ Move · 1-3 Select · Enter Confirm · Esc Skip", width));
    }
}

/// 字符位置 → 字节边界（防止多字节字符切片 panic；越界返回 len）。
fn char_boundary(s: &str, char_idx: usize) -> usize {
    let mut b = 0;
    for (i, ch) in s.char_indices() {
        if i == char_idx {
            return b;
        }
        b = i + ch.len_utf8();
    }
    s.len()
}

/// 水平居中（按 unicode 显示宽度补空格）
fn centered(text: &str, width: usize) -> Line<'static> {
    let w = text.width();
    let pad = width.saturating_sub(w) / 2;
    Line::from(format!("{}{}", " ".repeat(pad), text))
}

/// 选项行：选中 → ▸ 晶蓝加粗；未选中 → 灰暗
fn option_line(selected: bool, text: &str) -> Line<'static> {
    let (marker, style) = if selected {
        (
            "▸",
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        )
    } else {
        (" ", Style::default().fg(theme::dim()))
    };
    Line::from(vec![
        Span::styled(format!("    {} ", marker), style),
        Span::styled(text.to_string(), style),
    ])
}

/// 选项说明行（灰暗、缩进对齐选项文字；超长按显示宽度换行）
fn desc_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for piece in wrap_text(text, width.saturating_sub(8).max(10)) {
        out.push(Line::from(Span::styled(
            format!("      {}", piece),
            Style::default().fg(theme::faint()),
        )));
    }
    if out.is_empty() {
        out.push(Line::raw(""));
    }
    out
}

/// 按 unicode 显示宽度换行（CJK 双列，空格处断行）
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + w > width && !cur.is_empty() {
            out.push(cur);
            cur = String::new();
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_resolves_by_id() {
        let mut w = WizardState {
            active: true,
            step: 3,
            lang_cursor: 0,
            config_cursor: 0,
            effective_lang: Lang::Chinese,
            cfg_fields: default_model_fields(),
            model_fields: Vec::new(),
            cfg_cursor: 0,
            editing: false,
            edit_pos: 0,
            touched: vec![false; 7],
            result: None,
        };
        w.cfg_fields[0] = "qwen".to_string();
        w.apply_provider_preset();
        assert_eq!(w.cfg_fields[3], "openai");
        assert!(w.cfg_fields[4].contains("dashscope"));
        assert_eq!(w.cfg_fields[5], "qwen-max");
        // 手改 model_id 后不覆盖
        w.cfg_fields[5] = "custom-model".to_string();
        w.touched[5] = true;
        w.cfg_fields[0] = "deepseek".to_string();
        w.apply_provider_preset();
        assert_eq!(w.cfg_fields[5], "custom-model");
    }

    #[test]
    fn cycle_helpers() {
        assert_eq!(cycle_list("", &["a", "b"]), "a");
        assert_eq!(cycle_list("a", &["a", "b"]), "b");
        assert_eq!(cycle_list("b", &["a", "b"]), "a");
        assert_eq!(cycle_list("custom", &["a", "b"]), "custom");
        assert_eq!(toggle("true", &["true", "false"]), "false");
    }

    /// 模拟按键（回车 / 下 / Tab / 字符 / 退格）
    fn k(code: KeyCode) -> KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn full_wizard_flow_reaches_finish() {
        // finish() 会写 secrets.env / model.yaml / wizard.toml → 隔离到临时目录
        let tmp = std::env::temp_dir().join(format!("airymaxrt-wiz-test-{}", std::process::id()));
        std::env::set_var("AIRY_HOME", &tmp);
        let _ = std::fs::create_dir_all(tmp.join("config"));
        let _ = std::fs::create_dir_all(tmp.join("tui"));

        let mut w = WizardState {
            active: true,
            step: 1,
            lang_cursor: 0,
            config_cursor: 0,
            effective_lang: Lang::Chinese,
            cfg_fields: default_model_fields(),
            model_fields: Vec::new(),
            cfg_cursor: 0,
            editing: false,
            edit_pos: 0,
            touched: vec![false; 7],
            result: None,
        };
        // 步骤 1 → 2（Enter 确认语言）
        assert!(!w.handle_key(&k(KeyCode::Enter)));
        assert_eq!(w.step, 2);
        // 步骤 2 → 3（Enter 快速配置）
        assert!(!w.handle_key(&k(KeyCode::Enter)));
        assert_eq!(w.step, 3);
        assert_eq!(w.cfg_fields.len(), 7);
        // 提供商字段：清空默认值后编辑输入 "qwen" → Enter 确认应用预设
        w.handle_key(&k(KeyCode::Enter)); // 进入编辑
        for _ in 0.."deepseek".len() {
            w.handle_key(&k(KeyCode::Backspace)); // 清空默认
        }
        for c in "qwen".chars() {
            w.handle_key(&k(KeyCode::Char(c)));
        }
        w.handle_key(&k(KeyCode::Enter)); // 确认 → 应用预设
        assert!(w.cfg_fields[4].contains("dashscope"), "base_url 自动填充");
        assert_eq!(w.cfg_fields[5], "qwen-max", "model_id 自动填充");
        // 光标已移到字段 1；Down 到字段 6（API Key）
        for _ in 0..5 {
            w.handle_key(&k(KeyCode::Down));
        }
        assert_eq!(w.cfg_cursor, 6);
        // 编辑 API Key（含数字）
        w.handle_key(&k(KeyCode::Enter));
        for c in "sk-abc123".chars() {
            w.handle_key(&k(KeyCode::Char(c)));
        }
        assert_eq!(w.cfg_fields[6], "sk-abc123", "API Key 数字可输入");
        w.handle_key(&k(KeyCode::Enter)); // 确认 → 光标到 7
        assert_eq!(w.cfg_cursor, 7);
        // Enter → 步骤 4 高级
        assert!(!w.handle_key(&k(KeyCode::Enter)));
        assert_eq!(w.step, 4);
        assert_eq!(w.cfg_fields.len(), 5);
        // 高级选项全部默认 → 跳到完成（光标 5）→ 步骤 5
        for _ in 0..5 {
            w.handle_key(&k(KeyCode::Down));
        }
        assert_eq!(w.cfg_cursor, 5);
        assert!(!w.handle_key(&k(KeyCode::Enter)));
        assert_eq!(w.step, 5);
        assert_eq!(w.cfg_fields.len(), 4);
        // 双思考 → 完成
        for _ in 0..4 {
            w.handle_key(&k(KeyCode::Down));
        }
        assert!(w.handle_key(&k(KeyCode::Enter)), "向导应完成");
        assert_eq!(w.active, false);
        let r = w.result.expect("result set");
        assert_eq!(r.configured, true);
        assert_eq!(r.model, "qwen-max");
    }

    #[test]
    fn esc_skips_wizard() {
        let mut w = WizardState {
            active: true,
            step: 1,
            lang_cursor: 0,
            config_cursor: 0,
            effective_lang: Lang::English,
            cfg_fields: default_model_fields(),
            model_fields: Vec::new(),
            cfg_cursor: 0,
            editing: false,
            edit_pos: 0,
            touched: vec![false; 7],
            result: None,
        };
        assert!(w.handle_key(&k(KeyCode::Esc)));
        assert!(!w.active);
    }
}
