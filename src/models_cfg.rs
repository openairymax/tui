// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

//! model.yaml（v2 表格形式）读写 — 模型连接表 + 双思考系统段。
//!
//! 单一事实来源：`$AIRY_HOME/config/model.yaml`（llm_d / think_d / gateway_d
//! 共同读取）。本模块提供：
//!   - 读：行级容错解析 `models:` 表（每行一个模型）与 `think:` 段；
//!   - 写：就地 patch `models[idx]` 字段、`think:` 段与顶层 `default_model:`
//!     ——仅替换/补插已知键名所在行，保留注释、自定义键与其余模型行（不整
//!     文件重写，避免丢失用户手写内容与格式）。
//!
//! 供 TUI 向导（wizard 模块）与配置面板（panels/config.rs）共用，保证两处
//! 对 model.yaml 的读写口径一致（Unify Design SSoT）。

use std::path::PathBuf;

/// 模型连接表一行（与 model.yaml v2 字段一一对应；高级配置缺省走 llm_d 默认）。
#[derive(Debug, Clone, Default)]
pub struct ModelRow {
    pub name: String,
    pub mode: String,       // api | local
    pub api_format: String, // openai | anthropic
    pub base_url: String,
    pub model_id: String,
    pub api_key_env: String,
    pub context_window: String,
    pub max_output: String,
    pub tool_rounds: String,
    pub vision: String,
    pub thinking: String,
}

impl ModelRow {
    /// 高级配置是否有显式值（决定 patch 时是否写入该键）。
    fn advanced_pairs(&self) -> Vec<(&'static str, &str)> {
        let mut v = Vec::new();
        for (k, val) in [
            ("context_window", &self.context_window),
            ("max_output", &self.max_output),
            ("tool_rounds", &self.tool_rounds),
            ("vision", &self.vision),
            ("thinking", &self.thinking),
        ] {
            if !val.is_empty() {
                v.push((k, val.as_str()));
            }
        }
        v
    }

    /// 基础字段键值对（始终写入）。
    fn base_pairs(&self) -> Vec<(&'static str, &str)> {
        vec![
            ("name", &self.name),
            ("mode", &self.mode),
            ("api_format", &self.api_format),
            ("base_url", &self.base_url),
            ("model_id", &self.model_id),
            ("api_key_env", &self.api_key_env),
        ]
    }
}

/// 双思考系统配置：`slots:` 三角色（t2/t1f/t1p）+ `think:` 开关段。
///
/// C-19 起槽位只引用 `models:` 的 model_id，`think.*_model` 三键废弃
/// （仍可读、写入一律落到 slots）。字段名沿用 slow/fast/prof 以便向导
/// 复用，语义即 t2/t1f/t1p。
#[derive(Debug, Clone, Default)]
pub struct ThinkCfg {
    pub enabled: Option<bool>,
    pub slow_model: String,
    pub fast_model: String,
    pub prof_model: String,
    pub timeout_ms: String,
}

/// model.yaml 解析结果。
#[derive(Debug, Clone, Default)]
pub struct ModelYaml {
    pub rows: Vec<ModelRow>,
    pub default_model: String,
    pub think: Option<ThinkCfg>,
    /// 顶层 `slots:` 原始声明（三值为空 = 回落 default_model）。
    pub slots: Option<SlotsCfg>,
}

/// 顶层 `slots:` 段（角色 → model_id）。
#[derive(Debug, Clone, Default)]
pub struct SlotsCfg {
    pub t2: String,
    pub t1f: String,
    pub t1p: String,
}

/// model.yaml 路径：$AIRY_HOME/config/model.yaml（HOME 回退 ~/.airymaxrt）。
pub fn model_yaml_path() -> PathBuf {
    crate::paths::airy_home_path(&["config", "model.yaml"])
}

/// 读取并解析 model.yaml（文件缺失返回空结构，绝不 panic）。
pub fn read_model_yaml() -> ModelYaml {
    let content = match std::fs::read_to_string(model_yaml_path()) {
        Ok(c) => c,
        Err(_) => return ModelYaml::default(),
    };
    parse_model_yaml(&content)
}

/// 行级解析 model.yaml（仅识别 models 表 / default_model / think 段）。
fn parse_model_yaml(content: &str) -> ModelYaml {
    let mut out = ModelYaml::default();
    let mut cur_row: Option<ModelRow> = None;
    let mut in_think = false;
    let mut in_slots = false;
    let mut in_models = false;

    for raw in content.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 顶层键
        if indent == 0 {
            if let Some(rest) = trimmed.strip_prefix("default_model:") {
                out.default_model = unquote(rest.trim()).to_string();
                in_models = false;
                in_think = false;
                continue;
            }
            if trimmed == "models:" {
                in_models = true;
                in_think = false;
                continue;
            }
            if trimmed == "think:" {
                in_think = true;
                in_models = false;
                in_slots = false;
                out.think = Some(ThinkCfg::default());
                continue;
            }
            if trimmed == "slots:" {
                in_slots = true;
                in_models = false;
                in_think = false;
                out.slots = Some(SlotsCfg::default());
                continue;
            }
            in_models = false;
            in_think = false;
            in_slots = false;
            continue;
        }
        // models 表：列表项 `- name:`（2 空格缩进）
        if in_models && indent == 2 {
            if let Some(rest) = trimmed.strip_prefix("- name:") {
                if let Some(prev) = cur_row.take() {
                    out.rows.push(prev);
                }
                cur_row = Some(ModelRow {
                    name: unquote(rest.trim()).to_string(),
                    ..Default::default()
                });
                continue;
            }
        }
        // models 表字段（4 空格缩进）
        if in_models && indent == 4 {
            if let Some(row) = cur_row.as_mut() {
                if let Some((k, v)) = split_kv(trimmed) {
                    set_row_field(row, k, v);
                }
            }
            continue;
        }
        // think 段字段（2 空格缩进）
        if in_think && indent == 2 {
            if let (Some(cfg), Some((k, v))) = (out.think.as_mut(), split_kv(trimmed)) {
                match k {
                    "enabled" => cfg.enabled = Some(v == "true"),
                    "think2_slow_model" => cfg.slow_model = unquote(v).to_string(),
                    "think1_fast_model" => cfg.fast_model = unquote(v).to_string(),
                    "think1_prof_model" => cfg.prof_model = unquote(v).to_string(),
                    "timeout_ms" => cfg.timeout_ms = unquote(v).to_string(),
                    _ => {}
                }
            }
            continue;
        }
        // slots 段字段（2 空格缩进）：t2/t1f/t1p 只引用 models 的 model_id
        if in_slots && indent == 2 {
            if let (Some(cfg), Some((k, v))) = (out.slots.as_mut(), split_kv(trimmed)) {
                match k {
                    "t2" => cfg.t2 = unquote(v).to_string(),
                    "t1f" => cfg.t1f = unquote(v).to_string(),
                    "t1p" => cfg.t1p = unquote(v).to_string(),
                    _ => {}
                }
            }
            continue;
        }
        // 其他缩进内容：保持解析器状态（如 models 内注释/高级配置）由 set_row_field 兜底
        if in_models && indent >= 4 {
            if let Some(row) = cur_row.as_mut() {
                if let Some((k, v)) = split_kv(trimmed) {
                    set_row_field(row, k, v);
                }
            }
        }
    }
    if let Some(prev) = cur_row.take() {
        out.rows.push(prev);
    }
    // slots 优先：槽位非空即覆盖废弃 think.*_model；无 think 段时合成一份
    // 供向导展示（写入仍走 slots，不回写废弃键）。
    if let Some(slots) = out.slots.clone() {
        let t = out.think.get_or_insert_with(ThinkCfg::default);
        if !slots.t2.is_empty() {
            t.slow_model = slots.t2;
        }
        if !slots.t1f.is_empty() {
            t.fast_model = slots.t1f;
        }
        if !slots.t1p.is_empty() {
            t.prof_model = slots.t1p;
        }
    }
    out
}

/// 模型行字段赋值（含高级配置；cost 字段暂不展示由 llm_d 管理）。
fn set_row_field(row: &mut ModelRow, k: &str, v: &str) {
    let v = unquote(v.trim()).trim().to_string();
    match k {
        "name" => row.name = v,
        "mode" => row.mode = v,
        "api_format" => row.api_format = v,
        "base_url" => row.base_url = v,
        "model_id" => row.model_id = v,
        "api_key_env" => row.api_key_env = v,
        "context_window" => row.context_window = v,
        "max_output" => row.max_output = v,
        "tool_rounds" => row.tool_rounds = v,
        "vision" => row.vision = v,
        "thinking" => row.thinking = v,
        _ => {}
    }
}

/// 就地更新 model.yaml：patch models 表第 idx 行 + think 段。
///
/// 返回错误信息（None = 成功）。若文件不存在，按模板重建最小 v2 文件。
pub fn patch_model_yaml(idx: usize, row: &ModelRow, think: &ThinkCfg) -> Result<(), String> {
    let path = model_yaml_path();
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let patched = if original.is_empty() {
        build_minimal_yaml(row, think)
    } else {
        patch_lines(&original, idx, row, think)
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    std::fs::write(&path, patched).map_err(|e| format!("写入 model.yaml 失败: {}", e))
}

/// 就地更新 model.yaml 顶层 `default_model:`（/model 切换当前对话模型）。
///
/// 保留文件其余内容与注释，仅改一行：无该键时插入到 `think:` 段之前
/// （无 think 段则追加到文件末尾），YAML 顶层键解析不受影响。文件缺失时
/// 生成仅含 default_model 的最小文件。
pub fn set_default_model(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("模型名不能为空".to_string());
    }
    let path = model_yaml_path();
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let patched = patch_default_model(&original, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    std::fs::write(&path, patched).map_err(|e| format!("写入 model.yaml 失败: {}", e))
}

/// 行级替换/插入顶层 `default_model:`（见 `set_default_model`）。
fn patch_default_model(original: &str, name: &str) -> String {
    let mut lines: Vec<String> = if original.is_empty() {
        vec!["# AgentRT 大语言模型配置（default_model 由 TUI /model 维护）".to_string()]
    } else {
        original.lines().map(|l| l.to_string()).collect()
    };
    let mut replaced = false;
    let mut insert_at: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        let trimmed = l.trim_start();
        if l.len() != trimmed.len() {
            continue; // 仅顶层键（无缩进）
        }
        match split_kv(trimmed) {
            Some(("default_model", _)) => {
                lines[i] = format!("default_model: {}", name);
                replaced = true;
                break;
            }
            Some(("think", _)) if insert_at.is_none() => insert_at = Some(i),
            _ => {}
        }
    }
    if !replaced {
        match insert_at {
            Some(i) => lines.insert(i, format!("default_model: {}", name)),
            None => lines.push(format!("default_model: {}", name)),
        }
    }
    lines.join("\n") + "\n"
}

/// 行级 patch：替换/补插 models[idx] 字段与 think 段，保留其余内容。
fn patch_lines(original: &str, idx: usize, row: &ModelRow, think: &ThinkCfg) -> String {
    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());

    // 扫描模型项块范围：第 idx 个 `  - name:` 起，至下一 `  - ` / 顶层键止
    let mut item_starts: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let trimmed = l.trim_start();
        if trimmed.starts_with("- name:") && l.len() - trimmed.len() == 2 {
            item_starts.push(i);
        }
    }

    if idx < item_starts.len() {
        let start = item_starts[idx];
        let end = item_starts.get(idx + 1).copied().unwrap_or(lines.len());
        // 目标块之前的内容原样保留（文件头注释 / 其他顶层键）
        for l in &lines[..start] {
            out.push(l.to_string());
        }
        // 替换模型块：重写 name 行，逐行替换字段，末尾补缺失字段
        let mut block: Vec<String> = Vec::new();
        for (i, l) in lines[start..end].iter().enumerate() {
            let trimmed = l.trim_start();
            let indent = l.len() - trimmed.len();
            if i == 0 {
                block.push(format!("  - name: {}", row.name));
                continue;
            }
            if indent >= 4 {
                if let Some((k, _)) = split_kv(trimmed) {
                    if let Some(v) = row_value(k, row) {
                        block.push(format!("    {}: {}", k, v));
                        continue;
                    }
                }
            }
            // 非目标键行（注释/未知键）原样保留
            block.push(l.to_string());
        }
        // 补插模型块缺失字段
        let existing: Vec<String> = block
            .iter()
            .filter_map(|l| split_kv(l.trim_start()).map(|(k, _)| k.to_string()))
            .collect();
        for (k, v) in row_pairs(row) {
            if !existing.contains(&k.to_string()) {
                block.push(format!("    {}: {}", k, v));
            }
        }
        out.extend(block);
        // 旧 model_id：default_model 指向旧值时同步改写（保持默认模型跟随第一行）
        let old_model_id = lines[start..end]
            .iter()
            .filter_map(|l| split_kv(l.trim_start()))
            .find(|(k, _)| *k == "model_id")
            .map(|(_, v)| unquote(v.trim()).to_string());
        let mut i = end;
        // 之后的内容照抄，直到 slots/think 段（两段统一重写，避免重复键）
        while i < lines.len() {
            let l = lines[i];
            let trimmed = l.trim_start();
            let top = l.len() - trimmed.len() == 0;
            if top && (trimmed == "think:" || trimmed == "slots:") {
                break;
            }
            // default_model 指向被替换的旧 model_id → 跟随新 model_id
            if let (Some(old), Some((k, v))) = (old_model_id.as_deref(), split_kv(trimmed)) {
                if k == "default_model" && unquote(v.trim()) == old {
                    out.push(format!("default_model: {}", row.model_id));
                    i += 1;
                    continue;
                }
            }
            out.push(l.to_string());
            i += 1;
        }
        // 写 slots + think 段（追加或替换）
        write_tail_sections(&mut out, think, &lines[i..]);
        return out.join("\n") + "\n";
    }

    // models 表为空/索引越界：末尾追加模型行（补全 think 段后）
    for (i, l) in lines.iter().enumerate() {
        let trimmed = l.trim_start();
        let top = l.len() - trimmed.len() == 0;
        if top && (trimmed == "think:" || trimmed == "slots:") {
            let mut head = out.clone();
            append_model_block(&mut head, row);
            write_tail_sections(&mut head, think, &lines[i..]);
            return head.join("\n") + "\n";
        }
        out.push(l.to_string());
    }
    append_model_block(&mut out, row);
    write_tail_sections(&mut out, think, &[]);
    out.join("\n") + "\n"
}

/// 追加一个完整模型块（文件末尾 / models 表缺失场景）。
fn append_model_block(out: &mut Vec<String>, row: &ModelRow) {
    // 文件已存在顶层 `models:`（但目标索引越界）时不重复头键，仅补行
    let has_header = out.iter().any(|l| l.trim_start() == "models:");
    if !has_header {
        out.push("models:".to_string());
    }
    out.push(format!("  - name: {}", row.name));
    for (k, v) in row_pairs(row) {
        out.push(format!("    {}: {}", k, v));
    }
}

/// 写 slots + think 段：槽位统一落 `slots:`（C-19 单一事实源），
/// `think:` 只留 enabled/timeout_ms；两段已知键一律重写，未知行保留。
fn write_tail_sections(out: &mut Vec<String>, think: &ThinkCfg, rest: &[&str]) {
    out.push("".to_string());
    out.push("slots:".to_string());
    if !think.slow_model.is_empty() {
        out.push(format!("  t2: {}", think.slow_model));
    }
    if !think.fast_model.is_empty() {
        out.push(format!("  t1f: {}", think.fast_model));
    }
    if !think.prof_model.is_empty() {
        out.push(format!("  t1p: {}", think.prof_model));
    }
    out.push("".to_string());
    out.push("think:".to_string());
    if let Some(en) = think.enabled {
        out.push(format!("  enabled: {}", en));
    }
    if !think.timeout_ms.is_empty() {
        out.push(format!("  timeout_ms: {}", think.timeout_ms));
    }
    // rest（原 slots/think 段起始）：跳过两段头行，保留未知键行
    let known = [
        "t2",
        "t1f",
        "t1p",
        "enabled",
        "think2_slow_model",
        "think1_fast_model",
        "think1_prof_model",
        "timeout_ms",
    ];
    for l in rest.iter() {
        let trimmed = l.trim_start();
        let top = l.len() - trimmed.len() == 0;
        if top && (trimmed == "think:" || trimmed == "slots:") {
            continue;
        }
        if let Some((k, _)) = split_kv(trimmed) {
            if known.contains(&k) {
                continue;
            }
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push(l.to_string());
        }
    }
}

/// 模型行全部键值对（基础 + 高级非空项）。
fn row_pairs(row: &ModelRow) -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    for (k, val) in row.base_pairs() {
        if !val.is_empty() {
            v.push((k, val.to_string()));
        }
    }
    for (k, val) in row.advanced_pairs() {
        v.push((k, val.to_string()));
    }
    v
}

/// 某键在模型行中的写入值（None = 该键不在写入范围）。
fn row_value<'a>(k: &str, row: &'a ModelRow) -> Option<&'a str> {
    for (rk, rv) in row.base_pairs().iter().chain(row.advanced_pairs().iter()) {
        if *rk == k {
            return Some(rv);
        }
    }
    None
}

/// 文件缺失时生成最小 v2 model.yaml（models 表 + default_model + think 段）。
fn build_minimal_yaml(row: &ModelRow, think: &ThinkCfg) -> String {
    let mut s = String::new();
    s.push_str("# AgentRT 大语言模型配置（由 TUI 向导生成）\n");
    s.push_str("models:\n");
    // name 单独成行一次（此前在循环内重复写 `- name:`，生成 N 个碎片
    // 模型项，read_model_yaml 取 rows.first() 读到空 context_window，
    // 向导高级选项配置静默丢失——0.1.8 修复）
    s.push_str(&format!("  - name: {}\n", row.name));
    for (k, v) in row_pairs(row) {
        if k == "name" {
            continue;
        }
        s.push_str(&format!("    {}: {}\n", k, v));
    }
    s.push('\n');
    if !row.model_id.is_empty() {
        s.push_str(&format!("default_model: {}\n", row.model_id));
    }
    s.push('\n');
    s.push_str("slots:\n");
    if !think.slow_model.is_empty() {
        s.push_str(&format!("  t2: {}\n", think.slow_model));
    }
    if !think.fast_model.is_empty() {
        s.push_str(&format!("  t1f: {}\n", think.fast_model));
    }
    if !think.prof_model.is_empty() {
        s.push_str(&format!("  t1p: {}\n", think.prof_model));
    }
    s.push('\n');
    s.push_str("think:\n");
    if let Some(en) = think.enabled {
        s.push_str(&format!("  enabled: {}\n", en));
    }
    s
}

/// 拆解 `key: value` 行（value 可为空；剥离行内注释 ` # ...`）。
fn split_kv(trimmed: &str) -> Option<(&str, &str)> {
    let idx = trimmed.find(':')?;
    let k = trimmed[..idx].trim();
    if k.is_empty() || k.contains(' ') {
        return None;
    }
    let mut v = trimmed[idx + 1..].trim();
    // YAML 行内注释：空白后的 # 起为注释（值含 # 且前有空白时截断）
    if let Some(c) = v.find(" #") {
        v = v[..c].trim();
    }
    Some((k, v))
}

/// 去引号（YAML 字符串值可能带引号）。
fn unquote(v: &str) -> &str {
    let v = v.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# AgentRT 大语言模型配置文件
models:
  - name: "DeepSeek"
    mode: "api"
    api_format: "openai"
    base_url: "https://api.deepseek.com"
    model_id: "deepseek-v4-flash"
    api_key_env: "MODEL_1_API_KEY"

  - name: "GLM"
    mode: "api"
    api_format: "openai"
    base_url: "https://open.bigmodel.cn/api/paas/v4"
    model_id: "GLM-4.7-Flash"
    api_key_env: "MODEL_2_API_KEY"

default_model: "deepseek-v4-flash"

slots:
  t2: "deepseek-v4-pro"
  t1f: "deepseek-v4-flash"

think:
  enabled: true
"#;

    #[test]
    fn parse_models_table() {
        let m = parse_model_yaml(SAMPLE);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.rows[0].name, "DeepSeek");
        assert_eq!(m.rows[0].base_url, "https://api.deepseek.com");
        assert_eq!(m.rows[1].model_id, "GLM-4.7-Flash");
        assert_eq!(m.default_model, "deepseek-v4-flash");
        let slots = m.slots.expect("slots section");
        assert_eq!(slots.t2, "deepseek-v4-pro");
        assert_eq!(slots.t1f, "deepseek-v4-flash");
        let think = m.think.expect("think section");
        assert_eq!(think.enabled, Some(true));
        assert_eq!(think.slow_model, "deepseek-v4-pro");
    }

    #[test]
    fn parse_legacy_think_models_backward_compatible() {
        let src = "think:\n  enabled: true\n  think2_slow_model: \"m-slow\"\n";
        let m = parse_model_yaml(src);
        assert!(m.slots.is_none());
        assert_eq!(m.think.expect("think").slow_model, "m-slow");
    }

    #[test]
    fn patch_preserves_other_rows_and_comments() {
        let row = ModelRow {
            name: "DeepSeek".into(),
            mode: "api".into(),
            api_format: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            model_id: "deepseek-flash".into(),
            api_key_env: "MODEL_1_API_KEY".into(),
            ..Default::default()
        };
        let think = ThinkCfg {
            enabled: Some(false),
            slow_model: "deepseek-flash".into(),
            ..Default::default()
        };
        let patched = patch_lines(SAMPLE, 0, &row, &think);
        assert!(patched.contains("model_id: deepseek-flash"));
        assert!(patched.contains("GLM-4.7-Flash"), "第二行模型必须保留");
        assert!(patched.contains("# AgentRT 大语言模型配置文件"));
        assert!(patched.contains("enabled: false"), "think enabled 应更新");
        assert!(patched.contains("t2: deepseek-flash"), "槽位写入 slots 段");
        assert!(
            !patched.contains("think2_slow_model"),
            "不得再写废弃 think.*_model 键"
        );
        assert!(
            !patched.contains("deepseek-v4-flash\""),
            "旧 model_id 应被替换"
        );
    }

    #[test]
    fn append_when_models_empty() {
        let row = ModelRow {
            name: "Local".into(),
            mode: "local".into(),
            api_format: "openai".into(),
            base_url: "http://localhost:11434/v1".into(),
            model_id: "llama3".into(),
            ..Default::default()
        };
        let think = ThinkCfg {
            enabled: Some(true),
            slow_model: "llama3".into(),
            fast_model: "llama3".into(),
            ..Default::default()
        };
        let out = build_minimal_yaml(&row, &think);
        assert!(out.contains("models:"));
        assert!(out.contains("mode: local"));
        assert!(out.contains("t2: llama3"));
        assert!(out.contains("t1f: llama3"));
    }

    #[test]
    fn set_default_model_replaces_and_preserves() {
        let patched = patch_default_model(SAMPLE, "GLM-4.7-Flash");
        assert!(patched.contains("default_model: GLM-4.7-Flash"));
        assert!(
            !patched.contains("default_model: \"deepseek-v4-flash\""),
            "旧默认模型应被替换"
        );
        assert!(
            patched.contains("model_id: \"deepseek-v4-flash\""),
            "模型行本身不动"
        );
        assert!(
            patched.contains("# AgentRT 大语言模型配置文件"),
            "文件头注释保留"
        );
        assert!(patched.contains("t1f: \"deepseek-v4-flash\""), "slots 段保留");
        assert!(patched.contains("name: \"GLM\""), "其余模型行保留");
    }

    #[test]
    fn set_default_model_inserts_when_absent() {
        let src = "# header\nmodels:\n  - name: A\n    model_id: a\n\nthink:\n  enabled: true\n";
        let patched = patch_default_model(src, "a");
        let lines: Vec<&str> = patched.lines().collect();
        let dm = lines
            .iter()
            .position(|l| *l == "default_model: a")
            .expect("插入 default_model");
        let th = lines.iter().position(|l| *l == "think:").expect("think 段");
        assert!(dm < th, "应插入到 think 段之前");
    }

    #[test]
    fn set_default_model_creates_minimal_file() {
        let patched = patch_default_model("", "gpt-4o");
        assert!(patched.contains("default_model: gpt-4o"));
        assert!(patched.ends_with('\n'));
    }
}
