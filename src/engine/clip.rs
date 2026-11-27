// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// OSC 52 剪贴板（0.1.21 X5-10b）：把选区文本交给终端转发到系统剪贴板。
//
// 走 OSC 52 而非平台命令：SSH/容器里没有 xclip/pbcopy，OSC 52 由终端
// 自己落地，同一份代码在三种系统上可用。编码与转义是纯函数（可单测），
// 只有写 stdout 一步带副作用。终端不支持时序列被忽略，不影响画面。

use std::io::Write;

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 Base64（带 `=` 填充）。OSC 52 的载荷必须是它，不能有换行。
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// 完整 OSC 52 序列：`ESC ] 52 ; c ; <base64> BEL`。
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// 复制到系统剪贴板（终端转交）。失败只影响复制，不影响会话。
pub fn copy(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(osc52(text).as_bytes())?;
    out.flush()
}

/// Copy on select 开关（C8：释放即复制可关）。缺省开启；显式 `0`/`false`
/// 关闭，便于终端把 OSC 52 当未知序列回显的环境退化为纯选区高亮。
pub fn copy_on_select() -> bool {
    match std::env::var("AIRY_TUI_COPY_ON_SELECT") {
        Ok(v) => {
            let v = v.trim();
            v != "0" && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_utf8_cjk() {
        assert_eq!(base64("你好".as_bytes()), "5L2g5aW9");
    }

    #[test]
    fn osc52_wraps_payload_with_bel() {
        let seq = osc52("你好");
        assert_eq!(seq, "\x1b]52;c;5L2g5aW9\x07");
    }
}
