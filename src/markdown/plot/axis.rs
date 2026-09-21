// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// plot 坐标轴刻度层（0.1.18 W7）：自适应"漂亮"刻度与紧凑数值标注。
//
// 刻度步长取自 1/2/5×10ⁿ 序列——这是量程标注的可读性下界：任意步长会出现
// 3.333…、7.142… 这类无法口算的标注，使坐标轴失去意义。步长随量程自适应，
// 不依赖调用方传入。

/// 量程 `range` 在 `target` 个刻度下的"漂亮"步长（1/2/5×10ⁿ）。
///
/// `range` 非正或 `target` 为 0 时返回 1.0——调用方随后仍会因量程退化而
/// 放弃刻度，此处只需保证返回有限正值。
pub(super) fn nice_step(range: f64, target: usize) -> f64 {
    if !range.is_finite() || range <= 0.0 || target == 0 {
        return 1.0;
    }
    let raw = range / target as f64;
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(raw.log10().floor());
    if !mag.is_finite() || mag <= 0.0 {
        return 1.0;
    }
    let norm = raw / mag;
    let mult = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    mult * mag
}

/// 在 `[min, max]` 内生成刻度值（含端点附近的整步长点）。
///
/// 输出条数上界 `target * 4`：浮点累积可能让循环多走一两步，但绝不失控。
pub(super) fn ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() || max <= min || target == 0 {
        return Vec::new();
    }
    let step = nice_step(max - min, target);
    let start = (min / step).ceil() * step;
    let mut out: Vec<f64> = Vec::new();
    let mut v = start;
    let limit = target.saturating_mul(4).max(1);
    while v <= max + step * 1e-9 && out.len() < limit {
        out.push(v);
        v += step;
    }
    out
}

/// 紧凑数值标注：常规量级最多 3 位小数并去尾零，极端量级走科学计数。
pub(super) fn fmt(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-3..1e5).contains(&a) {
        return format!("{v:.1e}");
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_step_follows_one_two_five() {
        assert_eq!(nice_step(10.0, 5), 2.0);
        assert_eq!(nice_step(1.0, 5), 0.2);
        // 100/4 = 25 → 归一化 2.5 落在 (2,5] → 取 5 → 50
        assert_eq!(nice_step(100.0, 4), 50.0);
        assert_eq!(nice_step(3.0, 3), 1.0);
    }

    #[test]
    fn nice_step_is_finite_on_degenerate_input() {
        assert_eq!(nice_step(0.0, 5), 1.0);
        assert_eq!(nice_step(-1.0, 5), 1.0);
        assert_eq!(nice_step(10.0, 0), 1.0);
        assert_eq!(nice_step(f64::NAN, 5), 1.0);
    }

    #[test]
    fn ticks_stay_inside_range() {
        for (lo, hi, n) in [(0.0, 6.0, 6), (-1.0, 1.0, 4), (-3.7, 8.2, 5), (0.0, 0.05, 4)] {
            let t = ticks(lo, hi, n);
            assert!(!t.is_empty(), "量程 [{lo},{hi}] 应产出刻度");
            for v in &t {
                assert!(*v >= lo - 1e-9 && *v <= hi + 1e-9, "刻度越界: {v} in [{lo},{hi}]");
            }
            assert!(t.len() <= n * 4);
        }
    }

    #[test]
    fn ticks_reject_degenerate_range() {
        assert!(ticks(1.0, 1.0, 4).is_empty());
        assert!(ticks(5.0, 1.0, 4).is_empty());
        assert!(ticks(f64::NAN, 1.0, 4).is_empty());
    }

    #[test]
    fn ticks_are_evenly_spaced() {
        let t = ticks(0.0, 10.0, 5);
        assert!(t.len() >= 3);
        let d: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).collect();
        for x in &d {
            assert!((x - d[0]).abs() < 1e-9, "刻度不等距: {d:?}");
        }
    }

    #[test]
    fn fmt_is_compact() {
        assert_eq!(fmt(0.0), "0");
        assert_eq!(fmt(1.0), "1");
        assert_eq!(fmt(-0.5), "-0.5");
        assert_eq!(fmt(2.500), "2.5");
        assert_eq!(fmt(1.0 / 3.0), "0.333");
        assert_eq!(fmt(-0.0), "0");
        assert_eq!(fmt(f64::NAN), "—");
    }

    #[test]
    fn fmt_uses_scientific_for_extremes() {
        assert!(fmt(1.0e6).contains('e'));
        assert!(fmt(1.0e-5).contains('e'));
        assert_eq!(fmt(0.0), "0");
    }
}
