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
// 仅测试期编译：生产构建里不存在这套辅助，避免测试设施反哺运行路径。

use super::Projection;
use crate::engine::event::model::Event;

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

#[cfg(test)]
mod tests {
    use super::{assert_intact, assert_isolated, assert_pure, replay};
    use crate::engine::event::model::{Body, Cognition, Control, Event, Level, Outcome};

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
}
