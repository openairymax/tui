// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 首次启动向导（5 步，蓝晶风格）。
//
// 流程：
//   步骤 1/5：欢迎 + 版本 + 界面语言选择（自动检测 LC_ALL/LANG / English / 简体中文）
//   步骤 2/5：想怎么开始？（快速配置模型 / 跳过，先进 TUI 探索）
//   步骤 3/5：模型基本配置（v2 表格第 1 行：提供商/名称/连接方式/接口格式/
//             请求地址/模型 ID/API Key；提供商带内置预设，Tab 循环）
//   步骤 4/5：高级选项（上下文窗口/最大输出/工具轮数/图片输入/思考模式）
//   步骤 5/5：双思考系统（启用开关 + 慢/快/专业三个思考角色模型选择）
//
// 触发：
//   - 首次运行（$AIRY_HOME/data/agentrt/tui/wizard.toml 不存在）自动弹出；
//   - 对话中输入 /hiairy 随时重开。
//
// 完成后的选择写回：
//   - $AIRY_HOME/data/agentrt/tui/wizard.toml（lang + configured + 提供商/模型）
//   - $AIRY_HOME/config/secrets.env（MODEL_1_API_KEY，llm_d 热加载）
//   - $AIRY_HOME/config/model.yaml（models[0] 行 + think 段，llm_d/think_d 热加载）
//
// 模块划分（Unify Design SSoT）：`steps` 步骤/字段注册表是唯一声明源，
// `state` 状态机与 `view` 渲染都只按注册表 key 取值；`presets` 提供商预设、
// `lang` 语言检测、`text` 显示宽度文本算法、`persist` 落盘各自独立。

mod lang;
mod persist;
mod presets;
mod state;
mod steps;
mod text;
mod view;

#[cfg(test)]
mod e2e;

pub use state::{WizardResult, WizardState};
pub use view::render;
