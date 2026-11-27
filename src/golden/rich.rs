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
use crate::style;

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
