// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 命中测试（0.1.21 X5-07）。渲染每帧把实际落格的区域写入外壳快照，鼠标
// 事件按同一份事实反查目标，避免「渲染一套几何、命中共另一套几何」的漂移。
//
// 纯数据：不读终端、不引用 ratatui 的 Rect（由呈现层折算为 Area）。

/// 屏幕矩形（左上原点，列宽行高）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Area {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Area {
    pub(crate) fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    /// 点是否落在区域内（右/下边界开区间）。
    pub(crate) fn contains(&self, col: u16, row: u16) -> bool {
        self.w > 0
            && self.h > 0
            && col >= self.x
            && col < self.x.saturating_add(self.w)
            && row >= self.y
            && row < self.y.saturating_add(self.h)
    }
}

/// 外壳几何快照：渲染每帧回写，命中测试只读。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Shell {
    pub main: Area,
    pub approval: Option<Area>,
    pub input: Area,
    pub shortcuts: Area,
}

/// 命中目标：指向控件类别，不绑定具体面板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Main,
    Approval,
    Input,
    Shortcuts,
}

impl Shell {
    /// 按 z 序裁决命中（X5-07b）：审批条覆盖主区之上，先于主区判定；
    /// 输入栏与快捷键条是独立分区，不存在重叠，仍显式排序以便未来叠层。
    pub(crate) fn hit(&self, col: u16, row: u16) -> Option<Target> {
        if let Some(a) = self.approval {
            if a.contains(col, row) {
                return Some(Target::Approval);
            }
        }
        if self.input.contains(col, row) {
            return Some(Target::Input);
        }
        if self.shortcuts.contains(col, row) {
            return Some(Target::Shortcuts);
        }
        if self.main.contains(col, row) {
            return Some(Target::Main);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Shell {
        Shell {
            main: Area::new(0, 1, 80, 20),
            approval: Some(Area::new(0, 21, 80, 2)),
            input: Area::new(0, 23, 80, 1),
            shortcuts: Area::new(0, 24, 80, 1),
        }
    }

    #[test]
    fn regions_hit_their_own_targets() {
        let s = shell();
        assert_eq!(s.hit(10, 5), Some(Target::Main));
        assert_eq!(s.hit(10, 22), Some(Target::Approval));
        assert_eq!(s.hit(10, 23), Some(Target::Input));
        assert_eq!(s.hit(10, 24), Some(Target::Shortcuts));
        assert_eq!(s.hit(10, 25), None);
    }

    #[test]
    fn approval_wins_over_overlapping_main() {
        let mut s = shell();
        s.main = Area::new(0, 0, 80, 30);
        assert_eq!(s.hit(4, 21), Some(Target::Approval));
    }

    #[test]
    fn edges_are_half_open() {
        let a = Area::new(2, 3, 4, 2);
        assert!(a.contains(2, 3));
        assert!(a.contains(5, 4));
        assert!(!a.contains(6, 4));
        assert!(!a.contains(5, 5));
        assert!(!Area::default().contains(0, 0));
    }

    #[test]
    fn without_approval_banner_click_falls_to_main() {
        let mut s = shell();
        s.approval = None;
        assert_eq!(s.hit(10, 21), Some(Target::Main));
    }
}
