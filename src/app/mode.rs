// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 模式标记协议（0.1.18 B3 控制面/用户面隔离）：解析 LLM 返回的 [MODE:*]
// 标记判定对话/任务/大任务集。协议段（前导元话语 + 标记 + 未识别残留）
// 一律从用户面正文剥离、仅进入诊断通道；流式与非流式路径共享同一剥离
// 语义，流式中间态亦保证 `[MODE:` 零渲染。

/// LLM 判定的模式（任务集判定结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeMarker {
    /// 普通对话
    Chat,
    /// 任务集（简单任务，无需 GCCP）
    Task,
    /// 大任务集（需先任务事实确认 GCCP）
    TaskGccp,
}

/// 剥离结果：`body` 为用户面正文；`protocol` 为剥出的控制面文本
/// （前导元话语、标记、未识别残留），供诊断通道记录，禁止上屏。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedReply {
    pub mode: ModeMarker,
    pub body: String,
    pub protocol: String,
}

/// 标记段合法长度上限：`[MODE:*]` 闭合 `]` 距起点不超过 24 字节，超出按
/// 未闭合残渣处理（剥至行尾），防止误吞正文中远处的 `]`。
const MARKER_MAX: usize = 24;

/// 头部定位窗口（字节）：模式标记仅在此窗口内参与模式判定（T-01）。
/// 256 字节容纳现实长度的中文元话语前导（V3.3），同时隔离深部正文
/// 引用标记样式文字的误判型。
const HEAD_WINDOW: usize = 256;

/// 返回 `s` 中不超过 `max` 字节且落在 UTF-8 字符边界上的最大偏移。
///
/// T-01（P0-7 修复）：定位窗口此前直接按字节切片截断，当窗口末字节
/// 落在多字节字符（中文等）中间时 `&t[..win]` panic，中文长回复流式
/// 必现。回退到最近的前序字符边界，窗口语义最多损失 3 字节。
fn boundary_floor(s: &str, max: usize) -> usize {
    let mut idx = s.len().min(max);
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// `s` 是否为 `head` 的真前缀（跨 chunk 切分保护：`"[MODE"` 尚不能判定
/// 不是协议头，须等待后续字节）。
fn is_head_of(s: &str, head: &str) -> bool {
    s.len() < head.len() && head.starts_with(s)
}

/// `s` 尾部是 `"[MODE:"` 真前缀时的字节数（正文阶段跨 chunk 标记切分保护）。
fn proto_prefix_len(s: &str) -> usize {
    const H: &[u8] = b"[MODE:";
    let b = s.as_bytes();
    let max = H.len().min(b.len());
    (1..=max)
        .rev()
        .find(|k| b[b.len() - k..] == H[..*k])
        .unwrap_or(0)
}

/// 剥除 `s` 中全部 `[MODE:*]` 段与未闭合 `[MODE:` 行段（已知与未知标记
/// 一律剥离），剥除内容追加进 `protocol`。无 `]` 无换行时整段视为残渣。
/// 剥除位紧随的单个空格/制表一并折叠，避免正文残留双空白。
fn strip_mode_segments(s: &mut String, protocol: &mut String) {
    while let Some(p) = s.find("[MODE:") {
        let rest = &s[p..];
        let cut = rest
            .find(']')
            .filter(|er| er + 1 <= MARKER_MAX)
            .map(|er| er + 1)
            .or_else(|| rest.find('\n').map(|nl| nl + 1));
        match cut {
            Some(c) => {
                protocol.push_str(&rest[..c]);
                s.replace_range(p..p + c, "");
                if matches!(s[p..].chars().next(), Some(' ') | Some('\t')) {
                    s.remove(p);
                }
            }
            None => {
                protocol.push_str(rest);
                s.truncate(p);
            }
        }
    }
}

/// 结构化解析 LLM 响应（B3 修复②③）：头部窗口定位模式标记判型；前导
/// 元话语、标记本身、未识别残留全部剥离进 `protocol`，正文中的违规标记
/// 复读同样剥离。协议判定能力保留（V3.2），用户面正文零协议污染
/// （V3.1/V3.3）。
pub fn sanitize_reply(resp: &str) -> SanitizedReply {
    let mut out = SanitizedReply {
        mode: ModeMarker::Chat,
        body: resp.to_string(),
        protocol: String::new(),
    };
    let t = resp.trim_start();
    if t.is_empty() {
        return out;
    }
    let head = &t[..boundary_floor(t, HEAD_WINDOW)];
    if let Some(idx) = head.find("[MODE:") {
        let closed = head[idx..].find(']').filter(|er| er + 1 <= MARKER_MAX);
        match closed {
            Some(er) => {
                let marker = &head[idx..=idx + er];
                out.mode = match marker {
                    "[MODE:TASK:GCCP]" => ModeMarker::TaskGccp,
                    "[MODE:TASK]" => ModeMarker::Task,
                    _ => ModeMarker::Chat,
                };
                out.protocol.push_str(&head[..idx]);
                out.protocol.push_str(marker);
                out.body = t[idx + er + 1..].trim_start().to_string();
            }
            // 未闭合残渣：剥至行尾（无换行则整段隐藏）
            None => {
                let cut = t[idx..].find('\n').map_or(t.len(), |nl| idx + nl + 1);
                out.protocol.push_str(&t[idx..cut]);
                out.body = t[cut..].to_string();
            }
        }
    } else {
        out.body = t.to_string();
    }
    strip_mode_segments(&mut out.body, &mut out.protocol);
    out
}

/// 流式控制面净化器（B3 修复②③）：SSE 增量块 → 用户面正文。头部
/// （前导 + 模式标记）判定完成前缓冲不上屏（流式无法回撤已上屏字符）；
/// 正文阶段实时剥离标记与残留，跨 chunk 悬挂的 `[MODE:` 前缀留缓冲待
/// 后续块判定。累计剥出的协议文本存于 `protocol_buf`（诊断通道取用）。
pub struct StreamSanitizer {
    raw: String,
    header_done: bool,
    /// 头部刚闭合且标记后内容尚未到达：下一正文块需去前导空白（与
    /// sanitize_reply 头部 trim_start 语义对齐，覆盖跨 chunk 场景）。
    lead_ws: bool,
    /// 刚剥完一个闭合标记且其后内容尚未到达：下一正文块开头折叠一个
    /// 空格/制表，避免剥除位残留双空白。
    just_marker: bool,
    /// 本轮累计剥出的控制面文本（诊断通道在落定时取用并清空）
    pub protocol_buf: String,
}

impl Default for StreamSanitizer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamSanitizer {
    pub fn new() -> Self {
        Self {
            raw: String::new(),
            header_done: false,
            lead_ws: true,
            just_marker: false,
            protocol_buf: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.raw.clear();
        self.header_done = false;
        self.lead_ws = true;
        self.just_marker = false;
        self.protocol_buf.clear();
    }

    /// 消费一个增量块，返回 (净化正文, 本次剥出的协议文本)。
    pub fn feed(&mut self, chunk: &str) -> (String, String) {
        self.raw.push_str(chunk);
        let mut protocol = String::new();
        if !self.header_done {
            self.consume_header(&mut protocol);
        }
        let mut clean = String::new();
        if self.header_done {
            self.flush_body(&mut clean, &mut protocol);
        }
        if !protocol.is_empty() {
            self.protocol_buf.push_str(&protocol);
        }
        (clean, protocol)
    }

    /// 头部阶段：前导与模式标记判定。非 `[` 头立即放行；协议头在闭合
    /// 可判定前保持缓冲（流式无法回撤已上屏字符）。
    fn consume_header(&mut self, protocol: &mut String) {
        let trimmed = self.raw.trim_start();
        if trimmed.is_empty() {
            self.raw.clear();
            return;
        }
        if !trimmed.starts_with('[') {
            self.header_done = true;
            self.raw = trimmed.to_string();
            return;
        }
        if let Some(er) = trimmed.find(']').filter(|er| er + 1 <= MARKER_MAX) {
            self.header_done = true;
            if trimmed.starts_with("[MODE:") {
                protocol.push_str(&trimmed[..=er]);
                self.raw = trimmed[er + 1..].trim_start().to_string();
            } else {
                // 非标记的方括号头（如 [注意]）：正文起点，原样放行
                self.raw = trimmed.to_string();
            }
            return;
        }
        // 无合法闭合：协议头前缀等待；遇换行或超窗按残渣剥至行尾
        if trimmed.starts_with("[MODE:") || is_head_of(trimmed, "[MODE:") {
            if trimmed.starts_with("[MODE:") {
                if let Some(nl) = trimmed.find('\n') {
                    // 标记不跨行：换行即未闭合残渣实锤，其后即正文
                    self.header_done = true;
                    self.lead_ws = false;
                    protocol.push_str(&trimmed[..=nl]);
                    self.raw = trimmed[nl + 1..].to_string();
                    return;
                }
                if trimmed.len() > HEAD_WINDOW {
                    self.header_done = true;
                    self.lead_ws = false;
                    protocol.push_str(trimmed);
                    self.raw.clear();
                }
            }
            return;
        }
        // 未闭合且非协议的 `[` 头：正文起点放行
        self.header_done = true;
        self.raw = trimmed.to_string();
    }

    /// 正文阶段：剥离标记与残留；尾部悬挂前缀留待后续块判定。首块去
    /// 前导空白（对齐 sanitize_reply 头部 trim_start），剥除位紧随空白
    /// 折叠（对齐 strip_mode_segments），跨 chunk 空白经 just_marker 延续。
    fn flush_body(&mut self, clean: &mut String, protocol: &mut String) {
        if self.lead_ws && !self.raw.is_empty() {
            self.lead_ws = false;
            let cut = self.raw.len() - self.raw.trim_start().len();
            self.raw.drain(..cut);
        }
        if self.just_marker && !self.raw.is_empty() {
            if matches!(self.raw.chars().next(), Some(' ') | Some('\t')) {
                self.raw.remove(0);
            }
            self.just_marker = false;
        }
        loop {
            match self.raw.find("[MODE:") {
                None => {
                    let keep = proto_prefix_len(&self.raw);
                    let cut = self.raw.len() - keep;
                    clean.push_str(&self.raw[..cut]);
                    self.raw.drain(..cut);
                    return;
                }
                Some(p) => {
                    clean.push_str(&self.raw[..p]);
                    self.raw.drain(..p);
                    let rest = &self.raw;
                    let cut = rest
                        .find(']')
                        .filter(|er| er + 1 <= MARKER_MAX)
                        .map(|er| er + 1)
                        .or_else(|| rest.find('\n').map(|nl| nl + 1));
                    match cut {
                        Some(c) => {
                            protocol.push_str(&rest[..c]);
                            self.raw.drain(..c);
                            if matches!(self.raw.chars().next(), Some(' ') | Some('\t')) {
                                self.raw.remove(0);
                            } else if self.raw.is_empty() {
                                // 跨 chunk：空白可能落在下一块，挂起折叠
                                self.just_marker = true;
                            }
                        }
                        None => {
                            if self.raw.len() > HEAD_WINDOW {
                                protocol.push_str(&self.raw);
                                self.raw.clear();
                                continue;
                            }
                            return; // 悬挂：等后续块补齐闭合
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_floor_never_splits_multibyte_chars() {
        // ASCII：不超过 max 即可
        assert_eq!(boundary_floor("abc", 10), 3);
        assert_eq!(boundary_floor("abcdef", 4), 4);
        // 汉字 3 字节：截断落在第 2 个汉字中间时回退到前序边界
        assert_eq!(boundary_floor("中中中", 4), 3);
        assert_eq!(boundary_floor("中中中", 5), 3);
        assert_eq!(boundary_floor("中中中", 6), 6);
        // 4 字节 emoji：任何非边界截断回退
        assert_eq!(boundary_floor("\u{1F600}", 2), 0);
        assert_eq!(boundary_floor("\u{1F600}", 4), 4);
        // 空串
        assert_eq!(boundary_floor("", 64), 0);
    }

    #[test]
    fn window_truncation_on_multibyte_char_does_not_panic() {
        // P0-7 复现向量：85 个汉字（255 字节）+ 4 字节 emoji，256 字节
        // 窗口恰好落在 emoji 中间。修复前 `&t[..256]` 在此 panic。
        let body = format!("{}\u{1F600}", "汉".repeat(85));
        let _ = sanitize_reply(&body); // 不 panic 即通过
    }

    #[test]
    fn markers_still_recognized_with_leading_text() {
        // 中文前导 + 标记在窗口内：模式判定不回归，前导随标记入协议段
        let r = sanitize_reply("好的，[MODE:TASK:GCCP]\n先做任务事实确认");
        assert_eq!(r.mode, ModeMarker::TaskGccp);
        assert_eq!(r.body, "先做任务事实确认");
        assert!(r.protocol.contains("[MODE:TASK:GCCP]"));
        assert!(r.protocol.contains("好的，"));

        let r = sanitize_reply("这是中文前导，[MODE:TASK] 开始");
        assert_eq!(r.mode, ModeMarker::Task);
        assert_eq!(r.body, "开始");
    }

    #[test]
    fn marker_outside_window_still_hidden_from_body() {
        // 标记位于判定窗口之外：不触发模式切换，但字面仍从正文剥离
        // （V3.1），剥除位紧随空白一并折叠
        let prefix = "汉".repeat(100); // 300 字节 > 256
        let body = format!("{prefix}[MODE:TASK] 后续内容");
        let r = sanitize_reply(&body);
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, format!("{prefix}后续内容"));
        assert!(r.protocol.contains("[MODE:TASK]"));
    }

    #[test]
    fn unclosed_and_unknown_markers_hidden() {
        // 未闭合：剥至行尾，其后正文保留
        let r = sanitize_reply("[MODE:TASK 未闭合\n真实正文");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "真实正文");
        assert!(r.protocol.contains("[MODE:TASK"));

        // 未知标记：模式回落 Chat，标记本身仍隐藏（修复③）
        let r = sanitize_reply("[MODE:WHAT] 未知标记正文");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "未知标记正文");
        assert!(r.protocol.contains("[MODE:WHAT]"));
    }

    #[test]
    fn empty_and_whitespace_inputs_return_original() {
        let r = sanitize_reply("");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "");
        assert!(r.protocol.is_empty());

        let r = sanitize_reply("   \n\t ");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "   \n\t ");
        assert!(r.protocol.is_empty());
    }

    #[test]
    fn plain_reply_passes_through_untouched() {
        let r = sanitize_reply("普通回复，无任何标记。");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "普通回复，无任何标记。");
        assert!(r.protocol.is_empty());
    }

    #[test]
    fn meta_discourse_before_marker_never_reaches_body() {
        // V3.3：模型解释自身意图的前导元话语随标记入协议段，不进正文
        let r = sanitize_reply("我认为这是一个普通对话问题，所以我将直接回答。[MODE:CHAT] 你好！");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "你好！");
        assert!(r.protocol.contains("我认为这是一个普通对话问题"));
        assert!(r.protocol.contains("[MODE:CHAT]"));
    }

    #[test]
    fn markers_repeated_in_body_are_stripped() {
        // 模型在正文中违规复读协议标记：同样剥离
        let r = sanitize_reply("[MODE:CHAT] 提示：回复请以[MODE:CHAT]开头哦");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "提示：回复请以开头哦");
        assert_eq!(r.protocol.matches("[MODE:CHAT]").count(), 2);
    }

    #[test]
    fn bracket_head_without_mode_is_body() {
        // 正文以非协议方括号开头：原样保留，不误剥
        let r = sanitize_reply("[注意] 这是正文内容");
        assert_eq!(r.mode, ModeMarker::Chat);
        assert_eq!(r.body, "[注意] 这是正文内容");
        assert!(r.protocol.is_empty());
    }

    /* ---- StreamSanitizer ---- */

    fn feed_all(s: &mut StreamSanitizer, chunks: &[&str]) -> String {
        let mut clean = String::new();
        for c in chunks {
            let (part, _) = s.feed(c);
            clean.push_str(&part);
        }
        clean
    }

    #[test]
    fn stream_well_formed_marker_never_revealed() {
        // V3.1：合规模型输出在流式中间态零标记渲染
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["[MODE:", "TASK]", " 步骤一：分析", "问题"]);
        assert_eq!(clean, "步骤一：分析问题");
        assert_eq!(s.protocol_buf, "[MODE:TASK]");
    }

    #[test]
    fn stream_marker_split_across_chunks() {
        // 标记跨 chunk 切分：悬挂缓冲后正确剥离，正文无残留
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["[MO", "DE:CHA", "T] 你好", "呀"]);
        assert_eq!(clean, "你好呀");
        assert_eq!(s.protocol_buf, "[MODE:CHAT]");
    }

    #[test]
    fn stream_unknown_marker_hidden() {
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["[MODE:WHAT] 未知", "标记正文"]);
        assert_eq!(clean, "未知标记正文");
        assert!(s.protocol_buf.contains("[MODE:WHAT]"));
    }

    #[test]
    fn stream_leading_text_then_marker() {
        // 前导客套放行（已上屏不可回撤），正文中的标记仍实时剥离
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["好的 ", "[MODE:CHAT] 我们", "聊聊"]);
        assert_eq!(clean, "好的 我们聊聊");
        assert_eq!(s.protocol_buf, "[MODE:CHAT]");
    }

    #[test]
    fn stream_bracket_head_is_body() {
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["[注", "意] 正文", "继续"]);
        assert_eq!(clean, "[注意] 正文继续");
        assert!(s.protocol_buf.is_empty());
    }

    #[test]
    fn stream_marker_repeated_in_body_hidden() {
        // 剥除位紧随空白折叠：流式与非流式（strip_mode_segments）同语义
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["回答正文 [MODE:CHAT]", " 后续"]);
        assert_eq!(clean, "回答正文 后续");
        assert_eq!(s.protocol_buf, "[MODE:CHAT]");
    }

    #[test]
    fn stream_reset_clears_state() {
        let mut s = StreamSanitizer::new();
        let _ = feed_all(&mut s, &["[MODE:TASK] 正文"]);
        s.reset();
        let clean = feed_all(&mut s, &["新一轮", "正常正文"]);
        assert_eq!(clean, "新一轮正常正文");
        assert!(s.protocol_buf.is_empty());
    }

    #[test]
    fn stream_unclosed_residue_stripped_to_line_end() {
        let mut s = StreamSanitizer::new();
        let clean = feed_all(&mut s, &["[MODE:TASK 未闭合\n", "真实正文"]);
        assert_eq!(clean, "真实正文");
        assert!(s.protocol_buf.contains("[MODE:TASK"));
    }
}
