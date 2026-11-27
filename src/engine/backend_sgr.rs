// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-08a/b（§23.5.2 序 87/88）：前景/背景与修饰位的 SGR 参数编码。
//
// 纯函数层：只回答「通道颜色与修饰位的差量该拼成哪些 SGR 参数」，不持有
// 任何状态、不写字节。游程判定与已生效样式缓存归帧编码器（backend），
// 本文件因此可以被穷举用例覆盖——修饰位的复位语义（22 同时清粗体与
// 半亮、25 同时清慢闪与快闪）是终端语义里最容易写错的一处，必须有单点。
//
// 参数顺序固定为「前景区 → 背景区 → 修饰位」，同一 SGR 内合并发出，
// 为 X2-09 的最短化提供唯一拼装点。

use ratatui::style::{Color, Modifier};

/// 修饰位映射表：`(位, 开启参数, 关闭参数)`。
///
/// 粗体与半亮共用复位参数 22、两种闪烁共用 25，故关闭时需按同簇语义
/// 重新点亮仍应保留的位（见 `bold_dim`）。
const MOD_SGR: &[(Modifier, &str, &str)] = &[
    (Modifier::BOLD, "1", "22"),
    (Modifier::DIM, "2", "22"),
    (Modifier::ITALIC, "3", "23"),
    (Modifier::UNDERLINED, "4", "24"),
    (Modifier::SLOW_BLINK, "5", "25"),
    (Modifier::RAPID_BLINK, "6", "25"),
    (Modifier::REVERSED, "7", "27"),
    (Modifier::HIDDEN, "8", "28"),
    (Modifier::CROSSED_OUT, "9", "29"),
];

/// 单通道颜色到 SGR 参数；`Reset` 返回 `None`，由调用方回落 39/49。
pub(crate) fn color_param(color: Color, fg: bool) -> Option<String> {
    let base: u8 = if fg { 30 } else { 40 };
    let bright: u8 = if fg { 90 } else { 100 };
    let (kind, n): (u8, u8) = match color {
        Color::Reset => return None,
        Color::Black => (base, 0),
        Color::Red => (base, 1),
        Color::Green => (base, 2),
        Color::Yellow => (base, 3),
        Color::Blue => (base, 4),
        Color::Magenta => (base, 5),
        Color::Cyan => (base, 6),
        Color::Gray => (base, 7),
        Color::DarkGray => (bright, 0),
        Color::LightRed => (bright, 1),
        Color::LightGreen => (bright, 2),
        Color::LightYellow => (bright, 3),
        Color::LightBlue => (bright, 4),
        Color::LightMagenta => (bright, 5),
        Color::LightCyan => (bright, 6),
        Color::White => (bright, 7),
        Color::Indexed(i) => {
            return Some(format!("{};5;{}", if fg { 38 } else { 48 }, i));
        }
        Color::Rgb(r, g, b) => {
            return Some(format!("{};2;{};{};{}", if fg { 38 } else { 48 }, r, g, b));
        }
    };
    Some((kind + n).to_string())
}

/// 把 `from → to` 的修饰位差量追加为 SGR 参数。
pub(crate) fn mod_params(from: Modifier, to: Modifier, out: &mut Vec<String>) {
    bold_dim(from, to, out);
    for &(bit, on, off) in &MOD_SGR[2..] {
        let (was, now) = (from.contains(bit), to.contains(bit));
        if now && !was {
            push_once(out, on);
        } else if was && !now {
            push_once(out, off);
        }
    }
}

/// 粗体/半亮同簇：复位参数 22 会同时清掉两位，需按目标重新点亮保留位。
fn bold_dim(from: Modifier, to: Modifier, out: &mut Vec<String>) {
    let (fb, fd) = (
        from.contains(Modifier::BOLD),
        from.contains(Modifier::DIM),
    );
    let (tb, td) = (to.contains(Modifier::BOLD), to.contains(Modifier::DIM));
    if (fb, fd) == (tb, td) {
        return;
    }
    if !tb && !td {
        push_once(out, "22");
        return;
    }
    // 目标仍有位：先复位居中，再按目标重发（同簇内换位必走此路）。
    push_once(out, "22");
    if tb {
        out.push("1".to_string());
    }
    if td {
        out.push("2".to_string());
    }
}

/// 追加参数并去重（同簇关闭参数可能被多个位命中）。
fn push_once(out: &mut Vec<String>, param: &str) {
    if !out.iter().any(|p| p == param) {
        out.push(param.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_fg_and_bg() {
        assert_eq!(color_param(Color::Red, true).as_deref(), Some("31"));
        assert_eq!(color_param(Color::LightBlue, false).as_deref(), Some("104"));
        assert_eq!(color_param(Color::Reset, true), None);
    }

    #[test]
    fn indexed_and_rgb() {
        assert_eq!(color_param(Color::Indexed(9), true).as_deref(), Some("38;5;9"));
        assert_eq!(
            color_param(Color::Rgb(1, 2, 3), false).as_deref(),
            Some("48;2;1;2;3")
        );
    }

    #[test]
    fn modifier_on_off() {
        let mut p = Vec::new();
        mod_params(Modifier::empty(), Modifier::BOLD | Modifier::UNDERLINED, &mut p);
        assert_eq!(p, ["1", "4"]);
        p.clear();
        mod_params(Modifier::BOLD | Modifier::UNDERLINED, Modifier::empty(), &mut p);
        assert_eq!(p, ["22", "24"]);
    }

    #[test]
    fn bold_dim_cluster_repairs_survivor() {
        let mut p = Vec::new();
        mod_params(Modifier::BOLD | Modifier::DIM, Modifier::DIM, &mut p);
        assert_eq!(p, ["22", "2"], "关闭粗体不得连坐半亮");
        p.clear();
        mod_params(Modifier::DIM, Modifier::BOLD, &mut p);
        assert_eq!(p, ["22", "1"]);
    }

    #[test]
    fn italic_and_blink() {
        let mut p = Vec::new();
        mod_params(Modifier::ITALIC, Modifier::SLOW_BLINK, &mut p);
        assert_eq!(p, ["23", "5"]);
        p.clear();
        mod_params(Modifier::empty(), Modifier::SLOW_BLINK | Modifier::RAPID_BLINK, &mut p);
        assert_eq!(p, ["5", "6"]);
        p.clear();
        mod_params(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK, Modifier::empty(), &mut p);
        assert_eq!(p, ["25"], "同簇关闭参数只发一次");
    }
}
