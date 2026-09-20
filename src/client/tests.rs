// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// gateway 客户端回归测试（0.1.18 拆分自 client.rs）。
//
// 测试分两组：SSE 缓冲边界（纯函数，锁定 T-20 内存有界契约）与真实对话
// 回放（本地 TCP mock 服务器驱动 run_stream_turn 全链路）。帧字段一律走
// gen SSoT 常量，禁止手写 wire 字符串——信封形态变更时测试与实现同源失效。

use super::stream::{sse_buf_extend, sse_frame_end, MAX_SSE_BUFFER};
use super::*;

#[test]
fn sse_frame_end_finds_blank_line_separator() {
    assert_eq!(sse_frame_end(b"data: x\n\n"), Some(9));
    assert_eq!(sse_frame_end(b"data: x\n"), None);
    assert_eq!(sse_frame_end(b""), None);
}

#[test]
fn sse_buf_extend_appends_within_limit() {
    let mut buf = Vec::new();
    sse_buf_extend(&mut buf, b"data: 1\n\n");
    assert_eq!(buf, b"data: 1\n\n");
}

#[test]
fn sse_buf_extend_discards_half_frame_on_overflow() {
    // T-20：异常流无 "\n\n" 分隔时，缓冲超限必须丢弃半帧而不是无界增长。
    let mut buf = vec![b'a'; MAX_SSE_BUFFER];
    sse_buf_extend(&mut buf, b"overflow");
    // 半帧被清零，仅保留新 chunk（内存有界）。
    assert_eq!(buf, b"overflow");
    // 随后可正常重新累积帧。
    sse_buf_extend(&mut buf, b"data: x\n\n");
    assert_eq!(buf, b"overflowdata: x\n\n");
}

#[test]
fn sse_buf_extend_allows_frame_up_to_limit() {
    // 恰好等于上限的帧允许保留（不误伤合法大帧）。
    let mut buf = Vec::new();
    sse_buf_extend(&mut buf, &vec![b'x'; MAX_SSE_BUFFER]);
    assert_eq!(buf.len(), MAX_SSE_BUFFER);
}

/* ---- 0.1.15 WS-6 T-04：agent.run_stream 真实对话回放回归 ----
 * 方案 §WS-6 6.2：CI 无法起全栈 gateway，退化为录制回放——以本地
 * SSE mock 服务器驱动 run_stream_turn 真实消费链（解码/累积/回退/
 * 错误上报），锁定社区用户报障「agent.run_stream: empty result」
 * 及中文长回复、错误帧、中断帧三类真实场景。帧字段一律走 gen
 * SSoT 常量，禁止手写 wire 字符串。 */

use agentrt_rs::run_stream::gen;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// 起一次性本地 SSE 服务器：accept 一个连接，读完整个请求（头 +
/// 按 Content-Length 读满请求体，防止带未读数据 close 触发 RST）
/// 后按给定字节块序列回放响应体（模拟任意 TCP 分片），随后关闭。
fn spawn_sse_server(chunks: Vec<Vec<u8>>) -> String {
    spawn_sse_server_timed(chunks).0
}

/// 同 `spawn_sse_server`，额外回传**写出首个含 `token_delta` 帧的字节块**
/// 后的时刻（`Instant` 单调单调、跨线程可比）——B2 V2.4「服务端首字节 →
/// 首字上屏」时差取证的基准点。字节块未含该帧时保持 `None`。
fn spawn_sse_server_timed(
    chunks: Vec<Vec<u8>>,
) -> (String, Arc<Mutex<Option<std::time::Instant>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    let first_delta: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
    let mark = first_delta.clone();
    let needle = format!(
        "\"{}\":\"{}\"",
        gen::AIRY_RS_K_TYPE,
        gen::AIRY_RS_TYPE_TOKEN_DELTA
    )
    .into_bytes();
    std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut req: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        // 1. 读完请求头（到空行为止）
        let header_end = loop {
            match conn.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
            if let Some(pos) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        // 2. 按 Content-Length 读满请求体（避免 close 时接收缓冲残留 → RST）
        let content_length = req[..header_end]
            .split(|&b| b == b'\n')
            .find_map(|line| {
                let line = String::from_utf8_lossy(line);
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        while req.len() < header_end + content_length {
            match conn.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
        // 3. 回放：HTTP 响应头（无 Content-Length，靠连接关闭结束流）
        let _ = conn.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        );
        for c in &chunks {
            if conn.write_all(c).is_err() {
                break;
            }
            let _ = conn.flush();
            if c.windows(needle.len()).any(|w| w == needle.as_slice()) {
                let mut g = mark.lock().unwrap();
                if g.is_none() {
                    *g = Some(std::time::Instant::now());
                }
            }
        }
        // conn drop → FIN → 客户端 bytes_stream 结束
    });
    (format!("http://{}", addr), first_delta)
}

/// 构造 v1 事件信封（字段键走 gen SSoT 常量）。
fn frame_envelope(event_type: &str, id: i64, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        gen::AIRY_RS_K_V: gen::AIRY_RS_VERSION,
        gen::AIRY_RS_K_TYPE: event_type,
        gen::AIRY_RS_K_ID: id,
        gen::AIRY_RS_K_RUN_ID: "run_t04",
        gen::AIRY_RS_K_SESSION: "sess_t04",
        gen::AIRY_RS_K_DATA: data,
    })
}

fn frame_bytes(v: &serde_json::Value) -> Vec<u8> {
    format!("data: {}\n\n", v).into_bytes()
}

async fn drive_turn(base_url: &str) -> (Result<RunResponse>, Vec<String>, Vec<String>) {
    let text_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tool_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let client = GatewayClient::new(base_url).expect("client 构造");
    let t = text_seen.clone();
    let g = tool_seen.clone();
    let result = client
        .run_stream_turn(
            "请分析当前任务",
            "agents/main.agent.yaml",
            None,
            "sess_t04",
            None,
            None,
            None,
            |chunk| t.lock().unwrap().push(chunk.to_string()),
            |evt| g.lock().unwrap().push(evt.to_string()),
        )
        .await;
    let text = text_seen.lock().unwrap().clone();
    let tool = tool_seen.lock().unwrap().clone();
    (result, text, tool)
}

#[tokio::test]
async fn replay_chinese_long_reply_full_flow() {
    // 场景 1：中文长回复全流程——run_start → 多段 token_delta →
    // message（权威全文）→ run_end(completed)。断言权威内容采纳、
    // 打字机增量完整、token 消耗回传。
    let deltas = [
        "好的，我来分析这个任务。",
        "首先需要检查网关连接状态，",
        "然后核对 agent 配置文件，",
        "最后汇总执行计划并给出结论。",
    ];
    let authoritative = deltas.concat();
    let mut body = frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_START,
        0,
        serde_json::json!({}),
    ));
    for (i, d) in deltas.iter().enumerate() {
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            (i + 1) as i64,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: d }),
        )));
    }
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_MESSAGE,
        6,
        serde_json::json!({
            gen::AIRY_RS_K_ROLE: "assistant",
            gen::AIRY_RS_K_CONTENT: authoritative,
        }),
    )));
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_END,
        7,
        serde_json::json!({
            gen::AIRY_RS_K_STATUS: "completed",
            gen::AIRY_RS_K_USE_TICKS: 42,
        }),
    )));
    let url = spawn_sse_server(vec![body]);
    let (result, text, _tool) = drive_turn(&url).await;
    let resp = result.expect("中文长回复应成功");
    assert_eq!(resp.response, authoritative, "message 帧为权威全文");
    assert_eq!(text.concat(), authoritative, "打字机增量完整");
    assert_eq!(resp.tokens_used, Some(42), "use_ticks 回传 token 消耗");
}

#[tokio::test]
async fn replay_structured_error_frame_surfaces_message() {
    // 场景 2：错误帧——引擎失败发 error 帧，客户端转 Err 且消息
    // 可读（UI 只呈现可读消息，不暴露内部栈）。
    let err_text = "模型服务暂时不可用，请稍后重试";
    let body = [
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_START,
            0,
            serde_json::json!({}),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_ERROR,
            1,
            serde_json::json!({ gen::AIRY_RS_K_MSG: err_text }),
        )),
    ]
    .concat();
    let url = spawn_sse_server(vec![body]);
    let (result, _text, tool) = drive_turn(&url).await;
    let err = result.expect_err("error 帧必须转 Err");
    assert!(
        err.to_string().contains(err_text),
        "错误消息原文可见: {}",
        err
    );
    assert!(
        tool.iter().any(|l| l.contains("\"__airy_evt\":\"error\"")),
        "错误事件进入工具通道供 UI 呈现"
    );
}

#[tokio::test]
async fn replay_interrupted_run_keeps_accumulated_text() {
    // 场景 3：中断帧——cancelled 收尾且无 message 时，保留已收到的
    // 增量输出（用户中断不应丢失已生成内容）。
    let body = [
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_START,
            0,
            serde_json::json!({}),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            1,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: "已完成一半的结论：" }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            2,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: "网关连接正常。" }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            3,
            serde_json::json!({ gen::AIRY_RS_K_STATUS: "cancelled" }),
        )),
    ]
    .concat();
    let url = spawn_sse_server(vec![body]);
    let (result, _text, _tool) = drive_turn(&url).await;
    let resp = result.expect("中断但有输出应成功返回");
    assert_eq!(resp.response, "已完成一半的结论：网关连接正常。");
}

#[tokio::test]
async fn replay_empty_stream_reports_empty_result() {
    // 场景 4：空流回归锁定——run_start/run_end 收尾但无任何文本与
    // message，必须报「agent.run_stream: empty result」（社区用户
    // 实测报障原文），不得静默成功。
    let body = [
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_START,
            0,
            serde_json::json!({}),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            1,
            serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
        )),
    ]
    .concat();
    let url = spawn_sse_server(vec![body]);
    let (result, _text, _tool) = drive_turn(&url).await;
    let err = result.expect_err("空流必须报错");
    assert_eq!(err.to_string(), "agent.run_stream: empty result");
}

#[tokio::test]
async fn replay_tool_progress_events_reach_tool_channel() {
    // 场景 5：工具进度——tool_start/tool_end 经本地 tool_id 映射回填
    // 工具名，成功态 ok=1，进入工具通道渲染。
    let body = [
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_START,
            0,
            serde_json::json!({}),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOOL_START,
            1,
            serde_json::json!({
                gen::AIRY_RS_K_TOOL: "fs.write",
                gen::AIRY_RS_K_TOOL_ID: "t1",
            }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOOL_END,
            2,
            serde_json::json!({
                gen::AIRY_RS_K_TOOL_ID: "t1",
                gen::AIRY_RS_K_STATUS: "ok",
            }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_MESSAGE,
            3,
            serde_json::json!({ gen::AIRY_RS_K_CONTENT: "done" }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            4,
            serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
        )),
    ]
    .concat();
    let url = spawn_sse_server(vec![body]);
    let (result, _text, tool) = drive_turn(&url).await;
    let resp = result.expect("工具流程应成功");
    assert_eq!(resp.response, "done");
    assert!(
        tool[0].contains("\"__airy_evt\":\"tool_call\"") && tool[0].contains("fs.write"),
        "tool_call 事件携带工具名: {}",
        tool[0]
    );
    assert!(
        tool[1].contains("\"__airy_evt\":\"tool_result\"") && tool[1].contains("\"ok\":1"),
        "tool_result 成功态 ok=1: {}",
        tool[1]
    );
}

#[tokio::test]
async fn replay_bytes_split_across_tcp_chunks_still_parses() {
    // 场景 6：网络分片——同一 SSE 流按任意字节边界切成三块（帧 JSON
    // 中间截断）逐块到达，解析结果必须与单块一致（TCP 不保证按帧
    // 分段；回归网络层半帧粘包处理）。
    let deltas = ["第一段中文。", "第二段中文。", "第三段中文。"];
    let mut body = frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_START,
        0,
        serde_json::json!({}),
    ));
    for (i, d) in deltas.iter().enumerate() {
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            (i + 1) as i64,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: d }),
        )));
    }
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_MESSAGE,
        4,
        serde_json::json!({ gen::AIRY_RS_K_CONTENT: deltas.concat() }),
    )));
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_END,
        5,
        serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
    )));
    let third = body.len() / 3;
    let url = spawn_sse_server(vec![
        body[..third].to_vec(),
        body[third..third * 2].to_vec(),
        body[third * 2..].to_vec(),
    ]);
    let (result, _text, _tool) = drive_turn(&url).await;
    let resp = result.expect("分片流应与整流等价");
    assert_eq!(resp.response, deltas.concat());
}

#[tokio::test]
async fn replay_trailing_frame_without_newline_still_delivers_message() {
    // 场景 7：尾部残行——末帧 message 无 "\n\n" 收尾连接即关闭，
    // 残行仍须被采纳（防上游网关省略流结束符时丢权威内容）。
    let mut body = frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_START,
        0,
        serde_json::json!({}),
    ));
    let mut last = frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_MESSAGE,
        1,
        serde_json::json!({ gen::AIRY_RS_K_CONTENT: "结尾无换行的权威内容" }),
    ));
    // 去掉末帧的 "\n\n" 收尾
    last.truncate(last.len() - 2);
    body.extend(last);
    let url = spawn_sse_server(vec![body]);
    let (result, _text, _tool) = drive_turn(&url).await;
    let resp = result.expect("残行 message 必须被采纳");
    assert_eq!(resp.response, "结尾无换行的权威内容");
}

#[tokio::test]
async fn replay_delta_fragments_count_and_concat_exact() {
    // B2（0.1.18）V2.2：长回答必须由**多个** token_delta 分片构成
    // （禁止整段截断为单帧/512 字节上限截断），且分片拼接结果与最终
    // message 权威全文**逐字符一致**。此处显式断言分片计数 > 1。
    let deltas = [
        "第一段：正在核对网关连接状态。",
        "第二段：读取 agent 编排配置与模型参数。",
        "第三段：汇总执行计划。",
        "第四段：给出最终结论与后续建议。",
    ];
    let authoritative = deltas.concat();
    let mut body = frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_START,
        0,
        serde_json::json!({}),
    ));
    for (i, d) in deltas.iter().enumerate() {
        body.extend(frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            (i + 1) as i64,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: d }),
        )));
    }
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_MESSAGE,
        5,
        serde_json::json!({ gen::AIRY_RS_K_CONTENT: authoritative }),
    )));
    body.extend(frame_bytes(&frame_envelope(
        gen::AIRY_RS_TYPE_RUN_END,
        6,
        serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
    )));
    let url = spawn_sse_server(vec![body]);
    let (result, text, _tool) = drive_turn(&url).await;
    let resp = result.expect("真增量流应成功");
    assert!(
        text.len() > 1,
        "长回答必须分片多次下发，实测分片数 {}",
        text.len()
    );
    assert_eq!(
        text.concat(),
        authoritative,
        "分片拼接须与权威全文逐字符一致"
    );
    assert_eq!(resp.response, authoritative, "message 帧为权威全文");
}

#[tokio::test]
async fn replay_delta_first_char_within_100ms() {
    // B2（0.1.18）§5A.3 W4：本地打字机已移除，TUI 不得对增量做缓冲/节流——
    // 服务端写出含 token_delta 的字节块到首字上屏回调的时差须 ≤100ms
    // （整块上屏语义由 app::stream_delta_lands_on_arrival_and_settles 锁定，
    // 客户端侧此处取证传输/解码/回调链无附加延迟）。
    let delta = "首字即可见的答案。";
    let body = [
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_START,
            0,
            serde_json::json!({}),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_TOKEN_DELTA,
            1,
            serde_json::json!({ gen::AIRY_RS_K_DELTA: delta }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_MESSAGE,
            2,
            serde_json::json!({ gen::AIRY_RS_K_CONTENT: delta }),
        )),
        frame_bytes(&frame_envelope(
            gen::AIRY_RS_TYPE_RUN_END,
            3,
            serde_json::json!({ gen::AIRY_RS_K_STATUS: "completed" }),
        )),
    ]
    .concat();
    let (url, server_wrote) = spawn_sse_server_timed(vec![body]);
    let first_char: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
    let fc = first_char.clone();
    let client = GatewayClient::new(&url).expect("client 构造");
    let result = client
        .run_stream_turn(
            "请分析当前任务",
            "agents/main.agent.yaml",
            None,
            "sess_t04",
            None,
            None,
            None,
            move |_c| {
                let mut g = fc.lock().unwrap();
                if g.is_none() {
                    *g = Some(std::time::Instant::now());
                }
            },
            |_e| {},
        )
        .await;
    result.expect("回放应成功");
    let t0 = server_wrote
        .lock()
        .unwrap()
        .expect("服务端应写出含 token_delta 的字节块");
    let t1 = first_char.lock().unwrap().expect("首字回调应触发");
    let gap = t1.duration_since(t0);
    assert!(
        gap.as_millis() <= 100,
        "服务端首字节 → 首字上屏时差须 ≤100ms，实测 {:?}",
        gap
    );
}
