// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// golden 能力矩阵（0.1.19 TUI 建设方案 §6/§7.1）：用 ratatui `TestBackend`
// 驱动真实 `App` 与真实 `present::render`，断言用户真正能看到的屏幕文本。
// 本目录内所有用例共享一个夹具：临时 AIRY_HOME（隔离首次启动向导与真实
// 用户目录）+ 离线网关地址（构造期无网络 IO，主循环不参与渲染）。

mod approval;
mod boot;
mod degrade;
mod error;
mod input;
mod observe;
mod perf;
mod progress;
mod scroll;
mod size;
mod stream;
mod theme;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::{App, MessageRole};
use crate::client::GatewayClient;

/// 标准视口宽（100 列：宽屏分支全部展开，与 §6.9 断点 46/52/72/80 兼容）。
pub(crate) const W: u16 = 100;
/// 标准视口高。
pub(crate) const H: u16 = 32;

/// 不可达端口：客户端只做构造，不发起连接；用于隔离任何真实网关。
const GW: &str = "http://127.0.0.1:9";

/// 测试夹具：持有临时 AIRY_HOME 守卫（串行化 ENV_LOCK）与真实应用状态。
/// 守卫必须在断言 panic 时也能清理环境，因此由结构体 Drop 负责。
pub(crate) struct Fixture {
    _home: crate::test_env::Home,
    pub(crate) app: App,
}

impl Fixture {
    /// `tag` 仅用于临时目录前缀，便于排查残留。
    pub(crate) fn new(tag: &str) -> Self {
        let home = crate::test_env::Home::new(tag);
        let gw = GatewayClient::new(GW).expect("离线网关客户端");
        let mut app = App::new("agents/main.agent.yaml", gw);
        // 首次启动时 `WizardState::new` 会因临时 HOME 缺少 model.yaml 置位，
        // 向导会全屏接管渲染；golden 断言的是常规界面，故显式关闭。
        app.wizard.active = false;
        Self { _home: home, app }
    }

    /// 以标准视口渲染一帧，返回逐行文本（不含换行符的尾部填充）。
    pub(crate) fn draw(&mut self) -> Screen {
        draw(&mut self.app, W, H)
    }

    /// 指定视口渲染（§6.9 尺寸断点用例）。
    pub(crate) fn draw_at(&mut self, w: u16, h: u16) -> Screen {
        draw(&mut self.app, w, h)
    }
}

/// 渲染结果：保留原始矩形以便按行列断言，`text()` 去掉行尾空白。
pub(crate) struct Screen {
    rows: Vec<String>,
}

impl Screen {
    pub(crate) fn text(&self) -> String {
        self.rows.join("\n")
    }

    pub(crate) fn has(&self, needle: &str) -> bool {
        self.rows.iter().any(|r| r.contains(needle))
    }

    pub(crate) fn rows(&self) -> &[String] {
        &self.rows
    }

    pub(crate) fn height(&self) -> usize {
        self.rows.len()
    }
}

/// 真实渲染路径：`present::render` 是 TUI 唯一的屏上写入口。
///
/// 逐行提取必须按显示宽度推进：ratatui 对宽字符（CJK）只在首格写符号，
/// 续格被置为空格；若逐格拼接，中文会被拆成「当 前」而令所有子串断言
/// 失效。这里以符号显示宽度为步长跳读续格，还原用户真正看到的文本。
pub(crate) fn draw(app: &mut App, w: u16, h: u16) -> Screen {
    use unicode_width::UnicodeWidthStr;

    let mut term = Terminal::new(TestBackend::new(w, h)).expect("测试终端");
    term.draw(|f| crate::present::render(f, app))
        .expect("渲染一帧");
    let buf = term.backend().buffer();
    let rows = (0..h)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0u16;
            while x < w {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += (UnicodeWidthStr::width(sym).max(1)) as u16;
            }
            line.trim_end().to_string()
        })
        .collect();
    Screen { rows }
}

/// 追加一条对话消息（走真实 sanitize/截断/记账路径）。
pub(crate) fn add(app: &mut App, role: MessageRole, text: &str) {
    app.add_message(role, text.to_string());
}

/// 断言辅助：屏幕任一位置包含 `needle`，失败时打印整屏便于定位。
pub(crate) fn assert_has(screen: &Screen, needle: &str) {
    assert!(
        screen.has(needle),
        "屏幕未包含 {:?}\n---屏幕---\n{}",
        needle,
        screen.text()
    );
}

/// 断言辅助：屏幕不包含 `needle`。
pub(crate) fn assert_lacks(screen: &Screen, needle: &str) {
    assert!(
        !screen.has(needle),
        "屏幕不应包含 {:?}\n---屏幕---\n{}",
        needle,
        screen.text()
    );
}

#[test]
fn golden_harness_renders_real_app() {
    let mut fx = Fixture::new("harness");
    let s = fx.draw();
    assert_eq!(s.height(), H as usize);
    assert_has(&s, "AirymaxRT");
    assert_has(&s, "❯");
}
