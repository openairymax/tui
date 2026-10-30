// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 空闲态按键分派：面板切换、审批决议、输入编辑、视口滚动、历史与标签。

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent};
use log::{debug, info, warn};
use std::time::Instant;

use super::util::{read_only_panel, truncate_str};
use super::Flow;

use crate::app::{ActivePanel, App, MessageRole};
use crate::gccp::TaskControl;

/// 空闲态按键分派。返回 `Exit` 时调用方立即退出主循环（终端守卫 Drop 还原）。
pub(super) async fn idle_key(app: &mut App, key: KeyEvent) -> Result<Flow> {
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            // 双击退出守卫（0.1.19 L7 U2）：首次误触只提示，不中断会话。
            if !app.confirm_exit(Instant::now()) {
                debug!("Ctrl+C armed (idle); press again to exit");
                return Ok(Flow::Continue);
            }
            info!("User pressed Ctrl+C twice, shutting down...");
            app.shutdown().await?;
            return Ok(Flow::SigInt);
        }
        // Ctrl+X：人工中止当前后台请求（任务执行/对话等待）
        KeyCode::Char('x') | KeyCode::Char('X')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            if app.is_busy() {
                info!("User pressed Ctrl+X, aborting pending request");
                app.abort_task();
            } else if app.task_mode {
                // 空闲态且处于任务集：Ctrl+X 退出任务集（回到普通对话）
                info!("User pressed Ctrl+X, exiting task mode");
                app.exit_task_mode();
            }
        }
        // Ctrl+Z：暂停/恢复后台请求等待（请求继续在网关执行）
        KeyCode::Char('z') | KeyCode::Char('Z')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            if app.is_busy() {
                info!("User pressed Ctrl+Z, toggling pause");
                if app.task_control == TaskControl::Paused {
                    app.resume_task();
                } else {
                    app.pause_task();
                }
            }
        }
        // 首次启动向导激活：按键全部交给向导
        // （↑↓ 移动 · 1-3 直达 · Enter 确认/编辑 · Esc 跳过/返回）
        _ if app.wizard.active => {
            if app.wizard.handle_key(&key) {
                // 向导完成：快速配置 → 应用配置并打开配置面板；跳过 → 留在对话
                if let Some(r) = app.wizard.result.take() {
                    if r.configured {
                        app.apply_wizard_result(&r);
                        app.active_panel = ActivePanel::Config;
                        let model_txt = if r.model.is_empty() {
                            "默认模型（网关自动回落）".to_string()
                        } else {
                            r.model.clone()
                        };
                        let key_txt = if r.api_key_set {
                            "API Key 已写入 secrets.env，可直接开始对话。".to_string()
                        } else {
                            "未填写 API Key：请编辑模型配置（model.yaml）\
                             或设置对应环境变量后开始对话。"
                                .to_string()
                        };
                        app.add_message(
                            MessageRole::System,
                            format!("模型配置完成：{}（{}）。{}", model_txt, r.provider, key_txt),
                        );
                    } else {
                        app.add_message(
                            MessageRole::System,
                            "欢迎使用 AirymaxRT！已跳过模型配置，\
                             输入 /hiairy 可随时重新打开首次启动向导。"
                                .to_string(),
                        );
                    }
                }
            }
            return Ok(Flow::Continue);
        }
        // Ctrl+1：内置拼音输入法 中/英 切换（决策1 定案唤起键；
        // 落在向导分支之后，向导激活时不抢键）
        KeyCode::Char('1') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            app.ime_toggle();
        }
        // IME 拼音态：Esc 取消拼音（微信语义：清空缓冲，放弃组合）
        KeyCode::Esc if app.ime_visible() => {
            app.ime_cancel();
        }
        // §6.5 #3：有待审批时 Esc 等同拒绝（fail-closed 语义）
        KeyCode::Esc if !app.approvals.is_empty() => {
            app.approve_request("deny");
        }
        KeyCode::Esc if app.active_panel != ActivePanel::Chat => {
            debug!("Panel: Esc → return to Chat");
            app.active_panel = ActivePanel::Chat;
            // 与 toggle_panel 同源：离开 Board/Events 必须停 SSE，
            // 否则订阅泄漏，后台持续收流（空闲态 Esc 路径）。
            app.stop_hall_watch();
        }
        KeyCode::F(1) => {
            debug!("Panel: toggle Help");
            app.toggle_panel(ActivePanel::Help);
        }
        KeyCode::F(2) => {
            debug!("Panel: toggle Config");
            app.toggle_panel(ActivePanel::Config);
        }
        KeyCode::F(3) => {
            debug!("Panel: toggle Logs");
            app.toggle_panel(ActivePanel::Logs);
        }
        KeyCode::F(4) => {
            debug!("Panel: toggle Memory");
            app.toggle_panel(ActivePanel::Memory);
        }
        KeyCode::F(5) => {
            debug!("Panel: toggle Plugins");
            app.toggle_panel(ActivePanel::Plugins);
        }
        KeyCode::F(6) => {
            debug!("Panel: toggle Board");
            // 进入看板：强制立即刷新 + 订阅 hall.watch SSE 推送
            app.active_panel = ActivePanel::Board;
            app.force_hall_refresh();
            app.start_hall_watch();
        }
        KeyCode::F(7) => {
            debug!("Panel: toggle Events");
            app.active_panel = ActivePanel::Events;
            app.force_hall_refresh();
            app.start_hall_watch();
        }
        // F8：切换到 CLI（airy_cli）——恢复终端后 exec 替换进程
        KeyCode::F(8) => {
            debug!("F8: switching to CLI (airy_cli)");
            app.switch_to_cli = true;
            return Ok(Flow::Exit);
        }
        // §6.9 #3：空输入时 `?` 呼出键位浮层（Help 面板）。输入非空时
        // `?` 仍是普通字符，避免打断正在编辑的消息。
        KeyCode::Char('?')
            if app.active_panel == ActivePanel::Chat
                && app.input.is_empty()
                && !key.modifiers.contains(event::KeyModifiers::CONTROL)
                && !key.modifiers.contains(event::KeyModifiers::ALT) =>
        {
            app.toggle_panel(ActivePanel::Help);
        }
        KeyCode::Enter => {
            // 面板激活（Board/Events）：Enter = 查看选中条目详情
            if app.active_panel == ActivePanel::Board {
                debug!("Board: Enter → view selected decision chain");
                app.board_view_selected();
                return Ok(Flow::Continue);
            }
            if app.active_panel == ActivePanel::Events {
                debug!("Events: Enter → view selected event detail");
                app.events_view_selected();
                return Ok(Flow::Continue);
            }
            // 只读面板（Help/Config/Logs/Memory/Plugins）：Enter 不提交、
            // Alt+Enter 不换行——共享输入只属于对话面板，防止切到面板后
            // 误触 Enter 把输入框内容真的发出去（Board/Events 上面已处理）
            if read_only_panel(app.active_panel) {
                debug!("Panel: Enter 在只读面板被忽略（不提交共享输入）");
                return Ok(Flow::Continue);
            }
            // Alt+Enter 换行（多行输入，光标处插入），Enter 发送
            if key.modifiers.contains(event::KeyModifiers::ALT) {
                app.input_insert_text("\n");
                return Ok(Flow::Continue);
            }
            // 拼音态：先提交拼音原文（随后提交整行）
            app.ime_commit_enter();
            let input = app.take_input();
            debug!(
                "User submitted input: '{}' ({} chars)",
                truncate_str(&input, 80),
                input.len()
            );
            if let Err(e) = app.submit_input(&input) {
                warn!("submit_input error: {}", e);
                app.add_message(MessageRole::System, format!("Error: {}", e));
            }
            // 后台请求已发出 → 立即交还主循环：此后按键由 busy_key 分派
            //   - thinking... 动效与流式上屏由主循环按 L4 节拍成帧
            //   - 回复到达/流式增量/审批待决议由主循环 poll_pending 消费
            //   - Ctrl+X 中止 / Ctrl+Z 暂停（等待期间可人工控制）
            //   - 任务完成后插入对话队列由主循环逐条提交（单 pending 槽，
            //     每条等其完成再处理下一条，逻辑链连续不割裂）
            return Ok(Flow::Continue);
        }
        // ── 输入编辑：光标感知（readline 风格）──
        KeyCode::Tab => {
            // Tab 补全：/ 命令 + 技能名（仅对话面板）
            if app.active_panel == ActivePanel::Chat {
                app.tab_complete();
            }
        }
        KeyCode::Backspace => {
            // 删除光标前一个字符（IME 拼音态：删拼音缓冲）
            if !app.ime_backspace() {
                app.input_backspace();
            }
        }
        KeyCode::Delete => {
            // 删除光标后一个字符
            app.input_delete_after();
        }
        KeyCode::Left => {
            // ←：IME 拼音态移动候选高亮；否则光标左移（微信式）
            if app.ime_visible() {
                app.ime_move_sel(-1);
            } else {
                app.cursor_left();
            }
        }
        KeyCode::Right => {
            // →：IME 拼音态移动候选高亮；否则光标右移（微信式）
            if app.ime_visible() {
                app.ime_move_sel(1);
            } else {
                app.cursor_right();
            }
        }
        KeyCode::Home => {
            // Alt+Home：视口滚动到顶（Home 光标语义不变，§15.5.1）
            if key.modifiers.contains(event::KeyModifiers::ALT) {
                app.scroll_top();
            } else {
                app.cursor_home();
            }
        }
        KeyCode::End => {
            if key.modifiers.contains(event::KeyModifiers::ALT) {
                // Alt+End：视口滚动到底（最新消息）
                app.scroll_bottom();
            } else if app.input.is_empty() {
                // End：空输入 → 回到底部（最新消息）
                app.scroll_bottom();
            } else {
                app.cursor_end();
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            // Ctrl+A：光标到输入开头
            app.cursor_home();
        }
        KeyCode::Char('e') | KeyCode::Char('E')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            // Ctrl+E：光标到输入末尾
            app.cursor_end();
        }
        KeyCode::Char('w') | KeyCode::Char('W')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            // Ctrl+W：删除光标前一个词
            app.input_delete_word_before();
        }
        KeyCode::Char('u') | KeyCode::Char('U')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            // Ctrl+U：删除光标前全部内容
            app.input_delete_to_start();
        }
        // Ctrl+T：新建会话 tab（多会话；请求进行中不可用，见 app 守卫）
        KeyCode::Char('t') | KeyCode::Char('T')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            app.new_session_tab();
        }
        // Alt+1..9：切换会话 tab（Alt+1 = 主会话；优先于面板数字过滤）
        KeyCode::Char(c)
            if key.modifiers.contains(event::KeyModifiers::ALT)
                && c.is_ascii_digit()
                && c != '0' =>
        {
            app.switch_tab((c as u8 - b'0') as usize);
        }
        // Alt+E：打开思考链独立视图（0.1.18 B4：思考链默认不上屏、
        // 不落长期记忆，仅显式请求时在此按需查看；Ctrl+E 仍保留
        // 光标到行尾的 readline 惯例）
        KeyCode::Char('e') | KeyCode::Char('E')
            if key.modifiers.contains(event::KeyModifiers::ALT) =>
        {
            app.open_think_panel();
        }
        // Alt+F：焦点视图（0.1.18 B11）——全屏只读查看最近一条回复
        KeyCode::Char('f') | KeyCode::Char('F')
            if key.modifiers.contains(event::KeyModifiers::ALT) =>
        {
            app.focus_open();
        }
        // Ctrl+M：鼠标滚轮捕获会话级开关（0.1.18 B11，默认关；
        // 终端序列由 run_app 循环头的唯一执行点发送）
        KeyCode::Char('m') | KeyCode::Char('M')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            let on = app.toggle_mouse_capture();
            info!("Mouse capture: {}", if on { "ON" } else { "OFF" });
        }
        // Alt+O：展开/折叠全部长系统消息（0.1.7 折叠与滚动解耦，
        // 滚动基于稳定折叠视图；B4 起该功能从 Alt+E 迁至 Alt+O）
        KeyCode::Char('o') | KeyCode::Char('O')
            if key.modifiers.contains(event::KeyModifiers::ALT) =>
        {
            app.browse_expanded = !app.browse_expanded;
        }
        KeyCode::Char(c) if app.active_panel == ActivePanel::Board => {
            // F6 看板：0=全部 · 1-6=状态过滤（completed/running/pending/scheduled/failed/canceled）
            match c {
                '0' => app.board_set_filter(""),
                '1' => app.board_set_filter("completed"),
                '2' => app.board_set_filter("running"),
                '3' => app.board_set_filter("pending"),
                '4' => app.board_set_filter("scheduled"),
                '5' => app.board_set_filter("failed"),
                '6' => app.board_set_filter("canceled"),
                _ => {}
            }
        }
        KeyCode::Char(c) if app.active_panel == ActivePanel::Events => {
            // F7 事件流：0=全部 · 1-7=类别过滤（blueprint/command/progress/result/issue/verify/chain）
            match c {
                '0' => app.events_set_filter(""),
                '1' => app.events_set_filter("blueprint"),
                '2' => app.events_set_filter("command"),
                '3' => app.events_set_filter("progress"),
                '4' => app.events_set_filter("result"),
                '5' => app.events_set_filter("issue"),
                '6' => app.events_set_filter("verify"),
                '7' => app.events_set_filter("chain"),
                _ => {}
            }
        }
        KeyCode::Char(c) if app.active_panel == ActivePanel::Chat => {
            // 普通字符插入到光标位置（IME 拼音态：先经拼音输入法）。
            // 仅对话面板可写入共享输入；只读面板（Help/Config/Logs/
            // Memory/Plugins）不汇入输入框，Board/Events 由上方数字
            // 过滤分支消费、不落入此处。
            if !app.ime_input_char(c) {
                app.input_insert_char(c);
            }
        }
        KeyCode::Up => {
            // F6/F7 面板：↑ 移动选中光标（循环）；F3 日志面板滚动；
            // 思考链/焦点视图滚动；其余场景滚对话/浏览历史
            if app.active_panel == ActivePanel::Board {
                app.board_cursor_up();
            } else if app.active_panel == ActivePanel::Events {
                app.events_cursor_up();
            } else if app.active_panel == ActivePanel::Logs {
                app.logs_scroll_older();
            } else if app.active_panel == ActivePanel::Think {
                app.think_scroll_up();
            } else if app.active_panel == ActivePanel::Focus {
                app.focus_scroll_up();
            } else if app.active_panel == ActivePanel::Health {
                app.health_scroll_up();
            } else if app.active_panel == ActivePanel::Evolution {
                app.evo_scroll_up();
            } else if key.modifiers.contains(event::KeyModifiers::ALT) {
                app.history_prev();
            } else {
                app.scroll_up();
            }
        }
        KeyCode::Down => {
            if app.active_panel == ActivePanel::Board {
                app.board_cursor_down();
            } else if app.active_panel == ActivePanel::Events {
                app.events_cursor_down();
            } else if app.active_panel == ActivePanel::Logs {
                app.logs_scroll_newer();
            } else if app.active_panel == ActivePanel::Think {
                app.think_scroll_down();
            } else if app.active_panel == ActivePanel::Focus {
                app.focus_scroll_down();
            } else if app.active_panel == ActivePanel::Health {
                app.health_scroll_down();
            } else if app.active_panel == ActivePanel::Evolution {
                app.evo_scroll_down();
            } else if key.modifiers.contains(event::KeyModifiers::ALT) {
                app.history_next();
            } else {
                app.scroll_down();
            }
        }
        KeyCode::PageUp => {
            // PgUp：IME 拼音态翻上一页（微信式）；记忆面板翻记录窗口；
            // 思考链/焦点视图翻页；否则滚动上翻
            if app.ime_visible() {
                app.ime_page_flip(-1);
            } else if app.active_panel == ActivePanel::Memory {
                app.memory_page_up();
            } else if app.active_panel == ActivePanel::Think {
                app.think_page_up();
            } else if app.active_panel == ActivePanel::Focus {
                app.focus_page_up();
            } else if app.active_panel == ActivePanel::Health {
                app.health_page_up();
            } else if app.active_panel == ActivePanel::Evolution {
                app.evo_page_up();
            } else {
                app.scroll_page_up();
            }
        }
        KeyCode::PageDown => {
            // PgDn：IME 拼音态翻下一页（微信式）；记忆面板翻记录窗口；
            // 思考链/焦点视图翻页；否则滚动下翻
            if app.ime_visible() {
                app.ime_page_flip(1);
            } else if app.active_panel == ActivePanel::Memory {
                app.memory_page_down();
            } else if app.active_panel == ActivePanel::Think {
                app.think_page_down();
            } else if app.active_panel == ActivePanel::Focus {
                app.focus_page_down();
            } else if app.active_panel == ActivePanel::Health {
                app.health_page_down();
            } else if app.active_panel == ActivePanel::Evolution {
                app.evo_page_down();
            } else {
                app.scroll_page_down();
            }
        }
        _ => {}
    }
    Ok(Flow::Continue)
}
