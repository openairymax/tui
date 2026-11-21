// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 观察面查询帧辅助（D-15）：面板读经事件管道 kind=query 取值，应答按 id
// 暂存、由各 poll_* 摘取后自行判读。本模块只管收发与记忆/技能镜像水合，
// 不含任何面板语义。

use super::*;

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

    /// 记忆/技能镜像水合：一条 mem.recent 同时喂两个镜像（同一后端分区）。
    ///
    /// 启动（attach_lane 之后）与面板打开时调用；在途时跳过。管道不可用
    /// 不阻塞历史还原闸门——镜像保持现状并置就绪（离线可用）。
    pub(super) fn refresh_mem_mirrors(&mut self) {
        if self.mem_hydrate_q {
            return;
        }
        let params = serde_json::json!({ "limit": crate::memory::HYDRATE_LIMIT });
        match self.send_query("mem.hydrate", "mem.recent", &params) {
            Ok(()) => self.mem_hydrate_q = true,
            Err(e) => {
                log::warn!("mem.recent 水合下发失败：{e}");
                self.memory.ingest_recent(&serde_json::Value::Null);
                self.skills.ingest_recent(&serde_json::Value::Null);
            }
        }
    }

    /// 水合应答落地（主循环每帧调用，无在途时早退）。
    pub fn poll_mem_mirror(&mut self) {
        if !self.mem_hydrate_q {
            return;
        }
        let reply = self.take_query("mem.hydrate");
        let landed = match reply {
            Some(Ok(v)) => {
                self.memory.ingest_recent(&v);
                self.skills.ingest_recent(&v);
                true
            }
            Some(Err(e)) => {
                log::warn!("mem.recent 水合失败：{e}");
                false
            }
            None if !self.pipe_alive() => {
                log::warn!("mem.recent 水合未应答（事件管道已退出）");
                false
            }
            None => return,
        };
        if !landed {
            self.memory.ingest_recent(&serde_json::Value::Null);
            self.skills.ingest_recent(&serde_json::Value::Null);
        }
        self.mem_hydrate_q = false;
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
