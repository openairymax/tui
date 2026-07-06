**语言:** [English](README.md) | 简体中文

# Airymax TUI

[![Version](https://img.shields.io/badge/version-0.1.1-5a6b7e)](https://atomgit.com/openairymax/sdk-tui)
[![License](https://img.shields.io/badge/license-AGPL--3.0+Apache--2.0-4a90d9)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> [Airymax](https://atomgit.com/openairymax/airymaxhub) AI 智能体运行时平台的官方终端用户界面。
> [sdk](https://atomgit.com/openairymax/sdk) 管理仓聚合的叶子仓之一。
> 基于 Airymax Rust SDK（`agentrt-rs`）构建。

---

## 概述

**Airymax TUI**（`agentrt-tui`）是用 Rust 构建的终端用户界面，为开发者和运维人员提供可视化、交互式驱动 Airymax 运行时的方式。它基于 `ratatui` 与 `crossterm` 构建，提供多面板切换、实时对话、日志监控、记忆查看、配置编辑和插件管理 —— 全部在单个终端窗口内完成。

与 CLI 一样，TUI 是一等**运行时租户**：通过 HTTP 与 Airymax Gateway 通信，内部驱动 `agentrt-rs` 提供的相同双层 SDK 架构（Cognition / Safety / Tool / Chat）。

## 消费的双层 API 架构

TUI 是 Airymax Rust SDK 的消费者。其面板将用户操作转换为对 `AgentRTClient` 暴露的四个内嵌资源客户端的调用：

```
agentrt-tui <panel>
   └── AgentRTClient（来自 agentrt-rs）
       ├── CognitionClient   # 对话面板：任务提交 / 推理
       ├── SafetyClient      # 审计 / 策略视图
       ├── ToolClient        # 插件管理面板
       └── ChatClient        # 对话面板：流式响应
```

## 目录结构

```
tui/
├── src/
│   ├── main.rs              # 入口与终端初始化 / 清理
│   ├── app.rs               # 应用状态与事件循环
│   ├── client.rs            # Gateway HTTP 客户端
│   ├── ui.rs                # UI 渲染入口
│   └── panels/
│       ├── mod.rs           # 面板模块导出
│       ├── chat.rs          # 对话面板
│       ├── config.rs        # 配置面板
│       ├── help.rs          # 帮助面板
│       ├── logs.rs          # 日志面板
│       ├── memory.rs        # 记忆面板
│       └── plugins.rs       # 插件面板
├── Cargo.toml               # crate 清单（agentrt-tui，二进制：agentrt-tui）
└── README.md                # 本文件
```

## 上下游依赖

### 上游

- **Airymax Rust SDK（`agentrt-rs`）**：提供与运行时通信的类型化 `AgentRTClient` 与四个内嵌资源客户端。
- **运行时**：通过 HTTP 和 JSON-RPC 2.0 连接到运行中的 Airymax / AgentRT 实例（`gateway_d` / Gateway HTTP API）。
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

本叶子仓在 **`feature/official-hubs-01`** 分支上开发。聚合管理仓 `sdk` 仅使用 `main` 分支。

## 许可证

采用 **AGPL v3 + Apache 2.0** 双许可证（SPDX: `AGPL-3.0-or-later OR Apache-2.0`）。详见 [LICENSE](LICENSE)。

Copyright (c) 2025-2026 **SPHARX Ltd.** All Rights Reserved.
