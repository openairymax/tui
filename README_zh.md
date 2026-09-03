**语言:** [English](README.md) | 简体中文

# Airymax TUI

[![Version](https://img.shields.io/badge/version-0.1.9-5a6b7e)](https://atomgit.com/openairymax/tui)
[![License](https://img.shields.io/badge/license-AGPL--3.0+Apache--2.0-4a90d9)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> [Airymax](https://atomgit.com/openairymax/airymaxhub) AI 智能体运行时平台的官方终端用户界面。
> [sdk](https://atomgit.com/openairymax/sdk) 管理仓聚合的叶子仓之一。
> 独立 Rust 二进制 —— 通过 HTTP 与 Airymax Gateway 通信，不链接各语言 SDK（无 `agentrt-rs` 依赖）。

---

## 概述

**Airymax TUI**（`agentrt-tui`）是用 Rust 构建的终端用户界面，为开发者和运维人员提供可视化、交互式的运行时渲染仪表盘 —— 覆盖对话渲染、交互与可观测，业务逻辑保持在运行时侧。它基于 `ratatui` 与 `crossterm` 构建，在单个终端窗口内提供多面板导航、实时对话渲染、日志 / 记忆面板、配置与向导。

0.1.9（M5）起公开能力：对话流对接 gateway 的 `agent.run_stream` 事件帧协议（token 打字机 / 工具调用 / 思考链 / 结构化错误渲染）；首次启动向导数据驱动；主题 token 化（语义色 token，自动适配 TrueColor / 256 / 16 三档色深）；日志 / 记忆面板经 gateway 事件订阅；大历史对话虚拟渲染。

与 CLI 一样，TUI 是一等**运行时租户**：经 Gateway 通信（HTTP / JSON-RPC 2.0，对话执行轮走 SSE 事件流），使用自身的 `reqwest` 客户端。它不依赖、也不链接各语言 SDK（`agentrt-rs` 等）。

## 运行时通信

TUI 经 Gateway 与运行时通信：常规请求走 HTTP（JSON-RPC 2.0），对话执行轮经 `agent.run_stream` SSE 事件流接收事件帧（token 打字机 / 工具调用 / 思考链 / 结构化错误）。HTTP 客户端为 `src/client.rs`（基于 `reqwest`）。它不包装、也不消费语言 SDK：`Cargo.toml` 中没有 `agentrt-rs` 依赖。

```
agentrt-tui <panel>
   └── src/client.rs — reqwest HTTP / SSE 客户端
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
│   ├── client.rs            # Gateway 客户端（HTTP / JSON-RPC 2.0 + SSE 事件流）
│   ├── run_stream.rs        # agent.run_stream v1 事件帧解析（SSoT 常量由 build.rs 生成）
│   ├── ui.rs                # 主渲染与统一布局
│   ├── theme.rs             # 设计令牌：语义色 token + TrueColor / 256 / 16 三档色深
│   ├── gccp.rs              # GCCP 任务事实确认 / GRAD 流程图确认
│   ├── markdown.rs          # Markdown 渲染
│   ├── memory.rs            # 对话记忆
│   ├── skills.rs            # 本地技能库
│   ├── models_cfg.rs        # model.yaml 读写（模型连接表 + 思考系统段）
│   ├── secrets.rs           # secrets.env 读写
│   ├── paths.rs             # AIRY_HOME 路径解析单一来源
│   ├── ime.rs               # 内置拼音输入法
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
├── Cargo.toml               # crate 清单（agentrt-tui，二进制：agentrt-tui）
└── README.md                # 本文件
```

## 上下游依赖

### 上游

- **运行时**：经 HTTP（JSON-RPC 2.0）与 SSE 事件流连接到运行中的 Airymax / AgentRT 实例（Gateway HTTP / SSE API）。TUI 直接使用 `reqwest`，**无 `agentrt-rs` 依赖**（见 `Cargo.toml`）。
- **配置**：依次从 CLI 标志、环境变量（`AGENTRT_GATEWAY_URL`、`AGENTRT_API_KEY`）、默认值 `http://localhost:8080` 解析。

### 下游

- **开发者 / 运维人员**：对运行中 Agent 的交互式、一目了然视图，适合本地开发与运维排查。

## 面板一览

| 面板 | 快捷键 | 说明 |
|------|--------|------|
| **Chat** | `Esc` | 对话面板：输入消息并查看智能体回复 |
| **Help** | `F1` | 帮助面板：显示快捷键说明 |
| **Config** | `F2` | 配置面板：查看和编辑运行配置 |
| **Logs** | `F3` | 日志面板：实时运行时日志流 |
| **Memory** | `F4` | 记忆面板：查看智能体记忆内容 |
| **Plugins** | `F5` | 插件面板：管理已加载的插件 |

### 操作快捷键

| 快捷键 | 说明 |
|--------|------|
| `Ctrl+C` | 退出程序 |
| `Esc` | 返回对话面板 |
| `F1`–`F5` | 切换到对应面板 |
| `Enter` | 提交输入 |
| `Backspace` | 删除字符 |
| `↑` / `↓` | 上下滚动 |
| `PageUp` / `PageDown` | 翻页滚动 |

## 安装

### 从源码构建

```bash
cd tui
cargo build --release
# 二进制：./target/release/agentrt-tui
```

**环境要求：** Rust edition 2021（stable 工具链）。运行时依赖：`ratatui` 0.28 + `crossterm` 0.28（TUI 框架）、`reqwest` 0.12（HTTP）、`tokio` 1 + `tokio-stream` + `futures`（异步与流式）、`serde` / `serde_json`（序列化）、`clap` 4.5（参数解析）、`thiserror` / `anyhow`（错误）、`chrono`、`unicode-width`。

## 使用说明

### 启动

```bash
# 启动 TUI（连接默认 Gateway）
agentrt-tui

# 指定 Gateway 地址和智能体配置
agentrt-tui --gateway-url http://localhost:8080 --agent-file agents/main.agent.yaml

# 或使用环境变量
export AGENTRT_GATEWAY_URL=http://localhost:8080
agentrt-tui
```

### 对话流程

1. 启动后在 **Chat** 面板输入消息。
2. 按 `Enter` 提交，智能体实时流式返回回复。
3. 使用 `↑` / `↓` 翻阅历史消息。
4. 按 `F1` 查看完整帮助。

### 面板导航

- `F3` —— 切换到日志面板，实时查看运行时日志流。
- `F4` —— 切换到记忆面板，查看智能体已存储的记忆。
- `F5` —— 切换到插件面板，加载 / 卸载插件。
- `Esc` —— 快速回到对话面板。

## 构建与测试

```bash
cargo build --release
cargo test
./target/release/agentrt-tui
# 请先确保 Airymax Gateway 已在运行。
```

## 分支策略

本叶子仓在 **`develop/hubs-01`** 分支上开发，`main` 为发布快照。聚合管理仓 `sdk` 在 `main` 上直接开发。

## 许可证

采用 **AGPL v3 + Apache 2.0** 双许可证（SPDX: `AGPL-3.0-or-later OR Apache-2.0`）。详见 [LICENSE](LICENSE)。

Copyright (c) 2025-2026 **SPHARX Ltd.** All Rights Reserved.
