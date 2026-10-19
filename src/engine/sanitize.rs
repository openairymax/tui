// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端控制序列净化（0.1.19 TUI 建设方案 §5.5 #8）。
//
// 模型输出与用户粘贴在落屏前必须经过本模块：OSC / DCS / PM / APC / SOS
// 字符串序列整段吞掉，CSI 与其他 ESC 序列同样吞掉；C0/C1 控制字符除
// 换行与制表外一律丢弃。不做净化时，模型只需输出一段 OSC 就能改写窗口
// 标题、写系统剪贴板，甚至借终端仿真器漏洞逃逸出本进程——这是把不可信
// 文本直通 TTY 的结构性风险，不是显示瑕疵。
//
// 实现是纯状态机：L1 层，无 IO、无时钟、无锁，跨增量块保持状态，保证
// 流式路径与落定路径得到同一净化结果。序列长度受 budget 硬界约束，
// 未终止序列超限即中止剥离并回到地面态——宁可漏掉后续控制字节，也不
// 能把用户正文整段吞掉。

use crate::budget::{MAX_CTRL_CSI_CHARS, MAX_CTRL_STR_CHARS};

/// 字符串序列类别：OSC 额外接受 BEL 终止，其余仅接受 ST。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Osc,
    Other,
}

/// 状态机相位（ECMA-48 子集）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// 地面态：普通文本原样放行。
    #[default]
    Ground,
    /// 刚收到 ESC，等待引导字节。
    Esc,
    /// ESC 后中间字节段（0x20..=0x2F）。
    Interm,
    /// CSI 控制序列（参数 0x20..=0x3F，终字节 0x40..=0x7E）。
    Csi,
    /// 字符串序列主体。
    Str(Kind),
    /// 字符串序列内收到 ESC，等待 `\` 完成 ST。
    StrEsc(Kind),
}

/// 流式净化器：跨块保持相位，输入文本只增不改。
#[derive(Debug, Default)]
pub(crate) struct Stripper {
    state: State,
    /// 当前序列已消费的参数字符数，用于未终止序列的硬界中止。
    n: usize,
}

impl Stripper {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// 消费一段文本，返回可直接上屏的净化结果。悬挂序列的相位保留到
    /// 后续块，块边界与字符边界一致（`&str` 保证合法 UTF-8）。
    pub(crate) fn feed(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            self.step(ch, &mut out);
        }
        out
    }

    fn step(&mut self, ch: char, out: &mut String) {
        match self.state {
            State::Ground => self.ground(ch, out),
            State::Esc => self.esc(ch, out),
            State::Interm => self.interm(ch, out),
            State::Csi => self.csi(ch, out),
            State::Str(kind) => self.str_seq(ch, out, kind),
            State::StrEsc(kind) => self.str_esc(ch, out, kind),
        }
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.n = 0;
    }

    fn ground(&mut self, ch: char, out: &mut String) {
        match ch {
            '\x1b' => self.enter(State::Esc),
            '\u{9b}' => self.enter(State::Csi),
            '\u{9d}' => self.enter(State::Str(Kind::Osc)),
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => self.enter(State::Str(Kind::Other)),
            '\u{9c}' => {}
            _ if ch.is_control() && !matches!(ch, '\n' | '\t') => {}
            _ => out.push(ch),
        }
    }

    fn esc(&mut self, ch: char, out: &mut String) {
        match ch {
            '[' => self.enter(State::Csi),
            ']' => self.enter(State::Str(Kind::Osc)),
            'P' | 'X' | '^' | '_' => self.enter(State::Str(Kind::Other)),
            '\x1b' => self.n = 0,
            '\x20'..='\x2f' => self.enter(State::Interm),
            '\n' | '\t' => {
                out.push(ch);
                self.enter(State::Ground);
            }
            _ => self.enter(State::Ground),
        }
    }

    fn interm(&mut self, ch: char, out: &mut String) {
        match ch {
            '\x20'..='\x2f' => {
                self.n += 1;
                if self.n > MAX_CTRL_CSI_CHARS {
                    self.enter(State::Ground);
                }
            }
            '\x1b' => self.enter(State::Esc),
            '\x30'..='\x7e' => self.enter(State::Ground),
            '\n' | '\t' => {
                out.push(ch);
                self.enter(State::Ground);
            }
            _ => self.enter(State::Ground),
        }
    }

    fn csi(&mut self, ch: char, out: &mut String) {
        match ch {
            '\x40'..='\x7e' => self.enter(State::Ground),
            '\x1b' => self.enter(State::Esc),
            '\x20'..='\x3f' => {
                self.n += 1;
                if self.n > MAX_CTRL_CSI_CHARS {
                    self.enter(State::Ground);
                }
            }
            '\n' | '\t' => {
                out.push(ch);
                self.enter(State::Ground);
            }
            _ => self.enter(State::Ground),
        }
    }

    fn str_seq(&mut self, ch: char, out: &mut String, kind: Kind) {
        match ch {
            '\x1b' => self.state = State::StrEsc(kind),
            '\u{9c}' => self.enter(State::Ground),
            '\x07' if kind == Kind::Osc => self.enter(State::Ground),
            '\n' | '\t' => {
                out.push(ch);
                self.enter(State::Ground);
            }
            _ => {
                self.n += 1;
                if self.n > MAX_CTRL_STR_CHARS {
                    self.enter(State::Ground);
                }
            }
        }
    }

    fn str_esc(&mut self, ch: char, out: &mut String, kind: Kind) {
        match ch {
            '\\' | '\u{9c}' => self.enter(State::Ground),
            '\x1b' => self.n = 0,
            '\n' | '\t' => {
                out.push(ch);
                self.enter(State::Ground);
            }
            _ => self.state = State::Str(kind),
        }
    }
}

/// 一次性净化：适合落定消息等整段文本；流式路径改用 [`Stripper`]。
pub(crate) fn strip(text: &str) -> String {
    Stripper::new().feed(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(strip("普通正文 abc 123"), "普通正文 abc 123");
        assert_eq!(strip("line\nnext\tcell"), "line\nnext\tcell");
        assert_eq!(strip(""), "");
    }

    #[test]
    fn osc_terminated_by_bel_or_st() {
        assert_eq!(strip("\x1b]0;title\x07正文"), "正文");
        assert_eq!(strip("\x1b]52;c;AAAA\x1b\\正文"), "正文");
        // OSC 内的 BEL 之外控制字符一并吞掉
        assert_eq!(strip("\x1b]8;;http://x\x07链接\x1b]8;;\x07"), "链接");
    }

    #[test]
    fn dcs_pm_apc_sos_stripped() {
        assert_eq!(strip("\x1bPq#0;2;0;0;0\x1b\\正文"), "正文");
        assert_eq!(strip("\x1b^private\x1b\\正文"), "正文");
        assert_eq!(strip("\x1b_payload\x1b\\正文"), "正文");
        assert_eq!(strip("\x1bXstring\x1b\\正文"), "正文");
    }

    #[test]
    fn csi_and_esc_sequences_stripped() {
        assert_eq!(strip("\x1b[31m红\x1b[0m"), "红");
        assert_eq!(strip("\x1b[2J\x1b[H清屏"), "清屏");
        assert_eq!(strip("\x1b(B正文"), "正文");
        assert_eq!(strip("\x1b7\x1b8"), "");
        // C1 形式的 CSI / OSC 同样识别
        assert_eq!(strip("\u{9b}1;31m红\u{9b}0m"), "红");
        assert_eq!(strip("\u{9d}0;t\u{9c}正文"), "正文");
    }

    #[test]
    fn control_chars_dropped_but_layout_kept() {
        assert_eq!(strip("a\x00b\x07c"), "abc");
        assert_eq!(strip("a\u{85}b"), "ab");
        assert_eq!(strip("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn unterminated_string_is_bounded() {
        let mut s = String::from("\x1b]0;");
        s.push_str(&"A".repeat(MAX_CTRL_STR_CHARS + 8));
        s.push_str("正文");
        let out = strip(&s);
        // 超界即中止剥离，后续字节按普通文本处理，不整段吞没。
        assert!(out.contains('正'));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn unterminated_csi_is_bounded() {
        let mut s = String::from("\x1b[");
        s.push_str(&"1;".repeat(MAX_CTRL_CSI_CHARS));
        s.push('m');
        s.push_str("正文");
        let out = strip(&s);
        // 超界即中止剥离：剩余参数字节按普通文本放行，正文不被吞没。
        assert!(out.ends_with("正文"));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn dangling_escape_at_end_is_dropped() {
        assert_eq!(strip("正文\x1b"), "正文");
        assert_eq!(strip("正文\x1b["), "正文");
        assert_eq!(strip("正文\x1b]0;title"), "正文");
    }

    #[test]
    fn state_survives_chunk_boundary() {
        let mut s = Stripper::new();
        assert_eq!(s.feed("前\x1b"), "前");
        assert_eq!(s.feed("[31"), "");
        assert_eq!(s.feed("m中"), "中");
        assert_eq!(s.feed("\x1b"), "");
        assert_eq!(s.feed("]0;t"), "");
        assert_eq!(s.feed("\x07后"), "后");
    }

    #[test]
    fn reset_discards_pending_sequence() {
        let mut s = Stripper::new();
        assert_eq!(s.feed("前\x1b]0;t"), "前");
        s.reset();
        assert_eq!(s.feed("正文"), "正文");
    }
}
