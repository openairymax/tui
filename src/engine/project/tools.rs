// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 工具视图表（0.1.21 X1-13b，§23.1.5）。
//
// 工具面板要回答两件事：这次运行能用什么、正在跑什么。前者是目录计数，
// 后者是一串调用行。目录只记服务端给的数，不拿本地注册表去补——本地表
// 与服务端不一致时，界面显示的应是服务端事实，不是客户端的期望。
//
// 调用与结果靠 tool_id 对齐：tool_end 不带工具名，名字在 tool_start 那一行
// 上，结果回来只改该行的收尾字段。找不到对应调用的结果单独成行（名字留空）
// 而不是丢弃：那说明结果先到或上游漏发开始帧，面板应该显示这条异常，而不
// 是装作没发生。缺 tool_id 的调用无法对齐，逐条成行、互不覆盖，宁可多一行
// 也不要让两次调用挤进同一行。
//
// 行是就地收尾的：调用开始建行，结果只把状态与摘录填进同一行，不追加第二
// 行——同一次调用在面板上就该占一行。重传按帧序号去重，已收尾的行不再接受
// 第二次结果：一次调用只有一个终局，后来的"另一个结果"是链路重复或上游
// 缺陷，不该让这一行在两个终局之间来回跳。
//
// `epoch` 是渲染层的变更计数：工具表没有行号脏区，面板比较前后计数即可。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

use crate::engine::event::model::{Body, Event, Execution};

/// 调用行的收尾状态：开始即运行中，结果到达后不再回退。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Running,
    Ok,
    Failed,
}

/// 一行调用：身份 + 工具名 + 参数摘录 + 收尾状态 + 帧锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: String,
    pub(crate) status: Status,
    pub(crate) text: String,
    pub(crate) seq: u64,
    pub(crate) ts: u64,
}

impl Row {
    /// 是否已收尾。
    pub(crate) fn done(&self) -> bool {
        self.status != Status::Running
    }
}

/// 工具表：目录计数 + 按到达顺序排列的调用行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tools {
    catalog: Option<usize>,
    rows: Vec<Row>,
    epoch: u64,
}

impl Tools {
    /// 空表：未收目录、未有调用。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 折叠一条事件：只认执行面三类帧，其余原样放过。
    pub(crate) fn apply(&mut self, ev: &Event) -> bool {
        let Body::Execution(exe) = &ev.body else {
            return false;
        };
        match exe {
            Execution::Tools { count } => self.set_catalog(*count),
            Execution::Call { id, name, args } => {
                self.rows.push(Row {
                    id: id.clone(),
                    name: name.clone(),
                    args: preview(args),
                    status: Status::Running,
                    text: String::new(),
                    seq: ev.seq,
                    ts: ev.ts,
                });
                self.bump();
                true
            }
            Execution::Result { id, ok, text } => self.close(ev, id, *ok, text),
        }
    }

    /// 目录计数只随真实变化更新：重复的目录帧不算变更。
    fn set_catalog(&mut self, count: usize) -> bool {
        if self.catalog == Some(count) {
            return false;
        }
        self.catalog = Some(count);
        self.bump();
        true
    }

    /// 结果收尾：对齐最近一条同号未收尾的行；对不上就单独成行。
    ///
    /// 同号行已收尾时的结果帧不是孤儿：它是重传或迟到的二次结论，按「终态
    /// 不改写」丢弃。只有从未见过的调用号才单独成行——显示异常但不伪造行。
    fn close(&mut self, ev: &Event, id: &str, ok: bool, text: &str) -> bool {
        let status = if ok { Status::Ok } else { Status::Failed };
        let found = self
            .rows
            .iter()
            .rposition(|r| !r.done() && r.id == id && !id.is_empty());
        let Some(idx) = found else {
            if !id.is_empty() && self.rows.iter().any(|r| r.id == id) {
                return false;
            }
            self.rows.push(Row {
                id: id.to_owned(),
                name: String::new(),
                args: String::new(),
                status,
                text: text.to_owned(),
                seq: ev.seq,
                ts: ev.ts,
            });
            self.bump();
            return true;
        };
        let row = &mut self.rows[idx];
        if stale(ev.seq, row.seq) {
            return false;
        }
        row.status = status;
        row.text = text.to_owned();
        row.seq = ev.seq;
        row.ts = ev.ts;
        self.bump();
        true
    }

    /// 目录计数；未收到目录帧时为 None（与"零个工具"区分）。
    pub(crate) fn catalog(&self) -> Option<usize> {
        self.catalog
    }

    /// 变更计数：每次真实改动加一，渲染层比较前后值。
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    fn bump(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    /// 行数。
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// 是否空表。
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 全表只读视图（行序即到达顺序）。
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// 参数摘录：字符串原样取用，其余按 JSON 文本呈现，null 视为空。
fn preview(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 帧序号是否落在该行水位之前；序号 0 表示未编号，不参与去重。
fn stale(seq: u64, mark: u64) -> bool {
    seq != 0 && seq <= mark
}

#[cfg(test)]
mod tests {
    use super::{Status, Tools};
    use crate::engine::event::model::{Body, Cognition, Event, Execution};
    use serde_json::json;

    fn ev(seq: u64, exe: Execution) -> Event {
        Event {
            seq,
            ts: 1_000 + seq,
            body: Body::Execution(exe),
        }
    }

    fn call(seq: u64, id: &str, name: &str) -> Event {
        ev(
            seq,
            Execution::Call {
                id: id.into(),
                name: name.into(),
                args: json!("{\"cmd\":\"ls\"}"),
            },
        )
    }

    fn done(seq: u64, id: &str, ok: bool) -> Event {
        ev(
            seq,
            Execution::Result {
                id: id.into(),
                ok,
                text: if ok { "ok: 3 files" } else { "denied" }.into(),
            },
        )
    }

    /// 目录计数只随真实变化计一次变更。
    #[test]
    fn catalog_tracks_changes() {
        let mut t = Tools::new();
        assert_eq!(t.catalog(), None, "未收到目录帧与零个工具不同");
        assert!(t.apply(&ev(1, Execution::Tools { count: 7 })));
        assert_eq!(t.catalog(), Some(7));
        assert!(!t.apply(&ev(2, Execution::Tools { count: 7 })));
        assert!(t.apply(&ev(3, Execution::Tools { count: 9 })));
        assert_eq!((t.catalog(), t.epoch()), (Some(9), 2));
    }

    /// 调用开始建一行运行中；参数摘录取原文。
    #[test]
    fn call_appends_running_row() {
        let mut t = Tools::new();
        assert!(t.apply(&call(1, "t1", "shell")));
        let row = &t.rows()[0];
        assert_eq!(
            (row.id.as_str(), row.name.as_str(), row.status),
            ("t1", "shell", Status::Running)
        );
        assert_eq!(row.args, "{\"cmd\":\"ls\"}");
        assert!(!row.done());
        assert_eq!(t.len(), 1);
    }

    /// 结果收尾同一行：不追加第二行，名字仍在。
    #[test]
    fn result_closes_matching_call() {
        let mut t = Tools::new();
        t.apply(&call(1, "t1", "shell"));
        assert!(t.apply(&done(2, "t1", true)));
        assert_eq!(t.len(), 1, "一次调用只占一行");
        let row = &t.rows()[0];
        assert_eq!((row.name.as_str(), row.status), ("shell", Status::Ok));
        assert_eq!((row.text.as_str(), row.seq), ("ok: 3 files", 2));
        assert!(row.done());
    }

    /// 找不到开始帧的结果单独成行：显示异常，不装作没发生。
    #[test]
    fn orphan_result_gets_own_row() {
        let mut t = Tools::new();
        assert!(t.apply(&done(1, "t9", false)));
        assert_eq!(t.len(), 1);
        let row = &t.rows()[0];
        assert_eq!((row.id.as_str(), row.name.as_str()), ("t9", ""));
        assert_eq!((row.status, row.text.as_str()), (Status::Failed, "denied"));
    }

    /// 缺 tool_id 的调用逐条成行，结果不误配到这些行上。
    #[test]
    fn unnamed_calls_stay_apart() {
        let mut t = Tools::new();
        t.apply(&call(1, "", "read"));
        t.apply(&call(2, "", "read"));
        assert_eq!(t.len(), 2);
        t.apply(&ev(
            3,
            Execution::Result {
                id: String::new(),
                ok: true,
                text: "ok".into(),
            },
        ));
        assert_eq!(t.len(), 3, "无号结果无法对齐，自成一行的异常");
    }

    /// 重传与二次结果都不改动已收尾的行。
    #[test]
    fn terminal_row_is_stable() {
        let mut t = Tools::new();
        t.apply(&call(1, "t1", "shell"));
        t.apply(&done(2, "t1", true));
        assert!(!t.apply(&done(2, "t1", true)), "同号重放");
        assert!(!t.apply(&done(3, "t1", false)), "已收尾不再翻案");
        assert_eq!(t.len(), 1);
        assert_eq!(t.rows()[0].status, Status::Ok);
        assert_eq!(t.epoch(), 2);
    }

    /// 旧号结果不把新号的调用行拉回去。
    #[test]
    fn stale_result_ignored() {
        let mut t = Tools::new();
        t.apply(&call(5, "t1", "shell"));
        assert!(!t.apply(&done(4, "t1", true)));
        assert_eq!(t.rows()[0].status, Status::Running);
    }

    /// 非执行面事件不触碰工具表。
    #[test]
    fn ignores_other_domains() {
        let mut t = Tools::new();
        let other = Event {
            seq: 1,
            ts: 1,
            body: Body::Cognition(Cognition::Delta { text: "x".into() }),
        };
        assert!(!t.apply(&other));
        assert!(t.is_empty());
        assert_eq!((t.catalog(), t.epoch()), (None, 0));
    }
}
