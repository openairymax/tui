// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// busy 等待态按键分派：中止/暂停/审批、滚动阅读、插入对话输入。

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent};
use log::{debug, info};
use std::time::Instant;

use super::util::read_only_panel;
use super::Flow;

use crate::app::{ActivePanel, App};
use crate::gccp::TaskControl;

/// busy 等待态按键分派（主循环在 `App::is_busy()` 为真时调用）：等待在途请求
/// 完成期间处理中止/暂停/审批/滚动/插入对话提交。返回 `Exit` 表示用户请求
/// 退出（busy 期间 Ctrl+C）。
///
/// 本函数不循环、不睡眠、不成帧：等待节拍由 L4 调度器统一给出，在途结果由
/// 主循环 `poll_pending` 消费，插入对话队列由主循环在请求完成后逐条提交。
pub(super) async fn busy_key(app: &mut App, key: KeyEvent) -> Result<Flow> {
    match key.code {
        // 与空闲态一致：busy（等待回复/任务执行）期间按
        // Ctrl+C 同样退出 TUI，不再被 busy 内层循环吞键；
        // 同样受双击守卫约束（0.1.19 L7 U2）。
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            if !app.confirm_exit(Instant::now()) {
                debug!("Ctrl+C armed (busy); press again to exit");
                return Ok(Flow::Continue);
            }
            info!("User pressed Ctrl+C twice while busy, shutting down...");
            app.shutdown().await?;
            return Ok(Flow::SigInt);
        }
        KeyCode::Char('x') | KeyCode::Char('X')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            app.abort_task();
        }
        KeyCode::Char('z') | KeyCode::Char('Z')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            if app.task_control == TaskControl::Paused {
                app.resume_task();
            } else {
                app.pause_task();
            }
        }
        // 工具级权限审批（§6.5 #3：y=允许本次 / a=本会话总是 / n=拒绝，
        // Esc 亦为拒绝）。仅当存在待审批请求时生效——此前无条件拦截
        // a/y/n/d/A，busy 期间输入含这些字母的文本会被吞字并
        // 向对话区插入"没有待决议的权限请求"污染聊天。
        KeyCode::Char('y')
            if !app.approvals.is_empty()
                && !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            app.approve_request("allow");
        }
        KeyCode::Char('a') | KeyCode::Char('A')
            if !app.approvals.is_empty()
                && !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            app.approve_request("always");
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('d') | KeyCode::Char('D')
            if !app.approvals.is_empty()
                && !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            app.approve_request("deny");
        }
        // ── 2.3.13：等待回复（busy）期间 F6/F7 可切换看板/事件流 ──
        // 此前 F 键仅在非 busy 主循环处理，LLM 请求进行中（可能
        // 数十秒）按键落入 _ => {} 被吞，用户感知"看板不可操作"。
        // busy 中切换面板只改视图，不打断正在进行的请求。
        KeyCode::F(6) => {
            app.active_panel = ActivePanel::Board;
            app.force_hall_refresh();
            app.start_hall_watch();
        }
        KeyCode::F(7) => {
            app.active_panel = ActivePanel::Events;
            app.force_hall_refresh();
            app.start_hall_watch();
        }
        // Ctrl+1：内置拼音输入法切换（busy 插入对话场景同样可用）
        KeyCode::Char('1') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            app.ime_toggle();
        }
        // 0.1.7：busy 期间（LLM 生成可达数十秒）允许滚动阅读旧
        // 消息——此前落入 _ => {} 被吞，长生成期间用户无法
        // 回看上下文。0.1.18 B4：思考链改由 Alt+E 独立视图
        // 查看（流式中该视图数据源为实时增量）。
        KeyCode::Up if app.ime_visible() => {
            app.ime_move_sel(-1);
        }
        KeyCode::Down if app.ime_visible() => {
            app.ime_move_sel(1);
        }
        KeyCode::Up if app.active_panel == ActivePanel::Think => {
            app.think_scroll_up();
        }
        KeyCode::Down if app.active_panel == ActivePanel::Think => {
            app.think_scroll_down();
        }
        KeyCode::Up if app.active_panel == ActivePanel::Focus => {
            app.focus_scroll_up();
        }
        KeyCode::Down if app.active_panel == ActivePanel::Focus => {
            app.focus_scroll_down();
        }
        KeyCode::Up if app.active_panel == ActivePanel::Health => {
            app.health_scroll_up();
        }
        KeyCode::Down if app.active_panel == ActivePanel::Health => {
            app.health_scroll_down();
        }
        KeyCode::Up => app.scroll_up(),
        KeyCode::Down => app.scroll_down(),
        KeyCode::PageUp if app.ime_visible() => {
            app.ime_page_flip(-1);
        }
        KeyCode::PageDown if app.ime_visible() => {
            app.ime_page_flip(1);
        }
        KeyCode::PageUp if app.active_panel == ActivePanel::Think => {
            app.think_page_up();
        }
        KeyCode::PageDown if app.active_panel == ActivePanel::Think => {
            app.think_page_down();
        }
        KeyCode::PageUp if app.active_panel == ActivePanel::Focus => {
            app.focus_page_up();
        }
        KeyCode::PageDown if app.active_panel == ActivePanel::Focus => {
            app.focus_page_down();
        }
        KeyCode::PageUp if app.active_panel == ActivePanel::Health => {
            app.health_page_up();
        }
        KeyCode::PageDown if app.active_panel == ActivePanel::Health => {
            app.health_page_down();
        }
        KeyCode::PageUp => app.scroll_page_up(),
        KeyCode::PageDown => app.scroll_page_down(),
        // Alt+E：打开思考链独立视图（0.1.18 B4：思考链
        // 默认不上屏，仅显式请求时在此按需查看）
        KeyCode::Char('e') | KeyCode::Char('E')
            if key.modifiers.contains(event::KeyModifiers::ALT) && !app.ime_visible() =>
        {
            app.open_think_panel();
        }
        // Alt+F：焦点视图（0.1.18 B11，busy 期间同样可用）
        KeyCode::Char('f') | KeyCode::Char('F')
            if key.modifiers.contains(event::KeyModifiers::ALT) =>
        {
            app.focus_open();
        }
        // Ctrl+M：鼠标滚轮捕获开关（终端序列由主循环头执行点发送）
        KeyCode::Char('m') | KeyCode::Char('M')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            let on = app.toggle_mouse_capture();
            info!("Mouse capture: {}", if on { "ON" } else { "OFF" });
        }
        // Alt+O：展开/折叠长系统消息（B4 从 Alt+E 迁出）
        KeyCode::Char('o') | KeyCode::Char('O')
            if key.modifiers.contains(event::KeyModifiers::ALT) && !app.ime_visible() =>
        {
            app.browse_expanded = !app.browse_expanded;
        }
        // IME 拼音态：Esc 取消拼音（微信语义）
        KeyCode::Esc if app.ime_visible() => {
            app.ime_cancel();
        }
        // §6.5 #3：审批在场时 Esc 等同拒绝，优先于面板返回
        KeyCode::Esc if !app.approvals.is_empty() => {
            app.approve_request("deny");
        }
        // 非拼音态 Esc：与空闲态一致返回对话（busy 期间
        // 在只读面板/看板上也能 Esc 退出，不困在面板里）；
        // §6.2 #6：等待期 Esc 同时请求中断，立即呈递「已请求中断…」。
        KeyCode::Esc => {
            app.active_panel = ActivePanel::Chat;
            // busy 态同样必须停 SSE（订阅泄漏同源修复）
            app.stop_hall_watch();
            app.interrupt_task();
        }
        // ── 插入对话（2.3.7）：任务执行中输入文本 ──
        // 只读面板（Help/Config/Logs/Memory/Plugins）：
        // Enter 不入插入队列——共享输入只属于对话面板
        KeyCode::Enter if read_only_panel(app.active_panel) => {}
        KeyCode::Enter => {
            // 拼音态：先提交拼音原文（随后提交整行）
            app.ime_commit_enter();
            let input = app.take_input();
            if !input.trim().is_empty() {
                app.queue_insert_chat(&input);
            }
        }
        KeyCode::Backspace => {
            if !app.ime_backspace() {
                app.input_backspace();
            }
        }
        KeyCode::Delete => {
            app.input_delete_after();
        }
        KeyCode::Left => {
            // ←：IME 拼音态移动候选高亮（微信式）
            if app.ime_visible() {
                app.ime_move_sel(-1);
            } else {
                app.cursor_left();
            }
        }
        KeyCode::Right => {
            if app.ime_visible() {
                app.ime_move_sel(1);
            } else {
                app.cursor_right();
            }
        }
        KeyCode::Home => {
            app.cursor_home();
        }
        KeyCode::End => {
            app.cursor_end();
        }
        // 0.1.7：带修饰键的字符（Alt+E 思考链视图、Alt+O 展开/折叠、
        // Ctrl+E 光标到行尾、Alt+1..9 切换标签）不能落入普通字符
        // 插入——此前 busy 期间按 Alt+E/Ctrl+E 被当 'e' 插入输入框，
        // 交互动作静默丢失。
        // 只读面板（Help/Config/Logs/Memory/Plugins/Think）同样
        // 不写入共享输入。
        KeyCode::Char(c)
            if (key.modifiers.is_empty() || key.modifiers == event::KeyModifiers::SHIFT)
                && !read_only_panel(app.active_panel) =>
        {
            // ime_input_char 带副作用（消费候选/推进拼音串），
            // 不可上提进 match guard（clippy 建议在此不适用）
            #[allow(clippy::collapsible_match)]
            if !app.ime_input_char(c) {
                // 普通字符插入输入框（光标感知；IME 拼音态已消费时跳过）
                app.input_insert_char(c);
            }
        }
        _ => {}
    }
    Ok(Flow::Continue)
}
