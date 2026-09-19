// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 键位分派：空闲态按键 match、busy 等待/插入对话泵、只读面板判定。
// 自 main.rs 拆出（0.1.18：单一职责——main 只管启动编排与主循环骨架）。

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use log::{debug, info, warn};
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;

use crate::app::{ActivePanel, App, MessageRole};
use crate::gccp::TaskControl;
use crate::ui;

/// 键位处理出口信号：`Flow::Exit` 表示请求退出 TUI 主循环（Ctrl+C / F8 切 CLI），
/// `Flow::Continue` 表示回到主循环下一帧。
pub(crate) enum Flow {
    Continue,
    Exit,
}

/// 空闲态按键分派（主循环 Event::Key，仅 Press）。返回 `Exit` 时调用方
/// 立即退出主循环（终端守卫 Drop 还原）。
pub(crate) async fn idle_key<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    key: KeyEvent,
) -> Result<Flow> {
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            info!("User pressed Ctrl+C, shutting down...");
            app.shutdown().await?;
            return Ok(Flow::Exit);
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
        KeyCode::Esc if app.active_panel != ActivePanel::Chat => {
            debug!("Panel: Esc → return to Chat");
            app.active_panel = ActivePanel::Chat;
            // 与 toggle_panel 同源：离开 Board/Events 必须停 SSE，
            // 否则订阅泄漏，后台持续收流（空闲态 Esc 路径）。
            app.stop_hall_watch();
        }
        // IME 拼音态：Esc 取消拼音（微信语义：清空缓冲，放弃组合）
        KeyCode::Esc if app.ime_visible() => {
            app.ime_cancel();
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
        // F10：内置拼音输入法 中/英 切换（词典缺失时无效果）
        KeyCode::F(10) => {
            app.ime_toggle();
        }
        // F9：IME 备键（与 C CLI tui_ime.c 对齐，F10 被终端占用时可用）
        KeyCode::F(9) => {
            app.ime_toggle();
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
            let input = std::mem::take(&mut app.input);
            app.cursor = 0;
            debug!(
                "User submitted input: '{}' ({} chars)",
                truncate_str(&input, 80),
                input.len()
            );
            if let Err(e) = app.submit_input(&input) {
                warn!("submit_input error: {}", e);
                app.add_message(MessageRole::System, format!("Error: {}", e));
            }
            // 后台请求进行中 → 每 50ms 渲染 + 轮询：
            //   - thinking... 动效（chat.rs / ui.rs 按时间取帧，50ms 一帧更丝滑）
            //   - 回复到达后自动上屏（add_message 自动回到底部）
            //   - Ctrl+X 中止 / Ctrl+Z 暂停（等待期间可人工控制）
            //   - 工具权限审批：a=允许本次 · A=始终允许 · n=拒绝（Claude Code 风格）
            // 任务完成后若有插入对话队列，逐条 pop 处理（单 pending 槽，
            // 每条等其完成再处理下一条，逻辑链连续不割裂）。
            if matches!(insert_chat_pump(terminal, app).await?, Flow::Exit) {
                return Ok(Flow::Exit);
            }
            terminal.draw(|f| ui::render(f, app))?;
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
            } else {
                app.scroll_page_down();
            }
        }
        _ => {}
    }
    Ok(Flow::Continue)
}

/// busy 等待与插入对话泵（原 Enter 分支内嵌循环）：等待在途请求完成
/// （期间处理中止/暂停/审批/滚动/插入对话提交），请求完成后逐条消费
/// 插入队列。返回 `Exit` 表示用户请求退出（busy 期间 Ctrl+C）。
async fn insert_chat_pump<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<Flow> {
    loop {
        // ── 等待当前请求完成（期间可输入插入对话）──
        while app.is_busy() {
            terminal.draw(|f| ui::render(f, app))?;
            // 等待期间轮询按键：Ctrl+X 中止、Ctrl+Z 暂停/恢复、审批决议、
            // 任务执行中输入文本（Enter 提交 → 插入对话队列，任务不打断）
            if event::poll(Duration::ZERO)? {
                match event::read()? {
                    Event::Paste(text) => {
                        // busy 期间粘贴 → 插入输入框（Enter 提交为插入对话）
                        app.input_insert_text(&text);
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match key.code {
                            // 与空闲态一致：busy（等待回复/任务执行）期间按
                            // Ctrl+C 同样退出 TUI，不再被 busy 内层循环吞键
                            KeyCode::Char('c') | KeyCode::Char('C')
                                if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                            {
                                info!("User pressed Ctrl+C while busy, shutting down...");
                                app.shutdown().await?;
                                return Ok(Flow::Exit);
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
                            // 工具级权限审批（Claude Code 风格 permission prompt）
                            // 0.1.7：仅当存在待审批请求时生效——此前无条件拦截
                            // a/y/n/d/A，busy 期间输入含这些字母的文本会被吞字并
                            // 向对话区插入"没有待决议的权限请求"污染聊天。
                            KeyCode::Char('a') | KeyCode::Char('y')
                                if !app.approvals.is_empty()
                                    && !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                            {
                                app.approve_request("allow");
                            }
                            KeyCode::Char('A')
                                if !app.approvals.is_empty()
                                    && !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                            {
                                app.approve_request("always");
                            }
                            KeyCode::Char('n')
                            | KeyCode::Char('N')
                            | KeyCode::Char('d')
                            | KeyCode::Char('D')
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
                            // F10：内置拼音输入法切换（busy 插入对话场景同样可用）
                            KeyCode::F(10) => {
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
                            KeyCode::PageUp => app.scroll_page_up(),
                            KeyCode::PageDown => app.scroll_page_down(),
                            // Alt+E：打开思考链独立视图（0.1.18 B4：思考链
                            // 默认不上屏，仅显式请求时在此按需查看）
                            KeyCode::Char('e') | KeyCode::Char('E')
                                if key.modifiers.contains(event::KeyModifiers::ALT)
                                    && !app.ime_visible() =>
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
                                if key.modifiers.contains(event::KeyModifiers::ALT)
                                    && !app.ime_visible() =>
                            {
                                app.browse_expanded = !app.browse_expanded;
                            }
                            // IME 拼音态：Esc 取消拼音（微信语义）
                            KeyCode::Esc if app.ime_visible() => {
                                app.ime_cancel();
                            }
                            // 非拼音态 Esc：与空闲态一致返回对话（busy 期间
                            // 在只读面板/看板上也能 Esc 退出，不困在面板里）
                            KeyCode::Esc => {
                                app.active_panel = ActivePanel::Chat;
                                // busy 态同样必须停 SSE（订阅泄漏同源修复）
                                app.stop_hall_watch();
                            }
                            // ── 插入对话（2.3.7）：任务执行中输入文本 ──
                            // 只读面板（Help/Config/Logs/Memory/Plugins）：
                            // Enter 不入插入队列——共享输入只属于对话面板
                            KeyCode::Enter if read_only_panel(app.active_panel) => {}
                            KeyCode::Enter => {
                                // 拼音态：先提交拼音原文（随后提交整行）
                                app.ime_commit_enter();
                                let input = std::mem::take(&mut app.input);
                                app.cursor = 0;
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
                                if (key.modifiers.is_empty()
                                    || key.modifiers == event::KeyModifiers::SHIFT)
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
                    }
                    _ => {}
                }
            }
            app.poll_pending();
            if app.is_busy() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        // 队列空：结束处理；否则取一条提交（submit_input 会置 busy）
        let Some(msg) = app.insert_queue.pop_front() else {
            break;
        };
        if let Err(e) = app.submit_input(&msg) {
            log::warn!("insert queue submit failed: {}", e);
            app.add_message(MessageRole::System, format!("插入对话处理失败：{}", e));
        }
    }
    Ok(Flow::Continue)
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // T-02（P0-7 同族修复）：字节截断可能落在 UTF-8 多字节字符中间导致
    // panic（中文输入 > max 字节必现）。回退到最近的前序字符边界。
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// 只读展示面板：Help/Config/Logs/Memory/Plugins/Think/Focus 无输入语义，
/// Enter 与普通字符键不应写入或提交对话面板的共享输入（Board/Events 有
/// 各自的选中/过滤键位，不在此列）。
fn read_only_panel(p: ActivePanel) -> bool {
    matches!(
        p,
        ActivePanel::Help
            | ActivePanel::Config
            | ActivePanel::Logs
            | ActivePanel::Memory
            | ActivePanel::Plugins
            | ActivePanel::Think
            | ActivePanel::Focus
    )
}

#[cfg(test)]
mod tests {
    use super::truncate_str;

    #[test]
    fn truncate_str_never_splits_multibyte_chars() {
        // T-02（P0-7 同族）：中文输入 80 字节截断窗——修复前第 80 字节
        // 落在多字节字符中间时 panic。
        let s = "汉".repeat(40); // 120 字节
        let out = truncate_str(&s, 80);
        assert_eq!(out, "汉".repeat(26)); // 78 字节处边界回退
        assert_eq!(out.len(), 78);

        // 短文本原样返回
        assert_eq!(truncate_str("hello", 80), "hello");
        assert_eq!(truncate_str("中文", 80), "中文");
        // 恰好在边界
        assert_eq!(truncate_str("中", 3), "中");
        // 截断点为 4 字节 emoji 中间时回退到 0
        assert_eq!(truncate_str("\u{1F600}", 2), "");
        // 空串
        assert_eq!(truncate_str("", 10), "");
    }
}
