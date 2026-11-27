// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 浮层几何层（0.1.21 X2-18a）：浮层矩形与视口裁剪的唯一裁决点。
//
// 浮层此前各自用 `saturating_sub` 手算贴边坐标：浮层宽度大于视口时左端
// 仍从视口内起写、右端直接越界，后端把越界列裁掉后只剩半截浮层；同一套
// 坐标计算散落在各处，新增浮层只能照抄。本模块把「浮层多大、贴哪条边、
// 被视口裁成什么」收成纯几何函数，呈现层只声明期望尺寸，越界部分在这里
// 消掉，成画区域因此始终是一个完整矩形。
//
// 纯几何：无 IO、无时钟、不引用 ratatui（与 L2 布局计划同一分层边界）。
// 矩形以本模块 `Rect` 表达，呈现层负责与终端坐标互译，边界行为可脱离
// 终端单测。

/// 终端矩形：左上角坐标 + 宽高（列/行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) w: u16,
    pub(crate) h: u16,
}

impl Rect {
    pub(crate) const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    /// 右边界（不含）：坐标加宽高，饱和避免溢出回绕。
    pub(crate) const fn right(self) -> u16 {
        self.x.saturating_add(self.w)
    }

    /// 下边界（不含）：坐标加高，饱和避免溢出回绕。
    pub(crate) const fn bottom(self) -> u16 {
        self.y.saturating_add(self.h)
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// 与本视口的交集；任一为空或互不相交返回 `None`。
    ///
    /// 裁剪是浮层写入的前置条件：呈现层只会在返回值范围内落格，越界列
    /// 根本不会到达后端，故「半截浮层」不会出现。
    pub(crate) fn clip(self, view: Rect) -> Option<Rect> {
        if self.is_empty() || view.is_empty() {
            return None;
        }
        let x = self.x.max(view.x);
        let y = self.y.max(view.y);
        let right = self.right().min(view.right());
        let bottom = self.bottom().min(view.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(Rect::new(x, y, right - x, bottom - y))
    }
}

/// 贴视口右下角放置 `w × h` 浮层：尺寸先收敛到视口，再整体裁剪。
///
/// 尺寸先收敛保证浮层大于视口时贴住左上角而不是从负坐标起写（u16 无符号，
/// 直接相减会回绕成极大值）；裁剪兜底处理视口原点非零的嵌套区域。
pub(crate) fn at_bottom_right(view: Rect, w: u16, h: u16) -> Option<Rect> {
    if view.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let w = w.min(view.w);
    let h = h.min(view.h);
    Rect::new(view.right() - w, view.bottom() - h, w, h).clip(view)
}

/// 贴视口底部居中放置 `w × h` 浮层：宽度先收敛到视口，余数左偏。
///
/// 居中只解横向：底部锚点取 `bottom - h` 与右下角同源，纵向语义一致。
/// 余数为奇数时左偏一列——左右偏差恒定且可预测，比按奇偶切换偏向更好
/// 断言（golden 固定坐标不用随终端宽度奇偶变化）。
pub(crate) fn at_bottom_center(view: Rect, w: u16, h: u16) -> Option<Rect> {
    if view.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let w = w.min(view.w);
    let h = h.min(view.h);
    let x = view.x + (view.w - w) / 2;
    Rect::new(x, view.bottom() - h, w, h).clip(view)
}

#[cfg(test)]
mod tests {
    use super::{at_bottom_center, at_bottom_right, Rect};

    /// 完全落在视口内的矩形原样返回。
    #[test]
    fn clip_keeps_contained_rect() {
        let view = Rect::new(0, 0, 80, 24);
        let inner = Rect::new(10, 5, 20, 4);
        assert_eq!(inner.clip(view), Some(inner));
    }

    /// 四条边分别越界时逐边裁掉，保留完整交集。
    #[test]
    fn clip_trims_overhang_on_each_edge() {
        let view = Rect::new(10, 2, 20, 10);
        assert_eq!(
            Rect::new(15, 4, 30, 12).clip(view),
            Some(Rect::new(15, 4, 15, 8)),
            "右/下越界裁到视口边界"
        );
        assert_eq!(
            Rect::new(0, 0, 12, 4).clip(view),
            Some(Rect::new(10, 2, 2, 2)),
            "左/上越界裁到视口原点"
        );
        assert_eq!(
            Rect::new(0, 0, 80, 24).clip(view),
            Some(view),
            "完全覆盖返回整个视口"
        );
    }

    /// 不相交、零宽高、零面积视口一律 `None`，不产生退化矩形。
    #[test]
    fn clip_rejects_disjoint_and_empty() {
        let view = Rect::new(0, 0, 20, 10);
        assert_eq!(Rect::new(20, 0, 5, 5).clip(view), None, "右邻接不算相交");
        assert_eq!(Rect::new(0, 10, 5, 5).clip(view), None, "下邻接不算相交");
        assert_eq!(Rect::new(3, 3, 0, 5).clip(view), None);
        assert_eq!(Rect::new(3, 3, 5, 0).clip(view), None);
        assert_eq!(Rect::new(3, 3, 5, 5).clip(Rect::new(0, 0, 0, 10)), None);
    }

    /// 视口原点非零时按视口而非屏幕原点贴边（嵌套区域）。
    #[test]
    fn bottom_right_respects_view_origin() {
        let view = Rect::new(10, 3, 20, 5);
        assert_eq!(
            at_bottom_right(view, 6, 2),
            Some(Rect::new(24, 6, 6, 2)),
            "右下角贴 (right-w, bottom-h)"
        );
    }

    /// 浮层大于视口时收敛为整视口，不因无符号相减回绕。
    #[test]
    fn oversized_overlay_shrinks_to_view() {
        let view = Rect::new(0, 0, 10, 4);
        assert_eq!(at_bottom_right(view, 40, 9), Some(view));
        assert_eq!(at_bottom_right(view, u16::MAX, u16::MAX), Some(view));
    }

    /// 空请求与空视口不成画。
    #[test]
    fn empty_request_or_view_is_none() {
        let view = Rect::new(0, 0, 10, 4);
        assert_eq!(at_bottom_right(view, 0, 1), None);
        assert_eq!(at_bottom_right(view, 1, 0), None);
        assert_eq!(at_bottom_right(Rect::new(0, 0, 0, 4), 1, 1), None);
        assert_eq!(at_bottom_right(Rect::new(0, 0, 10, 0), 1, 1), None);
    }

    /// 极端坐标下边界饱和而非回绕成小值（否则裁剪误判为「在内」）。
    #[test]
    fn edges_saturate_at_u16_max() {
        let far = Rect::new(65_000, 65_000, 1_000, 1_000);
        assert_eq!(far.right(), u16::MAX);
        assert_eq!(far.bottom(), u16::MAX);
        let view = Rect::new(0, 0, 100, 100);
        assert_eq!(far.clip(view), None, "饱和边界不得伪装成相交");
    }

    /// 底部居中：奇数余数左偏，纵向贴住视口底边。
    #[test]
    fn bottom_center_biases_left_on_odd_remainder() {
        let view = Rect::new(0, 0, 80, 24);
        assert_eq!(
            at_bottom_center(view, 60, 1),
            Some(Rect::new(10, 23, 60, 1))
        );
        assert_eq!(
            at_bottom_center(Rect::new(4, 2, 21, 10), 6, 1),
            Some(Rect::new(11, 11, 6, 1))
        );
    }

    /// 底部居中同样先收敛后裁剪：超宽浮层退化为整视口。
    #[test]
    fn bottom_center_shrinks_and_rejects_empty() {
        let view = Rect::new(0, 0, 10, 4);
        assert_eq!(at_bottom_center(view, 40, 9), Some(view));
        assert_eq!(at_bottom_center(view, 0, 1), None);
        assert_eq!(at_bottom_center(Rect::new(0, 0, 10, 0), 1, 1), None);
    }
}
