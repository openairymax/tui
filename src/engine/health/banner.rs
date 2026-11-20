// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 单行降级横幅：常显最重依赖，恢复后进入冷却期抑制抖动重弹（§6.12 #1/#2/#6）。
// 文案与修复命令只由依赖名映射，不做错误串推断。

use crate::budget::DEGRADED_BANNER_COOLDOWN_MS;

/// 降级横幅的一次同步结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerAct {
    /// 无变化。
    None,
    /// 横幅出现或切换目标（调用方仅重绘）。
    Show,
    /// 降级解除（调用方提示恢复并撤横幅）。
    Restored,
}

/// 单行降级横幅：常显最重依赖，恢复后进入冷却期抑制抖动重弹。
#[derive(Debug, Default)]
pub(crate) struct Banner {
    dep: Option<String>,
    text: String,
    remedy: String,
    since_ms: u64,
    cool_until_ms: u64,
    flaps: u32,
}

impl Banner {
    /// 按健康表最重依赖同步横幅状态。调用方在每轮探测落地后调用一次。
    pub(crate) fn sync(&mut self, worst: Option<&str>, now_ms: u64) -> BannerAct {
        match worst {
            Some(name) => {
                if self.dep.as_deref() == Some(name) {
                    return BannerAct::None;
                }
                // 已有横幅时直接切换目标：严重度排序变化不应留下旧文案。
                let switching = self.dep.is_some();
                if !switching && now_ms < self.cool_until_ms {
                    self.flaps = self.flaps.saturating_add(1);
                    return BannerAct::None;
                }
                self.show(name, now_ms);
                BannerAct::Show
            }
            None => {
                if self.dep.is_none() {
                    return BannerAct::None;
                }
                self.dep = None;
                self.text.clear();
                self.remedy.clear();
                self.cool_until_ms = now_ms.saturating_add(DEGRADED_BANNER_COOLDOWN_MS);
                BannerAct::Restored
            }
        }
    }

    fn show(&mut self, name: &str, now_ms: u64) {
        self.dep = Some(name.to_string());
        self.text = banner_text(name);
        self.remedy = remedy(name);
        self.since_ms = now_ms;
    }

    pub(crate) fn visible(&self) -> bool {
        self.dep.is_some()
    }

    pub(crate) fn dep(&self) -> Option<&str> {
        self.dep.as_deref()
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 可一键复制的修复命令（真实存在的 airymaxrt 子命令）。
    pub(crate) fn remedy(&self) -> &str {
        &self.remedy
    }

    pub(crate) fn since_ms(&self) -> u64 {
        self.since_ms
    }

    /// 冷却期内被抑制的重弹次数（§6.12 #6，进健康面板）。
    pub(crate) fn flaps(&self) -> u32 {
        self.flaps
    }
}

fn banner_text(name: &str) -> String {
    match name {
        "llm" => "LLM 后端未连接 · 已切换本地降级路径".to_string(),
        "gateway" => "网关不可达 · 正在重连".to_string(),
        other => format!("{other} 不可达 · 功能降级"),
    }
}

fn remedy(name: &str) -> String {
    match name {
        "llm" => "airymaxrt logs llm_d".to_string(),
        _ => "airymaxrt doctor".to_string(),
    }
}
