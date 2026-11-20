// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 消息表（0.1.21 X1-11a，§23.1.5 / IS-24）。
//
// 只追加，不改写：流式增量（token_delta）逐条成行，已入表的行永远保持
// 入表时的样子。历史一旦可改，滚动回看就会看到"同一句话越看越长"的
// 鬼影；append-only 让任意时刻截下的前缀都是当时真实看到的内容。
//
// 与终端宽度的关系在渲染期处理：折行、拼接同轮流、省略号都在视图层按
// 当前宽度现算，表里存原文。宽度变化因此不需要重排整表。
//
// 定稿（message 的 content）作为独立一行追加而不是覆盖增量行：上游可能
// 只发增量、也可能增量后补定稿，两种序列都要能读；"替换占位"是呈现层
// 的规则（同一轮以最后一条 Final 为准），不是数据层的改写。

#![cfg_attr(not(test), allow(dead_code))]

use crate::engine::event::model::{Body, Cognition, Event};

/// 行的语义类别：决定呈现时的分组与样式，不参与本地化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Think,
    Plan,
    Delta,
    Final,
    Review,
}

/// 一行消息：帧号 + 时刻 + 类别 + 原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Msg {
    pub(crate) seq: u64,
    pub(crate) ts: u64,
    pub(crate) kind: Kind,
    pub(crate) text: String,
}

/// 追加式消息表：对外只给只读切片，写入只有 `push` 一个入口。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Msgs {
    rows: Vec<Msg>,
}

impl Msgs {
    /// 空表。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 追加一行，返回其行号；行号在表生命周期内稳定。
    ///
    /// 只认认知事件：其余域与消息表无关，返回 None 而不是记一行空话。
    pub(crate) fn push(&mut self, ev: &Event) -> Option<usize> {
        let Body::Cognition(cog) = &ev.body else {
            return None;
        };
        self.rows.push(Msg {
            seq: ev.seq,
            ts: ev.ts,
            kind: kind(cog),
            text: text(cog),
        });
        Some(self.rows.len() - 1)
    }

    /// 行数。
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// 是否空表。
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 按行号取行（越界 None）。
    pub(crate) fn get(&self, row: usize) -> Option<&Msg> {
        self.rows.get(row)
    }

    /// 最近一行（流式追加的呈现锚点）。
    pub(crate) fn last(&self) -> Option<&Msg> {
        self.rows.last()
    }

    /// 全表只读视图。
    pub(crate) fn rows(&self) -> &[Msg] {
        &self.rows
    }
}

/// 认知事件 → 行类别。
fn kind(cog: &Cognition) -> Kind {
    match cog {
        Cognition::Think { .. } => Kind::Think,
        Cognition::Plan { .. } => Kind::Plan,
        Cognition::Delta { .. } => Kind::Delta,
        Cognition::Final { .. } => Kind::Final,
        Cognition::Review { .. } => Kind::Review,
    }
}

/// 取原文：表里存字面内容，不做任何加工。
fn text(cog: &Cognition) -> String {
    match cog {
        Cognition::Think { text }
        | Cognition::Plan { text }
        | Cognition::Delta { text }
        | Cognition::Final { text }
        | Cognition::Review { text } => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Msgs};
    use crate::engine::event::model::{Body, Cognition, Event};

    fn ev(seq: u64, cog: Cognition) -> Event {
        Event {
            seq,
            ts: 1_000 + seq,
            body: Body::Cognition(cog),
        }
    }

    fn delta(seq: u64, text: &str) -> Event {
        ev(seq, Cognition::Delta { text: text.into() })
    }

    /// 追加语义：行号递增、内容与帧号原样留存。
    #[test]
    fn appends_rows_in_order() {
        let mut t = Msgs::new();
        assert!(t.is_empty());
        assert_eq!(t.push(&delta(1, "你")), Some(0));
        assert_eq!(
            t.push(&ev(
                2,
                Cognition::Final {
                    text: "你好".into()
                }
            )),
            Some(1)
        );

        assert_eq!(t.len(), 2);
        assert_eq!(t.get(0).map(|m| (m.seq, m.kind)), Some((1, Kind::Delta)));
        assert_eq!(t.last().map(|m| (m.seq, m.kind)), Some((2, Kind::Final)));
        assert_eq!(t.rows()[1].text, "你好");
    }

    /// 只增不改：后续追加不影响任何已入表行的内容。
    #[test]
    fn earlier_rows_never_change() {
        let mut t = Msgs::new();
        for i in 1..=3u64 {
            t.push(&ev(
                i,
                Cognition::Think {
                    text: format!("想{i}"),
                },
            ));
        }
        let frozen: Vec<_> = t.rows().to_vec();
        for i in 4..=6u64 {
            t.push(&ev(
                i,
                Cognition::Delta {
                    text: format!("片{i}"),
                },
            ));
        }
        assert_eq!(&t.rows()[..3], frozen.as_slice());
        assert_eq!(t.len(), 6);
    }

    /// 非认知事件不占消息表。
    #[test]
    fn ignores_other_domains() {
        let mut t = Msgs::new();
        let out = Event {
            seq: 9,
            ts: 9,
            body: Body::Control(crate::engine::event::model::Control::State {
                id: "t1".into(),
                name: "running".into(),
            }),
        };
        assert_eq!(t.push(&out), None);
        assert!(t.is_empty());
    }

    /// 五类认知事件各成一行，类别映射一一对应。
    #[test]
    fn maps_every_kind() {
        let cases = [
            (Cognition::Think { text: "a".into() }, Kind::Think),
            (Cognition::Plan { text: "b".into() }, Kind::Plan),
            (Cognition::Delta { text: "c".into() }, Kind::Delta),
            (Cognition::Final { text: "d".into() }, Kind::Final),
            (Cognition::Review { text: "e".into() }, Kind::Review),
        ];
        let mut t = Msgs::new();
        for (i, (c, _)) in cases.iter().enumerate() {
            t.push(&ev(i as u64 + 1, c.clone()));
        }
        let got: Vec<Kind> = t.rows().iter().map(|m| m.kind).collect();
        let want: Vec<Kind> = cases.iter().map(|(_, k)| *k).collect();
        assert_eq!(got, want);
        assert_eq!(t.get(5), None);
    }
}
