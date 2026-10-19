// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// L2 布局计划（0.1.19 TUI 建设方案 §4.2 引擎化）：把外壳的垂直分区从
// 渲染代码里抽出为纯数据计划，引擎裁决布局、业务只按命名槽位取区域。
//
// 此前 ui.rs 内联构造约束列表，再以 `main_at = if banner { 2 } else { 1 }`
// 手算下标——每增一个浮层都要同步改三处（约束、下标基数、后续 idx 递增），
// 漏改即整屏错位且无编译期提示。本模块把「有哪些槽位、槽位在第几行」的
// 唯一事实收进 `Plan`，渲染侧只问槽位名，不再持有序号。
//
// 纯函数：无 IO、无时钟、无全局状态，也不引用 ratatui——行高以自定义
// `RowH` 表达，由呈现层翻译为终端约束，边界行为可脱离终端单测。

use crate::budget::{
    SHELL_APPROVAL_ROWS, SHELL_BANNER_ROWS, SHELL_INPUT_IME_ROWS, SHELL_INPUT_ROWS,
    SHELL_LEADER_ROWS, SHELL_MAIN_MIN_ROWS, SHELL_SHORTCUT_ROWS, SHELL_STATUS_ROWS,
};

/// 外壳垂直槽位。顺序即屏幕自上而下的固定次序，不随浮层存在性重排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// 系统状态条（文字行 + 细分隔线）。
    Status,
    /// 依赖降级横幅。
    Banner,
    /// 主内容区（面板占位）。
    Main,
    /// 工具审批条。
    Approval,
    /// 瞬态提示队列。
    Toasts,
    /// leader 前缀键提示。
    Leader,
    /// 输入栏。
    Input,
    /// 快捷键行。
    Shortcuts,
}

/// 行高约束：定高或最小高（自适应吸收剩余空间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowH {
    Fixed(u16),
    Min(u16),
}

/// 一个布局行：槽位 + 行高。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) slot: Slot,
    pub(crate) h: RowH,
}

/// 命名槽位到行下标的映射：缺席的可选槽位为 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slots {
    pub(crate) status: usize,
    pub(crate) banner: Option<usize>,
    pub(crate) main: usize,
    pub(crate) approval: Option<usize>,
    pub(crate) toasts: Option<usize>,
    pub(crate) leader: Option<usize>,
    pub(crate) input: usize,
    pub(crate) shortcuts: usize,
}

/// 一帧的完整外壳布局计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) rows: Vec<Row>,
    pub(crate) slots: Slots,
}

impl Plan {
    /// 按槽位取行下标；可选槽位缺席时为 `None`。
    ///
    /// 生产路径直接读 `slots` 字段（编译期即可判空），此方法只服务于
    /// 单测里的「槽位表与行序一致性」不变量断言，故仅在测试构建中编译。
    #[cfg(test)]
    pub(crate) fn at(&self, slot: Slot) -> Option<usize> {
        match slot {
            Slot::Status => Some(self.slots.status),
            Slot::Banner => self.slots.banner,
            Slot::Main => Some(self.slots.main),
            Slot::Approval => self.slots.approval,
            Slot::Toasts => self.slots.toasts,
            Slot::Leader => self.slots.leader,
            Slot::Input => Some(self.slots.input),
            Slot::Shortcuts => Some(self.slots.shortcuts),
        }
    }
}

/// 计算外壳布局。
///
/// `toasts` 为当前瞬态提示条数（0 表示不占版面），`input_h` 由输入栏
/// 自身状态给出（常规/IME 态），其余为浮层存在性。
pub(crate) fn plan(
    banner: bool,
    approval: bool,
    toasts: usize,
    leader: bool,
    input_h: u16,
) -> Plan {
    let mut rows = Vec::with_capacity(8);
    let mut slots = Slots {
        status: 0,
        banner: None,
        main: 0,
        approval: None,
        toasts: None,
        leader: None,
        input: 0,
        shortcuts: 0,
    };
    rows.push(Row {
        slot: Slot::Status,
        h: RowH::Fixed(SHELL_STATUS_ROWS),
    });
    if banner {
        slots.banner = Some(rows.len());
        rows.push(Row {
            slot: Slot::Banner,
            h: RowH::Fixed(SHELL_BANNER_ROWS),
        });
    }
    slots.main = rows.len();
    rows.push(Row {
        slot: Slot::Main,
        h: RowH::Min(SHELL_MAIN_MIN_ROWS),
    });
    if approval {
        slots.approval = Some(rows.len());
        rows.push(Row {
            slot: Slot::Approval,
            h: RowH::Fixed(SHELL_APPROVAL_ROWS),
        });
    }
    if toasts > 0 {
        slots.toasts = Some(rows.len());
        rows.push(Row {
            slot: Slot::Toasts,
            h: RowH::Fixed(toasts as u16),
        });
    }
    if leader {
        slots.leader = Some(rows.len());
        rows.push(Row {
            slot: Slot::Leader,
            h: RowH::Fixed(SHELL_LEADER_ROWS),
        });
    }
    slots.input = rows.len();
    rows.push(Row {
        slot: Slot::Input,
        h: RowH::Fixed(input_h),
    });
    slots.shortcuts = rows.len();
    rows.push(Row {
        slot: Slot::Shortcuts,
        h: RowH::Fixed(SHELL_SHORTCUT_ROWS),
    });
    Plan { rows, slots }
}

/// 输入栏高度：IME 拼音态扩为三行，其余两行。
pub(crate) fn input_h(ime: bool) -> u16 {
    if ime {
        SHELL_INPUT_IME_ROWS
    } else {
        SHELL_INPUT_ROWS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无浮层：状态条 → 主内容 → 输入栏 → 快捷键，下标连续。
    #[test]
    fn bare_shell_has_contiguous_slots() {
        let p = plan(false, false, 0, false, SHELL_INPUT_ROWS);
        assert_eq!(p.rows.len(), 4);
        assert_eq!(p.slots.status, 0);
        assert_eq!(p.slots.banner, None);
        assert_eq!(p.slots.main, 1);
        assert_eq!(p.slots.approval, None);
        assert_eq!(p.slots.toasts, None);
        assert_eq!(p.slots.leader, None);
        assert_eq!(p.slots.input, 2);
        assert_eq!(p.slots.shortcuts, 3);
        assert_eq!(p.rows[1].h, RowH::Min(SHELL_MAIN_MIN_ROWS));
    }

    /// 仅横幅：主内容整体下移一行，槽位仍连续。
    #[test]
    fn banner_shifts_main_down() {
        let p = plan(true, false, 0, false, SHELL_INPUT_ROWS);
        assert_eq!(p.slots.banner, Some(1));
        assert_eq!(p.slots.main, 2);
        assert_eq!(p.at(Slot::Main), Some(2));
        assert_eq!(p.rows.len(), 5);
    }

    /// 浮层全开：顺序固定为 状态/横幅/主内容/审批/提示/leader/输入/快捷键。
    #[test]
    fn all_overlays_keep_declared_order() {
        let p = plan(true, true, 3, true, SHELL_INPUT_IME_ROWS);
        assert_eq!(p.rows.len(), 8);
        let order: Vec<Slot> = p.rows.iter().map(|r| r.slot).collect();
        assert_eq!(
            order,
            vec![
                Slot::Status,
                Slot::Banner,
                Slot::Main,
                Slot::Approval,
                Slot::Toasts,
                Slot::Leader,
                Slot::Input,
                Slot::Shortcuts,
            ]
        );
        for (idx, row) in p.rows.iter().enumerate() {
            assert_eq!(p.at(row.slot), Some(idx), "槽位下标与行序不一致");
        }
        assert_eq!(p.rows[4].h, RowH::Fixed(3), "提示条数即行高");
        assert_eq!(p.rows[6].h, RowH::Fixed(SHELL_INPUT_IME_ROWS));
    }

    /// 可选槽位缺席即 `None`，渲染侧据此跳过，不再手算下标。
    #[test]
    fn absent_overlays_are_none() {
        let p = plan(false, false, 0, false, SHELL_INPUT_ROWS);
        for slot in [Slot::Banner, Slot::Approval, Slot::Toasts, Slot::Leader] {
            assert_eq!(p.at(slot), None, "{slot:?} 应缺席");
        }
    }

    /// 输入栏高度只随 IME 态切换，且 IME 态更高。
    #[test]
    fn input_height_follows_ime() {
        assert_eq!(input_h(false), SHELL_INPUT_ROWS);
        assert_eq!(input_h(true), SHELL_INPUT_IME_ROWS);
        assert!(input_h(true) > input_h(false));
    }
}
