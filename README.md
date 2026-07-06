**Language:** English | [简体中文](README_zh.md)

# Airymax TUI

[![Version](https://img.shields.io/badge/version-0.1.1-5a6b7e)](https://atomgit.com/openairymax/tui)
[![License](https://img.shields.io/badge/license-AGPL--3.0+Apache--2.0-4a90d9)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> Official terminal user interface for the [Airymax](https://atomgit.com/openairymax/airymaxhub) AI Agent Runtime Platform.
> One of the leaf repositories aggregated by the [sdk](https://atomgit.com/openairymax/sdk) management repo.
> Built on top of the Airymax Rust SDK (`agentrt-rs`).

---

## Overview

The **Airymax TUI** (`agentrt-tui`) is a Rust-built terminal user interface that gives developers and operators a visual, interactive way to drive the Airymax runtime. Built with `ratatui` and `crossterm`, it offers multi-panel navigation, real-time chat, log monitoring, memory inspection, configuration editing, and plugin management — all from a single terminal window.

Like the CLI, the TUI is a first-class **runtime tenant**: it talks to the Airymax Gateway over HTTP and internally drives the same double-layer SDK architecture (Cognition / Safety / Tool / Chat) provided by `agentrt-rs`.

## Double-Layer API Architecture (consumed)

The TUI is a consumer of the Airymax Rust SDK. Its panels translate user actions into calls against the four nested resource clients exposed by `AgentRTClient`:

```
agentrt-tui <panel>
   └── AgentRTClient (from agentrt-rs)
       ├── CognitionClient   # Chat panel: task submission / inference
       ├── SafetyClient      # audit / policy views
       ├── ToolClient        # plugin management panel
       └── ChatClient        # Chat panel: streaming responses
```

## Directory Structure

```
tui/
├── src/
│   ├── main.rs              # Entry point + terminal init/teardown
│   ├── app.rs               # Application state + event loop
│   ├── client.rs            # Gateway HTTP client
│   ├── ui.rs                # UI rendering entry
│   └── panels/
│       ├── mod.rs           # Panel module exports
│       ├── chat.rs          # Chat panel
│       ├── config.rs        # Configuration panel
│       ├── help.rs          # Help panel
│       ├── logs.rs          # Logs panel
│       ├── memory.rs        # Memory panel
│       └── plugins.rs       # Plugin panel
├── Cargo.toml               # Crate manifest (agentrt-tui, binary: agentrt-tui)
└── README.md                # This file
```

## Upstream & Downstream Dependencies

### Upstream

- **Airymax Rust SDK (`agentrt-rs`)**: Provides the typed `AgentRTClient` and the four nested resource clients used to talk to the runtime.
- **Runtime**: Connects to a running Airymax / AgentRT instance (`gateway_d` / Gateway HTTP API) over HTTP and JSON-RPC 2.0.
- **Configuration**: Resolved from CLI flags, then environment variables (`AGENTRT_GATEWAY_URL`, `AGENTRT_API_KEY`), then a `http://localhost:8080` default.

### Downstream

- **Developers / operators**: An interactive, at-a-glance view of a running agent, suitable for local development and ops triage.

## Panels

| Panel | Shortcut | Description |
|-------|----------|-------------|
| **Chat** | `Esc` | Conversation panel: enter prompts and view agent replies |
| **Help** | `F1` | Help panel: keyboard shortcut reference |
| **Config** | `F2` | Configuration panel: view and edit runtime config |
| **Logs** | `F3` | Logs panel: live runtime log stream |
| **Memory** | `F4` | Memory panel: inspect agent memory contents |
| **Plugins** | `F5` | Plugins panel: manage loaded plugins |

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+C` | Quit |
| `Esc` | Return to the Chat panel |
| `F1`–`F5` | Switch to the corresponding panel |
| `Enter` | Submit input |
| `Backspace` | Delete character |
| `↑` / `↓` | Scroll up / down |
| `PageUp` / `PageDown` | Page scroll |

## Installation

### From source

```bash
cd tui
cargo build --release
# Binary: ./target/release/agentrt-tui
```

**Requirements:** Rust edition 2021 (stable toolchain). Runtime dependencies: `ratatui` 0.28 + `crossterm` 0.28 (TUI framework), `reqwest` 0.12 (HTTP), `tokio` 1 + `tokio-stream` + `futures` (async + streaming), `serde` / `serde_json` (serialization), `clap` 4.5 (argument parsing), `thiserror` / `anyhow` (errors), `chrono`, `unicode-width`.

## Usage

### Launch

```bash
# Start the TUI (connects to the default Gateway)
agentrt-tui

# Specify Gateway URL and agent config
agentrt-tui --gateway-url http://localhost:8080 --agent-file agents/main.agent.yaml

# Or use an environment variable
export AGENTRT_GATEWAY_URL=http://localhost:8080
agentrt-tui
```

### Conversational workflow

1. After launch, type a message in the **Chat** panel.
2. Press `Enter` to submit; the agent streams its reply in real time.
3. Use `↑` / `↓` to scroll through history.
4. Press `F1` for the full help reference.

### Panel navigation

- `F3` — switch to the logs panel for a live runtime log stream.
- `F4` — switch to the memory panel to inspect stored agent memory.
- `F5` — switch to the plugins panel to load/unload plugins.
- `Esc` — jump back to the chat panel.

## Build & Test

```bash
cargo build --release
cargo test
./target/release/agentrt-tui
# Ensure the Airymax Gateway is running first.
```

## Branch Strategy

This leaf repository is developed on **`feature/official-hubs-01`**. The aggregating `sdk` management repo stays on `main`.

## License

Dual-licensed under **AGPL v3 + Apache 2.0** (SPDX: `AGPL-3.0-or-later OR Apache-2.0`). See [LICENSE](LICENSE) for full text.

Copyright (c) 2025-2026 **SPHARX Ltd.** All Rights Reserved.
