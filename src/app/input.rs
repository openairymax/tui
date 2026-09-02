// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 输入编辑：光标移动与增删改、历史回溯、Tab 补全、IME 拼音候选联动。

use super::*;

impl App {
    /// 记录一条输入历史（去重：与最近一条相同则跳过；容量 50）。
    pub(super) fn push_history(&mut self, input: &str) {
        let t = input.trim();
        if t.is_empty() {
            return;
        }
        if self.input_history.last().map(|s| s.as_str()) == Some(t) {
            return;
        }
        self.input_history.push(t.to_string());
        if self.input_history.len() > 50 {
            self.input_history.remove(0);
        }
        // 新提交后回到手输状态
        self.history_pos = None;
    }

    /// 浏览上一条输入历史（Alt+↑；历史空时无操作）。
    pub fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            Some(p) => p.saturating_sub(1),
            None => self.input_history.len() - 1,
        };
        self.history_pos = Some(next);
        self.input = self.input_history[next].clone();
        // 历史回填后光标置于末尾（readline 惯例）
        self.cursor = self.input.len();
    }

    /// 浏览下一条输入历史（Alt+↓；越界回到手输状态并清空输入）。
    pub fn history_next(&mut self) {
        let Some(p) = self.history_pos else {
            return;
        };
        if p + 1 < self.input_history.len() {
            self.history_pos = Some(p + 1);
            self.input = self.input_history[p + 1].clone();
        } else {
            // 已到最后一条 → 回到手输状态
            self.history_pos = None;
            self.input.clear();
        }
        // 历史回填后光标置于末尾（readline 惯例）
        self.cursor = self.input.len();
    }

    // ─────────── 内置拼音输入法（F10 切换，语义与 CLI cli_tui.c 对齐） ───────────

    /// 输入框是否进入两行模式：引擎就绪 && 拼音态。
    /// 2.2.3 重新设计（2026-08-23）：F10 激活立即把输入框扩为两行
    /// （第一行输入 + 第二行候选区），即使拼音缓冲为空也有 [中] 模式
    /// 指示——此前仅缓冲非空时占行，激活瞬间无任何视觉反馈，用户
    /// 感知为"F10 没反应"。
    pub fn ime_visible(&self) -> bool {
        self.ime_engine.is_some() && self.ime_active
    }

    /// F10 切换中/英。切回英文时把拼音原文上屏（保留在输入行）。
    /// 词典缺失/库未链接（engine None）时无效果。
    pub fn ime_toggle(&mut self) {
        if self.ime_engine.is_none() {
            return;
        }
        self.ime_active = !self.ime_active;
        if !self.ime_active {
            self.ime_commit_raw();
        }
    }

    /// 以当前拼音缓冲刷新候选列表（微信式分页 0.1.3：一次取 27 个，
    /// 3 页 × 9；拼音变化后页码/高亮归零——新上下文从第一页首候选开始）。
    pub(super) fn ime_refresh(&mut self) {
        if let Some(eng) = &self.ime_engine {
            self.ime_cands = eng.query(&self.ime_buf);
        } else {
            self.ime_cands.clear();
        }
        self.ime_pages = (self.ime_cands.len().max(1) + 8) / 9;
        self.ime_page = self.ime_page.min(self.ime_pages.saturating_sub(1));
        self.ime_sel = self.ime_sel.min(8);
    }

    /// 拼音原文上屏（插入输入行光标处），清空拼音缓冲与候选。
    pub fn ime_commit_raw(&mut self) {
        if !self.ime_buf.is_empty() {
            let buf = std::mem::take(&mut self.ime_buf);
            self.input_insert_text(&buf);
        }
        self.ime_buf.clear();
        self.ime_cands.clear();
        self.ime_page = 0;
        self.ime_pages = 1;
        self.ime_sel = 0;
    }

    /// 候选字上屏：清空拼音缓冲并保持拼音模式（连续词组输入不中断）。
    pub(super) fn ime_commit_cand(&mut self, idx: usize) {
        if let Some(text) = self.ime_cands.get(idx).cloned() {
            self.input_insert_text(&text);
        }
        self.ime_buf.clear();
        self.ime_cands.clear();
        self.ime_page = 0;
        self.ime_pages = 1;
        self.ime_sel = 0;
    }

    /// 当前高亮候选在候选池中的绝对下标（None = 无候选）。
    pub fn ime_sel_index(&self) -> Option<usize> {
        let idx = self.ime_page * 9 + self.ime_sel;
        if idx < self.ime_cands.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// 翻页（微信式：,/. 或 PgUp/PgDn）。越界回绕。
    pub fn ime_page_flip(&mut self, dir: isize) {
        if self.ime_pages <= 1 {
            return;
        }
        let pages = self.ime_pages as isize;
        let mut p = self.ime_page as isize + dir;
        if p < 0 {
            p = pages - 1;
        }
        if p >= pages {
            p = 0;
        }
        self.ime_page = p as usize;
    }

    /// 页内高亮移动（微信式：←/→ 选中候选）。
    pub fn ime_move_sel(&mut self, dir: isize) {
        let start = self.ime_page * 9;
        let page_cnt = self.ime_cands.len().saturating_sub(start).min(9);
        if page_cnt == 0 {
            return;
        }
        let mut s = self.ime_sel as isize + dir;
        if s < 0 {
            s = 0;
        }
        if s >= page_cnt as isize {
            s = page_cnt as isize - 1;
        }
        self.ime_sel = s as usize;
    }

    /// 取消拼音（微信语义：清空缓冲，放弃组合，退出拼音态）。
    pub fn ime_cancel(&mut self) {
        self.ime_buf.clear();
        self.ime_cands.clear();
        self.ime_page = 0;
        self.ime_pages = 1;
        self.ime_sel = 0;
        self.ime_active = false;
    }

    /// 拼音态按键处理（仅 ime_active 时调用）。返回 true = 已消费该键。
    pub fn ime_input_char(&mut self, c: char) -> bool {
        if !self.ime_active || self.ime_engine.is_none() {
            return false;
        }
        match c {
            'a'..='z' => {
                self.ime_buf.push(c);
                self.ime_refresh();
            }
            '1'..='9' => {
                // 数字选字：当前页内第 N 个候选（微信式分页）
                let i = (c as usize) - ('1' as usize);
                let idx = self.ime_page * 9 + i;
                if idx < self.ime_cands.len() {
                    self.ime_commit_cand(idx);
                }
            }
            ' ' => {
                // 空格：上屏高亮候选（微信式，默认高亮第一个）
                if let Some(idx) = self.ime_sel_index() {
                    self.ime_commit_cand(idx);
                } else {
                    // 无候选：空格输出拼音原文
                    self.ime_commit_raw();
                }
            }
            ',' | '.' => {
                // 翻页（微信式：, 上一页 / . 下一页）；单页时标点走正常路径
                if self.ime_pages > 1 {
                    self.ime_page_flip(if c == '.' { 1 } else { -1 });
                } else {
                    self.ime_commit_raw();
                    self.ime_active = false;
                    return false;
                }
            }
            _ => {
                // 标点/数字等：先提交拼音原文并退出拼音模式，按键继续
                // 走正常输入路径（由调用方在返回 false 后处理）
                self.ime_commit_raw();
                self.ime_active = false;
                return false;
            }
        }
        true
    }

    /// 拼音态退格：删拼音（空则退出拼音态）。返回 true = 已消费。
    pub fn ime_backspace(&mut self) -> bool {
        if !self.ime_active {
            return false;
        }
        if !self.ime_buf.is_empty() {
            self.ime_buf.pop();
            self.ime_refresh();
        } else {
            self.ime_active = false;
        }
        true
    }

    /// 拼音态 Enter 提交（微信语义）：有候选时上屏高亮候选，无候选时
    /// 提交拼音原文；随后退出拼音态（由调用方提交整行）。返回 true =
    /// 已有拼音态需要先提交。
    pub fn ime_commit_enter(&mut self) -> bool {
        if !self.ime_active {
            return false;
        }
        if let Some(idx) = self.ime_sel_index() {
            self.ime_commit_cand(idx);
        } else {
            self.ime_commit_raw();
        }
        self.ime_active = false;
        true
    }

    // ─────────── 输入编辑（光标感知，readline 风格） ───────────

    /// 在光标处插入一个字符。
    pub fn input_insert_char(&mut self, c: char) {
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.insert(pos, c);
        self.cursor = pos + c.len_utf8();
    }

    /// 在光标处插入多字节文本（Alt+Enter 换行用）。
    pub fn input_insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.insert_str(pos, text);
        self.cursor = pos + text.len();
    }

    /// Backspace：删除光标前一个字符。
    pub fn input_backspace(&mut self) {
        if self.cursor == 0 || self.input.is_empty() {
            self.cursor = 0;
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        // 回退一个字符边界
        let start = match self.input[..pos].char_indices().next_back() {
            Some((i, _)) => i,
            None => return,
        };
        self.input.drain(start..pos);
        self.cursor = start;
    }

    /// Delete：删除光标后一个字符。
    pub fn input_delete_after(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos >= self.input.len() || !self.input.is_char_boundary(pos) {
            return;
        }
        let end = match self.input[pos..].char_indices().nth(1) {
            Some((i, _)) => pos + i,
            None => self.input.len(),
        };
        self.input.drain(pos..end);
    }

    /// ←：光标左移一个字符。
    pub fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        self.cursor = match self.input[..pos].char_indices().next_back() {
            Some((i, _)) => i,
            None => 0,
        };
    }

    /// →：光标右移一个字符。
    pub fn cursor_right(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos >= self.input.len() || !self.input.is_char_boundary(pos) {
            return;
        }
        self.cursor = match self.input[pos..].char_indices().nth(1) {
            Some((i, _)) => pos + i,
            None => self.input.len(),
        };
    }

    /// Home / Ctrl+A：光标到开头。
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// End / Ctrl+E：光标到末尾。
    pub fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Ctrl+W：删除光标前一个词（空白分隔）。
    pub fn input_delete_word_before(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos == 0 || !self.input.is_char_boundary(pos) {
            return;
        }
        let before = &self.input[..pos];
        // 先跳过词尾空白，再删到词首
        let mut end = before.len();
        while end > 0 {
            let prev = before[..end].chars().next_back().unwrap();
            if !prev.is_whitespace() {
                break;
            }
            end -= prev.len_utf8();
        }
        while end > 0 {
            let prev = before[..end].chars().next_back().unwrap();
            if prev.is_whitespace() {
                break;
            }
            end -= prev.len_utf8();
        }
        self.input.drain(end..pos);
        self.cursor = end;
    }

    /// Ctrl+U：删除光标前全部内容。
    pub fn input_delete_to_start(&mut self) {
        let pos = self.cursor.min(self.input.len());
        if pos == 0 || !self.input.is_char_boundary(pos) {
            return;
        }
        self.input.drain(..pos);
        self.cursor = 0;
    }

    /// Tab 补全：补全 / 命令或技能名。
    ///
    /// 取光标前的当前词，按前缀匹配候选；再次 Tab 在当前候选间循环
    /// （当前词已等于某候选时取下一个，天然支持循环，无需额外状态）。
    pub fn tab_complete(&mut self) {
        // 候选：/ 命令 + 本地技能名
        let mut cands: Vec<String> = vec![
            "/model".into(),
            "/set-key".into(),
            "/hiairy".into(),
            "/help".into(),
            "/clear".into(),
            "/status".into(),
            "/memory".into(),
            "/skills".into(),
            "/board".into(),
            "/events".into(),
            "/chain".into(),
            "/daemons".into(),
            "/agents".into(),
            "/tools".into(),
            "/models".into(),
            "/mem".into(),
            "/rpc".into(),
        ];
        cands.extend(self.skills.list().into_iter().map(|s| s.name));
        if cands.is_empty() {
            return;
        }

        // 光标前的当前词（最后一段空白分隔 token）
        let pos = self.cursor.min(self.input.len());
        if !self.input.is_char_boundary(pos) {
            return;
        }
        let before = &self.input[..pos];
        let word_start = before.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        let prefix = before[word_start..].to_string();

        let matches: Vec<&String> = cands
            .iter()
            .filter(|c| c.starts_with(&prefix))
            .collect();
        if matches.is_empty() {
            return;
        }
        // 当前词已等于某候选 → 取下一个；否则取第一个匹配
        let next = match matches.iter().position(|c| c.as_str() == prefix) {
            Some(p) => matches[(p + 1) % matches.len()].clone(),
            None => matches[0].clone(),
        };
        self.input.replace_range(word_start..pos, &next);
        self.cursor = word_start + next.len();
    }
}
