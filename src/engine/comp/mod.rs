// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 组件框架（0.1.21 X3）：把「测量 → 布局 → 渲染 → 事件」四段式契约下沉为
// 唯一入口，面板不再各自手写列宽、截断与行号。
//
// 设计依据（§23.5.2 X3-01~X3-12）：
//   - 渲染只写 `Buffer`，不触终端：帧合成仍由 L5 统一 flush，组件不得持有
//     `Frame`/`Terminal`，否则测试无法以 Buffer 快照断言渲染结果。
//   - 测量与布局分离：`measure` 接收可用空间并回答「我至少/至多要多大」，
//     `layout` 只接收上层裁决后的矩形。组件不得自行读取终端尺寸，否则
//     多组件争宽时无法复现（X3-02b 的争宽用例依赖此约束）。
//   - 事件统一入参：按键与鼠标都折算为 `Input`，返回与 UI 框架无关的
//     `Act`。上层（按键分派/命中测试）决定把动作映射到哪个组件，组件自身
//     不引用 `App`，避免 L2 反向依赖 L6。
//   - 宽度仍只由 `engine::grid` 裁决：本框架只编排矩形与列宽分配，不新增
//     宽度测量实现（§3.3 单一宽度源）。
//
// 不变量（table 用例实证）：列宽求解结果在可用宽度内永不超过右边界；
// 裁剪结果显示宽度 ≤ 列宽；排序稳定（等值键保持输入序）。

pub(crate) mod reg;
pub(crate) mod table;

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{buffer::Buffer, layout::Rect};

/// 空间尺寸（终端列/行）。`measure` 的入参是剩余可用空间，出参是期望尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Size {
    pub w: u16,
    pub h: u16,
}

/// 列宽约束（X3-07a）。
///
/// `Min` 为下限（空间不足时仍可被压缩），`Max` 为上限（内容更宽时裁剪并
/// 以省略号收尾），`Flex` 为弹性列（按权重吸收剩余空间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Width {
    Min(usize),
    Max(usize),
    Flex(u16),
}

/// 统一事件入参（X3-04a）：按键与鼠标坐标同构，组件不必感知事件来源。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Input {
    Key(KeyEvent),
    /// `row` 为屏幕行号（与 `layout` 的矩形同一坐标系，由命中测试原样透传）。
    Mouse { event: MouseEvent, row: u16 },
}

/// 语义动作（X3-04b）：组件把输入折算为动作，由上层决定落地方式。
///
/// 组件不直接改业务状态——同一动作既可能来自键盘也可能来自鼠标点击，
/// 上层据此写回 `App`，保证两条输入路径行为一致（X5-12a）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    None,
    Up,
    Down,
    Top,
    Bottom,
    Select(usize),
}

/// 组件四段式契约。
pub(crate) trait Component {
    /// 在 `avail` 内报告期望尺寸；不得修改自身状态。
    fn measure(&self, avail: Size) -> Size;

    /// 接收上层裁决后的矩形（X3-02a 分配规则见 [`table`]）。
    fn layout(&mut self, area: Rect);

    /// 只写 `Buffer`，不触终端（X3-03a）。
    fn render(&self, buf: &mut Buffer);

    /// 统一事件入口（X3-04a），返回语义动作。
    fn event(&mut self, input: Input) -> Act;
}
