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

    /// 启动形态裁定：显式 `AIRY_TUI_SCREEN` 优先，`auto`/未设置时读失败计数。
    ///
    /// 显式取值是用户对当前终端的判断，必须压过历史失败记录；`auto` 才代表
    /// 「按能力自动选」。连续失败达阈值后本次直接内联，跳过必然失败的清屏，
    /// 一次全屏成功由调用方清零计数后恢复自动重试。
    pub(crate) fn choose() -> Self {
        let Ok(raw) = std::env::var("AIRY_TUI_SCREEN") else {
            return Self::auto();
        };
        if raw.trim().eq_ignore_ascii_case("auto") {
            return Self::auto();
        }
        Self::parse(&raw).unwrap_or_else(|| {
            log::warn!("engine/screen: 忽略未知 AIRY_TUI_SCREEN={raw:?}，按 auto 处理");
            Self::auto()
        })
    }

    /// 自动形态：失败计数达阈值即内联，否则按默认（备用屏）。
    fn auto() -> Self {
        let fails = crate::engine::ui_state::fails();
        if fails >= crate::engine::ui_state::FAIL_LIMIT {
            log::warn!(
                "engine/screen: 备用屏连续进入失败 {fails} 次，本次直接内联（设 AIRY_TUI_SCREEN=alt 可强制重试）"
            );
            Self::Inline
        } else {
            Self::default()
        }
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

    /// 隔离 AIRY_HOME 与 AIRY_TUI_SCREEN 的用例夹具。
    struct Scratch {
        _g: std::sync::MutexGuard<'static, ()>,
        dir: std::path::PathBuf,
        home: Option<String>,
        screen: Option<String>,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let g = crate::test_env::lock_env();
            let dir = std::env::temp_dir()
                .join(format!("airy-screen-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let home = std::env::var("AIRY_HOME").ok();
            let screen = std::env::var("AIRY_TUI_SCREEN").ok();
            std::env::set_var("AIRY_HOME", &dir);
            std::env::remove_var("AIRY_TUI_SCREEN");
            Self {
                _g: g,
                dir,
                home,
                screen,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            match &self.home {
                Some(v) => std::env::set_var("AIRY_HOME", v),
                None => std::env::remove_var("AIRY_HOME"),
            }
            match &self.screen {
                Some(v) => std::env::set_var("AIRY_TUI_SCREEN", v),
                None => std::env::remove_var("AIRY_TUI_SCREEN"),
            }
        }
    }

    /// X8-08b：前三次请求备用屏并回滚，第四次启动直接内联。
    #[test]
    fn fourth_start_falls_back_to_inline() {
        let _s = Scratch::new("fourth");
        for _ in 0..crate::engine::ui_state::FAIL_LIMIT {
            assert_eq!(Mode::choose(), Mode::Alt, "未达阈值前仍应尝试备用屏");
            crate::engine::ui_state::note_fail();
        }
        assert_eq!(Mode::choose(), Mode::Inline, "第四次启动必须直接内联");
    }

    /// X8-08a：显式 alt 是用户对终端的判断，压过失败计数。
    #[test]
    fn explicit_alt_overrides_fallback() {
        let _s = Scratch::new("explicit");
        for _ in 0..crate::engine::ui_state::FAIL_LIMIT {
            crate::engine::ui_state::note_fail();
        }
        std::env::set_var("AIRY_TUI_SCREEN", "alt");
        assert_eq!(Mode::choose(), Mode::Alt);
        std::env::set_var("AIRY_TUI_SCREEN", "inline");
        assert_eq!(Mode::choose(), Mode::Inline);
        std::env::set_var("AIRY_TUI_SCREEN", "auto");
        assert_eq!(Mode::choose(), Mode::Inline, "auto 仍按失败计数回退");
    }
}
