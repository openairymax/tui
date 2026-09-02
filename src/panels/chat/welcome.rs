// SPDX-License-Identifier: Apache-2.0
// Copyright (c) AirymaxRT contributors. All rights distributed under the terms of this license.

//! 空态欢迎墙：2.0 分层品牌卡（品牌行 / 主题细线 / 能力矩阵 / 硬件摘要 / 引导行）。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::app::App;
use crate::panels::config::host_info;
use crate::theme;

/// 空态欢迎（无消息时，2.0 分层品牌卡重设计）。
///
/// 视觉结构（自上而下，垂直居中）：
///   1. 品牌行：◈ AirymaxRT（主色粗体）+ 版本徽章（主色底反白）+ tagline（灰）；
///   2. 主题细线（主色弱化，半宽居中）——与顶部状态条呼应；
///   3. 核心链路能力矩阵（胶囊 chips，次级表面底 + 语义色文字）；
///   4. 硬件摘要行（复用配置面板宿主机探测，静态展示独占欢迎墙）；
///   5. 引导行 + 项目上下文行。
///
/// 运行数据（连接灯/模型/token/成本/阶段）由顶部系统状态条独占，
/// 两区域职责分离、无重叠。极窄屏（<44 列）降级为单行精简品牌。
pub(super) fn append(out: &mut Vec<Line<'static>>, width: usize, height: usize, app: &App) {
    let ver = env!("AIRY_RT_VERSION");
    let proj = if app.project_context.is_empty() {
        "未加载项目上下文（F2 配置 / /project 加载）".to_string()
    } else {
        app.project_context
            .lines()
            .next()
            .unwrap_or("已加载项目上下文")
            .to_string()
    };

    // 极窄屏（<44 列）：仅一行精简品牌
    if width < 44 {
        out.push(Line::from(vec![
            Span::styled(
                "◈ AirymaxRT",
                Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  v{ver}"), Style::default().fg(theme::faint())),
            Span::styled(
                "  极境智能体运行平台 · 输入消息开始对话",
                Style::default().fg(theme::dim()),
            ),
        ]));
        return;
    }

    let content_max = width.saturating_sub(4).max(16);
    let mut hero: Vec<Line<'static>> = Vec::new();

    // 1. 品牌行：徽标 + 版本徽章 + tagline
    let brand = Line::from(vec![
        Span::styled(
            "◈ AirymaxRT",
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{ver} "),
            Style::default()
                .fg(theme::on_color())
                .bg(theme::primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  极境智能体运行平台", Style::default().fg(theme::dim())),
    ]);
    let pad = content_max.saturating_sub(2).saturating_sub(brand.width()) / 2;
    let mut centered: Vec<Span> = vec![Span::raw(" ".repeat(pad))];
    centered.extend(brand.spans.clone());
    hero.push(Line::from(centered));

    // 2. 主题细线（主色弱化，半宽居中）——"我思故我在"式收敛分隔
    let line_w = (content_max / 3).clamp(6, 24);
    let mut tag = String::new();
    for i in 0..line_w {
        if i == line_w / 2 {
            tag.push('◆');
        } else {
            tag.push('─');
        }
    }
    let tag_pad = content_max.saturating_sub(unicode_width::UnicodeWidthStr::width(tag.as_str())) / 2;
    hero.push(Line::from(vec![
        Span::raw(" ".repeat(tag_pad)),
        Span::styled(tag, Style::default().fg(theme::separator())),
    ]));
    hero.push(Line::raw(""));

    // 3. 核心链路能力矩阵（胶囊 chips，语义色）
    let chain = [
        ("llm", theme::accent()),
        ("think", theme::warning()),
        ("agent", theme::success()),
        ("tool", theme::magenta()),
        ("board", theme::primary()),
    ];
    let mut caps_line = Line::from(vec![
        Span::styled("核心链路  ", Style::default().fg(theme::faint())),
    ]);
    for (i, (name, c)) in chain.iter().enumerate() {
        if i > 0 {
            caps_line.spans.push(Span::styled(
                " ",
                Style::default().fg(theme::faint()),
            ));
        }
        caps_line.spans.push(Span::styled(
            format!(" {} ", name),
            Style::default()
                .fg(*c)
                .bg(theme::surface_2())
                .add_modifier(Modifier::BOLD),
        ));
    }
    let caps_pad = content_max.saturating_sub(2).saturating_sub(caps_line.width()) / 2;
    let mut padded_caps = caps_line.spans.clone();
    padded_caps.insert(0, Span::raw(" ".repeat(caps_pad)));
    hero.push(Line::from(padded_caps));
    hero.push(Line::raw(""));

    // 4. 硬件摘要行（复用配置面板宿主机探测，静态展示独占欢迎墙）
    let hw = host_info().summary_line();
    let mut hw_line = Line::from(vec![
        Span::styled("硬件  ", Style::default().fg(theme::faint())),
        Span::styled(hw, Style::default().fg(theme::dim())),
    ]);
    if hw_line.width() + 2 < content_max {
        let hw_pad = content_max.saturating_sub(hw_line.width()) / 2;
        hw_line.spans.insert(0, Span::raw(" ".repeat(hw_pad)));
        hero.push(hw_line);
        hero.push(Line::raw(""));
    }

    // 5. 引导行 + 项目上下文行
    hero.push(Line::from(vec![
        Span::styled("输入消息开始对话 · F1 帮助 · F2 配置 · F10 输入法", Style::default().fg(theme::faint())),
    ]));
    let proj_disp: String = proj.chars().take(content_max.saturating_sub(4)).collect();
    if !proj_disp.is_empty() {
        hero.push(Line::from(vec![
            Span::styled("项目  ", Style::default().fg(theme::faint())),
            Span::styled(proj_disp, Style::default().fg(theme::dim())),
        ]));
    }

    // 垂直居中：高度不足时顶部对齐
    let lead = height.saturating_sub(hero.len()).saturating_sub(2) / 2;
    for _ in 0..lead {
        out.push(Line::raw(""));
    }
    out.extend(hero);
}
