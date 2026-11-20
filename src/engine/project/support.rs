// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 纯函数断言框架（0.1.21 X1-10b，§23.1.5 / §23.4.3）。
//
// 投影的纪律只有一条：同一段事件流折出的快照必须逐字段相同。此后每张卡
// 接入新表（消息/任务/工具/审批）都用同一套断言，防止某次增量路径里混进
// 时钟、文件或全局态，让 golden 回放在换机器后悄悄漂移。
//
// 三条断言各自堵一个漏洞：
//   pure    同一输入折叠两次，结果必须相等（无随机、无时钟）；
//   isolated  两个投影交替折叠互不影响（无全局共享态）；
//   intact  折叠不修改输入事件（下游可安全重放同一段流）。
//
// 审批另有第四条：approvals 用原始事件流做一份独立的「应当如此」推算，逐行
// 对比表的结论。表本身是增量更新的，推算只读流，两边同时错的可能性远小于
// 单边漂移；这也是 IS-35「界面结论必须能由事件流解释」的可执行形式。
//
// 仅测试期编译：生产构建里不存在这套辅助，避免测试设施反哺运行路径。

use super::approvals::Verdict;
use super::Projection;
use crate::engine::event::model::{Body, Control, Event};

/// 回放一段事件流，得到折叠结果（各卡 golden 用例的共同入口）。
pub(crate) fn replay(events: &[Event]) -> Projection {
    let mut p = Projection::new();
    for ev in events {
        p.apply(ev);
    }
    p
}

/// 同一输入两次折叠必须一致。
pub(crate) fn assert_pure(events: &[Event]) {
    assert_eq!(replay(events), replay(events), "同一事件流折叠不唯一");
}

/// 交替折叠不得互相污染：两段流交错折叠后，各自等于独占回放。
pub(crate) fn assert_isolated(a: &[Event], b: &[Event]) {
    let (mut pa, mut pb) = (Projection::new(), Projection::new());
    for i in 0..a.len().max(b.len()) {
        if let Some(ev) = a.get(i) {
            pa.apply(ev);
        }
        if let Some(ev) = b.get(i) {
            pb.apply(ev);
        }
    }
    assert_eq!(pa, replay(a), "投影之间存在共享态");
    assert_eq!(pb, replay(b), "投影之间存在共享态");
}

/// 折叠不得修改输入：事件在 apply 前后的克隆必须相等。
pub(crate) fn assert_intact(events: &[Event]) {
    let mut p = Projection::new();
    for ev in events {
        let before = ev.clone();
        p.apply(ev);
        assert_eq!(*ev, before, "apply 修改了输入事件");
    }
}

/// 审批表与事件流对账：行数、行序与每个结论都必须能由流本身解释。
///
/// 推算只读原始事件（首见请求 → 第二条裁决帧或第一帧越过截止点），不共享
/// 审批表的任何增量状态；两边独立得出同一结论，才算「界面说的是真的」。
pub(crate) fn assert_approvals(events: &[Event], p: &Projection) {
    let asks = asks(events);
    assert_eq!(
        asks.len(),
        p.approvals().len(),
        "审批行数应等于去重后的请求数"
    );
    for (row, (id, asked, deadline, from)) in p.approvals().rows().iter().zip(asks) {
        assert_eq!(row.id, id, "行序应为请求到达顺序");
        assert_eq!(
            (row.asked, row.deadline),
            (asked, deadline),
            "锚点应取自请求帧"
        );
        assert_eq!(
            (row.verdict, row.at),
            settle(events, from, asked, deadline, &id),
            "审批结论应可由事件流解释"
        );
    }
}

/// 首次出现的请求：(凭据, 请求时刻, 截止点, 下标)。
fn asks(events: &[Event]) -> Vec<(String, u64, u64, usize)> {
    let mut out: Vec<(String, u64, u64, usize)> = Vec::new();
    for (idx, ev) in events.iter().enumerate() {
        let Body::Control(Control::Ask { id, timeout_ms, .. }) = &ev.body else {
            continue;
        };
        if id.is_empty() || out.iter().any(|(seen, ..)| seen == id) {
            continue;
        }
        out.push((id.clone(), ev.ts, deadline(ev.ts, *timeout_ms), idx));
    }
    out
}

/// 独立推算一条审批的结论：先看截止点，再看裁决帧，都没有就是未批。
fn settle(events: &[Event], from: usize, asked: u64, until: u64, id: &str) -> (Verdict, u64) {
    for ev in &events[from + 1..] {
        if until != 0 && ev.ts >= until {
            return (Verdict::Denied, ev.ts);
        }
        if let Body::Control(Control::Answer { id: a, ok }) = &ev.body {
            if a == id {
                let v = if *ok {
                    Verdict::Approved
                } else {
                    Verdict::Denied
                };
                return (v, ev.ts);
            }
        }
    }
    (Verdict::Pending, asked)
}

/// 截止点：未设上限时为 0。
fn deadline(asked: u64, wait: u64) -> u64 {
    if wait == 0 {
        0
    } else {
        asked.saturating_add(wait)
    }
}

#[cfg(test)]
mod tests {
    use super::{assert_approvals, assert_intact, assert_isolated, assert_pure, replay};
    use crate::engine::event::model::{Body, Cognition, Control, Event, Level, Outcome};
    use crate::engine::project::approvals::Verdict;

    fn ev(seq: u64, body: Body) -> Event {
        Event {
            seq,
            ts: 1_763_000_000_000 + seq,
            body,
        }
    }

    fn stream() -> Vec<Event> {
        vec![
            ev(
                1,
                Body::Control(Control::State {
                    id: "t1".into(),
                    name: "running".into(),
                }),
            ),
            ev(
                2,
                Body::Control(Control::Notice {
                    level: Level::Info,
                    text: "开始".into(),
                }),
            ),
            ev(
                3,
                Body::Cognition(Cognition::Delta {
                    text: "片段".into(),
                }),
            ),
            ev(
                4,
                Body::Outcome(Outcome::End {
                    status: "completed".into(),
                    data: "{\"status\":\"completed\",\"rounds_used\":2}".into(),
                }),
            ),
        ]
    }

    /// 三条纪律在同一条真实形状的流上同时成立。
    #[test]
    fn stream_fold_is_pure() {
        let s = stream();
        assert_pure(&s);
        assert_intact(&s);
        assert_isolated(&s, &s[..2]);
        assert_eq!(replay(&s).seq(), 4);
    }

    /// 空流折叠出空投影：框架自身不得引入隐式初值。
    #[test]
    fn empty_stream_is_empty() {
        assert_pure(&[]);
        let p = replay(&[]);
        assert_eq!((p.applied(), p.seq()), (0, 0));
    }

    fn at(seq: u64, ts: u64, body: Body) -> Event {
        Event { seq, ts, body }
    }

    fn ask(seq: u64, ts: u64, id: &str, wait: u64) -> Event {
        at(
            seq,
            ts,
            Body::Control(Control::Ask {
                id: id.into(),
                tool: "shell".into(),
                target: "rm -rf build".into(),
                timeout_ms: wait,
            }),
        )
    }

    fn answer(seq: u64, ts: u64, id: &str, ok: bool) -> Event {
        at(
            seq,
            ts,
            Body::Control(Control::Answer { id: id.into(), ok }),
        )
    }

    /// 审批流：批、拒、超时、未批、重传请求、孤立裁决各一条。
    fn approvals() -> Vec<Event> {
        vec![
            ask(1, 1_000, "a1", 100),
            answer(2, 1_050, "a1", true),
            ask(3, 2_000, "a2", 100),
            answer(4, 2_050, "a2", false),
            ask(5, 3_000, "a3", 10),
            at(6, 3_010, Body::Control(Control::Beat { seq: 6 })),
            ask(7, 4_000, "a4", 0),
            ask(8, 4_100, "a1", 100),
            answer(9, 4_200, "a9", true),
        ]
    }

    /// IS-35 前置：审批结论与原始事件流逐行对账，且折叠仍守三条纪律。
    #[test]
    fn approvals_match_stream() {
        let s = approvals();
        let p = replay(&s);
        assert_approvals(&s, &p);
        assert_pure(&s);
        assert_intact(&s);

        let verdict = |id: &str| p.approvals().get(id).map(|r| (r.verdict, r.at));
        assert_eq!(verdict("a1"), Some((Verdict::Approved, 1_050)));
        assert_eq!(verdict("a2"), Some((Verdict::Denied, 2_050)));
        assert_eq!(verdict("a3"), Some((Verdict::Denied, 3_010)));
        assert_eq!(verdict("a4"), Some((Verdict::Pending, 4_000)));
        assert_eq!(p.approvals().len(), 4, "重传请求与孤立裁决不入表");
    }

    /// 对账本身能抓到偏差：拿丢掉一条裁决的流去比，断言必须失败。
    #[test]
    #[should_panic(expected = "审批结论应可由事件流解释")]
    fn mismatch_is_detected() {
        let full = approvals();
        let mut cut = full.clone();
        cut.remove(3);
        assert_approvals(&full, &replay(&cut));
    }
}
