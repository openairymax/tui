// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端能力探测的纯解析层（0.1.19 TUI 建设方案 §5.5 #1、§6.8 #3）。
//
// 三源分级：OSC11（背景 RGB）> DEC2031（`CSI ? 997 ; Ps n` 明暗偏好）>
// COLORFGBG（由 style::init_from_env 消费）。本模块只把终端回包还原为
// 语义结果，解析不做任何 IO、不读时钟、不碰主题状态——探测线程（term）
// 负责读写，主题裁决（style）负责落地，解析规则在此唯一实现。查询帧的
// 编码与写出同在此处，使「问什么、怎么写」只有一个事实来源。
//
// OSC11 归一化沿用 C 版 airy_cli（cli_term.c term_query_bg）：每段 1~4 位
// 十六进制，1 位按 `(v<<4)|v` 扩展到 8 位，超过 255 的按 `/257` 压回 8 位。

use std::io::{self, Write};

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
    /// 环境覆盖（AIRY_TUI_CAPS_DARK），优先级最高但只在显式设置时出现。
    Env,
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
    /// 真彩能力：COLORTERM/TERM 的标记，或 OSC11 的 24 位通道回包；
    /// 缺失按不支持（回落 256 色，fail-safe）。
    pub(crate) truecolor: bool,
}

impl Caps {
    /// 明暗结论的布尔投影；未探明时 `None`（调用方不得猜）。
    ///
    /// 生产消费方（entry）直接匹配 `scheme` 以取得来源，投影仅供判定口径
    /// 的单测断言，避免测试自行解构元组。
    #[cfg(test)]
    pub(crate) fn dark(&self) -> Option<bool> {
        self.scheme.map(|(s, _)| s == Scheme::Dark)
    }

    /// 应用 `AIRY_TUI_CAPS_*` 覆盖：SYNC_OUTPUT / TRUECOLOR / KITTY / DARK。
    ///
    /// 取值 1/0、true/false、on/off（大小写不敏感）；缺失或非法时保留探测
    /// 结论，不把「写错环境变量」降级成「能力缺失」。
    pub(crate) fn override_env(mut self, var: impl Fn(&str) -> Option<String>) -> Self {
        self.sync_output = flag_of(var("AIRY_TUI_CAPS_SYNC_OUTPUT")).or(self.sync_output);
        if let Some(on) = flag_of(var("AIRY_TUI_CAPS_TRUECOLOR")) {
            self.truecolor = on;
        }
        self.kitty = flag_of(var("AIRY_TUI_CAPS_KITTY")).or(self.kitty);
        if let Some(scheme) = theme_of(var("AIRY_TUI_CAPS_DARK")) {
            self.scheme = Some((scheme, Src::Env));
        }
        self
    }
}

/// 布尔环境值解析：无法识别返回 None。
fn flag_of(val: Option<String>) -> Option<bool> {
    let low = val?.trim().to_ascii_lowercase();
    match low.as_str() {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// 明暗环境值解析：除布尔取值外还接受 dark/light 直书语义。
///
/// DARK 的取值域比纯开关宽——写 `light` 的人表达的是明暗语义而非布尔
/// 取反，两者都须识别；其余取值同样保留探测结论。
fn theme_of(val: Option<String>) -> Option<Scheme> {
    let low = val?.trim().to_ascii_lowercase();
    match low.as_str() {
        "1" | "true" | "on" | "dark" => Some(Scheme::Dark),
        "0" | "false" | "off" | "light" => Some(Scheme::Light),
        _ => None,
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

/// 写出查询帧：整帧一次写 + 冲刷，失败时整帧重试一次。
///
/// 探测是启动期的尽力而为动作：任何失败只意味着「未探明」，由调用方
/// 降级为默认结论，绝不阻断启动。重试一次覆盖瞬时错误（EINTR、管道
/// 背压）；整帧重发是安全的——解析侧对重复回包取首个有效值。
pub(crate) fn send_query(mut out: impl Write) -> io::Result<()> {
    let frame = QUERY.as_bytes();
    match out.write_all(frame).and_then(|_| out.flush()) {
        Ok(()) => Ok(()),
        Err(_) => out.write_all(frame).and_then(|_| out.flush()),
    }
}

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
    let mut merged = merge(buf);
    merged.truecolor = truecolor_ev(buf, |k| std::env::var(k).ok());
    merged.override_env(|k| std::env::var(k).ok())
}

/// 把三类回包合并为一份结论，环境证据在合并之后单独叠加。
///
/// 合并只读回包字节，不碰进程环境：使「终端说了什么」可独立断言，
/// 不被构建机上的 COLORTERM 或 `AIRY_TUI_CAPS_*` 干扰。真彩的 OSC11
/// 证据同属回包，故在合并内落定。
fn merge(buf: &[u8]) -> Caps {
    Caps {
        scheme: scheme_of(buf),
        sync_output: parse_sync(buf),
        kitty: parse_kitty(buf),
        truecolor: parse_osc11(buf).is_some(),
    }
}

/// 真彩判定：环境证据与 OSC11 回包任一成立即为真。
///
/// OSC11 应答形如 `rgb:RRRR/GGGG/BBBB`：终端能按 24 位通道回话，即表明
/// 其颜色解析走 RGB 三通道而非调色板索引。两条证据都不存在时按 256 色
/// 保守处理——缺失即不支持，宁可降色也不发出错位的 SGR。
fn truecolor_ev(buf: &[u8], var: impl Fn(&str) -> Option<String>) -> bool {
    parse_osc11(buf).is_some() || env_truecolor(var)
}

fn scheme_of(buf: &[u8]) -> Option<(Scheme, Src)> {
    match parse_osc11(buf) {
        Some(rgb) => Some((classify(rgb), Src::Osc11)),
        None => parse_scheme(buf).map(|s| (s, Src::Dec997)),
    }
}

/// DECRQM 2026 的三分支裁决：未识别 / 已设置 / 已重置。
///
/// 「未识别」是终端给出的明确否定回执，与「尚无回包」是两件事：前者可
/// 断言不支持，后者只能记为未知。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sync {
    Unsupported,
    On,
    Off,
}

impl Sync {
    fn enabled(self) -> bool {
        self == Sync::On
    }
}

/// 解析 DECRQM 2026 回执 `CSI ? 2026 ; Ps $ y`。
///
/// Ps：0 未识别（不支持）、1 已设置、2 已重置、3 永久设置、4 永久重置。
/// 前三者构成三分支：0 与 2/4 均落「不可用」，仅 1/3 为开。畸形回包
/// 继续向后扫描；整段无有效回执时返回 None——未知不得当作不支持。
pub(crate) fn parse_sync(buf: &[u8]) -> Option<bool> {
    let mut at = 0;
    while let Some(pos) = find(&buf[at..], DECRQ_SYNC) {
        let head = at + pos + DECRQ_SYNC.len();
        if let Some(state) = sync_state(&buf[head..]) {
            return Some(state.enabled());
        }
        at = head;
    }
    None
}

fn sync_state(rest: &[u8]) -> Option<Sync> {
    let end = rest.iter().position(|b| !b.is_ascii_digit())?;
    if rest.get(end) != Some(&b'$') || rest.get(end + 1) != Some(&b'y') {
        return None;
    }
    let ps: u32 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    match ps {
        0 => Some(Sync::Unsupported),
        1 | 3 => Some(Sync::On),
        2 | 4 => Some(Sync::Off),
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
        assert_eq!(parse_sync(b"\x1b[?2026;0$y"), Some(false), "未识别即不支持");
        assert_eq!(parse_sync(b"\x1b[?2026;1n"), None);
        assert_eq!(parse_sync(b"\x1b[?2026;"), None);
        assert_eq!(parse_sync(b"noise"), None);
    }

    /// 三分支状态机：未识别 / 已设置 / 已重置各归其位。
    #[test]
    fn decrqm_2026_branch_thirds() {
        assert_eq!(sync_state(b"0$y"), Some(Sync::Unsupported));
        assert_eq!(sync_state(b"1$y"), Some(Sync::On));
        assert_eq!(sync_state(b"2$y"), Some(Sync::Off));
        assert_eq!(sync_state(b"9$y"), None, "未定义取值不得归类");
    }

    #[test]
    fn decrqm_skips_malformed_occurrence() {
        assert_eq!(parse_sync(b"\x1b[?2026;x\x1b[?2026;2$y"), Some(false));
    }

    /// 四分支表驱动：不支持 / 支持且开 / 支持但关 / 窗口结束无应答。
    ///
    /// 无应答必须停在「未知」，不得被压成「不支持」——两者对上层是不同
    /// 的事实，混同会让健康面板谎报终端的真实能力。
    #[test]
    fn decrqm_2026_four_branch_table() {
        let cases: [(&[u8], Option<bool>); 4] = [
            (b"\x1b[?2026;0$y", Some(false)),
            (b"\x1b[?2026;1$y", Some(true)),
            (b"\x1b[?2026;2$y", Some(false)),
            (b"", None),
        ];
        for (raw, want) in cases {
            assert_eq!(parse_sync(raw), want, "回包 {raw:?}");
        }
    }

    /// 整个探测窗口无任何回包时，逐字段停在未知（真彩除外，它来自环境）。
    ///
    /// 这里直接喂空窗口给三个解析器而不经 `caps`，避免把宿主环境里的
    /// 覆盖变量误当成被测行为。
    #[test]
    fn silent_window_keeps_probe_fields_unknown() {
        let empty: &[u8] = b"";
        assert_eq!(scheme_of(empty), None, "明暗不得凭默认猜测");
        assert_eq!(parse_sync(empty), None, "同步输出不得当作不支持");
        assert_eq!(parse_kitty(empty), None, "键盘协议不得当作不支持");
        let unknown = Caps {
            scheme: None,
            sync_output: None,
            kitty: None,
            truecolor: false,
        };
        assert_eq!(unknown.dark(), None, "布尔投影须保持未知");
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

    /// 真彩证据二者其一即可：OSC11 的 24 位通道回包同样成立。
    #[test]
    fn osc11_reply_counts_as_truecolor_evidence() {
        let light = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        assert!(truecolor_ev(light, |_| None), "OSC11 回包即证据");
        assert!(!truecolor_ev(b"", |_| None), "无证据按 256 色");
        assert!(truecolor_ev(b"", |k| (k == "COLORTERM").then(|| "truecolor".into())));
    }

    /// 真彩与 kitty 两问必须同帧发出：缺任一都得不到完整结论。
    #[test]
    fn truecolor_and_kitty_probes_go_out_together() {
        let mut sink = Sink::default();
        assert!(send_query(&mut sink).is_ok());
        let sent = &sink.writes[0];
        assert!(sent.windows(6).any(|w| w == b"\x1b]11;?"), "OSC11 问");
        assert!(sent.windows(4).any(|w| w == b"\x1b[?u"), "kitty 问");
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

    /// dark 是背景色的布尔投影：亮底为浅色，暗底为深色。
    #[test]
    fn background_colour_derives_dark_flag() {
        let light = merge(b"\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert_eq!(light.dark(), Some(false), "亮底应为浅色");
        let dark = merge(b"\x1b]11;rgb:0000/0000/0000\x07");
        assert_eq!(dark.dark(), Some(true), "暗底应为深色");
    }

    /// 同帧三项回包合并为一份结论：各字段独立落位，互不覆盖。
    #[test]
    fn three_probes_merge_into_one_caps() {
        let raw = b"\x1b]11;rgb:1414/1414/1313\x07\x1b[?997;2n\x1b[?2026;1$y\x1b[?1u";
        let got = merge(raw);
        assert_eq!(got.scheme, Some((Scheme::Dark, Src::Osc11)), "OSC11 优先");
        assert_eq!(got.sync_output, Some(true));
        assert_eq!(got.kitty, Some(true));
        assert!(got.truecolor, "OSC11 回包即真彩证据");
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

    /// 可控写端：`fail` 为剩余失败次数，`writes` 记录每次成功的整段写入。
    #[derive(Default)]
    struct Sink {
        fail: usize,
        writes: Vec<Vec<u8>>,
        flushes: usize,
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail > 0 {
                self.fail -= 1;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            self.writes.push(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    /// 首写成功：逐字节等于查询帧，且以一次冲刷收尾。
    #[test]
    fn send_query_writes_exact_frame() {
        let mut sink = Sink::default();
        assert!(send_query(&mut sink).is_ok());
        assert_eq!(sink.writes, vec![QUERY.as_bytes().to_vec()]);
        assert_eq!(sink.flushes, 1);
    }

    /// 首写失败：整帧重试一次并成功，重试内容与首帧逐字节一致。
    #[test]
    fn send_query_retries_once_after_error() {
        let mut sink = Sink {
            fail: 1,
            ..Sink::default()
        };
        assert!(send_query(&mut sink).is_ok());
        assert_eq!(sink.writes.len(), 1, "失败一次后仅成功一次");
        assert_eq!(sink.writes[0], QUERY.as_bytes());
        assert_eq!(sink.flushes, 1);
    }

    /// 持续失败：只重试一次即上抛，由调用方降级为默认能力。
    #[test]
    fn send_query_gives_up_after_one_retry() {
        let mut sink = Sink {
            fail: 2,
            ..Sink::default()
        };
        assert!(send_query(&mut sink).is_err());
        assert!(sink.writes.is_empty());
        assert_eq!(sink.flushes, 0);
    }

    #[test]
    fn single_reply_frame_yields_all_caps() {
        let reply = b"\x1b]11;rgb:0000/0000/0000\x07\x1b[?997;1n\x1b[?2026;1$y\x1b[?1u";
        let got = caps(reply);
        assert_eq!(got.scheme, Some((Scheme::Dark, Src::Osc11)));
        assert_eq!(got.sync_output, Some(true));
        assert_eq!(got.kitty, Some(true));
    }

    #[test]
    fn default_caps_keep_every_field_unknown() {
        let got = Caps::default();
        assert_eq!(got.scheme, None);
        assert_eq!(got.sync_output, None);
        assert_eq!(got.kitty, None);
        assert!(!got.truecolor);
        assert_eq!(got.dark(), None);
    }

    #[test]
    fn env_override_wins_over_probe() {
        let vars = |k: &str| {
            Some(
                match k {
                    "AIRY_TUI_CAPS_SYNC_OUTPUT" => "1",
                    "AIRY_TUI_CAPS_TRUECOLOR" => "ON",
                    "AIRY_TUI_CAPS_KITTY" => "0",
                    "AIRY_TUI_CAPS_DARK" => "light",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let base = Caps {
            scheme: Some((Scheme::Dark, Src::Osc11)),
            sync_output: Some(false),
            kitty: Some(true),
            truecolor: false,
        };
        let got = base.override_env(vars);
        assert_eq!(got.sync_output, Some(true), "显式覆盖必须压过探测结论");
        assert_eq!(got.kitty, Some(false));
        assert!(got.truecolor);
        assert_eq!(got.scheme, Some((Scheme::Light, Src::Env)));
        assert_eq!(got.dark(), Some(false));
    }

    #[test]
    fn env_override_is_partial_and_lenient() {
        let vars = |k: &str| (k == "AIRY_TUI_CAPS_KITTY").then(|| "yes".to_string());
        let base = Caps {
            sync_output: Some(true),
            ..Caps::default()
        };
        let got = base.override_env(vars);
        assert_eq!(got.sync_output, Some(true), "缺失的覆盖项保持原值");
        assert_eq!(got.kitty, None, "非法取值不得当作关闭");
        assert_eq!(got.scheme, None);
    }
}
