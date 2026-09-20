// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L5 合成层（0.1.18 A 轨 §5A.3 W3）：唯一 terminal.draw 出口 + 无脏跳帧。
//
// 全仓只有本模块可以触碰 ratatui 后端（§3.6 L5 契约：副作用收敛点）。上层
// 把「整帧渲染回调」下传，本层只做两件事：判定本帧是否成帧、成帧时经唯一
// 出口交出。本层不引用任何 L6 组件（不 use crate::ui、不感知 App）——渲染
// 回调由调用方注入，否则构成 §3 禁止的 L5 → L6 反向依赖；因此本模块可被
// TestBackend 直接驱动，无需构造全局状态。
//
// 跳帧判据（§5A.3 W3 初版裁定）：不自研 diff。ratatui 的 Terminal 已是双
// 缓冲 + 内置 buffer diff，重复成帧的正确性由其兜底；本层自研的只是「本帧
// 是否值得成帧」——由显式失效（invalidate）驱动：无失效即不调用
// terminal.draw，ratatui 的排版/差异/合成成本整段省去，形成零绘制帧。
//
// 与 ratatui 的边界（draw 文档契约）：绘制回调必须整帧渲染全部区域，不允许
// 部分渲染。故本层的跳帧只有一种形态——完全不调用 terminal.draw；绝不出现
// 「只画脏区域」的调用方式。
//
// 时序策略边界（§5A.2 / §5A.3 W4 已落地）：本层只提供机制，不决定节拍。失效
// 时刻一律由 L4 调度器裁决（节拍到期或清算出待办帧），主循环每轮无条件请求
// 成帧——「成帧 or 零绘制」的唯一判据落在本层，调用方不得预判，否则形成第二
// 套判定标准。
//
// 帧度量（§5 W4 验收数据源）：成帧数（drawn）与零绘制数（skipped）自计数，按
// 帧数节拍以 info 级写入 agentrt-tui.log（按次数而非时间，确定性），与 L4 的
// 帧率/分段耗时同属 §8 门禁口径。静态界面连续 N 帧成帧数应为 0。
//
// 状态归属：Compositor 由主循环持有（渲染发生在事件循环单线程内），无共享、
// 不加锁；本层不做 IO，除 terminal.draw 与下方隔离窗口标志外无副作用。
//
// 故障隔离（§5A.3 W10）：成帧路径的 panic 与后端写失败在本层收敛，绝不向上
// 冒泡为致命错误——渲染故障不得中断会话推进。panic 发生在渲染回调内时，
// ratatui 的 flush/swap_buffers 均不可达，当前缓冲未切换，屏幕自然停在上一帧；
// 本层只需保住这层语义（隔离窗口内不还原终端）并退避重连。成功一帧即自愈。
//
// 时间无关（§5A.2）：本层不取时钟，重连退避以「帧请求轮次」而非毫秒计量，
// 故可用虚拟时钟之外的纯计数器在单测中确定性重放。

use std::io;
use std::panic::{self, AssertUnwindSafe};

use ratatui::backend::Backend;
use ratatui::Frame;
use ratatui::Terminal;

use crate::term;

/// 帧度量日志节拍：每 N 帧（成帧 + 零绘制）输出一条 info 日志。与 L4 帧度量
/// 节拍取同一数值，便于同窗口比对成帧比与分段耗时。
const LOG_EVERY: u64 = 512;

/// 重连退避上限（轮）：连续故障时暂停成帧的最大轮数。把故障自旋压成低频重连，
/// 会话与事件循环持续存活，画面停在上一帧。
const RETRY_CAP: u32 = 64;

/// 渲染故障成因（降级日志与恢复判据的数据源）。
enum Fault {
    /// 渲染回调 panic（业务缺陷，本层可隔离）。
    Panic,
    /// 后端写失败（终端/管道异常），对应「会话继续、渲染暂停」。
    Backend(io::Error),
}

/// 单帧结果：调用方据此上报帧耗时与降级横幅。判据只在本层产生，不回流调用方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameOutcome {
    /// 成帧出一画面。
    Drawn,
    /// 无脏：零绘制（未触碰后端）。
    Skipped,
    /// 渲染故障：会话继续、渲染暂停（上一帧保留，冷却中）。
    Degraded,
}

/// 重连退避：连续第 n 次故障后暂停 `2^(n-1)` 轮成帧重试，封顶 `RETRY_CAP`。
const fn backoff(faults: u32) -> u32 {
    if faults <= 1 {
        return 1;
    }
    let shift = faults - 1;
    if shift >= 6 {
        RETRY_CAP
    } else {
        1u32 << shift
    }
}

/// 单次成帧尝试：`catch_unwind` 边界（§5A.3 W10）。
///
/// 隔离窗口置位期间，panic 钩子不得还原终端——否则备用屏退出、上一帧丢失、
/// 隔离形同虚设。`AssertUnwindSafe` 的依据：回调 panic 时 ratatui 的
/// flush/swap_buffers 不可达，Terminal 无半帧状态，跨边界继续使用安全。
fn draw_once<B, F>(terminal: &mut Terminal<B>, render: F) -> Result<(), Fault>
where
    B: Backend,
    F: FnOnce(&mut Frame),
{
    term::set_render_guard(true);
    let drawn = panic::catch_unwind(AssertUnwindSafe(|| terminal.draw(render)));
    term::set_render_guard(false);
    match drawn {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(Fault::Backend(e)),
        Err(_) => Err(Fault::Panic),
    }
}

/// L5 合成器：成帧判定 + 唯一出口 + 帧度量 + 故障隔离。
pub(crate) struct Compositor {
    /// 下一帧是否成帧。构造即为真：首帧必须出画，否则空屏。
    dirty: bool,
    /// 成帧计数（实际调用 terminal.draw 的次数）。
    drawn: u64,
    /// 零绘制计数（无脏而未成帧的次数，即跳帧）。
    skipped: u64,
    /// 连续渲染故障次数：重连退避与降级判定的共用量。成帧成功即清零。
    faults: u32,
    /// 重连冷却剩余轮次：> 0 时暂停成帧尝试（不触碰后端），避免故障自旋。
    cooldown: u32,
}

impl Compositor {
    /// 构造：首帧置脏，无故障在途。
    pub(crate) const fn new() -> Self {
        Self {
            dirty: true,
            drawn: 0,
            skipped: 0,
            faults: 0,
            cooldown: 0,
        }
    }

    /// 请求重绘：任何影响画面的状态变更之后调用。
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// 渲染是否处于降级态（故障未自愈）。供状态条横幅读取（§5A.3 W10）。
    pub(crate) fn degraded(&self) -> bool {
        self.faults > 0
    }

    /// 唯一成帧出口（§3.6）：脏则整帧渲染，无脏则零绘制。
    ///
    /// 渲染故障（回调 panic / 后端写失败）在本层收敛为 `Degraded`，绝不向上冒泡：
    /// 会话推进路径与 daemon 不受渲染层影响（§5A.3 W10）。故障后进入冷却，暂停
    /// 成帧尝试若干轮（不触碰后端）以避免自旋；冷却耗尽自动重试，成功一帧即自愈。
    pub(crate) fn frame<B, F>(&mut self, terminal: &mut Terminal<B>, render: F) -> FrameOutcome
    where
        B: Backend,
        F: FnOnce(&mut Frame),
    {
        if !self.dirty {
            self.skipped = self.skipped.wrapping_add(1);
            self.note();
            return FrameOutcome::Skipped;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return FrameOutcome::Degraded;
        }
        match draw_once(terminal, render) {
            Ok(()) => {
                self.drawn = self.drawn.wrapping_add(1);
                self.dirty = false;
                if self.faults > 0 {
                    log::info!("engine/compose: 渲染自愈（连续故障 {} 次）", self.faults);
                    self.faults = 0;
                }
                self.note();
                FrameOutcome::Drawn
            }
            Err(fault) => {
                self.degrade(&fault);
                FrameOutcome::Degraded
            }
        }
    }

    /// 记一次故障并进入重连冷却。保持 `dirty`：冷却耗尽后仍待重绘。
    fn degrade(&mut self, fault: &Fault) {
        self.faults = self.faults.saturating_add(1);
        self.cooldown = backoff(self.faults);
        match fault {
            Fault::Panic => log::warn!(
                "engine/compose: 渲染回调 panic，已隔离（第 {} 次，暂停 {} 轮）",
                self.faults,
                self.cooldown
            ),
            Fault::Backend(e) => log::warn!(
                "engine/compose: 后端写失败，渲染暂停（第 {} 次，暂停 {} 轮）: {e}",
                self.faults,
                self.cooldown
            ),
        }
    }

    /// 帧度量自省（§5 W4：成帧比自计数，日志可查）。
    fn note(&self) {
        let frames = self.drawn + self.skipped;
        if frames == 0 || !frames.is_multiple_of(LOG_EVERY) {
            return;
        }
        log::info!(
            "engine/compose: 帧 {frames} 成帧 {} 零绘制 {}",
            self.drawn,
            self.skipped
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{backoff, Compositor, FrameOutcome, RETRY_CAP};
    use ratatui::backend::{Backend, TestBackend, WindowSize};
    use ratatui::buffer::{Buffer, Cell as TermCell};
    use ratatui::layout::{Position, Size};
    use ratatui::widgets::Paragraph;
    use ratatui::{Frame, Terminal};
    use std::cell::Cell;
    use std::io;

    /// 造一个独立合成器与测试终端（互不共享状态）。
    fn fixture() -> (Compositor, Terminal<TestBackend>) {
        let terminal = Terminal::new(TestBackend::new(80, 24)).expect("测试后端构造");
        (Compositor::new(), terminal)
    }

    /// 首帧成画的渲染回调（内容固定，便于比对上一帧）。
    fn paint(frame: &mut Frame, text: &str) {
        frame.render_widget(Paragraph::new(text), frame.area());
    }

    /// §5A.3 W3 验收：静态界面连续 1000 帧成帧次数为 0（计数实证）。
    #[test]
    fn static_screen_draws_nothing_for_1000_frames() {
        let (mut compose, mut terminal) = fixture();
        let renders = Cell::new(0u64);
        // 闭包只捕获 &Cell（Copy），故可反复按值传给 frame
        let render = |f: &mut Frame| {
            renders.set(renders.get() + 1);
            paint(f, "static");
        };

        assert_eq!(
            compose.frame(&mut terminal, render),
            FrameOutcome::Drawn,
            "首帧必须成画"
        );
        assert_eq!(compose.drawn, 1, "首帧必须成画");
        assert_eq!(renders.get(), 1);

        for _ in 0..1000 {
            assert_eq!(
                compose.frame(&mut terminal, render),
                FrameOutcome::Skipped,
                "无脏不得成帧"
            );
        }
        assert_eq!(compose.drawn, 1, "静态界面不得再成帧");
        assert_eq!(compose.skipped, 1000, "1000 帧全部零绘制");
        assert_eq!(renders.get(), 1, "跳帧不得进入渲染回调");
    }

    /// 失效一次恰成帧一次：下一次失效前不再成帧。
    #[test]
    fn invalidate_produces_exactly_one_frame() {
        let (mut compose, mut terminal) = fixture();
        let renders = Cell::new(0u64);
        let render = |_f: &mut Frame| renders.set(renders.get() + 1);

        assert_eq!(compose.frame(&mut terminal, render), FrameOutcome::Drawn);
        compose.invalidate();
        assert_eq!(compose.frame(&mut terminal, render), FrameOutcome::Drawn);

        assert_eq!(compose.drawn, 2);
        assert_eq!(compose.skipped, 0);
        assert_eq!(renders.get(), 2);
    }

    /// 无失效时永不空转成帧（连续失效则按次成帧，不合并、不丢失）。
    #[test]
    fn frames_track_invalidations_without_drift() {
        let (mut compose, mut terminal) = fixture();
        let render = |_f: &mut Frame| {};

        assert_eq!(compose.frame(&mut terminal, render), FrameOutcome::Drawn);
        for _ in 0..32 {
            compose.invalidate();
            assert_eq!(compose.frame(&mut terminal, render), FrameOutcome::Drawn);
            assert_eq!(compose.frame(&mut terminal, render), FrameOutcome::Skipped);
        }
        assert_eq!(compose.drawn, 33);
        assert_eq!(compose.skipped, 32);
    }

    /// 重连退避曲线：指数上升并封顶 `RETRY_CAP`（§5A.3 W10 重连尝试）。
    #[test]
    fn retry_backoff_is_exponential_and_capped() {
        assert_eq!(backoff(1), 1, "首次故障冷却 1 轮");
        assert_eq!(backoff(2), 2);
        assert_eq!(backoff(3), 4);
        assert_eq!(backoff(6), 32);
        assert_eq!(backoff(7), RETRY_CAP, "第 7 次故障起封顶");
        assert_eq!(backoff(u32::MAX), RETRY_CAP, "封顶不得随故障数溢出");
    }

    /// §5A.3 W10 验收：注入渲染回调 panic → 本层隔离、上一帧保留、随后自愈。
    #[test]
    fn render_panic_is_isolated_and_previous_frame_survives() {
        let (mut compose, mut terminal) = fixture();
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "上一帧")),
            FrameOutcome::Drawn
        );
        let last_frame: Buffer = terminal.backend().buffer().clone();

        // 注入 panic：必须降级返回而非冒泡（无 panic 逃逸即会话存活）
        compose.invalidate();
        assert_eq!(
            compose.frame(&mut terminal, |_f| panic!("注入渲染 panic")),
            FrameOutcome::Degraded
        );
        assert!(compose.degraded(), "故障后须进入降级态");
        assert_eq!(compose.drawn, 1, "故障帧不得计入成帧");
        assert_eq!(
            terminal.backend().buffer(),
            &last_frame,
            "渲染故障必须保留上一帧"
        );

        // 冷却 1 轮（backoff(1)）：不触碰后端，仍为降级态
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "不应出现")),
            FrameOutcome::Degraded
        );
        assert_eq!(
            terminal.backend().buffer(),
            &last_frame,
            "冷却期内不得改动画面的任何像素"
        );

        // 冷却耗尽：正常回调可成帧，降级态自愈
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "自愈帧")),
            FrameOutcome::Drawn
        );
        assert!(!compose.degraded(), "成功一帧即自愈");
        assert_eq!(compose.drawn, 2, "自愈帧计入成帧");
    }

    /// 写失败注入后端：`failing` 置位后所有写操作返回 `BrokenPipe`，其余委托
    /// `TestBackend`；`writes` 计数后端被触碰的次数（用于断言冷却期零触碰）。
    struct FlakyBackend {
        inner: TestBackend,
        failing: Cell<bool>,
        writes: Cell<u64>,
    }

    impl FlakyBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                inner: TestBackend::new(width, height),
                failing: Cell::new(false),
                writes: Cell::new(0),
            }
        }

        /// 记一次写尝试；处于故障态则失败（模拟终端断开/管道关闭）。
        fn write(&self) -> io::Result<()> {
            self.writes.set(self.writes.get() + 1);
            if self.failing.get() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "注入写失败"));
            }
            Ok(())
        }
    }

    impl Backend for FlakyBackend {
        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a TermCell)>,
        {
            self.write()?;
            self.inner.draw(content)
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.write()?;
            self.inner.hide_cursor()
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.write()?;
            self.inner.show_cursor()
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            self.inner.get_cursor_position()
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.write()?;
            self.inner.set_cursor_position(position)
        }

        fn clear(&mut self) -> io::Result<()> {
            self.write()?;
            self.inner.clear()
        }

        fn size(&self) -> io::Result<Size> {
            self.inner.size()
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            self.inner.window_size()
        }

        fn flush(&mut self) -> io::Result<()> {
            self.write()?;
            self.inner.flush()
        }
    }

    /// §5A.3 W10 验收：注入后端写失败 → 降级为「会话继续、渲染暂停」并重连自愈。
    #[test]
    fn backend_write_failure_pauses_rendering_then_recovers() {
        let backend = FlakyBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("测试后端构造");
        let mut compose = Compositor::new();

        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "在线")),
            FrameOutcome::Drawn
        );
        assert!(!compose.degraded());

        // 终端断开：写失败必须降级，且不得冒泡为致命错误
        terminal.backend_mut().failing.set(true);
        compose.invalidate();
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "不应出现")),
            FrameOutcome::Degraded
        );
        assert!(compose.degraded(), "写失败须进入降级态");
        assert_eq!(compose.drawn, 1, "写失败的帧不得计入成帧");

        // 冷却 1 轮内零后端触碰（否则形成故障自旋）
        let before = terminal.backend().writes.get();
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "不应出现")),
            FrameOutcome::Degraded
        );
        assert_eq!(
            terminal.backend().writes.get(),
            before,
            "冷却期内不得触碰后端"
        );

        // 终端恢复：重连成功即自愈，画面继续推进
        terminal.backend_mut().failing.set(false);
        assert_eq!(
            compose.frame(&mut terminal, |f| paint(f, "重连")),
            FrameOutcome::Drawn
        );
        assert!(!compose.degraded(), "重连成功即自愈");
        assert_eq!(compose.drawn, 2);
    }
}
