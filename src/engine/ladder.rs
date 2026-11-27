// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 帧写出降级阶梯（0.1.21 X2-30a，§23.5.2 序 111）。
//
// 终端写通道偶发失败（管道背压、pty 缓冲满、远端链路抖动）时，继续用同
// 一种策略重投只会让同一段字节反复失败，但也不能丢帧——界面会停在旧内容
// 上。阶梯给出四档由强到弱的写出策略，每档只保留「内容必达」这一条底线，
// 逐档放弃可选的增强能力：
//
//   L1 `Fused` 合帧：切片 + 每片自包同步块 + 单次写出（默认档）
//   L2 `Batch` 单写：整帧一次写出，去掉切片与同步包裹
//   L3 `Cell`  逐格：按单元格边界分段，单段失败不再拖垮整帧
//   L4 `Text`  纯文本：逐格且剥掉样式 SGR，只留定位与字符
//
// 级次只降不升。一次降级说明该通道在强策略下已被证伪，回到强策略只会把
// 可感的卡顿再演一遍；后端重建（新进程、新终端）即回到 L1。升档判据是同
// 档连续失败达到阈值，不看错误码——errno 在跨平台 pty 上并不可靠，能观
// 测的事实只有「这段字节有没有写出去」。
//
// 本模块只定义档位词汇与转移关系，不持有 writer、不决定何时转移：判据
// 阈值与重投时机属写出策略，在 `backend` 的 flush 落位。档位名与编码是
// X8-05 逐帧记录的字段值，落盘与聚合接线前在非测试构建下暂无所用，按
// frame.rs 同例放宽 dead_code，接线后此注记随之删除。

#![cfg_attr(not(test), allow(dead_code))]

/// 写出降级档位，判别值即档位序号（0 为默认档）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Ladder {
    #[default]
    Fused,
    Batch,
    Cell,
    Text,
}

impl Ladder {
    /// 档位总数：可枚举性判据与指标编码的取值范围均由此导出。
    pub(crate) const N: usize = 4;

    /// 定长档位表：顺序即由强到弱，供遍历与完整性校验。
    pub(crate) const ALL: [Ladder; Ladder::N] = [
        Ladder::Fused,
        Ladder::Batch,
        Ladder::Cell,
        Ladder::Text,
    ];

    /// 指标编码：随帧记录进入 X8-05 环，离线可判定该帧由哪档写出。
    pub(crate) const fn code(self) -> u8 {
        self as u8
    }

    /// 稳态名字：日志与帧记录使用，改名等于改样本含义。
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Ladder::Fused => "fused",
            Ladder::Batch => "batch",
            Ladder::Cell => "cell",
            Ladder::Text => "text",
        }
    }

    /// 下一档（更弱一档）；最弱档封顶，不再下沉。
    pub(crate) const fn next(self) -> Self {
        match self {
            Ladder::Fused => Ladder::Batch,
            Ladder::Batch => Ladder::Cell,
            Ladder::Cell => Ladder::Text,
            Ladder::Text => Ladder::Text,
        }
    }

    /// 由指标编码还原档位；越界编码视为未知，不猜测档位。
    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Ladder::Fused),
            1 => Some(Ladder::Batch),
            2 => Some(Ladder::Cell),
            3 => Some(Ladder::Text),
            _ => None,
        }
    }

    /// 是否为默认档（未降级）。
    pub(crate) const fn is_fused(self) -> bool {
        matches!(self, Ladder::Fused)
    }
}

#[cfg(test)]
mod tests {
    use super::Ladder;

    /// 四级可枚举：档位表覆盖全部变体，且编码与序号一致（X2-30a 判据）。
    #[test]
    fn ladder_is_enumerable() {
        assert_eq!(Ladder::ALL.len(), Ladder::N);
        for (i, level) in Ladder::ALL.iter().enumerate() {
            assert_eq!(level.code() as usize, i, "编码须与档位序号一致");
            assert_eq!(Ladder::from_code(level.code()), Some(*level));
        }
        // 逐臂穷举、无通配：新增档位而忘登记时此处编译失败。
        for level in Ladder::ALL {
            match level {
                Ladder::Fused | Ladder::Batch | Ladder::Cell | Ladder::Text => {}
            }
        }
    }

    /// 档位只降不升：逐级下沉，最弱档封顶。
    #[test]
    fn next_steps_down_and_saturates() {
        assert_eq!(Ladder::Fused.next(), Ladder::Batch);
        assert_eq!(Ladder::Batch.next(), Ladder::Cell);
        assert_eq!(Ladder::Cell.next(), Ladder::Text);
        assert_eq!(Ladder::Text.next(), Ladder::Text, "最弱档不得再下沉");
        for pair in Ladder::ALL.windows(2) {
            assert!(pair[1].code() > pair[0].code(), "档位编码须严格递增");
        }
    }

    /// 名字是帧记录的字段值，拼写与顺序一并锁定。
    #[test]
    fn names_are_stable_tokens() {
        let names: Vec<&str> = Ladder::ALL.iter().map(|l| l.name()).collect();
        assert_eq!(names, ["fused", "batch", "cell", "text"]);
    }

    /// 越界编码不还原成任何档位：宁可判未知，也不猜一个档位出来。
    #[test]
    fn unknown_code_is_rejected() {
        for code in 4..=u8::MAX {
            assert_eq!(Ladder::from_code(code), None, "code {code} 不应有档位");
        }
    }

    /// 默认档即最强档，未降级判据只依赖它。
    #[test]
    fn fused_is_the_default() {
        assert!(Ladder::default().is_fused());
        assert!(Ladder::ALL[1..].iter().all(|l| !l.is_fused()));
    }
}
