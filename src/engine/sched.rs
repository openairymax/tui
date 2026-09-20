// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L4 调度层（0.1.18 A 轨 §3.5 / §5A.3 W4）：帧预算 / 合帧 / 优先级 / 背压 / 帧度量。
//
// 职责边界：本层是唯一的节拍权威——「何时成帧」由这里判定；L5 合成器只提供
// 「失效 → 成帧」的机制（§5A.3 W3）。本层不引用 L5/L6，也不调用
// terminal.draw，只输出判定与到期时刻，由主循环据此驱动合成器，故可被虚拟
// 时钟直接驱动测试，无需构造终端或全局状态。
//
// 时间注入（§5A.4：L4 有状态层须以时间注入的单测驱动）：本层禁止调用
// Instant::now()，时刻一律由调用方经 advance() 注入（单调毫秒）。合帧窗口、
// 帧预算与背压行为因此可在单测里确定性重放。
//
// 合帧（§3.5）：高频事件（token 增量）在窗口内合并，一帧只画最终态。窗口由
// 帧率上限导出（§5 W4：帧率可配置且有上限），落在 §5A.3 W4 的 16–33ms 区间。
//
// 优先级（§3.5）：输入回显 > 流式输出 > 面板刷新 > 后台轮询。输入档抢占合帧
// 窗口立即成帧，其余档位等到窗口末——一帧渲染读的是最新状态，故低优先级档
// 无需自成一帧，搭上最近一帧即可。
//
// 背压（§3.5）：待办队列以 64 为上限，溢出时丢弃最低优先级档位里最靠中间的
// 条目——「丢弃中间态、保留首尾」。输入档不参与丢弃：业务事件不可丢，此时
// push() 返回假，调用方停止继续取事件（余量留在终端输入缓冲）。
//
// 帧度量（§3.5 最后一条）：帧率与单帧分段耗时自计数，按帧数节拍以 info 级写入
// agentrt-tui.log，作为 §8 三平台回归门禁的数据源。帧率取注入时刻的窗口均值
// （本层禁调 Instant::now()，故时基必须来自 advance() 注入值，方能确定性重放）；
// 分段口径：排版段在渲染回调内打点，差异与合成由 ratatui 的 flush() 合并执行
// （Terminal 的双缓冲字段私有，外部无法再分），故合为一档并如实标注。零绘制
// 计数不在此处——成帧/跳帧判定属 L5 合成层，其计数见 compose.rs（同一门禁口径）。
//
// 调用次序约定：advance() → beat 查询与业务轮询 → push() → take_frame()。
// push() 记的是「此刻到达」，故须在 advance() 之后调用；take_frame() 取走判定
// 并清算该帧合并量，每帧至多真一次。

use std::collections::VecDeque;

/// 空闲降频上限（ms）：无事件且无到期事项时的最长等待（§5A.3 W4）。
const IDLE_MS: u64 = 250;
/// 单帧时间预算（μs）：排版 + 差异 + 合成超过即降级（§3.5 帧预算）。
const BUDGET_US: u64 = 8_000;
/// 合帧窗口下限（ms）：§5A.3 W4 的 16–33ms 区间。
const COALESCE_MIN_MS: u64 = 16;
/// 合帧窗口上限（ms）：低帧率档也仍成帧，不因窗口过长而迟滞。
const COALESCE_MAX_MS: u64 = 33;
/// 待办队列上限（§3.5 背压）。
const QUEUE_CAP: usize = 64;
/// 帧率上限的容许区间（§5 W4：帧率可配置且有上限）。
const FPS_MIN: u32 = 1;
const FPS_MAX: u32 = 240;
/// 缺省帧率上限：未配置 `AIRY_TUI_MAX_FPS` 时取 60（合帧窗口 16ms）。
const FPS_DEFAULT: u32 = 60;
/// 帧度量日志节拍：每 N 帧一条 info 日志（按帧数而非时间，确定性）。
const METRIC_EVERY: u64 = 512;
/// 统一节拍表长度（须与 Beat 判别值一致，由单测守护）。
const BEAT_N: usize = 5;

const MS_PER_S: u64 = 1_000;
const US_PER_MS: u64 = 1_000;

/// 待办事件等级（§3.5 优先级）：声明序即优先级，越靠前越优先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Lane {
    /// 输入回显：按键、粘贴、鼠标、窗口改变——延迟直接可感。
    Input,
    /// 流式输出：token 增量等高频事件，合帧窗口内合并。
    Stream,
    /// 面板刷新：看板、事件流、审批数据落地。
    Panel,
    /// 后台轮询：节拍驱动的兜底拉取。
    Poll,
}

/// 统一节拍表的节拍标识（§5A.3 W4：轮询节拍并入调度表，不再硬编码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Beat {
    /// 活跃动画相位：思考动画与节点旋转的最小相位。
    Anim,
    /// chat 光标闪动半周期。
    Blink,
    /// 状态条时钟（秒级）。
    Clock,
    /// 看板/事件流拉取。
    Hall,
    /// 审批轮询。
    Approvals,
}

impl Beat {
    /// 节拍周期（ms）。取自各相位点的实测值，此处即其唯一权威。
    pub(crate) const fn period_ms(self) -> u64 {
        match self {
            Beat::Anim => 100,
            Beat::Blink => 265,
            Beat::Clock => 1_000,
            Beat::Hall => 1_000,
            Beat::Approvals => 1_500,
        }
    }
}

/// 节拍表（下标须与 Beat 判别值一致，由单测守护）。
const BEATS: [Beat; BEAT_N] = [
    Beat::Anim,
    Beat::Blink,
    Beat::Clock,
    Beat::Hall,
    Beat::Approvals,
];

/// 本轮到期节拍集合（位掩码，零分配）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Beats(u8);

impl Beats {
    /// 指定节拍本轮是否到期。
    pub(crate) fn has(self, beat: Beat) -> bool {
        self.0 & (1 << beat as u8) != 0
    }

    /// 本轮是否有任一节拍到期（有则必须重绘）。
    pub(crate) fn any(self) -> bool {
        self.0 != 0
    }

    fn set(&mut self, beat: Beat) {
        self.0 |= 1 << beat as u8;
    }
}

/// 调度参数（§3.5 / §5A.3 W4）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cfg {
    /// 合帧窗口（ms）。
    coalesce_ms: u64,
    /// 空闲降频上限（ms）。
    idle_ms: u64,
    /// 单帧时间预算（μs）。
    budget_us: u64,
}

impl Cfg {
    /// 由帧率上限导出合帧窗口：帧率越高窗口越短，窗口下限即帧率上限。
    ///
    /// 参数取自 `AIRY_TUI_MAX_FPS`（§5 W4：帧率可配置且有上限）；越界取值
    /// 收敛到容许区间，避免窗口退化为 0（忙等）或长于空闲节拍（迟滞）。
    pub(crate) fn from_fps(fps: u32) -> Self {
        let window = (MS_PER_S / u64::from(fps.clamp(FPS_MIN, FPS_MAX)))
            .clamp(COALESCE_MIN_MS, COALESCE_MAX_MS);
        Self {
            coalesce_ms: window,
            idle_ms: IDLE_MS,
            budget_us: BUDGET_US,
        }
    }

    /// 以 `AIRY_TUI_MAX_FPS` 构造（未设置或非数字时取 FPS_DEFAULT）。
    ///
    /// 帧率上限的用户配置入口（§5 W4 验收：帧率可配置且有上限）；取值范围
    /// 由 from_fps() 收敛，故此处对非法输入只做退回默认，不做额外校验。
    pub(crate) fn from_env() -> Self {
        let fps = std::env::var("AIRY_TUI_MAX_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(FPS_DEFAULT);
        Self::from_fps(fps)
    }
}

/// L4 调度器：节拍权威 + 合帧 + 优先级 + 背压 + 帧度量。
pub(crate) struct Sched {
    cfg: Cfg,
    /// 最近一次 advance() 注入的时刻（单调毫秒）。
    now_ms: u64,
    /// 下一次成帧时刻；None 表示无待办。
    due_ms: Option<u64>,
    /// 待办事件（等级 + 到达时刻），成帧时整批清算。
    queue: VecDeque<(Lane, u64)>,
    /// 背压丢弃计数。
    shed: u64,
    /// 各节拍是否启用（由上层按 App 状态同步）。
    beat_on: [bool; BEAT_N],
    /// 各节拍本周期待发（启用或显式强制即为真）。
    beat_hit: [bool; BEAT_N],
    /// 各节拍最近一次到期时刻。
    beat_at: [u64; BEAT_N],
    /// 上一帧是否超预算：为真则本轮抑制最低优先级工作。
    degrade: bool,
    /// 成帧数。
    frames: u64,
    /// 累计合并事件数。
    merges: u64,
    /// 最长合帧等待（μs）：队列首条事件从到达至成帧的间隔。
    wait_max_us: u64,
    /// 排版段耗时累计与峰值（μs）。
    layout_sum_us: u64,
    layout_max_us: u64,
    /// 差异 + 合成段耗时累计与峰值（μs）。
    present_sum_us: u64,
    present_max_us: u64,
    /// 超预算帧数。
    over: u64,
    /// 帧率窗口起点（注入毫秒）。构造为 0，与时刻起点一致，故首个窗口即为
    /// 「启动至首次上报」的真实跨度，无需额外哨兵。
    rate_ms: u64,
}

impl Sched {
    /// 以给定参数构造（时刻起点由首次 advance() 注入）。
    pub(crate) fn new(cfg: Cfg) -> Self {
        Self {
            cfg,
            now_ms: 0,
            due_ms: None,
            queue: VecDeque::new(),
            shed: 0,
            beat_on: [false; BEAT_N],
            beat_hit: [false; BEAT_N],
            beat_at: [0; BEAT_N],
            degrade: false,
            frames: 0,
            merges: 0,
            wait_max_us: 0,
            layout_sum_us: 0,
            layout_max_us: 0,
            present_sum_us: 0,
            present_max_us: 0,
            over: 0,
            rate_ms: 0,
        }
    }

    /// 推进时钟至 now_ms，返回本轮到期节拍（每周期每节拍至多一次）。
    pub(crate) fn advance(&mut self, now_ms: u64) -> Beats {
        self.now_ms = now_ms;
        let mut due = Beats::default();
        for (i, beat) in BEATS.iter().enumerate() {
            if !self.beat_on[i] {
                continue;
            }
            if !self.beat_hit[i] && now_ms.saturating_sub(self.beat_at[i]) < beat.period_ms() {
                continue;
            }
            self.beat_hit[i] = false;
            self.beat_at[i] = now_ms;
            due.set(*beat);
        }
        due
    }

    /// 同步节拍启用态；由关闭转启用时立即到期，避免激活后空等一个周期。
    pub(crate) fn beat_set(&mut self, beat: Beat, on: bool) {
        let i = beat as usize;
        if self.beat_on[i] == on {
            return;
        }
        self.beat_on[i] = on;
        self.beat_hit[i] = on;
        if on {
            self.beat_at[i] = self.now_ms;
        }
    }

    /// 强制某节拍立即到期（面板切换等显式刷新请求）；未启用时为 no-op。
    pub(crate) fn beat_now(&mut self, beat: Beat) {
        let i = beat as usize;
        if self.beat_on[i] {
            self.beat_hit[i] = true;
        }
    }

    /// 距下一件到期事项的等待毫秒数，上限为参数里的空闲节拍。
    ///
    /// 主循环以本值作为事件等待超时——有事件即被唤醒，无事件则降频到空闲
    /// 节拍（§3.5 空闲降频）。终端无「失焦」语义，故不做失焦降帧。
    pub(crate) fn wait_ms(&self) -> u64 {
        let mut wait = self.cfg.idle_ms;
        if let Some(t) = self.due_ms {
            wait = wait.min(t.saturating_sub(self.now_ms));
        }
        for (i, beat) in BEATS.iter().enumerate() {
            if !self.beat_on[i] {
                continue;
            }
            if self.beat_hit[i] {
                return 0;
            }
            let next = self.beat_at[i].saturating_add(beat.period_ms());
            wait = wait.min(next.saturating_sub(self.now_ms));
        }
        wait
    }

    /// 记入一个待渲染事件（§3.5 优先级 + 背压）。
    ///
    /// 返回是否入队：队列已满且无可丢弃的低优先级条目时为假（队列中仅有
    /// 输入档）。调用方应停止继续取事件，余量留在终端输入缓冲，勿丢业务事件。
    pub(crate) fn push(&mut self, lane: Lane) -> bool {
        if self.queue.len() >= QUEUE_CAP && !self.drop_lowest() {
            return false;
        }
        self.queue.push_back((lane, self.now_ms));
        // 输入回显抢占合帧窗口；其余档位搭最近一帧即可（渲染读最新状态）
        let at = match lane {
            Lane::Input => self.now_ms,
            _ => self.now_ms.saturating_add(self.cfg.coalesce_ms),
        };
        self.due_ms = Some(match self.due_ms {
            Some(prev) => prev.min(at),
            None => at,
        });
        true
    }

    /// 取走本帧成帧判定：有待办则清算并返回真（每帧至多真一次）。
    ///
    /// 清算即「合帧」的落点：窗口内到达的事件整批计入合并量后清空，故一帧
    /// 只画最终态。
    pub(crate) fn take_frame(&mut self) -> bool {
        if self.due_ms.is_none_or(|t| t > self.now_ms) {
            return false;
        }
        self.due_ms = None;
        if let Some((_, at)) = self.queue.front() {
            let wait = self.now_ms.saturating_sub(*at).saturating_mul(US_PER_MS);
            self.wait_max_us = self.wait_max_us.max(wait);
        }
        self.merges = self.merges.wrapping_add(self.queue.len() as u64);
        self.queue.clear();
        self.degrade = false;
        true
    }

    /// 上一帧是否超预算：为真时本轮抑制最低优先级工作（后台轮询）以保证不阻塞。
    ///
    /// 降级形态说明：L3 差异层初版不自研（§5A.3 W3 裁定），故降级不表现为
    /// 「只重绘脏区中最旧的一块」，而是抑制最低优先级工作并入下一帧——同样
    /// 保证永不阻塞，且不引入第二套差异实现。
    pub(crate) fn degraded(&self) -> bool {
        self.degrade
    }

    /// 上报一帧的分段耗时（μs）。
    ///
    /// 排版段在渲染回调内打点；差异与合成由 ratatui 的 flush() 合并执行
    /// （Terminal 双缓冲字段私有），故合为一档。超预算即置降级态。
    pub(crate) fn note_frame(&mut self, layout_us: u64, present_us: u64) {
        self.frames = self.frames.wrapping_add(1);
        self.layout_sum_us = self.layout_sum_us.wrapping_add(layout_us);
        self.present_sum_us = self.present_sum_us.wrapping_add(present_us);
        self.layout_max_us = self.layout_max_us.max(layout_us);
        self.present_max_us = self.present_max_us.max(present_us);
        let total = layout_us.saturating_add(present_us);
        if total > self.cfg.budget_us {
            self.over = self.over.wrapping_add(1);
        }
        self.degrade = total > self.cfg.budget_us;
        self.report();
    }

    /// 溢出丢弃：在最低优先级档位中丢弃最靠中间的条目（保首尾）。
    fn drop_lowest(&mut self) -> bool {
        let worst = self.queue.iter().map(|(lane, _)| *lane).max();
        if worst.is_none_or(|lane| lane == Lane::Input) {
            return false;
        }
        let mid = self.queue.len() / 2;
        let mut pick = 0usize;
        let mut best = usize::MAX;
        for (i, (lane, _)) in self.queue.iter().enumerate() {
            if Some(*lane) != worst {
                continue;
            }
            let d = i.abs_diff(mid);
            if d < best {
                best = d;
                pick = i;
            }
        }
        self.queue.remove(pick);
        self.shed = self.shed.wrapping_add(1);
        true
    }

    /// 帧率（帧/秒）：窗口成帧数除以窗口时长；时长为 0 时报 0（虚拟时钟冻结
    /// 时不得除零，也不得虚报无穷帧率）。
    fn fps_in(frames: u64, span_ms: u64) -> u64 {
        if span_ms == 0 {
            return 0;
        }
        frames.saturating_mul(MS_PER_S) / span_ms
    }

    /// 帧度量上报（§3.5）：按帧数节拍写 info 日志，落 agentrt-tui.log。
    fn report(&mut self) {
        if self.frames == 0 || !self.frames.is_multiple_of(METRIC_EVERY) {
            return;
        }
        let layout_avg = self.layout_sum_us / self.frames;
        let present_avg = self.present_sum_us / self.frames;
        // 帧率自计数（§3.5）：窗口跨度取注入时刻之差，窗口内恰有 METRIC_EVERY
        // 帧（上报节拍即窗口边界），故无需另设窗口计数器。
        let span_ms = self.now_ms.saturating_sub(self.rate_ms);
        self.rate_ms = self.now_ms;
        let fps = Self::fps_in(METRIC_EVERY, span_ms);
        log::info!(
            "engine/sched: 帧 {} 帧率 {}/s（窗口 {}ms）排版 均 {}us/峰 {}us \
             差异+合成 均 {}us/峰 {}us 超预算 {} 合并 {} 最长合帧等待 {}us 背压丢弃 {}",
            self.frames,
            fps,
            span_ms,
            layout_avg,
            self.layout_max_us,
            present_avg,
            self.present_max_us,
            self.over,
            self.merges,
            self.wait_max_us,
            self.shed,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Beat, Beats, Cfg, Lane, Sched, BEATS, BEAT_N, IDLE_MS, METRIC_EVERY, QUEUE_CAP};

    /// 60fps：合帧窗口为区间下限 16ms。
    fn fixture() -> Sched {
        Sched::new(Cfg::from_fps(60))
    }

    /// 节拍表须覆盖全部变体且下标即判别值（否则 advance 会串档）。
    #[test]
    fn beat_table_covers_all_variants() {
        assert_eq!(BEATS.len(), BEAT_N);
        for (i, beat) in BEATS.iter().enumerate() {
            assert_eq!(*beat as usize, i, "节拍表下标须与判别值一致");
        }
    }

    /// 帧率上限收敛到 16–33ms 窗口，越界取值不得退化。
    #[test]
    fn coalesce_window_follows_fps_within_bounds() {
        assert_eq!(Cfg::from_fps(60).coalesce_ms, 16);
        assert_eq!(Cfg::from_fps(240).coalesce_ms, 16);
        assert_eq!(Cfg::from_fps(30).coalesce_ms, 33);
        assert_eq!(Cfg::from_fps(0).coalesce_ms, 33, "越界下限收敛");
        assert_eq!(Cfg::from_fps(u32::MAX).coalesce_ms, 16, "越界上限收敛");
    }

    /// 帧率上限的用户配置入口：合法取值生效，非法/未设置退回默认。
    #[test]
    fn fps_env_configures_coalesce_window() {
        let _lock = crate::test_env::lock_env();
        std::env::set_var("AIRY_TUI_MAX_FPS", "30");
        assert_eq!(Cfg::from_env().coalesce_ms, 33, "合法取值生效");
        std::env::set_var("AIRY_TUI_MAX_FPS", "not-a-number");
        assert_eq!(Cfg::from_env().coalesce_ms, 16, "非法取值退回默认 60fps");
        std::env::remove_var("AIRY_TUI_MAX_FPS");
        assert_eq!(Cfg::from_env().coalesce_ms, 16, "未设置退回默认 60fps");
    }

    /// §3.5 合帧：窗口内的增量整批合并为一帧。
    #[test]
    fn coalesces_stream_burst_into_one_frame() {
        let mut sched = fixture();
        sched.advance(0);
        for _ in 0..10 {
            assert!(sched.push(Lane::Stream), "队列未满须入队");
        }
        assert!(!sched.take_frame(), "窗口未到不得成帧");
        sched.advance(15);
        assert!(!sched.take_frame(), "窗口内不得成帧");
        sched.advance(16);
        assert!(sched.take_frame(), "窗口到期成帧");
        assert_eq!(sched.merges, 10, "10 个增量合并为一帧");
        assert_eq!(sched.wait_max_us, 16_000, "首条事件的合帧等待被记录");
        assert!(!sched.take_frame(), "同一帧不得重复取走");
    }

    /// §3.5 优先级：输入回显抢占合帧窗口立即成帧。
    #[test]
    fn input_preempts_coalesce_window() {
        let mut sched = fixture();
        sched.advance(0);
        sched.push(Lane::Stream);
        assert!(!sched.take_frame(), "流式档须等窗口");
        sched.push(Lane::Input);
        assert!(sched.take_frame(), "输入档抢占并立即成帧");
    }

    /// §3.5 帧度量：帧率由窗口成帧数与注入时长导出，冻结时钟不得除零发散。
    #[test]
    fn frame_rate_derives_from_injected_window() {
        assert_eq!(
            Sched::fps_in(METRIC_EVERY, 8_000),
            64,
            "512 帧 / 8s = 64fps"
        );
        assert_eq!(
            Sched::fps_in(METRIC_EVERY, 16_000),
            32,
            "窗口拉长则帧率下降"
        );
        assert_eq!(Sched::fps_in(METRIC_EVERY, 0), 0, "冻结时钟报 0 而非发散");
        assert_eq!(Sched::fps_in(0, 1_000), 0, "零帧窗口为 0");
    }

    /// §3.5 帧度量：上报节拍即窗口边界，起点随上报推进（窗口不重叠不遗漏）。
    #[test]
    fn metric_report_advances_window_origin() {
        let mut sched = fixture();
        for i in 0..METRIC_EVERY {
            sched.advance(i * 16);
            sched.note_frame(100, 200);
        }
        assert_eq!(sched.frames, METRIC_EVERY);
        assert_eq!(
            sched.rate_ms,
            (METRIC_EVERY - 1) * 16,
            "窗口起点须推进到本次上报时刻"
        );
    }

    /// §3.5 背压：超上限丢中间态、保首尾。
    #[test]
    fn sheds_middle_hiding_head_and_tail() {
        let mut sched = fixture();
        let total = QUEUE_CAP + 36;
        for i in 0..total {
            sched.advance(i as u64);
            assert!(sched.push(Lane::Stream));
            assert!(sched.queue.len() <= QUEUE_CAP, "队列须恒以 64 为上限");
        }
        assert_eq!(sched.queue.front().map(|(_, at)| *at), Some(0), "保留首条");
        assert_eq!(
            sched.queue.back().map(|(_, at)| *at),
            Some((total - 1) as u64),
            "保留末条"
        );
        assert_eq!(sched.shed, 36, "溢出部分计为丢弃");
    }

    /// §3.5 背压：输入档不参与丢弃，满队列时拒收而非丢业务事件。
    #[test]
    fn input_lane_is_never_shed() {
        let mut sched = fixture();
        sched.advance(0);
        for _ in 0..QUEUE_CAP {
            assert!(sched.push(Lane::Input));
        }
        assert!(!sched.push(Lane::Input), "仅输入档占满时拒收");
        assert_eq!(sched.shed, 0, "业务事件不得丢弃");
        assert_eq!(sched.queue.len(), QUEUE_CAP);
    }

    /// §3.5 帧预算：超预算置降级态，新帧起算清零。
    #[test]
    fn over_budget_frame_degrades_and_resets() {
        let mut sched = fixture();
        sched.advance(0);
        sched.push(Lane::Input);
        assert!(sched.take_frame());
        assert!(!sched.degraded());

        sched.note_frame(4_000, 4_000);
        assert!(!sched.degraded(), "预算内不降级");

        sched.note_frame(5_000, 5_000);
        assert!(sched.degraded(), "超预算降级");

        sched.advance(1);
        sched.push(Lane::Input);
        assert!(sched.take_frame());
        assert!(!sched.degraded(), "降级态随新帧清零");
    }

    /// §3.5 空闲降频：无事件无到期事项时降到空闲节拍。
    #[test]
    fn idles_at_upper_bound_when_nothing_pending() {
        let mut sched = fixture();
        sched.advance(0);
        assert_eq!(sched.wait_ms(), IDLE_MS, "静止界面按空闲节拍等待");

        sched.push(Lane::Stream);
        assert_eq!(sched.wait_ms(), 16, "有待办时等待收窄到合帧窗口");

        sched.advance(16);
        assert!(sched.take_frame(), "窗口到期成帧");
        assert_eq!(sched.wait_ms(), IDLE_MS, "清算后回到空闲节拍");
    }

    /// §5A.3 W4 统一节拍表：启用即到期，每周期至多一次，停用即静默。
    #[test]
    fn beats_fire_once_per_period_and_stop() {
        let mut sched = fixture();
        sched.advance(0);
        assert_eq!(sched.wait_ms(), IDLE_MS, "无启用节拍时不介入等待");

        sched.beat_set(Beat::Hall, true);
        assert_eq!(sched.wait_ms(), 0, "启用即到期，须立即处理");
        let due: Beats = sched.advance(0);
        assert!(due.has(Beat::Hall), "启用后立即到期");
        assert!(!sched.advance(0).has(Beat::Hall), "同周期不重复");

        assert!(!sched.advance(999).has(Beat::Hall));
        assert!(sched.advance(1_000).has(Beat::Hall), "满周期后再次到期");
        assert_eq!(
            sched.wait_ms(),
            IDLE_MS,
            "下周期余量超出空闲上限，等待按上限封顶"
        );

        sched.beat_set(Beat::Hall, false);
        assert!(!sched.advance(9_999).has(Beat::Hall), "停用后不再到期");
        assert_eq!(sched.wait_ms(), IDLE_MS, "停用后退出等待计算");
    }

    /// 显式刷新请求：未启用的节拍不因强制而介入。
    #[test]
    fn forced_beat_requires_active_beat() {
        let mut sched = fixture();
        sched.advance(0);
        sched.beat_now(Beat::Hall);
        assert!(!sched.advance(0).has(Beat::Hall), "未启用不得被强制到期");

        sched.beat_set(Beat::Hall, true);
        sched.advance(0);
        sched.advance(100);
        sched.beat_now(Beat::Hall);
        assert!(sched.advance(100).has(Beat::Hall), "启用后强制立即到期");
    }

    /// 帧度量：按帧数节拍自计数，不因跳帧漂移。
    #[test]
    fn metrics_count_only_presented_frames() {
        let mut sched = fixture();
        sched.advance(0);
        for i in 0..8u64 {
            sched.advance(i * 100);
            assert!(!sched.take_frame(), "无待办不得成帧");
            sched.note_frame(100, 200);
        }
        assert_eq!(sched.frames, 8);
        assert_eq!(sched.layout_sum_us, 800);
        assert_eq!(sched.present_sum_us, 1_600);
        assert_eq!(sched.over, 0);
    }
}
