// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 五件套后三段模板（0.1.21 X9-01b）：回滚 / 评审 / 退役。
//
// 模板不是文档装饰，而是可执行的合入闸门：`Change::render` 对空字段、换行
// 注入、非法开关名一律拒绝；语义改动没有回答「为什么不在 CLI」同样拒绝
// （§23.7 第 5 条分层评审）。这样评审问题在编译期与单测里就被问过一遍，
// 不依赖任何人记得问。
//
// 输出固定三个 `##` 小节、每个字段一行，便于逐行 diff 与 grep；退役段
// 强制写清「开关 + 旧路径 + 登记项同一提交删除」，防止开关长期滞留。

use super::gate;

/// 改动归属：渲染类只需回滚与退役；语义类必须回答为什么不放 CLI。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layer {
    Render,
    Semantic,
}

impl Layer {
    fn label(self) -> &'static str {
        match self {
            Self::Render => "渲染",
            Self::Semantic => "语义",
        }
    }
}

/// 一次实验改动的五件套登记内容；字段均为单行文本。
pub(crate) struct Change<'a> {
    pub(crate) name: &'a str,
    pub(crate) switch: &'a str,
    pub(crate) legacy: &'a str,
    pub(crate) watch: &'a str,
    pub(crate) threshold: &'a str,
    pub(crate) layer: Layer,
    pub(crate) cli_reason: &'a str,
    pub(crate) retire_at: &'a str,
}

impl Change<'_> {
    /// 生成三段模板；返回 Err 时携带首个不合格字段名。
    pub(crate) fn render(&self) -> Result<String, &'static str> {
        self.check()?;
        let key = gate::env_key_of(self.name);
        let reason = self.reason();
        Ok(format!(
            "## 回滚\n\
             - 开关：{name}（{key}）\n\
             - 旧路径：{legacy}\n\
             - 观测：{watch}\n\
             - 触发阈值：{threshold}\n\
             \n\
             ## 评审\n\
             - 归属：{layer}\n\
             - 语义为何不在 CLI：{reason}\n\
             - 分层判据：{layer}\n\
             \n\
             ## 退役\n\
             - 删除范围：开关 + 旧路径 + 登记项，同一提交\n\
             - 退役时点：{retire}\n",
            name = self.name,
            key = key,
            legacy = self.legacy.trim(),
            watch = self.watch.trim(),
            threshold = self.threshold.trim(),
            layer = self.layer.label(),
            reason = reason,
            retire = self.retire_at.trim(),
        ))
    }

    fn check(&self) -> Result<(), &'static str> {
        if !gate::legal(self.name) {
            return Err("name");
        }
        let fields = [
            ("switch", self.switch),
            ("legacy", self.legacy),
            ("watch", self.watch),
            ("threshold", self.threshold),
            ("retire_at", self.retire_at),
        ];
        for (key, val) in fields {
            if !one_line(val) {
                return Err(key);
            }
        }
        if self.layer == Layer::Semantic && !one_line(self.cli_reason) {
            return Err("cli_reason");
        }
        Ok(())
    }

    fn reason(&self) -> &str {
        match self.layer {
            Layer::Render => "不适用（纯渲染改动）",
            Layer::Semantic => self.cli_reason.trim(),
        }
    }
}

/// 单行非空：模板字段不得为空，也不得夹带换行破坏三段结构。
fn one_line(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains(['\n', '\r'])
}

/// 填写范例（X2-25 同步输出）；与模板同源，改模板必须同步改范例。
pub(crate) const EXAMPLE: &str = "\
## 回滚
- 开关：frame_sync（AIRY_TUI_EXP_FRAME_SYNC）
- 旧路径：逐段 write 后 flush
- 观测：ANSI 录制中 2026h/2026l 配对率与单帧 write 次数
- 触发阈值：配对率低于 100% 或单帧 write 超过 1 次即回滚

## 评审
- 归属：渲染
- 语义为何不在 CLI：不适用（纯渲染改动）
- 分层判据：渲染

## 退役
- 删除范围：开关 + 旧路径 + 登记项，同一提交
- 退役时点：配对率连续两版全绿
";

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Change<'static> {
        Change {
            name: "frame_sync",
            switch: "同步输出包裹",
            legacy: "逐段 write 后 flush",
            watch: "ANSI 录制中 2026h/2026l 配对率与单帧 write 次数",
            threshold: "配对率低于 100% 或单帧 write 超过 1 次即回滚",
            layer: Layer::Render,
            cli_reason: "",
            retire_at: "配对率连续两版全绿",
        }
    }

    #[test]
    fn sections_are_exactly_three() {
        let text = example().render().expect("范例必须可渲染");
        let heads: Vec<&str> = text.lines().filter(|l| l.starts_with("## ")).collect();
        assert_eq!(heads, ["## 回滚", "## 评审", "## 退役"]);
    }

    #[test]
    fn example_matches_template() {
        assert_eq!(example().render().expect("范例必须可渲染"), EXAMPLE);
    }

    #[test]
    fn semantic_change_requires_cli_reason() {
        let mut change = example();
        change.layer = Layer::Semantic;
        assert_eq!(change.render(), Err("cli_reason"));
        change.cli_reason = "需要会话态与流式增量";
        assert!(change.render().is_ok());
    }

    #[test]
    fn blank_or_multiline_fields_are_rejected() {
        for (key, val) in [
            ("switch", "  "),
            ("legacy", "两行\n注入"),
            ("watch", "\r回车"),
            ("threshold", ""),
            ("retire_at", "\t"),
        ] {
            let mut change = example();
            match key {
                "switch" => change.switch = val,
                "legacy" => change.legacy = val,
                "watch" => change.watch = val,
                "threshold" => change.threshold = val,
                _ => change.retire_at = val,
            }
            assert_eq!(change.render(), Err(key), "字段 {key} 必须被拒绝");
        }
    }

    #[test]
    fn illegal_name_is_rejected() {
        let mut change = example();
        change.name = "FrameSync";
        assert_eq!(change.render(), Err("name"));
    }
}
