// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 对话虚拟视图（0.1.9 W8）：行高缓存 + 视口切片。
//
// 每帧流程：头部全物化（恒小）→ 线性扫全部消息仅做 HashMap 查找与
// 前缀和 → 只物化与窗口相交的消息块 → 流式尾段全物化（本身 O(1)）。
// 行高依赖宽度与折叠态，任一变化全量失效。
//
// 0.1.19 U9（TUI 方案 §6.10 #2）：投影再收两道界——单轮块行数钳到
// MAX_CELLS_PER_TURN（超出以裁剪标记收尾，Alt+O 展开取全量）；单帧物化
// 区间由 engine::window 按可视区间裁定，超出 60 条 / 1,000 行预算时裁剪。
// 行高记账仍按全量精确进行，总行数与滚动语义不随窗口变化。
//
// 0.1.19 U10（§6.10 #3）：视口外旧回合只保留行高摘要，已物化的完整块
// 最多保留 RETAINED_TURNS_LIMIT 条；上滚重新进入视口时按需重新投影。
//
// X2-17a：复用判定同时看稳定 id 与内容签名——同 id 内容变化（会话恢复的
// upsert、就地补写）不得命中旧行。

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::app::{App, ChatMessage, MessageRole};
use crate::budget::{MAX_CELLS_PER_TURN, RETAINED_TURNS_LIMIT};
use crate::engine::block::{slice_out, Index};
use crate::engine::window::{self, Span as Win};
use crate::gccp::FlowPhase;
use crate::style;

use super::{block, flow, welcome};

/// 回合分隔线固定行数（分隔线 + 空行）。内容与回合耗时相关不可缓存，
/// 但行数恒定，虚拟化按固定行数计入块高。
const SEP_LINES: u32 = 2;

/// 缓存保留余量：消息队列挤位（pop_front）后行高缓存可能残留被挤掉的
/// 条目，超过 MAX_CHAT_MESSAGES + 余量时按现存最老 id 修剪。
const CACHE_SLACK: usize = 512;

/// 一帧布局结果：视口内行 + 内容总行数 + 顶部行偏移。
pub(crate) struct ChatFrame {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) total: usize,
    pub(crate) from_top: usize,
    /// 视口内工具卡的行区间（相对视口首行，右开）与消息 id（X5-11a）。
    pub(crate) cards: Vec<(u16, u16, u64)>,
}

/// 已物化回合块：`compact`、逐条展开态与内容签名共同参与复用判定，
/// 任一不符即重投影。
struct Cell {
    id: u64,
    compact: bool,
    expanded: bool,
    sig: u64,
    lines: Vec<Line<'static>>,
}

/// 块内容签名（X2-17a）：身份只能证明「是同一条消息」，不能证明「内容没变」。
/// 渲染相关字段（角色/时间戳/原始字节数/长度）加内容首尾各 32 字节采样，
/// 保持每帧 O(1)——对整段内容做全量哈希会把逐帧成本拉回与历史字节数成正比，
/// 反而抵消虚拟化收益。
fn sig(msg: &ChatMessage) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(&msg.role).hash(&mut h);
    msg.timestamp.hash(&mut h);
    msg.original_bytes.hash(&mut h);
    msg.content.len().hash(&mut h);
    let bytes = msg.content.as_bytes();
    bytes[..bytes.len().min(32)].hash(&mut h);
    bytes[bytes.len().saturating_sub(32)..].hash(&mut h);
    h.finish()
}

/// 对话虚拟视图：按消息稳定 id 缓存（compact, 折叠后行数）。
///
/// 缓存随 App 生命周期跨帧、跨 tab 复用；消息 id 单调分配且从不复用，
/// 任何历史变化（/clear、挤位、新增）都不会产生假命中。
#[derive(Default)]
pub struct ChatView {
    width: usize,
    expanded: bool,
    heights: HashMap<u64, (bool, bool, u64, u32)>,
    /// 已物化旧回合（LRU 序）：视口外最多保留 RETAINED_TURNS_LIMIT 条。
    cells: VecDeque<Cell>,
    /// 行高缓存是否已完整走完一帧；冷启动首帧据此启用 30 条初帧预算。
    primed: bool,
    #[cfg(test)]
    measured: u64,
    #[cfg(test)]
    rendered: u64,
    /// 本帧复用的已物化行数（X2-17b）。
    #[cfg(test)]
    reused: u64,
    /// 本帧重新投影的行数（X2-17b）。
    #[cfg(test)]
    fresh: u64,
}

impl ChatView {
    pub fn new() -> Self {
        Self::default()
    }

    /// 计算一帧可见行。`all = true` 供测试：全量物化，用于等价性比对。
    pub(crate) fn layout(
        &mut self,
        app: &App,
        width: usize,
        viewport: usize,
        all: bool,
    ) -> ChatFrame {
        self.layout_from(
            app,
            width,
            viewport,
            all,
            app.scroll_offset,
            app.browse_expanded,
        )
    }

    /// 指定滚动偏移与展开态的布局（X8-04a）：全量会话视图与对话区共用
    /// 同一虚拟视图和行高缓存，仅「距底偏移 / 是否展开折叠」由调用方给出。
    /// 这样 Ctrl+O 回看长会话不会另起一套测量，切换视图也只多一次缓存失效。
    pub(crate) fn layout_from(
        &mut self,
        app: &App,
        width: usize,
        viewport: usize,
        all: bool,
        scroll: u16,
        expanded: bool,
    ) -> ChatFrame {
        self.sync(width, expanded);
        #[cfg(test)]
        {
            self.reused = 0;
            self.fresh = 0;
        }

        let mut head: Vec<Line<'static>> = Vec::new();
        flow::render_header(&mut head, app, width);

        // 空态欢迎页（无消息且处于对话阶段时占据正文）
        let mut welcome_seg: Vec<Line<'static>> = Vec::new();
        if app.messages.is_empty() && app.flow_phase == FlowPhase::Chat {
            welcome::append(&mut welcome_seg, width, viewport, app);
        }

        // 块索引：命中缓存即零内容渲染，未命中才物化测量
        let idx = Index::build(
            (0..app.messages.len()).map(|i| self.block_height(app, i, width)),
            head.len() + welcome_seg.len(),
        );

        let mut tail: Vec<Line<'static>> = Vec::new();
        flow::render_tail(&mut tail, app, width);

        let total = idx.end() + tail.len();
        // 行级滚动语义：scroll_offset 为「距底部向上滚的行数」，反转为距顶偏移
        let max_offset = total.saturating_sub(viewport);
        let from_top = max_offset.saturating_sub((scroll as usize).min(max_offset));
        let (w0, w1) = if all {
            (0, total)
        } else {
            (from_top, from_top + viewport)
        };

        // 只物化与窗口 [w0, w1) 相交的行块；窗口再经投影预算裁定
        let (m0, m1) = idx.visible(w0, w1);
        let win = if all {
            Win {
                start: 0,
                end: idx.len(),
            }
        } else if m0 >= m1 {
            Win { start: 0, end: 0 }
        } else {
            // 冷启动且视口贴底：首帧只投影最近 30 条，第二帧补满
            let initial = !self.primed && from_top + viewport >= total;
            window::project(idx.len(), m0, m1 - 1, idx.spans(), initial)
        };
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(w1.saturating_sub(w0));
        slice_out(&mut lines, &head, 0, w0, w1);
        if !welcome_seg.is_empty() {
            slice_out(&mut lines, &welcome_seg, head.len(), w0, w1);
        }
        let mut seg: Vec<Line<'static>> = Vec::new();
        let mut cards: Vec<(u16, u16, u64)> = Vec::new();
        for at in win.start..win.end {
            seg.clear();
            self.push_block(app, at, width, &mut seg, !all);
            let b0 = idx.start(at);
            slice_out(&mut lines, &seg, b0, w0, w1);
            // 卡片落格：只记与视口相交的工具卡（X5-11a 点击反查）。
            if !all && block::is_tool(app.messages[at].role) {
                let lo = b0.max(w0);
                let hi = (b0 + seg.len()).min(w1);
                if lo < hi {
                    cards.push(((lo - w0) as u16, (hi - w0) as u16, app.messages[at].id));
                }
            }
        }
        slice_out(&mut lines, &tail, idx.end(), w0, w1);

        self.trim(app, m0, m1);
        self.primed = true;
        self.prune(app);
        ChatFrame {
            lines,
            total,
            from_top,
            cards,
        }
    }

    /// 第 idx 条消息块行数：回合分隔线恒定 + 消息块（缓存或测量）。
    fn block_height(&mut self, app: &App, idx: usize, width: usize) -> u32 {
        let msg = &app.messages[idx];
        let sep = if msg.role == MessageRole::User && idx > 0 {
            SEP_LINES
        } else {
            0
        };
        let compact =
            idx > 0 && block::is_tool(app.messages[idx - 1].role) && block::is_tool(msg.role);
        let sig = sig(msg);
        let expanded = self.expanded || app.expanded_cards.contains(&msg.id);
        let body = match self.heights.get(&msg.id) {
            Some((c, e, s, h)) if *c == compact && *e == expanded && *s == sig => *h,
            _ => self.measure(app, idx, width, compact, expanded, sig),
        };
        sep + body
    }

    /// 物化一次测量块高并写入缓存（与窗口物化走同一渲染路径，保证一致）。
    fn measure(
        &mut self,
        app: &App,
        idx: usize,
        width: usize,
        compact: bool,
        expanded: bool,
        sig: u64,
    ) -> u32 {
        let mut seg: Vec<Line<'static>> = Vec::new();
        self.body(app, idx, width, compact, expanded, &mut seg);
        let h = seg.len() as u32;
        self.heights
            .insert(app.messages[idx].id, (compact, expanded, sig, h));
        #[cfg(test)]
        {
            self.measured += 1;
        }
        h
    }

    /// 物化窗口相交块的行（含前置回合分隔线）。`cache = false` 时
    /// 全量测试路径直接投影，不写物化缓存。
    fn push_block(
        &mut self,
        app: &App,
        idx: usize,
        width: usize,
        out: &mut Vec<Line<'static>>,
        cache: bool,
    ) {
        let msg = &app.messages[idx];
        if msg.role == MessageRole::User && idx > 0 {
            block::push_turn_separator(out, app);
        }
        let compact =
            idx > 0 && block::is_tool(app.messages[idx - 1].role) && block::is_tool(msg.role);
        let expanded = self.expanded || app.expanded_cards.contains(&msg.id);
        if cache {
            self.cell(app, idx, width, compact, out);
        } else {
            self.body(app, idx, width, compact, expanded, out);
        }
    }

    /// 取一条已物化回合块：命中即复用（LRU 移到队尾），未命中才重新投影。
    fn cell(
        &mut self,
        app: &App,
        idx: usize,
        width: usize,
        compact: bool,
        out: &mut Vec<Line<'static>>,
    ) {
        let id = app.messages[idx].id;
        let sig = sig(&app.messages[idx]);
        let expanded = self.expanded || app.expanded_cards.contains(&id);
        let hit = self
            .cells
            .iter()
            .position(|c| c.id == id && c.compact == compact && c.expanded == expanded && c.sig == sig);
        if let Some(pos) = hit {
            if let Some(cell) = self.cells.remove(pos) {
                out.extend(cell.lines.iter().cloned());
                #[cfg(test)]
                {
                    self.reused += cell.lines.len() as u64;
                }
                self.cells.push_back(cell);
            }
            return;
        }
        let mut seg: Vec<Line<'static>> = Vec::new();
        self.body(app, idx, width, compact, expanded, &mut seg);
        out.extend(seg.iter().cloned());
        #[cfg(test)]
        {
            self.fresh += seg.len() as u64;
        }
        self.cells.push_back(Cell {
            id,
            compact,
            expanded,
            sig,
            lines: seg,
        });
        #[cfg(test)]
        {
            self.rendered += 1;
        }
    }

    /// 视口外旧回合降为行高摘要：完整块只保留最近 RETAINED_TURNS_LIMIT 条。
    fn trim(&mut self, app: &App, lo: usize, hi: usize) {
        let bounds = if lo < hi {
            match (app.messages.get(lo), app.messages.get(hi - 1)) {
                (Some(first), Some(last)) => (first.id, last.id),
                _ => return,
            }
        } else {
            (u64::MAX, 0)
        };
        let mut outside = self
            .cells
            .iter()
            .filter(|c| c.id < bounds.0 || c.id > bounds.1)
            .count();
        while outside > RETAINED_TURNS_LIMIT {
            let evictable = self
                .cells
                .front()
                .is_some_and(|c| c.id < bounds.0 || c.id > bounds.1);
            if !evictable {
                break;
            }
            self.cells.pop_front();
            outside -= 1;
        }
    }

    /// 渲染单条消息块并按单轮 cell 上限钳位（展开态取全量）。
    fn body(
        &self,
        app: &App,
        idx: usize,
        width: usize,
        compact: bool,
        expanded: bool,
        out: &mut Vec<Line<'static>>,
    ) {
        let base = out.len();
        block::render(out, &app.messages[idx], width, compact, expanded);
        if expanded {
            return;
        }
        let len = out.len() - base;
        if len <= MAX_CELLS_PER_TURN {
            return;
        }
        let clipped = len - (MAX_CELLS_PER_TURN - 1);
        out.truncate(base + MAX_CELLS_PER_TURN - 1);
        out.push(Line::from(Span::styled(
            window::clip_marker(clipped),
            Style::default().fg(style::faint()),
        )));
    }

    /// 行数依赖宽度与折叠态：任一变化全量失效。
    fn sync(&mut self, width: usize, expanded: bool) {
        if self.width != width || self.expanded != expanded {
            self.heights.clear();
            self.cells.clear();
            self.width = width;
            self.expanded = expanded;
            self.primed = false;
        }
    }

    /// 修剪被消息队列挤掉的缓存残留（id 单调不复用，按下界 retain）。
    fn prune(&mut self, app: &App) {
        if self.heights.len() <= crate::app::MAX_CHAT_MESSAGES + CACHE_SLACK {
            return;
        }
        match app.messages.front() {
            Some(front) => {
                let min_id = front.id;
                self.heights.retain(|id, _| *id >= min_id);
            }
            None => self.heights.clear(),
        }
    }

    /// 本帧行复用率（X2-17b）：复用行 ÷ 投影行总数；无投影行时视为全复用。
    /// 只读本帧计数，供用例量化「未变行跳过」的实效。
    #[cfg(test)]
    fn reuse_rate(&self) -> f64 {
        let total = self.reused + self.fresh;
        if total == 0 {
            return 1.0;
        }
        self.reused as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MessageRole;

    /// 构造隔离 App（守卫持 AIRY_HOME 与 test_env 锁至测试结束，
    /// 与 app 模块既有测试同一范式）。
    fn make_app() -> (App, crate::test_env::Home) {
        let home = crate::test_env::Home::new("chat-view");
        let app = App::new("agents/main.agent.yaml");
        (app, home)
    }

    /// 按 6 类角色循环造历史：System 长思考链触发折叠，连续工具触发 compact。
    fn fill(app: &mut App, n: usize) {
        for i in 0..n {
            match i % 6 {
                0 => app.add_message(MessageRole::User, format!("question {i}")),
                1 => app.add_message(
                    MessageRole::System,
                    (0..12)
                        .map(|k| format!("thought segment {k}"))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                2 => app.add_message(MessageRole::ToolCall, format!("web_fetch q{i}")),
                3 => app.add_message(MessageRole::ToolCall, "shell_run ls -al".to_string()),
                4 => app.add_message(MessageRole::ToolResult, format!("{{\"ok\":{i}}}")),
                _ => app.add_message(
                    MessageRole::Agent,
                    format!("answer {i} with some length content here"),
                ),
            }
        }
    }

    /// 虚拟窗口 == 全量切片（折叠/分隔线/compact/滚动语义等价——
    /// 0.1.9 W8 验收核心）。
    #[test]
    fn virtual_window_equals_full_slicing() {
        let (mut app, _h) = make_app();
        fill(&mut app, 1200);
        let mut view = ChatView::new();
        let all = view.layout(&app, 80, 30, true);
        assert!(all.total > app.messages.len() * 3, "总行数应远超视口");
        for scroll in [0u16, 1, 13, 777, u16::MAX] {
            app.scroll_offset = scroll;
            let win = view.layout(&app, 80, 30, false);
            let max_off = all.total.saturating_sub(30);
            assert_eq!(
                win.from_top,
                max_off.saturating_sub((scroll as usize).min(max_off)),
                "from_top(scroll={scroll})"
            );
            let end = (win.from_top + 30).min(all.total);
            assert_eq!(
                win.lines.len(),
                end - win.from_top,
                "窗口行数(scroll={scroll})"
            );
            assert_eq!(
                win.lines,
                all.lines[win.from_top..end],
                "虚拟窗口与全量切片不一致(scroll={scroll})"
            );
        }
    }

    /// X2-17a：同 id 内容变化必须重投影，不得复用旧行。
    #[test]
    fn changed_content_is_not_reused() {
        let (mut app, _h) = make_app();
        fill(&mut app, 60);
        let mut view = ChatView::new();
        let before = view.layout(&app, 80, 30, false);
        let at = app.messages.len() - 1;
        app.messages[at].content.push_str("\n\n追加一段以改变块高");
        let after = view.layout(&app, 80, 30, false);
        let text: String = after
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("追加一段以改变块高"), "内容变化应重投影");
        assert!(after.total > before.total, "块高变化应反映到总行数");
    }

    /// 行高缓存增量性：同尺寸第二帧零测量；新增仅测一条；变宽全量重测。
    #[test]
    fn height_cache_is_incremental() {
        let (mut app, _h) = make_app();
        fill(&mut app, 300);
        let mut view = ChatView::new();
        view.layout(&app, 80, 30, false);
        assert_eq!(view.measured, 300, "首帧每条测量一次");
        view.layout(&app, 80, 30, false);
        assert_eq!(view.measured, 300, "第二帧零测量（缓存全命中）");
        app.add_message(MessageRole::User, "one more".to_string());
        view.layout(&app, 80, 30, false);
        assert_eq!(view.measured, 301, "新增消息仅测量一条");
        view.layout(&app, 60, 30, false);
        assert_eq!(view.measured, 602, "宽度变化触发全量重测");
    }

    /// 队列挤位后窗口仍与全量一致（上限 2000 生效）。
    #[test]
    fn trim_keeps_window_aligned() {
        let (mut app, _h) = make_app();
        fill(&mut app, crate::app::MAX_CHAT_MESSAGES + 500);
        assert_eq!(
            app.messages.len(),
            crate::app::MAX_CHAT_MESSAGES,
            "队列上限挤位生效"
        );
        let mut view = ChatView::new();
        let all = view.layout(&app, 80, 30, true);
        let win = view.layout(&app, 80, 30, false);
        let end = (win.from_top + 30).min(all.total);
        assert_eq!(win.lines, all.lines[win.from_top..end]);
    }

    /// 缓存修剪：先测满 2000，再挤位新增 600 → 残留按最老现存 id 修剪。
    #[test]
    fn cache_prunes_trimmed_history() {
        let (mut app, _h) = make_app();
        fill(&mut app, crate::app::MAX_CHAT_MESSAGES);
        let mut view = ChatView::new();
        view.layout(&app, 80, 30, false);
        fill(&mut app, 600);
        view.layout(&app, 80, 30, false);
        assert!(
            view.heights.len() <= crate::app::MAX_CHAT_MESSAGES + CACHE_SLACK,
            "缓存修剪到上限: {}",
            view.heights.len()
        );
        let front_id = app.messages.front().expect("front").id;
        assert!(view.heights.contains_key(&front_id), "现存消息缓存保留");
    }

    /// U9（§6.10 #2）：单轮 cell 钳位——长答复只物化 120 行、末行为裁剪
    /// 标记；展开态（Alt+O）取全量，总行数随之增长。
    #[test]
    fn cell_budget_clamps_single_turn() {
        let (mut app, _h) = make_app();
        app.add_message(
            MessageRole::Agent,
            (0..500)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        let mut view = ChatView::new();
        let frame = view.layout(&app, 80, 30, true);
        let id = app.messages.back().expect("message").id;
        let (_, _, _, h) = view.heights.get(&id).copied().expect("height cached");
        assert_eq!(
            h as usize,
            crate::budget::MAX_CELLS_PER_TURN,
            "单轮块应钳到上限"
        );
        let text: Vec<String> = frame.lines.iter().map(|l| l.to_string()).collect();
        assert!(
            text.iter().any(|l| l.contains("已裁剪")),
            "钳位应留下裁剪标记"
        );

        app.browse_expanded = true;
        let full = view.layout(&app, 80, 30, true);
        let (_, _, _, h) = view.heights.get(&id).copied().expect("height cached");
        assert!(
            h as usize > crate::budget::MAX_CELLS_PER_TURN,
            "展开态应取全量: {h}"
        );
        assert!(full.total > frame.total, "展开后总行数应增长");
    }

    /// U9（§6.10 #2）：超高视口下物化总量受投影预算约束（≤1,000 行）。
    #[test]
    fn projection_budget_bounds_materialization() {
        let (mut app, _h) = make_app();
        for i in 0..60 {
            app.add_message(
                MessageRole::Agent,
                (0..100)
                    .map(|k| format!("row {i}-{k}"))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
        }
        let mut view = ChatView::new();
        let all = view.layout(&app, 80, 4000, true);
        assert!(all.total > 4000, "前置条件：内容应远超视口");
        let win = view.layout(&app, 80, 4000, false);
        assert!(
            win.lines.len() <= crate::budget::MAX_PROJECTED_CELLS,
            "物化行数应受 cell 界约束: {}",
            win.lines.len()
        );
        assert!(win.lines.len() < all.total, "窗口应裁掉未读内容");
    }

    /// 窄屏空态渲染单行品牌（<44 列短路路径）。
    #[test]
    fn welcome_narrow_single_line() {
        let (app, _h) = make_app();
        let mut view = ChatView::new();
        let frame = view.layout(&app, 40, 12, false);
        let text: String = frame.lines.iter().map(|l| l.to_string()).collect();
        assert!(text.contains("AirymaxRT"), "窄屏欢迎行: {text}");
    }

    /// B2（0.1.18）V2.3 / V2.1：请求发出后的**任意一帧**尾部都已有可见受理
    /// 状态与已耗时计数——零反馈窗口不存在；状态随流阶段演进为
    /// 「思考中 → 生成中」，落定后状态行消失（不残留瞬态）。
    #[test]
    fn busy_tail_exposes_state_and_elapsed() {
        let (mut app, _h) = make_app();
        let mut view = ChatView::new();
        let text = |view: &mut ChatView, app: &App| {
            view.layout(app, 80, 30, true)
                .lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 请求刚发出（begin_busy 语义）：首帧即受理 + 已耗时
        app.loading = true;
        app.busy_started = std::time::Instant::now();
        let t = text(&mut view, &app);
        let line = t
            .lines()
            .find(|l| l.contains("已受理"))
            .expect("busy 帧应有「已受理」状态行");
        assert!(
            line.trim_end().ends_with('s'),
            "受理状态行应附已耗时计数: {line:?}"
        );

        // 思考链增量到达 → 思考中（含字数）
        app.stream_reasoning = "推演".into();
        let t = text(&mut view, &app);
        assert!(t.contains("思考中"), "思考链到达应显示思考中: {t}");

        // 正文增量到达 → 生成中
        app.streaming_text = "答案".into();
        let t = text(&mut view, &app);
        assert!(t.contains("生成中"), "正文到达应显示生成中: {t}");

        // 落定：瞬态状态行随之消失
        app.loading = false;
        let t = text(&mut view, &app);
        assert!(
            !t.contains("已受理") && !t.contains("生成中"),
            "落定后不应残留状态行: {t}"
        );
    }

    /// §6.2 #3：thinking 块折叠态显示前 2,000 字符与「思考中 · N s」计时，
    /// 超出部分折起；回合落定后自动收起，原文仅经 Alt+E 按需取用。
    #[test]
    fn thinking_preview_is_folded_and_bounded() {
        let (mut app, _h) = make_app();
        let mut view = ChatView::new();
        let text = |view: &mut ChatView, app: &App| {
            view.layout(app, 80, 30, true)
                .lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 短链：预览原文 + 计时 + 全文入口，不出现折叠尾
        app.loading = true;
        app.busy_started = std::time::Instant::now();
        app.stream_reasoning = "先看火焰图".into();
        app.stream_reasoning_start =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        let t = text(&mut view, &app);
        assert!(t.contains("思考中 · 2s"), "应有思考计时: {t}");
        assert!(t.contains("先看火焰图"), "折叠态应显示预览原文: {t}");
        assert!(t.contains("Alt+E 查看全文"), "应留全文入口: {t}");
        assert!(!t.contains("已折叠"), "短链不应折叠: {t}");

        // 超长链：只显示前 2,000 字符，余量记入折叠尾
        app.stream_reasoning = format!("{}{}", "甲".repeat(2_000), "乙".repeat(500));
        let t = text(&mut view, &app);
        assert!(t.contains('甲'), "应显示预览首段: {t}");
        assert!(!t.contains('乙'), "超出预算的字符不得上屏: {t}");
        assert!(t.contains("已折叠 500 字"), "折叠余量应记账: {t}");

        // 落定：预览块自动收起（原文转入 last_reasoning 供 Alt+E）
        app.loading = false;
        app.stream_reasoning.clear();
        app.last_reasoning = Some("甲".repeat(2_000));
        let t = text(&mut view, &app);
        assert!(!t.contains('甲'), "落定后预览应收起: {t}");
        assert!(!t.contains("思考中"), "落定后不应残留思考状态: {t}");
    }

    /// 钳位按字符而非字节：四字节 emoji 恰在边界时不误判折叠，超一字才折叠。
    #[test]
    fn thinking_preview_clamps_by_chars() {
        let (mut app, _h) = make_app();
        let mut view = ChatView::new();
        let text = |view: &mut ChatView, app: &App| {
            view.layout(app, 80, 30, true)
                .lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };

        app.loading = true;
        app.busy_started = std::time::Instant::now();
        app.stream_reasoning_start = Some(std::time::Instant::now());
        app.stream_reasoning = "🙂".repeat(crate::budget::THINKING_PREVIEW_MAX_CHARS);
        let t = text(&mut view, &app);
        assert!(!t.contains("已折叠"), "恰在预算上不得折叠: {t}");
        assert!(!t.contains('\u{fffd}'), "字符边界不得切坏 UTF-8: {t}");

        app.stream_reasoning.push('中');
        let t = text(&mut view, &app);
        assert!(t.contains("已折叠 1 字"), "超一字即折叠: {t}");
    }

    /// U10（§6.10 #3）：视口外旧回合只保留行高摘要，完整块 ≤ 8；
    /// 任意滚动位置物化缓存都有界，行高摘要覆盖全部历史。
    #[test]
    fn retained_cells_stay_bounded_across_scroll() {
        let (mut app, _h) = make_app();
        fill(&mut app, 400);
        let mut view = ChatView::new();
        let bound = crate::budget::MAX_PROJECTED_TURNS + RETAINED_TURNS_LIMIT;
        for scroll in [0u16, 1, 13, 777, u16::MAX] {
            app.scroll_offset = scroll;
            view.layout(&app, 80, 30, false);
            assert!(
                view.cells.len() <= bound,
                "物化缓存应有界(scroll={scroll}): {}",
                view.cells.len()
            );
        }
        assert_eq!(
            view.heights.len(),
            app.messages.len(),
            "每条历史仅留一行摘要"
        );
    }

    /// U10（§6.10 #3）：同视口第二帧零重投影；上滚离开缓存后按需重投影。
    #[test]
    fn retained_cells_reuse_on_revisit() {
        let (mut app, _h) = make_app();
        fill(&mut app, 120);
        let mut view = ChatView::new();
        view.layout(&app, 80, 30, false);
        let first = view.rendered;
        assert!(first > 0, "首帧应物化可见块");
        view.layout(&app, 80, 30, false);
        assert_eq!(view.rendered, first, "同视口第二帧零重投影");
        app.scroll_offset = 500;
        view.layout(&app, 80, 30, false);
        assert!(view.rendered > first, "上滚应重新投影旧回合");
    }

    /// IS-13：物化只发生在视口（及投影预算）内，离屏历史只留行高摘要。
    #[test]
    fn offscreen_blocks_stay_unmaterialized() {
        let (mut app, _h) = make_app();
        fill(&mut app, 1200);
        let mut view = ChatView::new();
        let budget = crate::budget::MAX_PROJECTED_TURNS as u64;
        view.layout(&app, 80, 30, false);
        let first = view.rendered;
        assert!(first > 0, "首帧应物化可见块");
        assert!(first <= budget, "首帧物化块数应受投影预算约束: {first}");
        assert!(
            first < app.messages.len() as u64 / 4,
            "离屏历史不得物化: {first} / {}",
            app.messages.len()
        );
        view.layout(&app, 80, 30, false);
        assert_eq!(view.rendered, first, "同视口不得重复物化");
        app.scroll_offset = u16::MAX;
        view.layout(&app, 80, 30, false);
        assert!(
            view.rendered - first <= budget,
            "跳转后新物化块数受预算约束: {}",
            view.rendered - first
        );
        assert!(
            view.rendered < app.messages.len() as u64 / 4,
            "累计物化仍应远离全量: {} / {}",
            view.rendered,
            app.messages.len()
        );
    }

    /// X2-17b：同视口第二帧不产生新行（复用率 1.0）；改末条内容后
    /// 该块重新投影，复用率下降但仍小于 1。
    #[test]
    fn reuse_rate_tracks_row_reuse() {
        let (mut app, _h) = make_app();
        fill(&mut app, 1200);
        let mut view = ChatView::new();
        view.layout(&app, 80, 30, false);
        assert!(view.fresh > 0, "首帧应投影可见行");
        view.layout(&app, 80, 30, false);
        assert_eq!(view.fresh, 0, "同视口第二帧不得重新投影任何行");
        assert!(view.reused > 0, "第二帧应全部命中已物化块");
        assert_eq!(view.reuse_rate(), 1.0);

        let at = app.messages.len() - 1;
        app.messages[at].content.push_str("\n\n追加一段以改变块高");
        view.layout(&app, 80, 30, false);
        assert!(view.fresh > 0, "内容变化应重新投影该块");
        let rate = view.reuse_rate();
        assert!(rate > 0.5 && rate < 1.0, "复用率应下降但不塌陷: {rate}");
    }
}
