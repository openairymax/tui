// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 观察面查询帧辅助（D-15）：面板读经事件管道 kind=query 取值，应答按 id
// 暂存、由各 poll_* 摘取后自行判读。本模块只管收发与记忆/技能镜像水合，
// 不含任何面板语义。

use super::page::PAGE_LIMIT;
use super::*;

use crate::memory::HydrateLog;

/// 观察面查询通道标签（§33.2 #4）：查询帧只走事件管道（D-15），日志据此
/// 标注来源。将来若增设备用通道，各通道须填自己的标签，不得沿用本值。
const MEM_CHANNEL: &str = "event-pipe";

/// 水合查询帧标识（应答按 id 配对）。
const MEM_HYDRATE_ID: &str = "mem.hydrate";

impl App {
    /// 下发一帧观察面查询（D-15 扩展，kind=query）。
    ///
    /// 面板读只经事件管道取值，不自建 HTTP、不持有网关客户端；应答按 id
    /// 暂存，由各 poll_* 用 `take_query` 摘取。管道不可用即如实报错。
    pub(super) fn send_query(
        &mut self,
        id: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(ev) = self.ev.as_ref() else {
            return Err("事件管道不可用".to_string());
        };
        ev.query(id, method, params).map_err(|e| e.to_string())
    }

    /// 摘取一条查询应答：Ok(原始 result) / Err(错误文本)；None = 尚未应答。
    pub(super) fn take_query(&self, id: &str) -> Option<Result<serde_json::Value, String>> {
        let v = self.ev.as_ref()?.take_query(id)?;
        Some(query_reply(v))
    }

    /// 事件管道是否仍在（查询在途判定用）。管道已退出即在途请求永远不会应答。
    pub(super) fn pipe_alive(&self) -> bool {
        self.ev.as_ref().is_some_and(|ev| ev.alive())
    }

    /// 记忆/技能镜像水合：按 tab 的会话标识逐段拉 `mem.recent` 首页，末位
    /// 补一条全局窗口（技能镜像与无会话归属的记录只在这条里出现）。
    ///
    /// 启动（attach_lane 之后）与面板打开时调用；在途时跳过。管道不可用
    /// 不阻塞历史还原闸门——镜像保持现状并置就绪（离线可用）。
    ///
    /// P1-3 分段 + X8-01~03 分页：每段只取一页（≤ PAGE_LIMIT 条），首屏
    /// 快速就绪；更早的记录由 `request_older_history` 在滚到顶部时按
    /// 游标续取。一次取满千条会把应答撑到数百 KB 被网关截断（M-01/02）。
    pub(super) fn refresh_mem_mirrors(&mut self) {
        if self.mem_hydrate_q {
            return;
        }
        self.mem_pages.clear();
        let mut queue: Vec<(String, i64)> = self.conv_ids().into_iter().map(|c| (c, -1)).collect();
        queue.push((String::new(), -1));
        self.mem_hydrate_queue = queue;
        self.send_next_mem_hydrate();
    }

    /// 滚到顶部时续取更早一页（X8-03a）：只对当前会话、游标未到头且无
    /// 在途请求时下发；失败不置状态，下次滚到顶会以同一游标重试。
    pub(super) fn request_older_history(&mut self) {
        if self.mem_hydrate_q || !self.pipe_alive() {
            return;
        }
        let Some(conv) = self.active_conv() else {
            return;
        };
        if self.mem_pages.is_done(&conv) {
            return;
        }
        let Some(before) = self.mem_pages.older(&conv) else {
            return;
        };
        let params = serde_json::json!({
            "limit": PAGE_LIMIT,
            "before": before,
            "conv": conv.clone(),
        });
        match self.send_query(MEM_HYDRATE_ID, "mem.recent", &params) {
            Ok(()) => {
                self.mem_hydrate_q = true;
                self.mem_hydrate_cur = conv;
                self.mem_hydrate_before = before;
                self.mem_hydrate_at = Some(std::time::Instant::now());
            }
            Err(e) => log::warn!(
                "memory: page channel={MEM_CHANNEL} id={MEM_HYDRATE_ID} conv={conv:?} \
                 before={before} result=dispatch-failed err={e}"
            ),
        }
    }

    /// 一次水合的传输事实（§33.2 #4）：耗时从成功下发时刻起算，未记时刻
    /// 记 0（下发即失败、无在途可言）。
    fn mem_hydrate_log<'a>(&'a self, conv: Option<&'a str>) -> HydrateLog<'a> {
        HydrateLog {
            channel: MEM_CHANNEL,
            id: MEM_HYDRATE_ID,
            conv,
            elapsed_ms: self.mem_elapsed_ms(),
        }
    }

    /// 在途水合已等待的毫秒数。
    fn mem_elapsed_ms(&self) -> u128 {
        self.mem_hydrate_at
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0)
    }

    /// 派发队列中的下一段水合；队列排空即收口（置就绪闸门）。
    fn send_next_mem_hydrate(&mut self) {
        if self.mem_hydrate_queue.is_empty() {
            self.mem_hydrate_q = false;
            self.mem_hydrate_cur.clear();
            self.mem_hydrate_before = -1;
            self.mem_hydrate_at = None;
            // 收口：多次分段水合到此结束，历史还原闸门此刻才放行。空结果
            // 只置就绪、不记水合日志——各分段的落地日志已分别记录。
            self.memory
                .ingest_recent(&serde_json::Value::Null, &self.mem_hydrate_log(None));
            self.skills.ingest_recent(&serde_json::Value::Null);
            return;
        }
        let (conv, before) = self.mem_hydrate_queue.remove(0);
        let mut params = serde_json::json!({ "limit": PAGE_LIMIT });
        if before >= 0 {
            params["before"] = serde_json::Value::from(before);
        }
        if !conv.is_empty() {
            params["conv"] = serde_json::Value::String(conv.clone());
        }
        match self.send_query(MEM_HYDRATE_ID, "mem.recent", &params) {
            Ok(()) => {
                self.mem_hydrate_q = true;
                self.mem_hydrate_cur = conv;
                self.mem_hydrate_before = before;
                self.mem_hydrate_at = Some(std::time::Instant::now());
            }
            Err(e) => {
                log::warn!(
                    "memory: hydrate channel={MEM_CHANNEL} id={MEM_HYDRATE_ID} conv={conv:?} \
                     elapsed_ms={} result=dispatch-failed err={e}",
                    self.mem_elapsed_ms()
                );
                self.mem_hydrate_queue.clear();
                self.mem_hydrate_q = false;
                self.mem_hydrate_cur.clear();
                self.mem_hydrate_before = -1;
                self.mem_hydrate_at = None;
                self.memory
                    .ingest_recent(&serde_json::Value::Null, &self.mem_hydrate_log(None));
                self.skills.ingest_recent(&serde_json::Value::Null);
            }
        }
    }

    /// 水合应答落地（主循环每帧调用，无在途时早退）。
    pub fn poll_mem_mirror(&mut self) {
        if !self.mem_hydrate_q {
            return;
        }
        let conv = self.mem_hydrate_cur.clone();
        let before = self.mem_hydrate_before;
        let reply = self.take_query(MEM_HYDRATE_ID);
        let scope = if conv.is_empty() {
            None
        } else {
            Some(conv.as_str())
        };
        let landed = match reply {
            Some(Ok(v)) => {
                let at = self.mem_hydrate_log(scope);
                self.memory.ingest_recent_page(&v, &at, before < 0);
                if conv.is_empty() {
                    self.skills.ingest_recent(&v);
                }
                if !conv.is_empty() {
                    let next = field_i64(&v, "next").unwrap_or(0);
                    let more = v.get("has_more").and_then(|m| m.as_bool()).unwrap_or(false);
                    self.mem_pages.note(&conv, before, next, more);
                    log::debug!(
                        "memory: page conv={conv:?} before={before} next={next} \
                         has_more={more} pages={} spans={} gaps={}",
                        self.mem_pages.pages(&conv),
                        self.mem_pages.merged(&conv),
                        self.mem_pages.gaps(&conv).len()
                    );
                    if before >= 0 {
                        self.prepend_older_page(&conv);
                    }
                }
                true
            }
            Some(Err(e)) => {
                log::warn!(
                    "memory: hydrate channel={MEM_CHANNEL} id={MEM_HYDRATE_ID} conv={conv:?} \
                     elapsed_ms={} result=rejected err={e}",
                    self.mem_elapsed_ms()
                );
                false
            }
            None if !self.pipe_alive() => {
                log::warn!(
                    "memory: hydrate channel={MEM_CHANNEL} id={MEM_HYDRATE_ID} conv={conv:?} \
                     elapsed_ms={} result=offline reason=pipe-closed",
                    self.mem_elapsed_ms()
                );
                false
            }
            None => return,
        };
        if !landed {
            // 后端拒绝/管道退出：放弃剩余分段，直接收口（离线可用）。
            self.mem_hydrate_queue.clear();
        }
        self.mem_hydrate_q = false;
        self.mem_hydrate_before = -1;
        self.send_next_mem_hydrate();
    }

    /// 工具目录加载（D-11）：tool.list 查询帧，管道就绪后一次。
    ///
    /// 目录是增强信息，失败只记日志：调用方按原始 tool_id 呈现，不隐藏行。
    /// 管道不可用时不置在途标记，下次挂接可重试。
    pub(super) fn refresh_toolcat(&mut self) {
        if self.toolcat_q {
            return;
        }
        match self.send_query("toolcat.list", "tool.list", &serde_json::json!({})) {
            Ok(()) => self.toolcat_q = true,
            Err(e) => log::warn!("toolcat: tool.list 下发失败：{e}"),
        }
    }

    /// 工具目录应答落地（主循环每帧调用，无在途时早退）。
    pub fn poll_toolcat(&mut self) {
        if !self.toolcat_q {
            return;
        }
        match self.take_query("toolcat.list") {
            Some(Ok(v)) => crate::app::toolcat::absorb(&v),
            Some(Err(e)) => log::warn!("toolcat: tool.list 失败：{e}"),
            None if !self.pipe_alive() => log::warn!("toolcat: tool.list 未应答（事件管道已退出）"),
            None => return,
        }
        self.toolcat_q = false;
    }
}

/// 取整数应答字段：cJSON 输出整数，但网关转码后可能成浮点，两种都收。
fn field_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    let n = v.get(key)?;
    n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))
}

/// query_result 载荷 → 结果或错误文本（契约见 cli_ev.h）。
fn query_reply(v: serde_json::Value) -> Result<serde_json::Value, String> {
    if v.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false) {
        Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        Err(v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("查询失败")
            .to_string())
    }
}
