// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 富内容 golden（0.1.21 X4-07b）：以真实渲染路径断言代码高亮与折行组合。
//
// golden 断言的是用户真正看到的帧缓冲：除文本外还读取单元格前景色，因为
// 「跨折行 token 续色」正是样式层面的性质，纯文本快照看不出断层。

use super::*;
use crate::app::MessageRole;
use crate::engine::grid;
use crate::style;

/// 家庭 ZWJ 序列：三个字符靠零宽连接符合成一个字形。
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
/// 挥手 + 肤色修饰符：修饰符必须随基字符同段。
const WAVE: &str = "\u{1F44B}\u{1F3FD}";
/// 键帽序列：变体选择符 + 组合用围框键帽，显示宽度为 2。
const KEYCAP: &str = "\u{0031}\u{FE0F}\u{20E3}";

/// 一份富内容样本：名称（同时用作临时目录前缀）、正文、必须完整出现的片段。
///
/// 样本是数据而非期待文本：新增脚本、新增语言或新增字形组合时只追加条目，
/// 断言逻辑不变。片段要求出现在同一行内，等于同时守住「簇不被拆行」。
struct Sample {
    name: &'static str,
    body: &'static str,
    must: &'static [&'static str],
}

/// X4-08a 样本库：emoji / ZWJ / 全角，覆盖行内与代码块两条装配路径。
const SAMPLES: &[Sample] = &[
    Sample {
        name: "rich-inline",
        body: "家庭：\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} 挥手：\u{1F44B}\u{1F3FD} 键帽：\u{0031}\u{FE0F}\u{20E3}",
        must: &[FAMILY, WAVE, KEYCAP],
    },
    Sample {
        name: "rich-fullwidth",
        body: "全角标点：（测试）、「引用」——破折号 全角字母：ＡＢＣ１２３",
        must: &["全角标点", "ＡＢＣ１２３"],
    },
    Sample {
        name: "rich-code",
        body: "```\n// \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{1F44B}\u{1F3FD}\nlet s = \"ＡＢＣ\";\n```",
        must: &[FAMILY, WAVE, "ＡＢＣ"],
    },
];

/// X4-07b：不可断的长字符串 token 折行后，每段都保持字符串色。
#[test]
fn code_token_color_continues_across_wrap() {
    let mut fx = Fixture::new("rich-code-wrap");
    let payload = "x".repeat(180);
    add(
        &mut fx.app,
        MessageRole::Agent,
        &format!("```rust\nlet s = \"{payload}\";\n```"),
    );
    let buf = frame_buffer(&mut fx.app, W, H);
    let success = style::success();

    let (mut total, mut rows) = (0usize, 0usize);
    for y in 0..H {
        let mut has_x = false;
        for x in 0..W {
            let cell = &buf[(x, y)];
            if cell.symbol() == "x" {
                total += 1;
                has_x = true;
                assert_eq!(
                    cell.fg,
                    success,
                    "token 跨折行续色断裂 @({x},{y})，屏幕:\n{}",
                    rows_of(&buf, W, H).join("\n")
                );
            }
        }
        if has_x {
            rows += 1;
        }
    }
    assert_eq!(total, 180, "折行不得丢字符");
    assert!(rows >= 2, "180 列字符串应在屏上折成多行，实际 {rows} 行");
}

/// X4-08a：样本库逐条走真实渲染，簇完整、行宽合规。
#[test]
fn rich_samples_render_losslessly() {
    for sample in SAMPLES {
        let mut fx = Fixture::new(sample.name);
        add(&mut fx.app, MessageRole::Agent, sample.body);
        let screen = fx.draw();
        for needle in sample.must {
            assert_has(&screen, needle);
        }
        for row in screen.rows() {
            assert!(
                grid::width(row) <= W as usize,
                "[{}] 行宽越界: {row:?}",
                sample.name
            );
        }
    }
}
