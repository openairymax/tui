**语言:** [English](README.md) | 简体中文

# Airymax TUI

[![Version](https://img.shields.io/badge/version-0.1.16-5a6b7e)](https://atomgit.com/openairymax/tui)
[![License](https://img.shields.io/badge/license-AGPL--3.0+Apache--2.0-4a90d9)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> [Airymax](https://atomgit.com/openairymax) AI 智能体运行时平台的官方终端用户界面。
> [sdk](https://atomgit.com/openairymax/sdk) 管理仓聚合的叶子仓之一。
> 独立 Rust 二进制 —— 通过共享协议客户端 `agentrt-rs` 与 Airymax Gateway 通信。

---

## 概述

**Airymax TUI**（`agentrt-tui`）是用 Rust 构建的终端用户界面，为开发者和运维人员提供可视化、交互式的运行时仪表盘 —— 覆盖对话渲染、交互与可观测，业务逻辑保持在运行时侧。它基于 `ratatui` 与 `crossterm` 构建，在单个终端窗口内提供多面板导航、实时对话渲染、日志 / 记忆面板、配置与首次启动向导。

公开能力：对话流对接 gateway 的 `agent.run_stream` 事件帧协议（token 打字机 / 工具调用 / 思考链 / 结构化错误渲染）；首次启动向导数据驱动；主题 token 化（语义色 token，自动适配 TrueColor / 256 / 16 三档色深）；日志 / 记忆面板经 gateway 事件订阅；大历史对话虚拟渲染；中文输入交由终端 / OS 输入法承担。

与 CLI 一样，TUI 是一等**运行时租户**：经共享协议客户端 `agentrt-rs` 与 Gateway 通信（HTTP / JSON-RPC 2.0，对话执行轮走 SSE 事件流），协议线格式因此只有单一事实源。

## 运行时通信

TUI 经 Gateway 与运行时通信：常规请求走 HTTP（JSON-RPC 2.0），对话执行轮经 `agent.run_stream` SSE 事件流接收事件帧（token 打字机 / 工具调用 / 思考链 / 结构化错误）。传输与帧解码统一交由 `agentrt-rs` 协议客户端（`agentrt_rs::run_stream`）承担；协议常量由唯一 C 头 `airy_run_stream.h` 生成，此处不书写任何协议字面量。`src/client.rs` 只保留 UI 语义转译层。

```
agentrt-tui
   └── src/client.rs — gateway 客户端（UI 语义转译层）
       ├── agentrt-rs → HTTP / SSE 协议客户端（run_stream 事件帧）
       ├── chat    → 对话 / 任务提交（run_stream 事件流）
       ├── memory  → 记忆面板（gateway 事件订阅）
       ├── logs    → 日志面板（gateway 事件订阅）
       └── plugins → 插件管理
```

## 目录结构

```
tui/
├── src/
│   ├── main.rs              # 入口与终端初始化 / 清理
│   ├── client.rs            # Gateway 客户端（基于 agentrt-rs 的 UI 语义转译）
│   ├── ui.rs                # 主渲染与统一布局
│   ├── theme.rs             # 设计令牌：语义色 token + TrueColor / 256 / 16 三档色深
│   ├── gccp.rs              # 任务事实 / 流程确认对话框
│   ├── markdown.rs          # Markdown 渲染
│   ├── memory.rs            # 对话记忆
│   ├── skills.rs            # 共享技能库（经网关 mem.*，kind=skill）
│   ├── models_cfg.rs        # model.yaml 读写（模型表 + 思考系统段）
│   ├── secrets.rs           # secrets.env 读写
│   ├── paths.rs             # AIRY_HOME 路径解析单一来源
│   ├── ime.rs               # 拼音输入法 FFI（ime_linked 未置位：fail-closed 休眠）
│   ├── app/                 # 应用状态域（分发 / 轮询 / 面板 / 任务 / 会话 / 输入）
│   ├── panels/              # 渲染面板
│   │   ├── mod.rs           # 面板模块导出
│   │   ├── chat/            # 对话面板（大历史虚拟视图 / 消息块）
│   │   ├── config.rs        # 配置面板
│   │   ├── logs.rs          # 日志面板
│   │   ├── memory.rs        # 记忆面板
│   │   ├── plugins.rs       # 插件面板
│   │   ├── help.rs          # 帮助面板
│   │   └── board.rs / events.rs
│   └── wizard/              # 首次启动向导（数据驱动：steps 注册表即 SSoT）
├── Cargo.toml               # crate 清单（agentrt-tui，二进制：agentrt-tui；依赖 agentrt-rs）
└── README.md                # 本文件
```

## 前置条件

TUI 是运行中 AgentRT 运行时的客户端，请先安装运行时：

```bash
curl -fsSL "https://api.atomgit.com/api/v5/repos/openairymax/agentrt/contents/scripts/install.sh?ref=main" | python3 -c 'import json,sys,base64;sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["content"]))' | bash
```

（另有兼容入口 `https://atomgit.com/openairymax/agentrt/releases/download/latest/install.sh`。）随后启动网关，例如 `airymaxrt start`。

### Gateway 地址

网关地址按以下顺序解析：

1. `--gateway-url <url>` 命令行标志；
2. `AGENTRT_GATEWAY_URL` 环境变量；
3. `$AIRY_HOME/run/gateway.port` —— 默认端口被占用时，启动器会把实际网关端口记录在该文件；
4. 内置回退值 `http://127.0.0.1:8080`。

其他环境变量：`AIRY_HOME`（数据根目录，默认 `~/.airymaxrt`）、`AGENTRT_TUI_LOG`（日志文件路径，默认 `$AIRY_HOME/logs/agentrt-tui.log`）、`AIRY_TUI_THEME`（主题覆盖；`COLORFGBG` 用于自动检测）。

## 面板一览

| 面板 | 快捷键 | 说明 |
|------|--------|------|
| **Chat** | 默认 | 对话面板：输入消息并查看智能体回复 |
| **Help** | `F1` | 帮助面板：快捷键说明 |
| **Config** | `F2` | 配置面板：查看和编辑运行配置 |
| **Logs** | `F3` | 日志面板：实时运行时日志流 |
| **Memory** | `F4` | 记忆面板：查看智能体记忆内容 |
| **Plugins** | `F5` | 插件面板：管理已加载的插件 |
| **Board** | `F6` | 任务看板（经 gateway 事件流实时更新） |
| **Events** | `F7` | 事件流视图 |

### 操作快捷键

| 快捷键 | 说明 |
|--------|------|
| `Ctrl+C` | 退出程序 |
| `Ctrl+X` | 中止当前后台请求（任务执行 / 等待中的回复） |
| `Ctrl+Z` | 暂停 / 恢复等待（请求继续在后台执行） |
| `Ctrl+T` | 新建会话标签（任务执行中不可用） |
| `Alt+1`–`Alt+9` | 切换会话标签（`Alt+1` 回到主会话） |
| `Esc` | 返回对话面板 |
| `F1`–`F7` | 切换到对应面板 |
| `F8` | 切换到终端 CLI（`airy_cli`） |
| `Tab` | 补全 `/` 命令与技能名（Chat 面板） |
| `Enter` | 提交输入 |
| `Alt+Enter` | 换行（多行输入） |
| `Backspace` | 删除字符 |
| `↑` / `↓` | 上下滚动 |
| `Alt+↑` / `Alt+↓` | 浏览输入历史（`Alt+↓` 回到手输状态） |
| `PageUp` / `PageDown` | 翻页滚动 |
| `End` | 回到底部（最新消息） |

## 安装

### 从源码构建

```bash
cd tui
cargo build --release
# 二进制：./target/release/agentrt-tui
```

或安装到 cargo bin 路径：

```bash
cargo install --path .
```

**环境要求：** Rust edition 2021（stable 工具链）。`agentrt-rs`（共享协议客户端；协议常量由 C 头 `airy_run_stream.h` 生成）。运行时依赖：`ratatui` 0.28 + `crossterm` 0.28（TUI 框架）、`reqwest` 0.12（HTTP，rustls）、`tokio` 1 + `tokio-stream` + `futures`（异步与流式）、`serde` / `serde_json`（序列化）、`clap` 4.5（参数解析）、`thiserror` / `anyhow`（错误）、`log` / `env_logger`（日志）、`chrono`、`unicode-width`。

## 使用说明

### 启动

```bash
# 启动 TUI（连接默认 Gateway）
agentrt-tui

# 指定 Gateway 地址和智能体配置
agentrt-tui --gateway-url http://127.0.0.1:8080 --agent-file agents/main.agent.yaml

# 恢复上次会话 / 打开指定项目目录
agentrt-tui --resume
agentrt-tui --project ./my-agent-project

# 或使用环境变量
export AGENTRT_GATEWAY_URL=http://127.0.0.1:8080
agentrt-tui
```

`--agent-file` 默认值为 `agents/main.agent.yaml`。

### 对话流程

1. 启动后在 **Chat** 面板输入消息（首次启动会进入向导辅助初始配置）。
2. 按 `Enter` 提交，智能体实时流式返回回复。
3. 使用 `↑` / `↓` 翻阅历史消息。
4. 按 `F1` 查看完整帮助。

### 面板导航

- `F3` —— 切换到日志面板，实时查看运行时日志流。
- `F4` —— 切换到记忆面板，查看智能体已存储的记忆。
- `F6` / `F7` —— 任务看板与事件流视图。
- `Esc` —— 快速回到对话面板。

## 构建与测试

```bash
cargo build --release
cargo test
./target/release/agentrt-tui
# 请先确保 Airymax Gateway 已在运行。
```

## 许可证

采用 **AGPL v3 + Apache 2.0** 双许可证（SPDX: `AGPL-3.0-or-later OR Apache-2.0`）。详见 [LICENSE](LICENSE)。

Copyright (c) 2025-2026 **SPHARX Ltd.** All Rights Reserved.
