// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 首次启动向导（极简 2 步，蓝晶风格）。
//
// 流程：
//   步骤 1/2：欢迎 + 版本 + 界面语言选择（自动检测 LC_ALL/LANG / English / 简体中文）
//   步骤 2/2：想怎么开始？（手动配置模型 / 跳过，先进 TUI 探索）
//
// 触发：
//   - 首次运行（$AIRY_HOME/tui/wizard.toml 不存在）自动弹出；
//   - 对话中输入 /hiairy 随时重开。
//
// 完成后的选择写回 wizard.toml（lang + configured），下次启动不再弹出。
// 参考 atomcode 首次向导模式（语言检测 + 键位），但不照抄：保持 2 步极简结构，
// 视觉统一走「极简蓝晶」设计令牌（theme.rs），步骤 2 文案随所选语言切换。

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

use crate::theme;

/// 向导配置文件（$AIRY_HOME/tui/wizard.toml），存在即非首次运行。
const WIZARD_FILE: &str = "wizard.toml";

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

/// 向导完成结果（供 App 使用）
#[derive(Debug, Clone)]
pub struct WizardResult {
    /// true = 完成快速配置；false = 跳过先探索
    pub configured: bool,
    /// 设置的默认模型名（空 = 未设置）
    pub model: String,
    /// 选择的提供商（空 = 未设置）
    pub provider: String,
    /// API Key 是否已写入 secrets.env
    pub api_key_set: bool,
}

/// 首次启动向导状态
pub struct WizardState {
    /// 是否激活（首次运行自动激活；/hiairy 可随时重开）
    pub active: bool,
    /// 当前步骤（1 | 2 | 3）
    pub step: u8,
    /// 步骤 1 语言选项光标（0=自动检测, 1=English, 2=简体中文）
    pub lang_cursor: usize,
    /// 步骤 2 配置选项光标（0=快速配置模型, 1=跳过）
    pub config_cursor: usize,
    /// 步骤 1 确认后的实际语言（驱动步骤 2/3 文案）
    pub effective_lang: Lang,
    /// 步骤 3 配置字段（0=提供商, 1=API Key, 2=默认模型）
    pub cfg_fields: [String; 3],
    /// 步骤 3 字段/完成光标（0..=3，3 = 完成配置）
    pub cfg_cursor: usize,
    /// 步骤 3 是否正在编辑当前字段
    pub editing: bool,
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
            cfg_fields: default_cfg_fields(),
            cfg_cursor: 0,
            editing: false,
            result: None,
        }
    }

    /// 重新打开向导（/hiairy 调用；即使已完成也允许重开）。
    pub fn reopen(&mut self) {
        self.active = true;
        self.step = 1;
        self.lang_cursor = 0;
        self.config_cursor = 0;
        self.effective_lang = Lang::detect();
        self.cfg_fields = default_cfg_fields();
        self.cfg_cursor = 0;
        self.editing = false;
        self.result = None;
        log::info!("wizard: reopened via /hiairy");
    }

    /// 处理向导按键；返回 true 表示向导已关闭（完成或跳过）。
    ///
    /// 键位：↑↓ 移动光标 · 1-3 数字直达 · Enter 确认/编辑 · Esc 跳过/返回。
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if !self.active {
            return false;
        }
        match key.code {
            KeyCode::Up => self.cursor_move(-1),
            KeyCode::Down => self.cursor_move(1),
            KeyCode::Char('1') => self.cursor_jump(0),
            KeyCode::Char('2') => self.cursor_jump(1),
            KeyCode::Char('3') => self.cursor_jump(2),
            KeyCode::Enter => return self.confirm(),
            KeyCode::Esc => {
                if self.step == 3 {
                    // 步骤 3：编辑中先取消编辑，否则返回步骤 2
                    if self.editing {
                        self.editing = false;
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
            KeyCode::Backspace => {
                if self.step == 3 && self.editing {
                    self.cfg_fields[self.cfg_cursor].pop();
                }
            }
            KeyCode::Char(c) => {
                if self.step == 3 && self.editing && self.cfg_cursor < 3 {
                    self.cfg_fields[self.cfg_cursor].push(c);
                }
            }
            _ => {}
        }
        false
    }

    /// 移动光标：步骤 1 语言（0..=2）、步骤 2 选项（0..=1）、步骤 3 字段（0..=3）。
    fn cursor_move(&mut self, delta: i8) {
        if self.step == 3 && self.editing {
            return;
        }
        let (cur, max) = match self.step {
            1 => (&mut self.lang_cursor, 2),
            2 => (&mut self.config_cursor, 1),
            _ => (&mut self.cfg_cursor, 3),
        };
        *cur = if delta < 0 {
            cur.saturating_sub(1)
        } else {
            (*cur + 1).min(max)
        };
    }

    /// 数字直达：步骤 3 时 1-3 选中字段（编辑态忽略）。
    fn cursor_jump(&mut self, idx: usize) {
        if self.step == 3 {
            if !self.editing && idx < 3 {
                self.cfg_cursor = idx;
            }
            return;
        }
        let (cur, max) = match self.step {
            1 => (&mut self.lang_cursor, 2),
            _ => (&mut self.config_cursor, 1),
        };
        if idx <= max {
            *cur = idx;
        }
    }

    /// 确认当前步骤：步骤 1 → 步骤 2；步骤 2 → 快速配置（步骤 3）或完成；
    /// 步骤 3 → 编辑当前字段或完成配置。
    fn confirm(&mut self) -> bool {
        match self.step {
            1 => {
                // Auto → 解析为具体语言；English / 简体中文 直接采用
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
                    self.cfg_cursor = 0;
                    self.editing = false;
                    false
                } else {
                    self.finish(false)
                }
            }
            _ => {
                // 步骤 3：光标在字段上 → 进入/结束编辑；在「完成」→ 完成配置
                if self.editing {
                    self.editing = false;
                    if self.cfg_cursor < 3 {
                        self.cfg_cursor = (self.cfg_cursor + 1).min(3);
                    }
                    return false;
                }
                if self.cfg_cursor < 3 {
                    self.editing = true;
                    false
                } else {
                    self.finish(true)
                }
            }
        }
    }

    /// 完成向导：跳过（configured=false）或快速配置（configured=true，写回 secrets.env）。
    fn finish(&mut self, configured: bool) -> bool {
        let lang_code = self.effective_lang.code();
        if configured {
            let provider = self.cfg_fields[0].trim().to_string();
            let api_key = self.cfg_fields[1].trim().to_string();
            let model = self.cfg_fields[2].trim().to_string();
            let api_key_set = if api_key.is_empty() {
                false
            } else {
                let env_name = api_key_env_for(&provider);
                write_secret(&env_name, &api_key)
            };
            persist(lang_code, true, &provider, &model);
            self.result = Some(WizardResult {
                configured: true,
                model,
                provider,
                api_key_set,
            });
            log::info!(
                "wizard: finished (lang={}, configured=true, api_key_set={})",
                lang_code,
                api_key_set
            );
        } else {
            persist(lang_code, false, "", "");
            self.result = Some(WizardResult {
                configured: false,
                model: String::new(),
                provider: String::new(),
                api_key_set: false,
            });
            log::info!("wizard: finished (lang={}, configured=false)", lang_code);
        }
        self.active = false;
        true
    }
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

/// 向导目录：$AIRY_HOME/tui → ~/.airymaxrt/tui（与 memory/skills 同目录约定）
fn wizard_dir() -> PathBuf {
    if let Ok(home) = std::env::var("AIRY_HOME") {
        return PathBuf::from(home).join("tui");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".airymaxrt").join("tui");
    }
    PathBuf::from(".airymaxrt").join("tui")
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

/// 步骤 3 配置字段默认值（与 model.yaml global.default_provider/default_model 对齐）。
fn default_cfg_fields() -> [String; 3] {
    [
        std::env::var("AIRY_LLM_PROVIDER").unwrap_or_else(|_| "deepseek".to_string()),
        String::new(),
        std::env::var("AIRY_AGENT_MODEL")
            .unwrap_or_else(|_| "deepseek-v4-flash".to_string()),
    ]
}

/// 提供商 → secrets.env 中的 API Key 变量名（与 model.yaml providers[].api_key_env 对齐）。
fn api_key_env_for(provider: &str) -> &str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => "OPENAI_API_KEY",
        "anthropic" | "claude" => "ANTHROPIC_API_KEY",
        "glm" | "zhipu" | "bigmodel" => "GLM_API_KEY",
        "qwen" | "tongyi" | "dashscope" => "DASHSCOPE_API_KEY",
        "moonshot" | "kimi" => "MOONSHOT_API_KEY",
        "siliconflow" => "SILICONFLOW_API_KEY",
        "spark" | "xinghuo" => "SPARK_API_KEY",
        "local" | "ollama" => "",
        "custom" => "CUSTOM_LLM_API_KEY",
        _ => "DEEPSEEK_API_KEY",
    }
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
        // 追加到文件末尾（保留注释与已有内容）
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

    let width = (area.width as usize).min(74).max(20);
    let mut lines: Vec<Line> = Vec::new();

    match w.step {
        1 => build_step1(&mut lines, w, width),
        3 => build_step3(&mut lines, w, width),
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
    lines.push(centered("步骤 1/2 · Step 1 of 2", width));
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
    lines.push(centered(if zh { "步骤 2/3" } else { "Step 2 of 3" }, width));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered(
        if zh { "想怎么开始？" } else { "How would you like to start?" },
        width,
    ));
    lines.push(Line::raw(""));

    let opts: [(&str, &str); 2] = if zh {
        [
            ("快速配置模型", "输入提供商、API Key 与默认模型，立即开始对话"),
            ("跳过，先进 TUI 探索", "直接进入对话，随时可用 /hiairy 重开向导"),
        ]
    } else {
        [
            ("Quick model setup", "Enter provider, API key & default model to start chatting now"),
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

/// 步骤 3：快速配置模型表单（提供商 / API Key / 默认模型 + 完成）。
fn build_step3(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang == Lang::Chinese;

    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered(
        if zh { "首次启动向导 · 快速配置模型" } else { "First-run Setup · Configure Model" },
        width,
    ));
    lines.push(centered(if zh { "步骤 3/3" } else { "Step 3 of 3" }, width));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));

    // 三个配置字段（provider / api_key / model）
    let labels: [&str; 3] = if zh {
        ["模型提供商", "API Key", "默认模型"]
    } else {
        ["Provider", "API Key", "Default model"]
    };
    let hints: [&str; 3] = if zh {
        ["deepseek / openai / glm / qwen / …", "粘贴 API Key（写入 secrets.env）", "对话默认使用的模型名"]
    } else {
        ["deepseek / openai / glm / qwen / …", "Paste API key (saved to secrets.env)", "Default model used in chat"]
    };

    for i in 0..3 {
        let selected = w.cfg_cursor == i;
        let editing = selected && w.editing;
        let (marker, mstyle) = if selected {
            (
                "▸",
                Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
            )
        } else {
            (" ", Style::default().fg(theme::dim()))
        };
        let value = if editing {
            format!("{}▍", w.cfg_fields[i])
        } else if w.cfg_fields[i].is_empty() {
            if i == 2 && !selected {
                // 默认模型空 → 显示回落提示
                "（默认）".to_string()
            } else {
                String::new()
            }
        } else {
            w.cfg_fields[i].clone()
        };
        let value_style = if editing {
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if w.cfg_fields[i].is_empty() {
            Style::default().fg(theme::faint())
        } else {
            Style::default().fg(theme::text())
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    {} ", marker), mstyle.clone()),
            Span::styled(
                format!("{:<6}", labels[i]),
                Style::default().fg(if selected { theme::PRIMARY } else { theme::dim() }),
            ),
            Span::styled(format!("[{}]", value), value_style),
        ]));
        if !editing {
            // 非编辑态展示字段提示（灰暗小字）
            for dl in desc_lines(hints[i], width) {
                lines.push(dl);
            }
        }
        if i < 2 {
            lines.push(Line::raw(""));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(option_line(
        w.cfg_cursor == 3,
        if zh {
            "完成配置，开始对话"
        } else {
            "Finish & start chatting"
        },
    ));
}

fn push_footer(lines: &mut Vec<Line>, step: u8, width: usize) {
    lines.push(Line::raw(""));
    if step == 3 {
        lines.push(centered("↑↓ 选择字段 · Enter 编辑/确认 · Esc 返回", width));
        lines.push(centered("↑↓ Select · Enter Edit/Confirm · Esc Back", width));
    } else {
        lines.push(centered("↑↓ 移动 · 1-3 直达 · Enter 确认 · Esc 跳过", width));
        lines.push(centered("↑↓ Move · 1-3 Select · Enter Confirm · Esc Skip", width));
    }
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
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
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
    for piece in wrap_text(text, width.saturating_sub(6).max(10)) {
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
