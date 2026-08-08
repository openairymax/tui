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
#[derive(Debug, Clone, Copy)]
pub struct WizardResult {
    /// true = 选择「手动配置模型」，false = 跳过先探索
    pub configured: bool,
}

/// 首次启动向导状态
pub struct WizardState {
    /// 是否激活（首次运行自动激活；/hiairy 可随时重开）
    pub active: bool,
    /// 当前步骤（1 | 2）
    pub step: u8,
    /// 步骤 1 语言选项光标（0=自动检测, 1=English, 2=简体中文）
    pub lang_cursor: usize,
    /// 步骤 2 配置选项光标（0=手动配置模型, 1=跳过）
    pub config_cursor: usize,
    /// 步骤 1 确认后的实际语言（驱动步骤 2 文案）
    pub effective_lang: Lang,
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
        self.result = None;
        log::info!("wizard: reopened via /hiairy");
    }

    /// 处理向导按键；返回 true 表示向导已关闭（完成或跳过）。
    ///
    /// 键位：↑↓ 移动光标 · 1-3 数字直达 · Enter 确认 · Esc 跳过。
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if !self.active {
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if self.step == 1 {
                    self.lang_cursor = self.lang_cursor.saturating_sub(1);
                } else {
                    self.config_cursor = self.config_cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.step == 1 {
                    self.lang_cursor = (self.lang_cursor + 1).min(2);
                } else {
                    self.config_cursor = (self.config_cursor + 1).min(1);
                }
            }
            KeyCode::Char('1') => {
                if self.step == 1 {
                    self.lang_cursor = 0;
                } else {
                    self.config_cursor = 0;
                }
            }
            KeyCode::Char('2') => {
                if self.step == 1 {
                    self.lang_cursor = 1;
                } else {
                    self.config_cursor = 1;
                }
            }
            KeyCode::Char('3') => {
                if self.step == 1 {
                    self.lang_cursor = 2;
                }
            }
            KeyCode::Enter => return self.confirm(),
            KeyCode::Esc => {
                // Esc 随时跳过整个向导（不持久化，下次启动仍会自动弹出）
                log::info!("wizard: skipped via Esc");
                self.active = false;
                return true;
            }
            _ => {}
        }
        false
    }

    /// 确认当前步骤：步骤 1 → 进入步骤 2；步骤 2 → 完成向导。
    fn confirm(&mut self) -> bool {
        if self.step == 1 {
            // Auto → 解析为具体语言；English / 简体中文 直接采用
            self.effective_lang = match self.lang_cursor {
                1 => Lang::English,
                2 => Lang::Chinese,
                _ => Lang::detect(),
            };
            log::info!("wizard: step1 confirmed, lang={:?}", self.effective_lang);
            self.step = 2;
            false
        } else {
            let configured = self.config_cursor == 0;
            let lang_code = self.effective_lang.code();
            persist(lang_code, configured);
            self.result = Some(WizardResult { configured });
            self.active = false;
            log::info!(
                "wizard: finished (lang={}, configured={})",
                lang_code,
                configured
            );
            true
        }
    }
}

// ─────────────────────────── 持久化 ───────────────────────────

#[derive(Serialize, Deserialize)]
struct WizardConfig {
    lang: String,
    configured: String,
    version: String,
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

fn config_path() -> PathBuf {
    wizard_dir().join(WIZARD_FILE)
}

fn is_first_run() -> bool {
    !config_path().exists()
}

/// 将选择写回 wizard.toml（lang + configured + version）。
fn persist(lang: &str, configured: bool) {
    let cfg = WizardConfig {
        lang: lang.to_string(),
        configured: if configured {
            "manual".to_string()
        } else {
            "skipped".to_string()
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
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
    let width = (area.width as usize).min(74).max(20);
    let mut lines: Vec<Line> = Vec::new();

    if w.step == 1 {
        build_step1(&mut lines, w, width);
    } else {
        build_step2(&mut lines, w, width);
    }
    push_footer(&mut lines, width);

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
        &format!("欢迎使用 AirymaxRT v{}", env!("CARGO_PKG_VERSION")),
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
    lines.push(centered(if zh { "步骤 2/2" } else { "Step 2 of 2" }, width));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered(
        if zh { "想怎么开始？" } else { "How would you like to start?" },
        width,
    ));
    lines.push(Line::raw(""));

    let opts: [(&str, &str); 2] = if zh {
        [
            ("手动配置模型", "选择提供商、设置 API Key 与默认模型"),
            ("跳过，先进 TUI 探索", "直接进入对话，随时可用 /hiairy 重开向导"),
        ]
    } else {
        [
            ("Configure model manually", "Choose a provider, set API key & default model"),
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

fn push_footer(lines: &mut Vec<Line>, width: usize) {
    lines.push(Line::raw(""));
    lines.push(centered("↑↓ 移动 · 1-3 直达 · Enter 确认 · Esc 跳过", width));
    lines.push(centered("↑↓ Move · 1-3 Select · Enter Confirm · Esc Skip", width));
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
