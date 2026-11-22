// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! 本地推理端点目录（与 agentrt/commons/include/airy_defaults.h 同值）。
//!
//! 三处「本地」语义不同，禁止互相顶替：providers 适配器缺省 vLLM /
//! OpenAI 兼容口 8080；platform_gpu 探测平台加速服务口 8000；自托管
//! llama-server 用 11435，Ollama 保留 11434。CI 值一致性门禁逐项比对
//! 两侧字面量，本文件是 TUI 侧唯一权威源。

#![allow(dead_code)]

/// providers 适配器缺省口（vLLM / OpenAI 兼容）
pub(crate) const PROVIDERS_PORT: u16 = 8080;
/// 平台加速服务口（platform_gpu 探测）
pub(crate) const PLATFORM_PORT: u16 = 8000;
/// 自托管 llama-server 口
pub(crate) const SELFHOST_PORT: u16 = 11435;
/// Ollama 口
pub(crate) const OLLAMA_PORT: u16 = 11434;

/// Ollama 的 OpenAI 兼容入口（向导「本地模型」预设）
pub(crate) const OLLAMA_URL: &str = "http://127.0.0.1:11434/v1";
/// llama-server 自托管入口（model.yaml 本地条目）
pub(crate) const SELFHOST_URL: &str = "http://127.0.0.1:11435/v1";
/// vLLM / OpenAI 兼容入口（providers 适配器代码级默认）
pub(crate) const PROVIDERS_URL: &str = "http://127.0.0.1:8080/v1";
