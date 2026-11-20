// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端能力探测的纯解析层（0.1.19 TUI 建设方案 §5.5 #1、§6.8 #3）。
//
// 三源分级：OSC11（背景 RGB）> DEC2031（`CSI ? 997 ; Ps n` 明暗偏好）>
// COLORFGBG（由 style::init_from_env 消费）。本模块只把终端回包还原为
// 语义结果，不做任何 IO、不读时钟、不碰主题状态——探测线程（term）负责
// 读写，主题裁决（style）负责落地，解析规则在此唯一实现。
//
// OSC11 归一化沿用 C 版 airy_cli（cli_term.c term_query_bg）：每段 1~4 位
// 十六进制，1 位按 `(v<<4)|v` 扩展到 8 位，超过 255 的按 `/257` 压回 8 位。

/// 终端背景明暗（OSC11 亮度或 DEC2031 偏好的语义化结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    Dark,
    Light,
}

/// 明暗结论的来源，供日志与优先级裁决使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Src {
    Osc11,
    Dec997,
}

/// 探测窗口内收集到的能力结论。
///
/// 三态口径：`None` = 终端未回包（超时或无应答），不得当作「不支持」；
/// 只有明确回执才落 `Some`。`truecolor` 来自环境（COLORTERM/TERM），
/// 没有回执概念，缺失即 `false`（回落 256 色，fail-safe）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caps {
    pub(crate) scheme: Option<(Scheme, Src)>,
    /// DECRQM 2026（同步输出）：`Some(true)` 已开启，`Some(false)` 支持但关闭。
    pub(crate) sync_output: Option<bool>,
    /// kitty 键盘协议：见到 `CSI ? <flags> u` 即为支持。
    pub(crate) kitty: Option<bool>,
    /// 真彩能力：COLORTERM 或 TERM 的 direct 标记，缺失按不支持。
    pub(crate) truecolor: bool,
}

impl Caps {
    /// 明暗结论的布尔投影；未探明时 `None`（调用方不得猜）。
    pub(crate) fn dark(&self) -> Option<bool> {
        self.scheme.map(|(s, _)| s == Scheme::Dark)
    }
}

const OSC11: &[u8] = b"\x1b]11;rgb:";
const DEC997: &[u8] = b"\x1b[?997;";
const DECRQ_SYNC: &[u8] = b"\x1b[?2026;";
const CSI_PRIVATE: &[u8] = b"\x1b[?";

/// 探测查询帧：一次写出全部四问，顺序固定便于录制比对。
///
/// OSC11（背景色）→ DEC2031（明暗）→ DECRQM 2026（同步输出）→ kitty 键盘
/// 协议查询。一条 `write` 完成，避免终端在两次写之间插入回显。
pub(crate) const QUERY: &str = "\x1b]11;?\x1b\\\x1b[?996n\x1b[?2026$p\x1b[?u";

/// 解析 OSC11 背景色应答，返回归一化后的 (r, g, b)。
///
/// 应答形如 `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` 后接 BEL 或 ST；段数不足时
/// 缺失通道补 0（部分终端省略尾段），任一段非十六进制即拒绝。
pub(crate) fn parse_osc11(buf: &[u8]) -> Option<(u8, u8, u8)> {
    let start = find(buf, OSC11)? + OSC11.len();
    let rest = &buf[start..];
    let end = rest
        .iter()
        .position(|&b| b == 0x07 || b == 0x1b)
        .unwrap_or(rest.len());
    let mut vals = [0u8; 3];
    let mut seen = 0;
    for (slot, part) in vals.iter_mut().zip(rest[..end].split(|&b| b == b'/')) {
        *slot = hex_chan(part)?;
        seen += 1;
    }
    (seen > 0).then_some((vals[0], vals[1], vals[2]))
}

/// 解析 DEC2031 明暗偏好应答：`CSI ? 997 ; 1 n` 为深色、`2` 为浅色。
pub(crate) fn parse_scheme(buf: &[u8]) -> Option<Scheme> {
    let mut at = 0;
    while let Some(pos) = find(&buf[at..], DEC997) {
        let i = at + pos + DEC997.len();
        if buf.get(i + 1) == Some(&b'n') {
            match buf.get(i) {
                Some(b'1') => return Some(Scheme::Dark),
                Some(b'2') => return Some(Scheme::Light),
                _ => {}
            }
        }
        at = i;
    }
    None
}

/// 背景 RGB 的明暗判定：亮度 ≥ 0.5（128/255）视为浅色。
pub(crate) fn classify((r, g, b): (u8, u8, u8)) -> Scheme {
    let lum = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
    if lum >= 128 {
        Scheme::Light
    } else {
        Scheme::Dark
    }
}

/// 汇总一个探测窗口的原始回包：OSC11 优先，其次 DEC2031。
pub(crate) fn caps(buf: &[u8]) -> Caps {
    Caps {
        scheme: scheme_of(buf),
        sync_output: parse_sync(buf),
        kitty: parse_kitty(buf),
        truecolor: env_truecolor(|k| std::env::var(k).ok()),
    }
}

fn scheme_of(buf: &[u8]) -> Option<(Scheme, Src)> {
    match parse_osc11(buf) {
        Some(rgb) => Some((classify(rgb), Src::Osc11)),
        None => parse_scheme(buf).map(|s| (s, Src::Dec997)),
    }
}

/// 解析 DECRQM 2026 回执 `CSI ? 2026 ; Ps $ y`。
///
/// Ps：0 未识别、1 已设置、2 已重置、3 永久设置、4 永久重置。支持与否按
/// 1~4 判定，开关状态按 1/3 为开；0 与畸形回包一律返回 None，不猜。
pub(crate) fn parse_sync(buf: &[u8]) -> Option<bool> {
    let mut at = 0;
    while let Some(pos) = find(&buf[at..], DECRQ_SYNC) {
        let head = at + pos + DECRQ_SYNC.len();
        if let Some(state) = sync_state(&buf[head..]) {
            return Some(state);
        }
        at = head;
    }
    None
}

fn sync_state(rest: &[u8]) -> Option<bool> {
    let end = rest.iter().position(|b| !b.is_ascii_digit())?;
    if rest.get(end) != Some(&b'$') || rest.get(end + 1) != Some(&b'y') {
        return None;
    }
    let ps: u32 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    match ps {
        1 | 3 => Some(true),
        2 | 4 => Some(false),
        _ => None,
    }
}

/// 解析 kitty 键盘协议回执：`CSI ? <flags> u` 出现即为支持。
///
/// 只认带 `?` 私有前缀且以 `u` 结尾的形式；其余 CSI 序列不参与判定。
pub(crate) fn parse_kitty(buf: &[u8]) -> Option<bool> {
    let mut at = 0;
    while let Some(pos) = find(&buf[at..], CSI_PRIVATE) {
        let head = at + pos + CSI_PRIVATE.len();
        let rest = &buf[head..];
        let Some(end) = rest.iter().position(|b| !b.is_ascii_digit()) else {
            break;
        };
        if end > 0 && rest.get(end) == Some(&b'u') {
            return Some(true);
        }
        at = head;
    }
    None
}

/// 真彩判定：COLORTERM 含 truecolor/24bit，或 TERM 含 direct。
pub(crate) fn env_truecolor(var: impl Fn(&str) -> Option<String>) -> bool {
    match var("COLORTERM") {
        Some(val) => has_truecolor(&val),
        None => var("TERM").is_some_and(|val| has_direct(&val)),
    }
}

fn has_truecolor(val: &str) -> bool {
    let low = val.to_ascii_lowercase();
    low.contains("truecolor") || low.contains("24bit")
}

fn has_direct(val: &str) -> bool {
    val.to_ascii_lowercase().contains("direct")
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 单通道归一化：1 位扩展为重复半字节，4 位按 16 位值压缩到 8 位。
fn hex_chan(part: &[u8]) -> Option<u8> {
    if part.is_empty() || part.len() > 4 {
        return None;
    }
    let mut v: u32 = 0;
    for &b in part {
        v = (v << 4) | (b as char).to_digit(16)?;
    }
    Some(match part.len() {
        1 => ((v << 4) | v) as u8,
        _ if v > 255 => (v / 257) as u8,
        _ => v as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc11_four_digit_channels() {
        let raw = b"\x1b]11;rgb:1a1a/2626/3333\x07";
        assert_eq!(parse_osc11(raw), Some((26, 38, 51)));
    }

    #[test]
    fn osc11_single_digit_expands_nibble() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/f/f\x07"), Some((255, 255, 255)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0/8/0\x07"), Some((0, 136, 0)));
    }

    #[test]
    fn osc11_st_terminated() {
        let raw = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(parse_osc11(raw), Some((255, 255, 255)));
    }

    #[test]
    fn osc11_missing_segments_default_to_zero() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:2020\x07"), Some((32, 0, 0)));
    }

    #[test]
    fn osc11_rejects_malformed() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:xyz\x07"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:12345/0/0\x07"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:\x07"), None);
        assert_eq!(parse_osc11(b"no answer"), None);
    }

    #[test]
    fn dec997_scheme_replies() {
        assert_eq!(parse_scheme(b"\x1b[?997;1n"), Some(Scheme::Dark));
        assert_eq!(parse_scheme(b"\x1b[?997;2n"), Some(Scheme::Light));
        assert_eq!(parse_scheme(b"\x1b[?997;3n"), None);
        assert_eq!(parse_scheme(b"\x1b[?997;1"), None);
        assert_eq!(parse_scheme(b"\x1b[?997;"), None);
    }

    #[test]
    fn classify_boundary_is_half_luma() {
        assert_eq!(classify((128, 128, 128)), Scheme::Light);
        assert_eq!(classify((127, 127, 127)), Scheme::Dark);
        assert_eq!(classify((250, 249, 245)), Scheme::Light);
        assert_eq!(classify((20, 20, 19)), Scheme::Dark);
    }

    #[test]
    fn osc11_outranks_dec997() {
        let raw = b"\x1b[?997;2n\x1b]11;rgb:1414/1414/1313\x07";
        let got = caps(raw);
        assert_eq!(got.scheme, Some((Scheme::Dark, Src::Osc11)));
    }

    #[test]
    fn dec997_is_used_without_osc11() {
        let got = caps(b"\x1b[?997;2n");
        assert_eq!(got.scheme, Some((Scheme::Light, Src::Dec997)));
        assert_eq!(caps(b"noise").scheme, None);
    }

    #[test]
    fn decrqm_2026_states() {
        assert_eq!(parse_sync(b"\x1b[?2026;1$y"), Some(true));
        assert_eq!(parse_sync(b"\x1b[?2026;2$y"), Some(false));
        assert_eq!(parse_sync(b"\x1b[?2026;3$y"), Some(true));
        assert_eq!(parse_sync(b"\x1b[?2026;4$y"), Some(false));
        assert_eq!(parse_sync(b"\x1b[?2026;0$y"), None);
        assert_eq!(parse_sync(b"\x1b[?2026;1n"), None);
        assert_eq!(parse_sync(b"\x1b[?2026;"), None);
        assert_eq!(parse_sync(b"noise"), None);
    }

    #[test]
    fn decrqm_skips_malformed_occurrence() {
        assert_eq!(parse_sync(b"\x1b[?2026;x\x1b[?2026;2$y"), Some(false));
    }

    #[test]
    fn kitty_flags_reply_marks_support() {
        assert_eq!(parse_kitty(b"\x1b[?1u"), Some(true));
        assert_eq!(parse_kitty(b"\x1b[?0u"), Some(true));
        assert_eq!(parse_kitty(b"\x1b[?1n"), None);
        assert_eq!(parse_kitty(b"\x1b[?u"), None);
        assert_eq!(parse_kitty(b"noise"), None);
    }

    #[test]
    fn truecolor_follows_env_evidence() {
        assert!(env_truecolor(|_| Some("truecolor".to_string())));
        assert!(env_truecolor(|k| (k == "COLORTERM").then(|| "24BIT".into())));
        assert!(!env_truecolor(|k| (k == "COLORTERM").then(|| "yes".into())));
        assert!(env_truecolor(|k| (k == "TERM").then(|| "xterm-direct".into())));
        assert!(!env_truecolor(|k| (k == "TERM").then(|| "xterm-256color".into())));
        assert!(!env_truecolor(|_| None));
    }

    #[test]
    fn caps_merges_every_evidence_source() {
        let raw = b"\x1b[?997;1n\x1b[?2026;2$y\x1b[?1u\x1b]11;rgb:1414/1414/1313\x07";
        let got = caps(raw);
        assert_eq!(got.scheme, Some((Scheme::Dark, Src::Osc11)));
        assert_eq!(got.sync_output, Some(false));
        assert_eq!(got.kitty, Some(true));
        assert_eq!(got.dark(), Some(true));
        assert_eq!(Caps::default().dark(), None, "未探明时不得猜测明暗");
    }

    #[test]
    fn query_frame_asks_every_probe_in_order() {
        let parts = ["\x1b]11;?", "\x1b[?996n", "\x1b[?2026$p", "\x1b[?u"];
        let at: Vec<usize> = parts
            .iter()
            .map(|p| QUERY.find(p).expect("查询帧必须含全部四问"))
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "查询顺序必须固定");
        assert_eq!(QUERY.lines().count(), 1, "查询帧不得含换行");
    }

    #[test]
    fn single_reply_frame_yields_all_caps() {
        let reply = b"\x1b]11;rgb:0000/0000/0000\x07\x1b[?997;1n\x1b[?2026;1$y\x1b[?1u";
        let got = caps(reply);
        assert_eq!(got.scheme, Some((Scheme::Dark, Src::Osc11)));
        assert_eq!(got.sync_output, Some(true));
        assert_eq!(got.kitty, Some(true));
    }
}
