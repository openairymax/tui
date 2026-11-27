// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// X2-11a/b、X2-14a（§23.5.2 序 93/94/99）：帧字节的分片与同步块包裹。
//
// 分片的硬约束是「边界不落在转义序列内部」。字节流按到达顺序解析，
// 单看显示效果切在序列中间也能出字；但同步块要求每个分片自包一对
// `h/l`，一旦某片在序列中间结束，下一片开头就落在残留序列里，配对
// 与失败回退全部失效。故切分点只能取完整序列之后。
//
// 本模块只做纯计算：给定一帧字节与上限，返回安全切点；给定分片，
// 返回包好同步块的分片序列。不持有 writer、不知道终端能力。

/// 同步输出开启/关闭序列（DEC 2026）。
pub(crate) const SYNC_ON: &[u8] = b"\x1b[?2026h";
pub(crate) const SYNC_OFF: &[u8] = b"\x1b[?2026l";

/// 把一帧切成分片字节区间，任何边界都不落在转义序列内部。
///
/// 单个超长序列（如大 OSC52）无法再切，独占一个分片——宁可该片超过
/// 上限，也不切断序列。
pub(crate) fn split(buf: &[u8], max: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < buf.len() {
        let end = safe_end(buf, start, max.max(1));
        out.push((start, end));
        start = end;
    }
    out
}

/// 从 `start` 起、不超过 `max` 字节的安全终点。
fn safe_end(buf: &[u8], start: usize, max: usize) -> usize {
    let limit = (start + max).min(buf.len());
    if limit == buf.len() {
        return limit;
    }
    let mut i = start;
    while i < limit {
        if buf[i] != 0x1b {
            i += 1;
            continue;
        }
        match seq_len(&buf[i..]) {
            // 序列跨过上限：回退到序列起点；若序列本就起于分片起点，
            // 则让它独占整片，绝不切开。
            Some(len) if i + len > limit => {
                return if i > start { i } else { i + len };
            }
            Some(len) => i += len,
            // 未闭合序列（异常输入）：同样回退到起点，交给下一片整段处理。
            None => return if i > start { i } else { limit },
        }
    }
    limit
}

/// 从 `buf[0] == ESC` 起的转义序列总长；序列未闭合返回 None。
fn seq_len(buf: &[u8]) -> Option<usize> {
    match buf.get(1) {
        Some(b'[') => csi_len(buf),
        Some(b']') => osc_len(buf),
        Some(_) => Some(2),
        None => None,
    }
}

/// CSI：参数/中间字节 0x20..=0x3f 后跟一个终止字节 0x40..=0x7e。
fn csi_len(buf: &[u8]) -> Option<usize> {
    for (i, &b) in buf.iter().enumerate().skip(2) {
        match b {
            0x20..=0x3f => {}
            0x40..=0x7e => return Some(i + 1),
            _ => return None,
        }
    }
    None
}

/// OSC：以 BEL 或 ST（ESC \）结束。
fn osc_len(buf: &[u8]) -> Option<usize> {
    let mut i = 2;
    while i < buf.len() {
        match buf[i] {
            0x07 => return Some(i + 1),
            0x1b if buf.get(i + 1) == Some(&b'\\') => return Some(i + 2),
            _ => i += 1,
        }
    }
    None
}

/// 同步块包裹：`sync` 为真时每片自包一对 `h/l`，否则原样返回区间。
pub(crate) fn wrap(buf: &[u8], parts: &[(usize, usize)], sync: bool) -> Vec<u8> {
    if !sync {
        return buf.to_vec();
    }
    let mut out = Vec::with_capacity(buf.len() + parts.len() * 16);
    for &(start, end) in parts {
        out.extend_from_slice(SYNC_ON);
        out.extend_from_slice(&buf[start..end]);
        out.extend_from_slice(SYNC_OFF);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分片内所有转义序列都必须完整闭合。
    fn closed(chunk: &[u8]) -> bool {
        let mut i = 0;
        while i < chunk.len() {
            if chunk[i] == 0x1b {
                match seq_len(&chunk[i..]) {
                    Some(n) => i += n,
                    None => return false,
                }
            } else {
                i += 1;
            }
        }
        true
    }

    #[test]
    fn short_buffer_is_single_part() {
        let buf = b"plain text";
        assert_eq!(split(buf, 1024), vec![(0, buf.len())]);
    }

    #[test]
    fn plain_bytes_split_at_limit() {
        let buf = vec![b'x'; 10];
        assert_eq!(split(&buf, 4), vec![(0, 4), (4, 8), (8, 10)]);
    }

    #[test]
    fn boundary_never_inside_csi() {
        // 4 个普通字节后接一条 5 字节 CSI，上限 6 会正好落在序列中段。
        let mut buf = vec![b'a'; 4];
        buf.extend_from_slice(b"\x1b[31m");
        buf.extend_from_slice(b"zz");
        let parts = split(&buf, 6);
        for &(s, e) in &parts {
            assert!(closed(&buf[s..e]), "chunk {:?}", &buf[s..e]);
        }
        assert_eq!(parts[0], (0, 4), "第一片应在序列前收口");
        // 贪心装箱：序列与其后的普通字节同片，边界只落在完整序列之后。
        assert_eq!(parts[1], (4, 10), "序列之后才收口");
        assert!(closed(&buf[4..10]));
    }

    #[test]
    fn osc_is_never_cut() {
        let mut buf = b"ab".to_vec();
        buf.extend_from_slice(b"\x1b]52;c;QUJD\x07");
        buf.extend_from_slice(b"tail");
        let parts = split(&buf, 5);
        for &(s, e) in &parts {
            assert!(closed(&buf[s..e]), "chunk {:?}", &buf[s..e]);
        }
        assert!(parts.iter().any(|&(s, e)| &buf[s..e] == b"\x1b]52;c;QUJD\x07"));
    }

    #[test]
    fn oversized_sequence_owns_a_part() {
        let mut buf = b"x".to_vec();
        buf.extend_from_slice(b"\x1b]0;");
        buf.extend_from_slice(&[b'q'; 40]);
        buf.push(0x07);
        let parts = split(&buf, 8);
        let osc = &buf[1..];
        assert_eq!(parts[0], (0, 1));
        assert_eq!(parts[1], (1, 1 + osc.len()));
        assert!(closed(osc));
    }

    #[test]
    fn wrap_pairs_every_part() {
        let buf = b"\x1b[31mabc";
        let parts = split(buf, 4);
        let out = wrap(buf, &parts, true);
        assert_eq!(on_count(&out), parts.len());
        assert_eq!(on_count(&out), off_count(&out));
        assert!(out.starts_with(SYNC_ON));
        assert!(out.ends_with(SYNC_OFF));
    }

    #[test]
    fn wrap_off_keeps_bytes() {
        let buf = b"\x1b[31mabc";
        let parts = split(buf, 4);
        assert_eq!(wrap(buf, &parts, false), buf);
    }

    /// 超 1MiB 的整帧必须被切成多片，且每片不超上限（X2-11b）。
    #[test]
    fn megabyte_frame_splits_under_limit() {
        const LIMIT: usize = 1024 * 1024;
        let buf = vec![b'.'; LIMIT + 512];
        let parts = split(&buf, LIMIT);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|&(s, e)| e - s <= LIMIT));
        assert_eq!(parts.last().copied(), Some((LIMIT, buf.len())));
    }

    fn on_count(buf: &[u8]) -> usize {
        buf.windows(SYNC_ON.len()).filter(|w| *w == SYNC_ON).count()
    }

    fn off_count(buf: &[u8]) -> usize {
        buf.windows(SYNC_OFF.len())
            .filter(|w| *w == SYNC_OFF)
            .count()
    }
}
