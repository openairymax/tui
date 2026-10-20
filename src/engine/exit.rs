// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 退出码冻结表（0.1.19 §8.2）：CLI/TUI 的进程退出码是脚本与 CI 的公开
// 契约，一经发布不得漂移。本模块只做「枚举 → 数字 + 人话」的穷尽映射，
// TUI 自身不产生退出码，只把上层给出的码翻译成可读文案。
//
// 新增任何码必须同时更新本表与 §8.2 文档；数字不可复用。

/// 冻结退出码。
///
/// 变体是跨版本公开契约（§8.2），构造点分布在 CLI/策略层与 TUI 自退出
/// 路径；TUI 生产侧只会构造其中一部分，故未接线变体保留定义并以
/// `cfg_attr` 抑制 dead_code——契约表本身不得因「当前无人构造」而删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    /// 正常结束。
    Ok,
    /// 命令行用法错误。
    #[cfg_attr(not(test), allow(dead_code))]
    Usage,
    /// 认证失败。
    #[cfg_attr(not(test), allow(dead_code))]
    Auth,
    /// 网络/网关不可达。
    Net,
    /// 权限拒绝。
    #[cfg_attr(not(test), allow(dead_code))]
    Deny,
    /// 内部错误。
    Internal,
    /// 软退出（可重试）。
    #[cfg_attr(not(test), allow(dead_code))]
    Soft,
    /// Ctrl+C 中断。
    SigInt,
    /// 输出管道关闭。
    Pipe,
}

impl Code {
    pub(crate) const fn code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Usage => 2,
            Self::Auth => 3,
            Self::Net => 4,
            Self::Deny => 6,
            Self::Internal => 7,
            Self::Soft => 70,
            Self::SigInt => 130,
            Self::Pipe => 141,
        }
    }

    /// 人话文案（§6.6 #4）：解释「发生了什么」与「下一步怎么做」。
    pub(crate) const fn human(self) -> &'static str {
        match self {
            Self::Ok => "正常结束",
            Self::Usage => "命令行参数有误，请检查用法后重试",
            Self::Auth => "身份认证失败，请重新登录或检查凭据",
            Self::Net => "无法连接网关，请确认 AgentRT 服务已启动",
            Self::Deny => "操作被权限策略拒绝",
            Self::Internal => "内部错误，请保留日志并上报问题",
            Self::Soft => "本次执行未完成，可稍后重试",
            Self::SigInt => "已被用户中断",
            Self::Pipe => "输出管道已关闭，提前结束",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Code;

    const ALL: [Code; 9] = [
        Code::Ok,
        Code::Usage,
        Code::Auth,
        Code::Net,
        Code::Deny,
        Code::Internal,
        Code::Soft,
        Code::SigInt,
        Code::Pipe,
    ];

    #[test]
    fn frozen_numbers_never_drift() {
        let got: Vec<i32> = ALL.iter().map(|c| c.code()).collect();
        assert_eq!(got, vec![0, 2, 3, 4, 6, 7, 70, 130, 141]);
    }

    #[test]
    fn every_code_has_human_text() {
        for code in ALL {
            assert!(!code.human().is_empty());
        }
    }
}
