// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// TUI 当前模型读写：统一落到配置权威源 $AIRY_HOME/config/model.yaml 的顶层
// `default_model:`（与 llm_d / think_d / gateway_d 同源，见 models_cfg）。
// 0.1.16 架构改造移除 TUI 独占的 $AIRY_HOME/data/agentrt/tui/config.toml，
// 当前模型不再单独持久化——它只是 model.yaml 的 default_model 的读写视图。

use super::{ActivePanel, App};

/// AIRY_HOME（用于展示统一配置文件路径）
pub(super) fn airy_home() -> String {
    crate::paths::airy_home().to_string_lossy().into_owned()
}

/// 配置面板的执行动作。
///
/// 面板不另建写回路径：模型切换与 `/model` 同落 `model.yaml` 的
/// `default_model`，双思考与 `/hiairy` 同落 `models[].think`，Key 与
/// `/set-key` 同一入口。动作清单在此单点构造，渲染与 Enter 分发共用，
/// 避免"看得见的一行"与"按下去执行的那一行"各写一份而错位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CfgAction {
    /// 将某模型设为当前对话模型
    SetModel(String),
    /// 双思考总开关（携带当前的"是否开启"，执行时取反）
    Think(bool),
    /// 深/浅主题互换（仅本次会话，主题来自环境变量与启动期初始化）
    Theme,
    /// 预填 /set-key，密钥值仍由输入栏键入，不进面板状态
    SetKey,
    /// 重开五步配置向导
    Wizard,
}

impl CfgAction {
    /// 动作行文案（与执行同一份定义，随当前配置实时变化）。
    pub(crate) fn label(&self) -> String {
        match self {
            CfgAction::SetModel(name) => format!("设为当前模型：{}", name),
            CfgAction::Think(true) => "双思考：关闭".to_string(),
            CfgAction::Think(false) => "双思考：开启".to_string(),
            CfgAction::Theme => match crate::style::ThemeMode::current() {
                crate::style::ThemeMode::Dark => "主题：切换为浅色".to_string(),
                crate::style::ThemeMode::Light => "主题：切换为深色".to_string(),
            },
            CfgAction::SetKey => "设置模型 API Key（预填 /set-key）".to_string(),
            CfgAction::Wizard => "重开配置向导（/hiairy）".to_string(),
        }
    }
}

/// 当前可选动作：逐个模型行在前，其后是双思考、主题、Key、向导。
///
/// 双思考三态（未声明 / 开 / 关）：未声明按"开"呈现——llm_d 缺省即开启，
/// 面板不能给出与实际相反的暗示。
pub(crate) fn cfg_actions() -> Vec<CfgAction> {
    let m = crate::models_cfg::read_model_yaml();
    let mut out: Vec<CfgAction> = m
        .rows
        .iter()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| CfgAction::SetModel(r.name.trim().to_string()))
        .collect();
    if !m.rows.is_empty() {
        let on = m.think.as_ref().and_then(|t| t.enabled).unwrap_or(true);
        out.push(CfgAction::Think(on));
    }
    out.extend([CfgAction::Theme, CfgAction::SetKey, CfgAction::Wizard]);
    out
}

/// 加载当前模型名（model.yaml 的 default_model；缺省/为空时返回 None，
/// 表示由网关 / llm_d 自动回落默认）。
pub(super) fn load_saved_model() -> Option<String> {
    let m = crate::models_cfg::read_model_yaml();
    if m.default_model.trim().is_empty() {
        None
    } else {
        Some(m.default_model.trim().to_string())
    }
}

/// 持久化当前模型名到 model.yaml 的 default_model（保留文件其余内容）。
pub(super) fn persist_model(model: &str) {
    match crate::models_cfg::set_default_model(model) {
        Ok(()) => log::info!(
            "model config: default_model → {}（{}）",
            model.trim(),
            crate::models_cfg::model_yaml_path().display()
        ),
        Err(e) => log::warn!("model config: {}", e),
    }
}

/// 配置面板 PgUp/PgDn 一次翻过的行数（与健康面板的翻页粒度一致）。
const CFG_PAGE: u16 = 5;

impl App {
    /// 配置面板：↑ 上移选中动作，到顶回到底部（循环，行数随配置变化）。
    pub fn cfg_sel_up(&mut self) {
        let n = cfg_actions().len();
        if n > 0 {
            self.cfg_sel = (self.cfg_sel + n - 1) % n;
        }
    }

    /// 配置面板：↓ 下移选中动作，到底回到顶部。
    pub fn cfg_sel_down(&mut self) {
        let n = cfg_actions().len();
        if n > 0 {
            self.cfg_sel = (self.cfg_sel + 1) % n;
        }
    }

    pub fn cfg_scroll_up(&mut self) {
        self.cfg_scroll = self.cfg_scroll.saturating_sub(CFG_PAGE);
    }

    pub fn cfg_scroll_down(&mut self) {
        self.cfg_scroll = self.cfg_scroll.saturating_add(CFG_PAGE);
    }

    /// 配置面板：Enter 执行选中动作。
    ///
    /// 动作清单在按键当场重取：配置可能被 `/hiairy`、`/model` 或外部编辑
    /// 改动，用渲染时缓存的清单会执行到上一帧的那一行。
    pub fn cfg_activate(&mut self) {
        let Some(act) = cfg_actions().get(self.cfg_sel).cloned() else {
            self.add_log("WARN", "配置面板：当前没有可执行动作".to_string());
            return;
        };
        match act {
            CfgAction::SetModel(name) => self.cmd_model(&format!("/model {}", name)),
            CfgAction::Think(on) => self.cfg_toggle_think(!on),
            CfgAction::Theme => {
                let next = match crate::style::ThemeMode::current() {
                    crate::style::ThemeMode::Dark => crate::style::ThemeMode::Light,
                    crate::style::ThemeMode::Light => crate::style::ThemeMode::Dark,
                };
                next.set();
                self.add_log(
                    "INFO",
                    "主题已切换（仅本次会话；持久值由 AIRY_TUI_THEME 决定）".to_string(),
                );
            }
            CfgAction::SetKey => {
                self.active_panel = ActivePanel::Chat;
                self.input = "/set-key ".to_string();
                self.cursor = self.input.chars().count();
                self.status_message = "补全 key 值后回车写入（密钥不进面板状态）".to_string();
            }
            CfgAction::Wizard => self.open_wizard(),
        }
    }

    /// 双思考开关写回 model.yaml 的 think 段（与 /hiairy 同一落点）。
    fn cfg_toggle_think(&mut self, enable: bool) {
        let m = crate::models_cfg::read_model_yaml();
        let Some(row) = m.rows.first().cloned() else {
            self.add_log("WARN", "双思考：model.yaml 无模型行，未写入".to_string());
            return;
        };
        let mut think = m.think.clone().unwrap_or_default();
        think.enabled = Some(enable);
        match crate::models_cfg::patch_model_yaml(0, &row, &think) {
            Ok(()) => self.add_log(
                "INFO",
                format!("双思考已{}（{}）", if enable { "开启" } else { "关闭" }, crate::models_cfg::model_yaml_path().display()),
            ),
            Err(e) => self.add_log("ERROR", format!("双思考写入失败：{}", e)),
        }
    }
}
