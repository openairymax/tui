// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 键位分派：空闲态与 busy 等待态两条分派面，共用只读面板判定。
// 自 main.rs 拆出（0.1.18：单一职责——main 只管启动编排与主循环骨架）。
//
// 本模块只改应用状态，不持时钟、不成帧、不等待：渲染节奏由 L4 调度器
// 统一判定、成帧由 L5 合成层唯一出口发出（0.1.18 §5A.3 W4 节拍权威），
// 故此处没有内层忙等轮询，也没有固定睡眠。

mod busy;
mod idle;
mod util;

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent};

use crate::app::App;

use busy::busy_key;
use idle::idle_key;

/// 键位处理出口信号：`Flow::Exit` 表示请求退出 TUI 主循环（F8 切 CLI），
/// `Flow::SigInt` 表示用户以 Ctrl+C 主动中断，二者退出码不同（§8.2），
/// `Flow::Continue` 表示回到主循环下一帧。
pub(crate) enum Flow {
    Continue,
    Exit,
    SigInt,
}

/// 键位分派总入口（主循环 Event::Key，仅 Press）：按在途请求状态选择分派面。
/// 两条分支均只改应用状态——等待节拍、结果消费与插入对话推进都归主循环
/// （0.1.18 §5A.3 W4：等待泵并入统一调度，不再有第二套节拍）。
pub(crate) async fn handle(app: &mut App, key: KeyEvent) -> Result<Flow> {
    if !app.wizard.active {
        if let Some(flow) = leader_key(app, key) {
            return Ok(flow);
        }
    }
    if app.is_busy() {
        busy_key(app, key).await
    } else {
        idle_key(app, key).await
    }
}

/// 两段式 leader 拦截（0.1.19 L7 U8，TUI 方案 §5.4 #3 / §6.4 #7）。
///
/// 前缀键登记后进入待续态：续键 `y` 复制最近完整代码块，其余键取消并吞掉，
/// 避免误触落到普通输入面。wizard 打开时整段让行（见 `handle`）。
fn leader_key(app: &mut App, key: KeyEvent) -> Option<Flow> {
    let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    if app.leader_pending() {
        app.leader_cancel();
        // Ctrl+C 始终回到退出守卫，不被前缀序列吞掉。
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return None;
        }
        match key.code {
            KeyCode::Char('g') | KeyCode::Char('G') if ctrl => app.leader_press(),
            KeyCode::Char('y') | KeyCode::Char('Y') if !ctrl => copy_code(app),
            _ => {}
        }
        return Some(Flow::Continue);
    }
    if ctrl && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G')) {
        app.leader_press();
        return Some(Flow::Continue);
    }
    None
}

/// leader 续键：把最近完整代码块送入剪贴板（OSC52 优先，平台命令兜底）。
fn copy_code(app: &mut App) {
    let Some(code) = app.last_code() else {
        app.toast("没有可复制的代码块");
        return;
    };
    if crate::engine::term::copy_text(&code) {
        app.toast("已复制代码块");
    } else {
        app.toast("复制失败：剪贴板不可用");
    }
}
