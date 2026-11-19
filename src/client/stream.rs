// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 流式执行轮通道（0.1.18 拆分自 client.rs）：agent.run_stream 事件消费。
//
// 传输与帧解码由 agentrt-rs 协议客户端统一承担（端点与 v1 信封不在此处
// 重写），本模块只做 UI 语义转译与 SSE 缓冲边界控制。所有权衡：文本增量
// 即时回调（打字机）与最终权威文本分离——message 帧存在时以帧内容为准，
// 否则回退增量累积，避免引擎不发 message 时丢内容。
//
// C-15：TUI 与 CLI 共用统一任务编排入口——对话/任务轮均走
// task.submit_stream（服务端裁决 chat/task），计划与子目标消费 plan/step，
// 阶段迁移以 task_state 事件为准；agent.run_stream 保留为兼容入口。

use anyhow::Result;
use log::{debug, info};

use agentrt_rs::error::AgentOSError;
use agentrt_rs::run_stream::{gen, run_stream_events_with_method, METHOD_TASK_SUBMIT_STREAM};

use super::GatewayClient;
use super::RunResponse;

/// 下游服务正在重启（gateway 503 UNAVAILABLE）：本回合没有失败。
///
/// 携带服务端给出的重试提示；界面据此进入可续跑暂停态，而不是失败卡片，
/// 并保留「继续」按钮所需的语义（令牌由服务端 agent.turns 保存）。
#[derive(Debug)]
pub struct Unavailable {
    pub detail: String,
    pub retry_ms: u64,
    /// 断流前若已收到续跑令牌则直接携带，省一次 agent.turns 查询。
    pub token: Option<String>,
}

impl Unavailable {
    fn from_error(e: &AgentOSError) -> Self {
        let detail = e.to_string();
        Self {
            retry_ms: parse_retry_ms(&detail).unwrap_or(800),
            detail,
            token: None,
        }
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "执行服务正在重启，{}ms 后可继续：{}", self.retry_ms, self.detail)
    }
}

impl std::error::Error for Unavailable {}

/// 引擎侧失败事件的结构化归因。
///
/// `reason` 是机制层登记的稳定枚举，`text` 是人话摘要。摘要会随措辞与
/// 本地化漂移，成因判定只能用 `reason`；保留 `text` 仅作展示。
#[derive(Debug, Clone)]
pub struct RunFail {
    pub reason: String,
    pub text: String,
}

impl std::fmt::Display for RunFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for RunFail {}

/// 从错误文本中提取 `retry_after_ms`（网关 503 应答体）。
fn parse_retry_ms(s: &str) -> Option<u64> {
    const KEY: &str = "\"retry_after_ms\":";
    let i = s.find(KEY)? + KEY.len();
    let digits: String = s[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

impl GatewayClient {
    /// task.submit_stream 事件流（C-15 统一任务编排入口，方案 §2.4 v1 信封）。
    ///
    /// 传输与帧解码由 agentrt-rs 协议客户端统一承担（端点与信封不在此处
    /// 重写），本方法只做 UI 语义转译，投递两个回调通道：
    ///   - `on_text`：token_delta.delta 增量文本（对话打字机实时渲染）；
    ///   - `on_tool`：工具进度 / 计划步骤 / 任务状态 / 错误事件（__airy_evt
    ///     语义 JSON，与 stream_chat 工具事件通道同格式，poll_pending 复用
    ///     同一套消费）。
    ///
    /// 返回最终 RunResponse：response = message 帧 content（引擎权威最终
    /// 文本）；引擎无 message 时回退 token_delta 累积。结构化错误帧
    /// （error 事件 code/message）以 Err 呈现，原始 JSON 不上屏。
    ///
    /// `params` 透传语义与 send_message 一致（prompt/agent_file/model/
    /// session_id/agent/messages/gccp_answers）；`goal` 取 prompt。
    ///
    /// `resume_token`（C-14-07）：预算暂停后的断点续跑。携带时不再重放
    /// 历史——prompt/messages 均不下发，引擎按令牌内的步骤摘要与剩余目标
    /// 自行重建上下文。
    ///
    /// `declared_mode`：客户端显式模式（None 交由服务端 auto 裁决）；
    /// `confirmed`：用户已确认目标/计划（GCCP 门禁通过）。对话轮不带确认，
    /// 长任务在门禁处得到 needs_intent（零引擎/零工具调用），由上层进入
    /// 目标确认流程后带 confirmed=true 重新提交。
    // 12 参数 = 9 业务参数 + 确认位 + 双回调（流式渲染通道）：签名冻结
    // 理由同 send_message；回调独立泛型不可并入参数结构体。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_task_stream_turn<F, E>(
        &self,
        prompt: &str,
        agent_file: &str,
        model: Option<&str>,
        session_id: &str,
        agent: Option<serde_json::Value>,
        messages: Option<serde_json::Value>,
        gccp_answers: Option<&str>,
        resume_token: Option<&str>,
        declared_mode: Option<&str>,
        confirmed: bool,
        on_text: F,
        on_tool: E,
    ) -> Result<RunResponse>
    where
        F: FnMut(&str),
        E: FnMut(&str),
    {
        self.stream_turn(
            METHOD_TASK_SUBMIT_STREAM,
            declared_mode,
            confirmed,
            prompt,
            agent_file,
            model,
            session_id,
            agent,
            messages,
            gccp_answers,
            resume_token,
            on_text,
            on_tool,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn stream_turn<F, E>(
        &self,
        method: &str,
        declared_mode: Option<&str>,
        confirmed: bool,
        prompt: &str,
        agent_file: &str,
        model: Option<&str>,
        session_id: &str,
        agent: Option<serde_json::Value>,
        messages: Option<serde_json::Value>,
        gccp_answers: Option<&str>,
        resume_token: Option<&str>,
        mut on_text: F,
        mut on_tool: E,
    ) -> Result<RunResponse>
    where
        F: FnMut(&str),
        E: FnMut(&str),
    {
        let mut params = serde_json::json!({
            "agent_file": agent_file,
            "interactive": true,
        });
        if !prompt.is_empty() {
            params["prompt"] = serde_json::Value::String(prompt.to_string());
        }
        if method == METHOD_TASK_SUBMIT_STREAM {
            // 统一入口：goal 为任务目标（daemon 优先读 goal，其次历史/prompt）；
            // 未确认时绝不携带 gccp_confirmed（门禁只在用户确认后放行）
            if !prompt.is_empty() {
                params["goal"] = serde_json::Value::String(prompt.to_string());
            }
            if let Some(m) = declared_mode {
                params["mode"] = serde_json::Value::String(m.to_string());
            }
            if confirmed {
                params["gccp_confirmed"] = serde_json::Value::Bool(true);
            }
        }
        if let Some(tok) = resume_token {
            // 续跑：引擎以令牌为唯一上下文来源（不整轮重放）
            params["resume_token"] = serde_json::Value::String(tok.to_string());
        }
        if let Some(m) = model {
            if !m.is_empty() {
                params["model"] = serde_json::Value::String(m.to_string());
            }
        }
        if !session_id.is_empty() {
            params["session_id"] = serde_json::Value::String(session_id.to_string());
        }
        if let Some(a) = agent {
            // gateway 判定 params.agent（JSON 对象）存在才进入编排分支
            params["agent"] = a;
        }
        if let Some(h) = messages {
            params["messages"] = h;
        }
        if let Some(a) = gccp_answers {
            if !a.is_empty() {
                params["gccp_answers"] = serde_json::Value::String(a.to_string());
            }
        }
        info!(
            "POST {}/api/v1/agent/run/stream ({} mode={:?} confirmed={} prompt_len={}, session={})",
            self.base_url,
            method,
            declared_mode,
            confirmed,
            prompt.len(),
            session_id
        );

        // v1 事件消费状态（本方法内局部；渲染状态经回调推给 UI）
        let mut final_content: Option<String> = None;
        let mut text_acc = String::new();
        let mut err_msg: Option<String> = None;
        let mut err_reason: Option<String> = None;
        let mut tokens: Option<u64> = None;
        // D-11：结构化 usage 账本（run_end.usage），界面直接显示 total。
        let mut usage: Option<super::Usage> = None;
        // tool_end 只带 tool_id：以本地映射回填工具名（渲染需要动作短语）
        let mut tool_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // 完整轨迹只在 run_end 下发一次（stream 路径无 RPC result 可读）
        let mut tool_trace: Option<Vec<super::ToolTrace>> = None;
        // run_end 终态（C-14-07）：paused/failed 是任务集状态迁移的事实源
        let mut run_status: Option<String> = None;
        let mut resume_out: Option<String> = None;
        let mut rounds_used: Option<i64> = None;
        let mut rounds_limit: Option<i64> = None;
        // C-15：服务端任务身份/模式裁决（run_start 元数据）与门禁/降级事实
        let mut task_id: Option<String> = None;
        let mut plan_id: Option<String> = None;
        let mut mode: Option<String> = None;
        let mut mode_reason: Option<String> = None;
        let mut needs_intent: Option<serde_json::Value> = None;
        // C-17：显式声明 task 但缺可执行判据 → needs_criteria 门禁对象
        let mut needs_criteria: Option<serde_json::Value> = None;
        let mut plan_unavailable: Option<String> = None;
        // C-17：完成判据复核证据（tool_d verifier 逐项结果）
        let mut verification: Option<serde_json::Value> = None;
        let mut plan_steps: i64 = 0;

        run_stream_events_with_method(&self.rs, method, params, |ev| {
            match ev.event_type.as_str() {
                // token_delta：增量文本 → 打字机实时渲染 + 本地累积
                gen::AIRY_RS_TYPE_TOKEN_DELTA => {
                    if let Some(d) = ev.data_str(gen::AIRY_RS_K_DELTA) {
                        if !d.is_empty() {
                            text_acc.push_str(d);
                            on_text(d);
                        }
                    }
                }
                // tool_start / tool_end：工具进度行。D-11：可见性以
                // tool.list 目录驱动，参数摘要随事件上屏，未登记工具回退
                // 原始 tool_id，不再隐藏。
                gen::AIRY_RS_TYPE_TOOL_START => {
                    let tool = ev
                        .data_str(gen::AIRY_RS_K_TOOL)
                        .unwrap_or("tool")
                        .to_string();
                    if let Some(tid) = ev.data_str(gen::AIRY_RS_K_TOOL_ID) {
                        tool_names.insert(tid.to_string(), tool.clone());
                    }
                    let mut line = serde_json::json!({
                        "__airy_evt": "tool_call",
                        "tool": tool,
                    });
                    if let Some(a) = ev.data_str(gen::AIRY_RS_K_ARGS) {
                        if !a.is_empty() {
                            // 单行化与截断由 toolcat::event_line 统一裁决
                            line["args"] = serde_json::Value::String(a.to_string());
                        }
                    }
                    on_tool(&line.to_string());
                }
                gen::AIRY_RS_TYPE_TOOL_END => {
                    let tid = ev
                        .data_str(gen::AIRY_RS_K_TOOL_ID)
                        .unwrap_or("")
                        .to_string();
                    let tool = tool_names.get(&tid).cloned().unwrap_or_else(|| {
                        if tid.is_empty() {
                            "tool".to_string()
                        } else {
                            tid.clone()
                        }
                    });
                    let ok = ev.data_str(gen::AIRY_RS_K_STATUS) == Some("ok");
                    // 失败附引擎结果预览首行（≤80 字符）便于诊断
                    let mut line = serde_json::json!({
                        "__airy_evt": "tool_result",
                        "tool": tool,
                        "ok": if ok { 1 } else { 0 },
                    });
                    if !ok {
                        if let Some(summary) = ev.data_str(gen::AIRY_RS_K_RESULT_HASH) {
                            let err: String = summary
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(80)
                                .collect();
                            line["summary"] = serde_json::Value::String(err);
                        }
                    }
                    on_tool(&line.to_string());
                }
                // message：引擎权威最终文本（role=assistant）；
                // reasoning 为全流程思考链（一次性投递）
                gen::AIRY_RS_TYPE_MESSAGE => {
                    if let Some(c) = ev.data_str(gen::AIRY_RS_K_CONTENT) {
                        if !c.is_empty() {
                            final_content = Some(c.to_string());
                        }
                    }
                    if let Some(r) = ev.data_str(gen::AIRY_RS_K_REASONING) {
                        if !r.is_empty() {
                            let line = serde_json::json!({
                                "__airy_evt": "reasoning",
                                "content": r,
                            });
                            on_tool(&line.to_string());
                        }
                    }
                }
                // run_start：服务端任务身份与模式裁决（C-15 唯一事实源）
                gen::AIRY_RS_TYPE_RUN_START => {
                    task_id = ev.data_str(gen::AIRY_RS_K_TASK_ID).map(str::to_string);
                    plan_id = ev.data_str(gen::AIRY_RS_K_PLAN_ID).map(str::to_string);
                    mode = ev.data_str(gen::AIRY_RS_K_MODE).map(str::to_string);
                    mode_reason = ev.data_str(gen::AIRY_RS_K_MODE_REASON).map(str::to_string);
                    debug!(
                        "{method}: run_start mode={:?} reason={:?} task={:?}",
                        mode, mode_reason, task_id
                    );
                }
                // plan：计划先于执行下发（C-15 顺序修正）——记步数，子目标
                // 由随后的 step 帧逐条渲染
                gen::AIRY_RS_TYPE_PLAN => {
                    if let Some(p) = ev.data_str(gen::AIRY_RS_K_PLAN_ID) {
                        plan_id = Some(p.to_string());
                    }
                    if let Some(n) = ev.data_i64(gen::AIRY_RS_K_STEP_COUNT) {
                        plan_steps = n;
                        let line = serde_json::json!({
                            "__airy_evt": "plan",
                            "step_count": n,
                        });
                        on_tool(&line.to_string());
                    }
                }
                // step：执行前的子目标（计划顺序）→ 计划步骤行
                gen::AIRY_RS_TYPE_STEP => {
                    let idx = ev.data_i64(gen::AIRY_RS_K_STEP_INDEX).unwrap_or(0);
                    let cnt = ev
                        .data_i64(gen::AIRY_RS_K_STEP_COUNT)
                        .unwrap_or(plan_steps);
                    let text = ev.data_str(gen::AIRY_RS_K_STEP).unwrap_or("").to_string();
                    let status = ev
                        .data_str(gen::AIRY_RS_K_STEP_STATUS)
                        .unwrap_or("planned");
                    let line = serde_json::json!({
                        "__airy_evt": "plan_step",
                        "index": idx,
                        "count": cnt,
                        "status": status,
                        "text": text,
                    });
                    on_tool(&line.to_string());
                }
                // task_state：任务状态机迁移（submitted/planned/executing/
                // completed/paused/failed/cancelled/unverified）。needs_intent 与
                // needs_criteria 是门禁对象（零引擎/零工具调用），交由上层进入
                // 目标确认 / 判据补充。
                gen::AIRY_RS_TYPE_TASK_STATE => {
                    let state = ev.data_str(gen::AIRY_RS_K_STATE).unwrap_or("");
                    if state == "needs_intent" {
                        needs_intent = Some(ev.data.clone());
                    } else if state == gen::AIRY_RS_STATE_NEEDS_CRITERIA {
                        needs_criteria = Some(ev.data.clone());
                    } else if !state.is_empty() {
                        let line = serde_json::json!({
                            "__airy_evt": "task_state",
                            "state": state,
                            "reason": ev.data_str(gen::AIRY_RS_K_REASON).unwrap_or(""),
                        });
                        on_tool(&line.to_string());
                    }
                }
                // plan_unavailable：规划降级（think 不可达/无计划）——如实告知
                gen::AIRY_RS_TYPE_PLAN_UNAVAILABLE => {
                    let reason = ev
                        .data_str(gen::AIRY_RS_K_REASON)
                        .unwrap_or("plan_unavailable");
                    plan_unavailable = Some(reason.to_string());
                    let line = serde_json::json!({
                        "__airy_evt": "plan_unavailable",
                        "reason": reason,
                    });
                    on_tool(&line.to_string());
                }
                // error：结构化错误帧 → Err。D-10：daemon 已给出失败语义，
                // 这里只翻成一行"原因 + 下一步"，不再让调用方猜 -31 是什么。
                gen::AIRY_RS_TYPE_ERROR => {
                    let msg = ev.data_str(gen::AIRY_RS_K_MSG).unwrap_or("");
                    let fail = ev.data.get(gen::AIRY_RS_K_FAILURE);
                    let text = fail_text(fail, msg);
                    if !text.is_empty() {
                        if err_msg.is_none() {
                            err_msg = Some(text.clone());
                            err_reason = fail_reason(fail);
                        }
                        let line = serde_json::json!({
                            "__airy_evt": "error",
                            "message": text,
                        });
                        on_tool(&line.to_string());
                    }
                }
                // run_end：流收尾（completed/cancelled/failed）；
                // usage 为 provider 同源 token 账本，use_ticks 仅作旧引擎回退
                gen::AIRY_RS_TYPE_RUN_END => {
                    if let Some(u) = ev.data.get(gen::AIRY_RS_K_USAGE) {
                        usage = serde_json::from_value::<super::Usage>(u.clone()).ok();
                    }
                    // D-11：total 即 llm_d 增量，优先采用；缺失时才回退
                    // use_ticks（旧引擎），不做任何换算或估算。
                    tokens = usage
                        .as_ref()
                        .map(|u| u.total)
                        .filter(|t| *t > 0)
                        .or_else(|| ev.data_i64(gen::AIRY_RS_K_USE_TICKS).map(|t| t as u64));
                    let status = ev.data_str(gen::AIRY_RS_K_STATUS).unwrap_or("completed");
                    debug!("agent.run_stream: run_end (status={})", status);
                    run_status = Some(status.to_string());
                    resume_out = ev.data_str(gen::AIRY_RS_K_RESUME_TOKEN).map(str::to_string);
                    rounds_used = ev.data_i64(gen::AIRY_RS_K_ROUNDS_USED);
                    rounds_limit = ev.data_i64(gen::AIRY_RS_K_ROUNDS_LIMIT);
                    // C-17：复核证据随终态帧下发（成功亦含 ok=true），
                    // 界面据此逐项呈现判据结果。
                    verification = ev.data.get(gen::AIRY_RS_K_VERIFICATION).cloned();
                    // 工具轨迹随终态帧整体下发（tool/arguments/result/ok）：
                    // tool_start/tool_end 只有动作名与成败，不足以还原调用。
                    if let Some(tr) = ev.data.get(gen::AIRY_RS_K_TOOL_TRACE) {
                        tool_trace = serde_json::from_value::<Vec<super::ToolTrace>>(tr.clone()).ok();
                    }
                    // B2（0.1.18）V2.1：引擎三段耗时（think/llm/tool，毫秒）
                    // 随 run_end 透传。首字延迟不达标时，这三项即归因证据
                    // （区分模型长思考 / 网关串行 / 工具阻塞）；旧引擎无此
                    // 三键时不记录，保持日志安静。
                    let seg = |k: &str| ev.data_i64(k);
                    if let (Some(think), Some(llm), Some(tool)) = (
                        seg(gen::AIRY_RS_K_THINK_MS),
                        seg(gen::AIRY_RS_K_LLM_MS),
                        seg(gen::AIRY_RS_K_TOOL_MS),
                    ) {
                        info!(
                            "agent.run_stream: 分段耗时 think_ms={think} llm_ms={llm} \
                             tool_ms={tool} total_ms={}",
                            ev.data_i64(gen::AIRY_RS_K_DURATION).unwrap_or(-1)
                        );
                    }
                }
                // 其余（run_start / 未知）：宽容忽略（§2.4.4）
                _ => {}
            }
        })
        .await
        .map_err(|e| {
            // UNAVAILABLE 带类别跨过 string 转换：上层按暂停态处理，
            // 其余错误仍走失败呈现。
            if e.is_unavailable() {
                anyhow::Error::new(Unavailable::from_error(&e))
            } else {
                anyhow::anyhow!("{e}")
            }
        })?;

        // 没有终态帧就断开：agent_d 重启或被网关中止，回合未失败。
        // message/error 帧已落定则视为正常收尾（旧引擎不带 run_end 的兼容
        // 契约）；只有「仅有增量、没有任何裁决帧」才是中途断流，必须让上层
        // 进入暂停态（可续跑），不能把半截结果当成 completed 呈现。
        let settled = final_content.is_some() || err_msg.is_some();
        if run_status.is_none() && needs_intent.is_none() && needs_criteria.is_none() && !settled {
            debug!("agent.run_stream: stream closed before run_end");
            return Err(anyhow::Error::new(Unavailable {
                detail: "event stream closed before run_end".to_string(),
                retry_ms: 800,
                token: resume_out.take(),
            }));
        }

        // 预算暂停与判据不可取证都是可达终态而非错误：paused 携带续跑令牌，
        // unverified 携带逐项证据，两者都按成功帧返回，界面分别呈现
        // 「暂停三选」与「未验证」。
        let paused = run_status.as_deref() == Some("paused") && resume_out.is_some();
        let unverified = run_status.as_deref() == Some(gen::AIRY_RS_STATE_UNVERIFIED);
        let soft = paused || unverified;
        if let Some(m) = err_msg {
            if !soft {
                // 带结构化 reason 的失败以 RunFail 抛出：上层按载荷归类，
                // 不依赖摘要措辞（摘要只用于展示）。
                return Err(match err_reason {
                    Some(r) if !r.is_empty() => anyhow::Error::new(RunFail { reason: r, text: m }),
                    _ => anyhow::anyhow!("{}", m),
                });
            }
            info!("agent.run_stream: 软终态（{}），以结果帧返回", m);
        }
        // 引擎成功以 message 帧为权威内容；无 message 时回退增量累积。
        // needs_intent / needs_criteria 门禁帧不触达引擎，文本为空是预期结果。
        let response = final_content.unwrap_or(text_acc);
        let gated = needs_intent.is_some() || needs_criteria.is_some();
        if response.trim().is_empty() && !soft && !gated {
            return Err(anyhow::anyhow!("agent.run_stream: empty result"));
        }
        info!(
            "← {} OK ({} chars, tokens={:?}, status={:?}, mode={:?}, gated={})",
            method,
            response.len(),
            tokens,
            run_status,
            mode,
            gated
        );
        Ok(RunResponse {
            session_id: session_id.to_string(),
            response,
            tokens_used: tokens,
            cost_usd: None,
            thinking: None,
            tool_trace,
            gccp_need_interaction: false,
            gccp_questions: Vec::new(),
            status: run_status,
            resume_token: resume_out,
            rounds_used,
            rounds_limit,
            task_id,
            plan_id,
            mode,
            mode_reason,
            needs_intent,
            needs_criteria,
            plan_unavailable,
            usage,
            verification,
        })
    }
}

/// T-20：单条 SSE 帧缓冲上限（1 MiB）。hall 事件为紧凑 JSON，正常远小于此值；
/// 若网关推送异常（长流无 "\n\n" 分隔），无界累积会导致内存耗尽。
pub(super) const MAX_SSE_BUFFER: usize = 1 << 20;

/// T-20：将 chunk 追加进 SSE 帧缓冲；追加后总量超限时丢弃半帧并从零累积
/// （不完整帧本就无法解析，保持内存有界即协议自愈）。
pub(super) fn sse_buf_extend(buf: &mut Vec<u8>, chunk: &[u8]) {
    if buf.len() + chunk.len() > MAX_SSE_BUFFER {
        buf.clear();
    }
    buf.extend_from_slice(chunk);
}

/// 定位 SSE 帧结束位置（首个空行 "\n\n"，含末尾分隔符）。
pub(super) fn sse_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2)
}

/// C-17：失败载荷中的未达成判据 → 单行摘要（type/expect/实际值）。
/// 只列前 3 条，其余以「等 N 项」收口，保证错误行可读。
fn unmet_items(f: &serde_json::Value) -> Option<String> {
    let items = f
        .get(gen::AIRY_RS_K_VERIFICATION)?
        .get("items")?
        .as_array()?;
    let mut unmet: Vec<String> = Vec::new();
    let mut total = 0usize;
    for it in items {
        if it.get("pass").and_then(|v| v.as_bool()) != Some(false) {
            continue;
        }
        total += 1;
        if unmet.len() >= 3 {
            continue;
        }
        let ty = it.get("type").and_then(|v| v.as_str()).unwrap_or("?");
        let expect = it.get("expect").and_then(|v| v.as_str()).unwrap_or("");
        let actual = it.get("actual").and_then(|v| v.as_str()).unwrap_or("");
        unmet.push(format!("{ty} {expect}（实际 {actual}）"));
    }
    if unmet.is_empty() {
        return None;
    }
    let mut out = unmet.join("、");
    if total > unmet.len() {
        out.push_str(&format!("等 {total} 项"));
    }
    Some(out)
}

/// 失败载荷里的稳定原因码；摘要展示与成因归类共用这一处提取。
fn fail_reason(fail: Option<&serde_json::Value>) -> Option<String> {
    fail?
        .get(gen::AIRY_RS_K_REASON)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// D-10：失败载荷 → 一行用户可读结论。判据（origin/reason/next_action）
/// 由 daemon 唯一给出，这里只做中文呈现；未知原因原样透出，避免新原因
/// 上线后被吞掉。
fn fail_text(fail: Option<&serde_json::Value>, msg: &str) -> String {
    let Some(f) = fail else {
        return msg.to_string();
    };
    let reason = fail_reason(fail).unwrap_or_default();
    let name = match reason.as_str() {
        gen::AIRY_RS_REASON_BUDGET => "轮次预算耗尽",
        gen::AIRY_RS_REASON_TOOL => "工具连续失败已熔断",
        gen::AIRY_RS_REASON_DENIED => "操作未获审批",
        gen::AIRY_RS_REASON_EMPTY => "模型返回空结果",
        gen::AIRY_RS_REASON_PROVIDER => "模型服务不可达",
        gen::AIRY_RS_REASON_PROVIDER_ERR => "模型服务拒绝了请求（检查 API Key / 模型名 / 配额）",
        gen::AIRY_RS_REASON_CONFIG => "未配置可用模型",
        gen::AIRY_RS_REASON_UNMET => "完成判据未达成",
        gen::AIRY_RS_REASON_UNVERIFIED => "完成判据无法取证",
        gen::AIRY_RS_REASON_AGENT => "执行失败",
        "" => return msg.to_string(),
        other => other,
    };
    let hint = match f
        .get(gen::AIRY_RS_K_NEXT_ACTION)
        .and_then(|v| v.as_str())
        .unwrap_or("")
    {
        gen::AIRY_RS_NEXT_RETRY => "可原样重试",
        gen::AIRY_RS_NEXT_RESUME => "请用 continue 续跑，不要原样重发",
        gen::AIRY_RS_NEXT_ADJUST => "请按未达成项调整后重试",
        _ => "建议人工排查",
    };
    let mut out = format!("失败原因：{name}；下一步：{hint}");
    if let Some(items) = unmet_items(f) {
        out.push_str("；未达成：");
        out.push_str(&items);
    }
    let used = f.get(gen::AIRY_RS_K_BUDGET_USED).and_then(|v| v.as_u64());
    let total = f.get(gen::AIRY_RS_K_BUDGET_TOTAL).and_then(|v| v.as_u64());
    if let (Some(u), Some(t)) = (used, total) {
        out.push_str(&format!("（预算 {u}/{t}）"));
    }
    if let Some(n) = f
        .get(gen::AIRY_RS_K_NODE)
        .and_then(|v| v.as_str())
        .filter(|n| !n.is_empty())
    {
        out.push_str(&format!("（影响节点 {n}）"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{parse_retry_ms, Unavailable};
    use agentrt_rs::error::AgentOSError;
    use agentrt_rs::CODE_UNAVAILABLE;

    #[test]
    fn retry_hint_is_parsed_from_gateway_body() {
        let body = r#"{"error":{"code":-32000,"data":{"code":"UNAVAILABLE","retry_after_ms":1500}}}"#;
        let e = AgentOSError::with_code(CODE_UNAVAILABLE, body);
        let u = Unavailable::from_error(&e);
        assert_eq!(u.retry_ms, 1500);
        assert!(u.to_string().contains("1500ms"), "display must show hint");
    }

    #[test]
    fn retry_hint_defaults_when_absent() {
        let e = AgentOSError::with_code(CODE_UNAVAILABLE, "down");
        assert_eq!(Unavailable::from_error(&e).retry_ms, 800);
        assert_eq!(parse_retry_ms("no hint here"), None);
        assert_eq!(parse_retry_ms(r#""retry_after_ms":"x""#), None);
    }
}
