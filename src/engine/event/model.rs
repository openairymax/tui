// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件模型（0.1.21 X1-05a，§23.1.5 事件入口的语义层）。
//
// D-15 的每一帧都携带 `type`，但渲染层不该按字符串各自猜语义：同一类事情
// 若在两个组件里被归成不同类别，状态栏与主区就会各说各话。归域只在本模块
// 做一次，下游只认四域：
//   control   —— 需要人或对端裁决、确认的状态（审批/取消/状态/配置/通知/心跳）
//   cognition —— 思考的中间态（思维链/计划/正文增量与定稿/评审）
//   execution —— 对系统产生的动作（工具目录/调用/结果）
//   outcome   —— 一轮运行的收口（终态/失败/失败详情）
//
// 归域不等于裁剪：载荷字段保留服务端给出的形态（`Value` 原样承接未知部分），
// 归域只决定「谁消费」。字段级映射在 X1-06/X1-07 按域补齐，本卡只冻结形状。

#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

/// 四域标签：状态栏与路由只需这一个判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Domain {
    Control,
    Cognition,
    Execution,
    Outcome,
}

impl Domain {
    /// 稳定短名（日志与诊断用，不参与本地化）。
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Cognition => "cognition",
            Self::Execution => "execution",
            Self::Outcome => "outcome",
        }
    }
}

/// 通知级别：只区分影响呈递的三档，不引入业务语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

/// 一条已归域的事件：信封随行，载荷按域定型。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    pub(crate) ts: u64,
    pub(crate) body: Body,
}

impl Event {
    pub(crate) fn domain(&self) -> Domain {
        match self.body {
            Body::Control(_) => Domain::Control,
            Body::Cognition(_) => Domain::Cognition,
            Body::Execution(_) => Domain::Execution,
            Body::Outcome(_) => Domain::Outcome,
        }
    }
}

/// 四域载荷。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Body {
    Control(Control),
    Cognition(Cognition),
    Execution(Execution),
    Outcome(Outcome),
}

/// 控制面：凡是「要有人点头」或「链路自身状态」都归这里。
///
/// 状态迁移与取消都带任务身份：任务表按身份寻址；无身份的状态帧只驱动
/// 全局运行相位，不进任务表（不得凭空造一行没有身份的任务）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Control {
    /// 审批请求：TUI 须以 control_response 回应，超时按拒绝处理。
    ///
    /// `timeout_ms` 是服务端给的等待上限，0 表示不限时；判定以事件自带的
    /// `ts` 为准，投影不读本地时钟。
    Ask {
        id: String,
        tool: String,
        target: String,
        timeout_ms: u64,
    },
    /// 本地裁决：用户对某条审批点了允许或拒绝。
    ///
    /// 该帧只存在于本地事件流（control_response 是上行帧，不会回灌），
    /// 但也必须成事件入流，否则回放看不出当时批过什么。
    Answer { id: String, ok: bool },
    /// 取消：本地或对端发起的中断。
    Cancel { id: String, reason: String },
    /// 运行状态迁移（idle/running/done/…）；`id` 是任务身份，缺席时为空。
    State { id: String, name: String },
    /// 配置项告知（键 + 原始值）。
    Cfg { key: String, value: Value },
    /// 一般通知：级别 + 文本。
    Notice { level: Level, text: String },
    /// 心跳：携带会话序号，供 TUI 判断链路是否停滞。
    Beat { seq: u64 },
}

/// 认知面：思考过程与正文增量，呈递上互斥于工具执行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cognition {
    /// 思维链增量。
    Think { text: String },
    /// 计划/蓝图（先于动手的那部分）。
    Plan { text: String },
    /// 正文增量。
    Delta { text: String },
    /// 正文定稿：一轮回答的权威全文，替换流式增量在界面上的占位。
    Final { text: String },
    /// 评审意见（对已产出内容的裁定）。
    Review { text: String },
}

/// 执行面：对系统产生的动作，与认知面分开呈递，避免思考混进正文。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Execution {
    /// 工具目录（可用工具数量与名称由后续映射补齐）。
    Tools { count: usize },
    /// 工具调用开始。
    Call {
        id: String,
        name: String,
        args: Value,
    },
    /// 工具结果：成败 + 文本。
    Result { id: String, ok: bool, text: String },
}

/// 收口面：一轮运行只应有一个终态。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    /// 正常终态（含暂停/未验证等非失败收尾）。
    End { status: String },
    /// 失败三件套：机器可读原因 + 一句人话 + 下一步动作。
    Err {
        reason: String,
        text: String,
        next: String,
    },
    /// 运行结果摘要（成功路径的统计性收尾）。
    Result { status: String },
}

#[cfg(test)]
mod tests {
    use super::{Body, Cognition, Control, Domain, Event, Execution, Level, Outcome};
    use serde_json::json;

    fn ev(body: Body) -> Event {
        Event {
            seq: 1,
            ts: 0,
            body,
        }
    }

    /// 四域各至少可构造一例，且域标签互不混淆。
    #[test]
    fn four_domains_construct() {
        let ctl = ev(Body::Control(Control::Beat { seq: 3 }));
        let cog = ev(Body::Cognition(Cognition::Think {
            text: "先看依赖".into(),
        }));
        let exe = ev(Body::Execution(Execution::Call {
            id: "c1".into(),
            name: "read".into(),
            args: json!({"path": "a"}),
        }));
        let out = ev(Body::Outcome(Outcome::End {
            status: "completed".into(),
        }));
        assert_eq!(ctl.domain(), Domain::Control);
        assert_eq!(cog.domain(), Domain::Cognition);
        assert_eq!(exe.domain(), Domain::Execution);
        assert_eq!(out.domain(), Domain::Outcome);
        assert_eq!(
            [ctl, cog, exe, out].map(|e| e.domain().name()),
            ["control", "cognition", "execution", "outcome"]
        );
    }

    /// 控制面全部子类可构造（审批/裁决/取消/状态/配置/通知/心跳）。
    #[test]
    fn control_variants_construct() {
        let cases = [
            Control::Ask {
                id: "a1".into(),
                tool: "shell".into(),
                target: "rm -rf build".into(),
                timeout_ms: 30_000,
            },
            Control::Answer {
                id: "a1".into(),
                ok: true,
            },
            Control::Cancel {
                id: "t1".into(),
                reason: "user".into(),
            },
            Control::State {
                id: "t1".into(),
                name: "running".into(),
            },
            Control::Cfg {
                key: "theme".into(),
                value: json!("dark"),
            },
            Control::Notice {
                level: Level::Warn,
                text: "daemon 重连中".into(),
            },
            Control::Notice {
                level: Level::Error,
                text: "链路已断".into(),
            },
            Control::Beat { seq: 9 },
        ];
        for case in cases {
            assert_eq!(ev(Body::Control(case)).domain(), Domain::Control);
        }
    }

    /// 三个域的其余子类同样可构造，且归域不随子类改变。
    #[test]
    fn remaining_variants_construct() {
        let cog = [
            Cognition::Plan {
                text: "先读依赖".into(),
            },
            Cognition::Delta {
                text: "输出片段".into(),
            },
            Cognition::Final {
                text: "整段回答".into(),
            },
            Cognition::Review {
                text: "评审通过".into(),
            },
        ];
        for case in cog {
            assert_eq!(ev(Body::Cognition(case)).domain(), Domain::Cognition);
        }

        let exe = [
            Execution::Tools { count: 12 },
            Execution::Result {
                id: "c1".into(),
                ok: true,
                text: "ok".into(),
            },
        ];
        for case in exe {
            assert_eq!(ev(Body::Execution(case)).domain(), Domain::Execution);
        }

        let out = [
            Outcome::Err {
                reason: "no_terminal".into(),
                text: "没有终态帧".into(),
                next: "inspect".into(),
            },
            Outcome::Result {
                status: "completed".into(),
            },
        ];
        for case in out {
            assert_eq!(ev(Body::Outcome(case)).domain(), Domain::Outcome);
        }

        assert_eq!(
            ev(Body::Control(Control::Notice {
                level: Level::Info,
                text: "已就绪".into(),
            }))
            .domain(),
            Domain::Control
        );
    }

    /// 载荷保留服务端原样：未知字段不丢、不报错。
    #[test]
    fn payload_keeps_unknown_fields() {
        let args = json!({"path": "a", "extra": [1, 2, 3]});
        let call = Execution::Call {
            id: "c9".into(),
            name: "read".into(),
            args: args.clone(),
        };
        let ev = ev(Body::Execution(call));
        match ev.body {
            Body::Execution(Execution::Call { args: got, .. }) => assert_eq!(got, args),
            other => panic!("域或子类被改判：{other:?}"),
        }
    }
}
