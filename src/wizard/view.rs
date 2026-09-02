// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导渲染：全屏居中、双语 footer、字段行（选中 ▸ / 编辑窗口 / 掩码）、
// 选项行与说明换行。所有文案取自 `steps` 注册表，渲染层不再复制任何
// 字段下标与默认文案（Unify Design SSoT）。

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme;

use super::lang::Lang;
use super::state::WizardState;
use super::steps::{form_step, FieldSpec, START_CHOICES, TOTAL_STEPS, LANG_CHOICES};
use super::text::{byte_to_char, wrap_text};

/// 全屏渲染向导（首次运行 / /hiairy 时由 ui.rs 接管整个终端）。
pub fn render(f: &mut Frame, area: Rect, w: &WizardState) {
    // 全屏填充背景：确保居中视图外无任何残留（视觉干净，居中明确）
    f.render_widget(
        Paragraph::new(Text::raw("")).style(Style::default().bg(theme::bg())),
        area,
    );

    let width = (area.width as usize).clamp(24, 78);
    let mut lines: Vec<Line> = Vec::new();

    match w.step {
        1 => build_welcome(&mut lines, w, width),
        2 => build_start(&mut lines, w, width),
        _ => build_form(&mut lines, w, width),
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

/// (中文, English) 文案对按语言取值
fn t<'a>(zh: bool, pair: (&'a str, &'a str)) -> &'a str {
    if zh {
        pair.0
    } else {
        pair.1
    }
}

/// 步骤 1：欢迎 + 版本 + 界面语言选择（中英双语展示）。
fn build_welcome(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let detected = Lang::detect();
    let detected_hint = match detected {
        Lang::Chinese => "简体中文",
        _ => "English",
    };

    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered("首次启动向导 · First-run Setup", width));
    lines.push(centered(
        &format!("步骤 1/{} · Step 1 of {}", TOTAL_STEPS, TOTAL_STEPS),
        width,
    ));
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

    for (i, lang) in LANG_CHOICES.iter().enumerate() {
        let label = match lang {
            Lang::Auto => format!("{}  [推荐]", lang.label()),
            _ => lang.label().to_string(),
        };
        lines.push(option_line(i == w.choice_cursor, &label));
    }
}

/// 步骤 2：想怎么开始？（文案随步骤 1 所选语言切换）。
fn build_start(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let zh = w.effective_lang.zh();

    lines.push(Line::raw(""));
    lines.push(centered("◈ AirymaxRT", width));
    lines.push(Line::raw(""));
    lines.push(centered(t(zh, ("首次启动向导", "First-run Setup")), width));
    lines.push(centered(
        &format!("步骤 2/{} · Step 2 of {}", TOTAL_STEPS, TOTAL_STEPS),
        width,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(centered(
        t(zh, ("想怎么开始？", "How would you like to start?")),
        width,
    ));
    lines.push(Line::raw(""));

    for (i, c) in START_CHOICES.iter().enumerate() {
        lines.push(option_line(i == w.choice_cursor, t(zh, c.label)));
        for dl in desc_lines(t(zh, c.desc), width) {
            lines.push(dl);
        }
        lines.push(Line::raw(""));
    }
}

/// 表单步骤（3/4/5）：标题 + 副标题 + 可见字段 + 动作按钮。
fn build_form(lines: &mut Vec<Line>, w: &WizardState, width: usize) {
    let Some(spec) = form_step(w.step) else { return };
    let zh = w.effective_lang.zh();

    step_header(lines, w.step, t(zh, spec.title), width);
    match spec.subtitle {
        Some(sub) => {
            lines.push(centered(t(zh, sub), width));
            lines.push(Line::raw(""));
        }
        None => lines.push(Line::raw("")),
    }

    let visible = spec.visible(w.mode_local());
    for (pos, &idx) in visible.iter().enumerate() {
        field_line(lines, w, pos, idx, &spec.fields[idx], zh);
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(option_line(
        w.field_cursor >= visible.len(),
        t(zh, spec.action),
    ));
}

/// 表单步骤标题（◈ 标志 + 标题 + 步骤计数）。
fn step_header(lines: &mut Vec<Line>, step: u8, title: &str, width: usize) {
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
fn field_line(
    lines: &mut Vec<Line>,
    w: &WizardState,
    pos: usize,
    idx: usize,
    f: &FieldSpec,
    zh: bool,
) {
    let selected = w.field_cursor == pos;
    let editing = selected && w.editing;
    let (marker, mstyle) = if selected {
        (
            "▸",
            Style::default()
                .fg(theme::primary())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (" ", Style::default().fg(theme::dim()))
    };
    let raw = w.form.get(idx).map(|x| x.value.as_str()).unwrap_or("");
    let value = if editing {
        // 编辑态：窗口跟随插入点滚动——长文本（API Key / base_url）不截断，
        // 始终可见光标附近内容；edit_pos 为字节索引，先折算字符索引再开窗
        // （0.1.8：把字节索引当字符索引切片曾致 CJK 字段 panic）。
        window_value(raw, w.edit_pos)
    } else if f.masked && !raw.is_empty() {
        masked_value(raw)
    } else {
        raw.to_string()
    };
    let value_style = if editing {
        Style::default()
            .fg(theme::primary())
            .add_modifier(Modifier::BOLD)
    } else if value.is_empty() {
        Style::default().fg(theme::faint())
    } else {
        Style::default().fg(theme::text())
    };
    let label_w = if zh { 10 } else { 12 };
    lines.push(Line::from(vec![
        Span::styled(format!("    {} ", marker), mstyle),
        Span::styled(
            format!("{:<w$}", t(zh, f.label), w = label_w),
            Style::default().fg(if selected {
                theme::primary()
            } else {
                theme::dim()
            }),
        ),
        Span::styled(format!("[{}]", value), value_style),
    ]));
    if !editing {
        for dl in desc_lines(t(zh, f.hint), 64) {
            lines.push(dl);
        }
    }
}

/// 编辑窗口：48 字符可视段跟随插入点，越界前缀 …，▍ 标示插入点。
fn window_value(raw: &str, edit_pos: usize) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let cpos = byte_to_char(raw, edit_pos.min(raw.len()));
    let max_vis = 48usize;
    let mut start = cpos.saturating_sub(max_vis / 2);
    if start + max_vis > n {
        start = n.saturating_sub(max_vis);
    }
    let prefix_ell = start > 0;
    let inner = cpos.saturating_sub(start);
    let head: String = chars[start..(start + inner).min(n)].iter().collect();
    let tail: String = chars[(start + inner).min(n)..(start + max_vis).min(n)]
        .iter()
        .collect();
    if prefix_ell {
        format!("…{}▍{}", head, tail)
    } else {
        format!("{}▍{}", head, tail)
    }
}

/// 掩码值：非编辑态仅显示尾 4 字符（按字符取，避免多字节切片 panic）。
fn masked_value(raw: &str) -> String {
    let nc = raw.chars().count();
    if nc > 4 {
        let tail: String = raw.chars().skip(nc - 4).collect();
        format!("{}…{}", "•".repeat(12), tail)
    } else {
        "•".repeat(nc)
    }
}

/// 底部操作提示（表单步骤双语两行；选项步骤同上）。
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
            Style::default()
                .fg(theme::primary())
                .add_modifier(Modifier::BOLD),
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
