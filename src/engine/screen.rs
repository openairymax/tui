// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 渲染器形态选择（0.1.19 §5.5 #7）：inline + alt-screen 双路径可回滚。
//
// 两条路径共用同一渲染层（present / compose），只有视口不同：
//   Alt    备用屏全屏，输入框固定底部，退出还原主屏画布；
//   Inline 渲染区锚定终端底部固定行数，历史输出留在滚动回滚区，
//          备用屏不可用（限制型终端、复用会话、部分 IDE 面板）时回滚至此。
//
// 选择在启动期一次裁定：`AIRY_TUI_SCREEN`（auto / alt / alternate /
// inline），未知取值告警后按 auto 处理。运行期不重建 Terminal——备用屏
// 进入失败由 term::TerminalGuard 内部回滚，无需换视口。

use ratatui::Viewport;

use crate::budget::INLINE_VIEWPORT_ROWS;

/// 渲染器形态。默认备用屏，内联仅作回滚。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Alt,
    Inline,
}

impl Mode {
    /// 日志与诊断用名称。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Alt => "alt-screen",
            Self::Inline => "inline",
        }
    }

    /// 视口规格；内联高度由 ratatui 按终端行数收敛。
    pub(crate) fn viewport(self) -> Viewport {
        match self {
            Self::Alt => Viewport::Fullscreen,
            Self::Inline => Viewport::Inline(INLINE_VIEWPORT_ROWS),
        }
    }

    /// 解析配置取值；未知取值返回 None（调用方负责回退与告警）。
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "alt" | "alternate" | "alt-screen" | "fullscreen" => Some(Self::Alt),
            "inline" => Some(Self::Inline),
            _ => None,
        }
    }

    /// 读 `AIRY_TUI_SCREEN`；未设置或取值非法时回退默认。
    pub(crate) fn from_env() -> Self {
        let Ok(raw) = std::env::var("AIRY_TUI_SCREEN") else {
            return Self::default();
        };
        Self::parse(&raw).unwrap_or_else(|| {
            log::warn!("engine/screen: 忽略未知 AIRY_TUI_SCREEN={raw:?}，按 auto 处理");
            Self::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_known_spellings() {
        for raw in ["auto", "alt", "ALTERNATE", " alt-screen ", "fullscreen"] {
            assert_eq!(Mode::parse(raw), Some(Mode::Alt), "raw={raw:?}");
        }
        assert_eq!(Mode::parse("Inline"), Some(Mode::Inline));
        assert_eq!(Mode::parse(""), None);
        assert_eq!(Mode::parse("tmux"), None, "未知取值不得静默成内联");
    }

    #[test]
    fn default_is_alternate_screen() {
        assert_eq!(Mode::default(), Mode::Alt);
        assert_eq!(Mode::default().label(), "alt-screen");
    }

    #[test]
    fn viewport_matches_mode() {
        assert_eq!(Mode::Alt.viewport(), Viewport::Fullscreen);
        assert_eq!(
            Mode::Inline.viewport(),
            Viewport::Inline(INLINE_VIEWPORT_ROWS)
        );
        assert_ne!(Mode::Alt.viewport(), Mode::Inline.viewport());
    }
}
