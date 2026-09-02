// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 向导状态机：步骤流转、编辑、预设联动、完成写回。字段语义全部来自
// `steps` 注册表。编辑态统一字节索引（0.1.8 根因修复保留：字节/字符混用
// 曾致 CJK 字段 Backspace panic），光标按可见位置计数（local 隐藏 API Key
// 后不再可停到隐藏行）。

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent};

use crate::models_cfg::{self, ModelRow, ThinkCfg};

use super::lang::Lang;
use super::persist::{self, API_KEY_ENV};
use super::presets::{self, PresetSource};
use super::steps::{
    choice_len, cycle_value, form_step, is_form_step, seed_value, FieldKey, FieldKind, StepSpec,
    LANG_CHOICES, TOTAL_STEPS,
};
use super::text::{next_char_boundary, prev_char_boundary};

/// 表单项：值 + 是否被用户手动编辑（预设自动填充不置位）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormField {
    pub value: String,
    pub touched: bool,
}

/// 向导完成结果（供 App 使用）
#[derive(Debug, Clone)]
pub struct WizardResult {
    /// true = 完成快速配置；false = 跳过先探索
    pub configured: bool,
    pub model: String,
    pub provider: String,
    pub api_key_set: bool,
    pub think_enabled: bool,
}

/// 首次启动向导状态
pub struct WizardState {
    pub active: bool,
    pub step: u8,
    pub result: Option<WizardResult>,

    pub(crate) choice_cursor: usize,
    pub(crate) effective_lang: Lang,
    pub(crate) form: Vec<FormField>,
    pub(crate) field_cursor: usize,
    pub(crate) editing: bool,
    pub(crate) edit_pos: usize,
    pub(crate) snapshots: BTreeMap<u8, Vec<FormField>>,
}

impl WizardState {
    /// 新建：首次运行（wizard.toml 不存在）自动激活；字段值进入步骤时现取
    pub fn new() -> Self {
        let active = persist::is_first_run();
        if active {
            log::info!("wizard: first run detected, auto-activating");
        }
        Self {
            active,
            step: 1,
            result: None,
            choice_cursor: 0,
            effective_lang: Lang::detect(),
            form: Vec::new(),
            field_cursor: 0,
            editing: false,
            edit_pos: 0,
            snapshots: BTreeMap::new(),
        }
    }

    /// 重新打开向导（/hiairy；即使已完成也允许）
    pub fn reopen(&mut self) {
        self.active = true;
        self.step = 1;
        self.result = None;
        self.choice_cursor = 0;
        self.effective_lang = Lang::detect();
        self.form.clear();
        self.field_cursor = 0;
        self.editing = false;
        self.edit_pos = 0;
        self.snapshots.clear();
        log::info!("wizard: reopened via /hiairy");
    }

    /// 处理按键；返回 true 表示向导已关闭（完成或跳过）
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if !self.active {
            return false;
        }
        match key.code {
            KeyCode::Char(c) if self.editing => self.edit_char(c),
            KeyCode::Backspace if self.editing => self.edit_pop(),
            KeyCode::Left if self.editing => self.edit_move(-1),
            KeyCode::Right if self.editing => self.edit_move(1),
            KeyCode::Home if self.editing => self.edit_head(),
            KeyCode::End if self.editing => self.edit_tail(),
            KeyCode::Enter if self.editing => self.edit_done(),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Enter => return self.confirm(),
            KeyCode::Tab => self.cycle(),
            KeyCode::Char(c)
                if !self.editing && self.step <= 2 && c.is_ascii_digit() && c != '0' =>
            {
                let n = (c as u8 - b'0') as usize;
                if n <= choice_len(self.step) {
                    self.choice_cursor = n - 1;
                    return self.confirm();
                }
            }
            KeyCode::Esc => return self.on_esc(),
            _ => {}
        }
        false
    }

    /// 粘贴（bracketed paste）：编辑态插入当前字段（剥离换行）
    pub fn handle_paste(&mut self, text: &str) {
        if !self.active || !self.editing {
            return;
        }
        let Some(i) = self.cursor_index() else { return };
        let cleaned: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        let pos = self.edit_pos.min(self.form[i].value.len());
        self.form[i].value.insert_str(pos, &cleaned);
        self.edit_pos = pos + cleaned.len();
        self.form[i].touched = true;
    }

    // ─────────── 编辑态 ───────────

    fn edit_char(&mut self, c: char) {
        let Some(i) = self.cursor_index() else { return };
        let pos = self.edit_pos.min(self.form[i].value.len());
        self.form[i].value.insert(pos, c);
        self.edit_pos = pos + c.len_utf8();
        self.form[i].touched = true;
    }

    fn edit_pop(&mut self) {
        let Some(i) = self.cursor_index() else { return };
        if self.edit_pos == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.form[i].value, self.edit_pos);
        self.form[i].value.remove(prev);
        self.edit_pos = prev;
        self.form[i].touched = true;
    }

    fn edit_move(&mut self, dir: i8) {
        let Some(i) = self.cursor_index() else {
            self.edit_pos = 0;
            return;
        };
        self.edit_pos = if dir < 0 {
            prev_char_boundary(&self.form[i].value, self.edit_pos)
        } else {
            next_char_boundary(&self.form[i].value, self.edit_pos)
        };
    }

    fn edit_head(&mut self) {
        self.edit_pos = 0;
    }

    fn edit_tail(&mut self) {
        if let Some(i) = self.cursor_index() {
            self.edit_pos = self.form[i].value.len();
        }
    }

    /// 结束编辑：提供商字段应用预设，光标下移（封顶动作按钮位）
    fn edit_done(&mut self) {
        self.editing = false;
        self.edit_pos = 0;
        let Some(i) = self.cursor_index() else { return };
        if matches!(self.kind_at(i), Some(FieldKind::Provider)) {
            self.preset_fill(true);
        }
        self.field_cursor = (self.field_cursor + 1).min(self.vis_len());
    }

    // ─────────── 光标与步骤 ───────────

    fn spec(&self) -> Option<&'static StepSpec> {
        form_step(self.step)
    }

    /// 可见字段下标（local 隐藏 API Key；选项步空）
    fn visible(&self) -> Vec<usize> {
        match self.spec() {
            Some(s) => s.visible(self.mode_local()),
            None => Vec::new(),
        }
    }

    fn vis_len(&self) -> usize {
        self.visible().len()
    }

    /// 光标所在字段下标（动作按钮位 = None）
    fn cursor_index(&self) -> Option<usize> {
        self.visible().get(self.field_cursor).copied()
    }

    fn key_idx(&self, key: FieldKey) -> Option<usize> {
        let i = self.spec()?.field_idx(key)?;
        if i < self.form.len() {
            Some(i)
        } else {
            None
        }
    }

    fn kind_at(&self, idx: usize) -> Option<FieldKind> {
        Some(self.spec()?.fields.get(idx)?.kind)
    }

    /// 当前步字段 trim 值（不在当前步 → 空串）
    fn trim_of(&self, key: FieldKey) -> String {
        self.key_idx(key)
            .map(|i| self.form[i].value.trim().to_string())
            .unwrap_or_default()
    }

    /// 步骤 3 连接方式是否 local（视图/字段显隐）
    pub(crate) fn mode_local(&self) -> bool {
        self.trim_of(FieldKey::Mode) == "local"
    }

    /// 步骤 3 已选模型 ID（思考角色无 default_model 时回落）
    fn model_hint(&self) -> String {
        let spec = match form_step(3) {
            Some(s) => s,
            None => return String::new(),
        };
        let idx = match spec.field_idx(FieldKey::ModelId) {
            Some(i) => i,
            None => return String::new(),
        };
        let src = if self.step == 3 {
            Some(&self.form)
        } else {
            self.snapshots.get(&3)
        };
        match src.and_then(|v| v.get(idx)) {
            Some(f) if !f.value.trim().is_empty() => f.value.trim().to_string(),
            _ => String::new(),
        }
    }

    /// 移动光标：表单步 0..=vis_len（末位=动作）；选项步 0..=choice_len-1
    fn move_cursor(&mut self, delta: i8) {
        if let Some(spec) = self.spec() {
            if self.editing {
                return;
            }
            let max = spec.visible(self.mode_local()).len();
            self.field_cursor = if delta < 0 {
                self.field_cursor.saturating_sub(1)
            } else {
                (self.field_cursor + 1).min(max)
            };
            return;
        }
        let max = choice_len(self.step).saturating_sub(1);
        self.choice_cursor = if delta < 0 {
            self.choice_cursor.saturating_sub(1)
        } else {
            (self.choice_cursor + 1).min(max)
        };
    }

    /// Esc：表单步编辑态→退出；否则快照回上一步；选项步→跳过向导（不持久化）
    fn on_esc(&mut self) -> bool {
        if is_form_step(self.step) {
            if self.editing {
                self.editing = false;
                self.edit_pos = 0;
            } else {
                self.leave_step();
                self.step -= 1;
                if is_form_step(self.step) {
                    self.enter_form();
                } else {
                    self.choice_cursor = 0;
                }
            }
            return false;
        }
        log::info!("wizard: skipped via Esc");
        self.active = false;
        true
    }

    /// Enter 确认：步骤 1 定语言 / 步骤 2 分流 / 表单步走字段-动作分派
    fn confirm(&mut self) -> bool {
        match self.step {
            1 => {
                let lang = LANG_CHOICES
                    .get(self.choice_cursor.min(LANG_CHOICES.len() - 1))
                    .copied()
                    .unwrap_or(Lang::Auto)
                    .resolve();
                self.effective_lang = lang;
                log::info!("wizard: step1 confirmed, lang={:?}", lang);
                self.step = 2;
                self.choice_cursor = 0;
                false
            }
            2 => {
                if self.choice_cursor == 0 {
                    self.step = 3;
                    self.enter_form();
                    false
                } else {
                    self.finish(false)
                }
            }
            _ => self.form_confirm(),
        }
    }

    fn form_confirm(&mut self) -> bool {
        if let Some(i) = self.cursor_index() {
            if self.kind_at(i).is_some_and(|k| k.editable()) {
                self.editing = true;
                self.edit_pos = self.form[i].value.len();
                self.form[i].touched = true;
            } else {
                self.cycle();
            }
            return false;
        }
        self.advance()
    }

    fn advance(&mut self) -> bool {
        self.leave_step();
        if self.step < TOTAL_STEPS {
            self.step += 1;
            self.enter_form();
            false
        } else {
            self.finish(true)
        }
    }

    /// 进入表单步骤：优先恢复快照（保留 touched），否则按注册表播种
    fn enter_form(&mut self) {
        self.field_cursor = 0;
        self.editing = false;
        self.edit_pos = 0;
        let Some(spec) = self.spec() else { return };
        if let Some(snap) = self.snapshots.get(&self.step) {
            if snap.len() == spec.fields.len() {
                self.form = snap.clone();
                return;
            }
        }
        let m = models_cfg::read_model_yaml();
        let hint = self.model_hint();
        self.form = spec
            .fields
            .iter()
            .map(|f| FormField {
                value: seed_value(f.key, &m, &hint),
                touched: false,
            })
            .collect();
    }

    /// 离开表单步骤：strict 校验 + 空白预设补全 + 快照（0.1.8：下一步与
    /// Esc 返回两条路径统一，往返不丢编辑；空白补全修复直通 finish 空行）
    fn leave_step(&mut self) {
        self.normalize();
        self.preset_fill(false);
        if !self.form.is_empty() {
            let snap = self.form.clone();
            self.snapshots.insert(self.step, snap);
        }
    }

    /// strict 选项字段收敛到候选集（防外部 yaml 脏值写回）
    fn normalize(&mut self) {
        let Some(spec) = self.spec() else { return };
        for (i, f) in spec.fields.iter().enumerate().take(self.form.len()) {
            if let FieldKind::Options { opts, strict: true } = f.kind {
                let cur = self.form[i].value.trim().to_string();
                if !opts.contains(&cur.as_str()) {
                    self.form[i].value = opts[0].to_string();
                }
            }
        }
    }

    // ─────────── 循环与预设 ───────────

    /// Tab / 非编辑字段 Enter 的循环（cycle 不置 touched，防预设回填）
    fn cycle(&mut self) {
        if self.editing {
            return;
        }
        let Some(i) = self.cursor_index() else { return };
        match self.kind_at(i) {
            Some(FieldKind::Options { opts, strict }) => {
                self.form[i].value = cycle_value(&self.form[i].value, opts, strict);
            }
            Some(FieldKind::Provider) => self.cycle_provider(),
            Some(FieldKind::PresetModels) => self.cycle_model(i),
            _ => {}
        }
    }

    fn cycle_provider(&mut self) {
        let Some(i) = self.key_idx(FieldKey::Provider) else { return };
        let next = match presets::preset_index(&self.form[i].value) {
            Some(p) => presets::PRESETS[(p + 1) % presets::PRESETS.len()].id,
            None => presets::PRESETS[0].id,
        };
        self.form[i].value = next.to_string();
        self.preset_fill(true);
    }

    /// 模型 ID 在当前提供商候选内循环（未命中候选保留）
    fn cycle_model(&mut self, idx: usize) {
        let models = self.provider_preset().models;
        let cur = self.form[idx].value.trim().to_string();
        if let Some(pos) = models.iter().position(|m| *m == cur.as_str()) {
            self.form[idx].value = models[(pos + 1) % models.len()].to_string();
        } else if cur.is_empty() {
            self.form[idx].value = models[0].to_string();
        }
    }

    fn provider_preset(&self) -> &'static presets::Preset {
        match self.key_idx(FieldKey::Provider) {
            Some(i) => presets::preset_by(&self.form[i].value),
            None => presets::preset_by(""),
        }
    }

    /// 按当前提供商预设回填字段。follow=true（Tab/编辑结束）：mode/format
    /// 恒覆盖、其余 !touched；follow=false（离步）：仅填空白不冲已选值
    fn preset_fill(&mut self, follow: bool) {
        let Some(spec) = self.spec() else { return };
        let p = self.provider_preset();
        for (i, f) in spec.fields.iter().enumerate() {
            let Some(src) = f.preset else { continue };
            if i >= self.form.len() {
                continue;
            }
            let v = presets::preset_text(p, src).to_string();
            if follow {
                if src == PresetSource::Mode
                    || src == PresetSource::ApiFormat
                    || !self.form[i].touched
                {
                    self.form[i].value = v;
                }
            } else if self.form[i].value.trim().is_empty() {
                self.form[i].value = v;
            }
        }
    }

    // ─────────── 取值与完成 ───────────

    /// 完成取值：已进入步骤（当前 form/快照）原样 trim（允许留空）；
    /// 未进入步骤 → model.yaml 现值 → 注册表默认
    fn value_of(&self, step: u8, key: FieldKey) -> String {
        let spec = match form_step(step) {
            Some(s) => s,
            None => return String::new(),
        };
        let idx = match spec.field_idx(key) {
            Some(i) => i,
            None => return String::new(),
        };
        let src = if step == self.step {
            Some(&self.form)
        } else {
            self.snapshots.get(&step)
        };
        match src.and_then(|v| v.get(idx)) {
            Some(field) => field.value.trim().to_string(),
            None => seed_value(key, &models_cfg::read_model_yaml(), &self.model_hint()),
        }
    }

    /// 完成向导：跳过（仅写 wizard.toml）或快速配置（写回 secrets.env +
    /// model.yaml + wizard.toml，llm_d/think_d 热加载）
    fn finish(&mut self, configured: bool) -> bool {
        self.normalize();
        let lang_code = self.effective_lang.code().to_string();
        if configured {
            let provider = self.value_of(3, FieldKey::Provider);
            let mut name = self.value_of(3, FieldKey::Name);
            let mode = self.value_of(3, FieldKey::Mode);
            let api_format = self.value_of(3, FieldKey::ApiFormat);
            let base_url = self.value_of(3, FieldKey::BaseUrl);
            let model_id = self.value_of(3, FieldKey::ModelId);
            let api_key = self.value_of(3, FieldKey::ApiKey);
            let context_window = self.value_of(4, FieldKey::CtxWindow);
            let max_output = self.value_of(4, FieldKey::MaxOutput);
            let tool_rounds = self.value_of(4, FieldKey::ToolRounds);
            let vision = self.value_of(4, FieldKey::Vision);
            let thinking = self.value_of(4, FieldKey::Thinking);
            let think_enabled = self.value_of(5, FieldKey::ThinkEnabled) == "true";
            let slow_model = self.value_of(5, FieldKey::SlowModel);
            let fast_model = self.value_of(5, FieldKey::FastModel);
            let prof_model = self.value_of(5, FieldKey::ProfModel);
            if name.is_empty() {
                name = model_id.clone();
            }
            let api_key_env = if mode == "local" {
                String::new()
            } else {
                API_KEY_ENV.to_string()
            };
            let api_key_set = !api_key_env.is_empty()
                && !api_key.is_empty()
                && persist::write_secret(&api_key_env, &api_key);
            let row = ModelRow {
                name,
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
            persist::persist(&lang_code, true, &provider, &model_id);
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
            persist::persist(&lang_code, false, "", "");
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
}
