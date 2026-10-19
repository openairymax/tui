// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 终端通知方法表（0.1.19 TUI 建设方案 §5.5 #5、§6.7 #4）。
//
// 偏好顺序固定为 WT_SESSION → OSC9 → OSC777 → OSC99 → BEL：前三者按
// 终端能力择一，全不支持时退终端响铃，并由调用方做状态条兜底。本模块
// 只做纯决策与序列生成，零 IO；唯一写出点在 engine/term。
//
// 通知文本来自会话状态，必须先净化（控制字符替换为空格、按字符截断），
// 否则 CR/LF 或 ESC 会提前终止 OSC 序列，退化为对终端的注入。

use std::fmt::Write as _;

use crate::budget::NOTIFY_TEXT_MAX;

/// 通知投递方法（按偏好降序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Osc9,
    Osc777,
    Osc99,
    Bell,
}

impl Method {
    /// 日志用短名，避免 Debug 派生格式混入可读性差的结构体噪声。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Method::Osc9 => "osc9",
            Method::Osc777 => "osc777",
            Method::Osc99 => "osc99",
            Method::Bell => "bell",
        }
    }
}

/// 能力判定所需的环境视图（由调用方注入，便于纯函数单测）。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Env<'a> {
    pub(crate) wt: bool,
    pub(crate) prog: Option<&'a str>,
    pub(crate) term: Option<&'a str>,
    pub(crate) konsole: bool,
    pub(crate) kitty: bool,
    pub(crate) vte: Option<u32>,
}

/// 按方法表选择投递方式。
///
/// 只认确定的终端标记，不做乐观猜测：未知终端一律 BEL，宁可少一次
/// 弹窗也不把裸 OSC 写进不支持的终端（部分终端会回显转义残片）。
pub(crate) fn pick(env: &Env<'_>) -> Method {
    if env.wt {
        return Method::Osc9;
    }
    match env.prog.unwrap_or_default() {
        "iTerm.app" | "WezTerm" => return Method::Osc9,
        "ghostty" => return Method::Osc777,
        _ => {}
    }
    if env.term.is_some_and(|t| t.starts_with("rxvt-unicode")) {
        return Method::Osc777;
    }
    if env.kitty || env.konsole {
        return Method::Osc99;
    }
    if env.vte.is_some_and(|v| v >= 7200) {
        return Method::Osc777;
    }
    Method::Bell
}

/// 读取真实环境后选择方法。
pub(crate) fn detect() -> Method {
    let prog = std::env::var("TERM_PROGRAM").ok();
    let term = std::env::var("TERM").ok();
    let vte = std::env::var("VTE_VERSION")
        .ok()
        .and_then(|v| v.parse().ok());
    pick(&Env {
        wt: std::env::var_os("WT_SESSION").is_some(),
        prog: prog.as_deref(),
        term: term.as_deref(),
        konsole: std::env::var_os("KONSOLE_VERSION").is_some(),
        kitty: std::env::var_os("KITTY_WINDOW_ID").is_some(),
        vte,
    })
}

/// 生成通知序列。OSC9 无标题字段，标题并入正文；OSC99 按 kitty 协议
/// 拆成标题与正文两条（同 id，正文条置 `d=1` 表示完成）。
pub(crate) fn frame(method: Method, title: &str, body: &str) -> String {
    let (title, body) = (clean(title), clean(body));
    let mut seq = String::new();
    match method {
        Method::Osc9 => {
            let text = if title.is_empty() {
                body
            } else if body.is_empty() {
                title
            } else {
                format!("{title}: {body}")
            };
            let _ = write!(seq, "\x1b]9;{text}\x07");
        }
        Method::Osc777 => {
            let _ = write!(seq, "\x1b]777;notify;{title};{body}\x07");
        }
        Method::Osc99 => {
            let _ = write!(seq, "\x1b]99;i=airy:t;{title}\x1b\\");
            let _ = write!(seq, "\x1b]99;i=airy:d=1:p=body;{body}\x1b\\");
        }
        Method::Bell => seq.push('\x07'),
    }
    seq
}

/// 净化通知文本：控制字符折为空格，按字符数截断后去首尾空白。
fn clean(text: &str) -> String {
    text.chars()
        .take(NOTIFY_TEXT_MAX)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wt_session_wins_the_table() {
        let env = Env {
            wt: true,
            prog: Some("ghostty"),
            kitty: true,
            ..Env::default()
        };
        assert_eq!(pick(&env), Method::Osc9);
    }

    #[test]
    fn osc9_terminals_are_known() {
        for prog in ["iTerm.app", "WezTerm"] {
            let env = Env {
                prog: Some(prog),
                ..Env::default()
            };
            assert_eq!(pick(&env), Method::Osc9, "{prog}");
        }
    }

    #[test]
    fn osc777_terminals_are_known() {
        let ghostty = Env {
            prog: Some("ghostty"),
            ..Env::default()
        };
        assert_eq!(pick(&ghostty), Method::Osc777);
        let rxvt = Env {
            term: Some("rxvt-unicode-256color"),
            ..Env::default()
        };
        assert_eq!(pick(&rxvt), Method::Osc777);
        let vte = Env {
            vte: Some(7200),
            ..Env::default()
        };
        assert_eq!(pick(&vte), Method::Osc777);
    }

    #[test]
    fn osc99_terminals_are_known() {
        let kitty = Env {
            kitty: true,
            ..Env::default()
        };
        assert_eq!(pick(&kitty), Method::Osc99);
        let konsole = Env {
            konsole: true,
            ..Env::default()
        };
        assert_eq!(pick(&konsole), Method::Osc99);
    }

    #[test]
    fn unknown_and_old_vte_fall_back_to_bell() {
        assert_eq!(pick(&Env::default()), Method::Bell);
        let old = Env {
            vte: Some(5000),
            ..Env::default()
        };
        assert_eq!(pick(&old), Method::Bell);
    }

    #[test]
    fn osc9_frames_carry_title_into_body() {
        assert_eq!(
            frame(Method::Osc9, "Airymax", "回合完成"),
            "\x1b]9;Airymax: 回合完成\x07"
        );
    }

    #[test]
    fn osc777_frames_keep_title_and_body_separate() {
        assert_eq!(
            frame(Method::Osc777, "Airymax", "回合完成"),
            "\x1b]777;notify;Airymax;回合完成\x07"
        );
    }

    #[test]
    fn osc99_frames_split_title_and_body() {
        let seq = frame(Method::Osc99, "Airymax", "回合完成");
        assert!(seq.contains("\x1b]99;i=airy:t;Airymax\x1b\\"));
        assert!(seq.contains("\x1b]99;i=airy:d=1:p=body;回合完成\x1b\\"));
    }

    #[test]
    fn bell_frame_is_a_single_bel() {
        assert_eq!(frame(Method::Bell, "Airymax", "回合完成"), "\x07");
    }

    #[test]
    fn control_chars_never_reach_the_sequence() {
        let seq = frame(Method::Osc777, "t\x1b]0;x", "a\r\nb\x07");
        assert_eq!(seq.matches('\x1b').count(), 1, "仅序列起始的 ESC");
        assert_eq!(seq.matches('\x07').count(), 1, "仅终止 BEL");
        assert!(!seq.contains('\r'));
    }

    #[test]
    fn text_is_capped_by_chars() {
        let long = "字".repeat(NOTIFY_TEXT_MAX + 20);
        assert_eq!(clean(&long).chars().count(), NOTIFY_TEXT_MAX);
    }
}
