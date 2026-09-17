**Language:** English | [简体中文](README_zh.md)

# Airymax TUI

[![Version](https://img.shields.io/badge/version-0.1.16-5a6b7e)](https://atomgit.com/openairymax/tui)
[![License](https://img.shields.io/badge/license-AGPL--3.0+Apache--2.0-4a90d9)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> Official terminal user interface for the Airymax AI Agent Runtime Platform ([openairymax](https://atomgit.com/openairymax)).
> One of the leaf repositories aggregated by the [sdk](https://atomgit.com/openairymax/sdk) management repo.
> Standalone Rust binary — talks to the Airymax Gateway over HTTP through the shared `agentrt-rs` protocol client.

---

## Overview

The **Airymax TUI** (`agentrt-tui`) is a Rust-built terminal user interface that gives developers and operators a visual, interactive dashboard for the runtime — covering conversation rendering, interaction and observability, with business logic staying on the runtime side. Built with `ratatui` and `crossterm`, it offers multi-panel navigation, real-time conversation rendering, logs / memory panels, configuration and a first-run setup wizard — all from a single terminal window.

Public capabilities: the conversation flow speaks the gateway's `agent.run_stream` event-frame protocol (rendering token typewriter / tool calls / thought chains / structured errors); the first-run wizard is data-driven; theming is token-based (semantic color tokens auto-adapting to TrueColor / 256 / 16 color depths); logs / memory panels subscribe to gateway events; large conversation histories use virtual rendering; CJK input is handled by the terminal / OS input method.

Like the CLI, the TUI is a first-class **runtime tenant**: it talks to the Gateway (HTTP / JSON-RPC 2.0, with execution turns over an SSE event stream) through `agentrt-rs`, the shared protocol client, so the wire contract has a single source of truth.

## Runtime Communication

The TUI talks to the runtime through the Gateway: regular requests go over HTTP (JSON-RPC 2.0), while execution turns consume `agent.run_stream` SSE event frames (token typewriter / tool calls / thought chains / structured errors). Transport and frame decoding are delegated to the `agentrt-rs` protocol client (`agentrt_rs::run_stream`); the wire constants are generated from the single C header `airy_run_stream.h`, so no literal protocol strings are written here. `src/client.rs` keeps only the UI-semantic translation layer.

```
agentrt-tui
   └── src/client.rs — gateway client (UI-semantic translation layer)
       ├── agentrt-rs → HTTP / SSE protocol client (run_stream event frames)
       ├── chat    → conversation / task submission (run_stream event stream)
       ├── memory  → memory panel (gateway event subscription)
       ├── logs    → logs panel (gateway event subscription)
       └── plugins → plugin management
```

## Directory Structure

```
tui/
├── src/
│   ├── main.rs              # Entry point + terminal init/teardown
│   ├── client.rs            # Gateway client (UI-semantic translation over agentrt-rs)
│   ├── ui.rs                # Main rendering + unified layout
│   ├── theme.rs             # Design tokens: semantic color tokens + TrueColor / 256 / 16 depths
│   ├── gccp.rs              # Task fact / flow confirmation dialogs
│   ├── markdown.rs          # Markdown rendering
│   ├── memory.rs            # Conversation memory
│   ├── skills.rs            # Shared skills library (via gateway mem.*, kind=skill)
│   ├── models_cfg.rs        # model.yaml read/write (model table + thinking system section)
│   ├── secrets.rs           # secrets.env read/write
│   ├── paths.rs             # Single source for AIRY_HOME path resolution
│   ├── ime.rs               # Pinyin IME FFI (ime_linked unset: dormant / fail-closed)
│   ├── app/                 # Application state domain (dispatch / poll / panels / tasks / sessions / input)
│   ├── panels/              # Rendering panels
│   │   ├── mod.rs           # Panel module exports
│   │   ├── chat/            # Chat panel (virtual view for large histories / message blocks)
│   │   ├── config.rs        # Configuration panel
│   │   ├── logs.rs          # Logs panel
│   │   ├── memory.rs        # Memory panel
│   │   ├── plugins.rs       # Plugins panel
│   │   ├── help.rs          # Help panel
│   │   └── board.rs / events.rs
│   └── wizard/              # First-run setup wizard (data-driven: steps registry is the SSoT)
├── Cargo.toml               # Crate manifest (agentrt-tui, binary: agentrt-tui; depends on agentrt-rs)
└── README.md                # This file
```

## Prerequisites

The TUI is a client of a running AgentRT runtime. Install the runtime first:

```bash
curl -fsSL "https://api.atomgit.com/api/v5/repos/openairymax/agentrt/contents/scripts/install.sh?ref=main" | python3 -c 'import json,sys,base64;sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["content"]))' | bash
```

(A compatibility entry `https://atomgit.com/openairymax/agentrt/releases/download/latest/install.sh` is also available.) Then start the gateway, e.g. `airymaxrt start`.

### Gateway Endpoint

The gateway address is resolved in this order:

1. `--gateway-url <url>` command-line flag;
2. `AGENTRT_GATEWAY_URL` environment variable;
3. `$AIRY_HOME/run/gateway.port` — the launcher records the actual gateway port there when the default port is taken;
4. built-in fallback `http://127.0.0.1:8080`.

Other environment variables: `AIRY_HOME` (data root, default `~/.airymaxrt`), `AGENTRT_TUI_LOG` (log file path, default `$AIRY_HOME/logs/agentrt-tui.log`), `AIRY_TUI_THEME` (theme override; `COLORFGBG` is used for auto-detection).

## Panels

| Panel | Shortcut | Description |
|-------|----------|-------------|
| **Chat** | default | Conversation panel: enter prompts and view agent replies |
| **Help** | `F1` | Help panel: keyboard shortcut reference |
| **Config** | `F2` | Configuration panel: view and edit runtime config |
| **Logs** | `F3` | Logs panel: live runtime log stream |
| **Memory** | `F4` | Memory panel: inspect agent memory contents |
| **Plugins** | `F5` | Plugins panel: manage loaded plugins |
| **Board** | `F6` | Task board (live updates via gateway event stream) |
| **Events** | `F7` | Event stream view |

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+C` | Quit |
| `Ctrl+X` | Abort the current background request (task execution / pending reply) |
| `Ctrl+Z` | Pause / resume waiting (the request keeps running in the background) |
| `Ctrl+T` | Open a new session tab (unavailable while a task is running) |
| `Alt+1`–`Alt+9` | Switch session tabs (`Alt+1` returns to the primary session) |
| `Esc` | Return to the Chat panel |
| `F1`–`F7` | Switch to the corresponding panel |
| `F8` | Switch to the terminal CLI (`airy_cli`) |
| `Tab` | Complete `/` commands and skill names (Chat panel) |
| `Enter` | Submit input |
| `Alt+Enter` | Insert a newline (multi-line input) |
| `Backspace` | Delete character |
| `↑` / `↓` | Scroll up / down |
| `Alt+↑` / `Alt+↓` | Browse input history (`Alt+↓` returns to manual typing) |
| `PageUp` / `PageDown` | Page scroll |
| `End` | Jump to the bottom (latest message) |

## Installation

### From source

```bash
cd tui
cargo build --release
# Binary: ./target/release/agentrt-tui
```

Or install it into your cargo bin path:

```bash
cargo install --path .
```

**Requirements:** Rust edition 2021 (stable toolchain). `agentrt-rs` (shared protocol client; wire constants generated from the C header `airy_run_stream.h`). Runtime dependencies: `ratatui` 0.28 + `crossterm` 0.28 (TUI framework), `reqwest` 0.12 (HTTP, rustls), `tokio` 1 + `tokio-stream` + `futures` (async + streaming), `serde` / `serde_json` (serialization), `clap` 4.5 (argument parsing), `thiserror` / `anyhow` (errors), `log` / `env_logger` (logging), `chrono`, `unicode-width`.

## Usage

### Launch

```bash
# Start the TUI (connects to the default Gateway)
agentrt-tui

# Specify Gateway URL and agent config
agentrt-tui --gateway-url http://127.0.0.1:8080 --agent-file agents/main.agent.yaml

# Resume a previous session / open a specific project directory
agentrt-tui --resume
agentrt-tui --project ./my-agent-project

# Or use an environment variable
export AGENTRT_GATEWAY_URL=http://127.0.0.1:8080
agentrt-tui
```

`--agent-file` defaults to `agents/main.agent.yaml`.

### Conversational workflow

1. After launch, type a message in the **Chat** panel (a first-run wizard helps with initial setup).
2. Press `Enter` to submit; the agent streams its reply in real time.
3. Use `↑` / `↓` to scroll through history.
4. Press `F1` for the full help reference.

### Panel navigation

- `F3` — switch to the logs panel for a live runtime log stream.
- `F4` — switch to the memory panel to inspect stored agent memory.
- `F6` / `F7` — task board and event stream views.
- `Esc` — jump back to the chat panel.

## Build & Test

```bash
cargo build --release
cargo test
./target/release/agentrt-tui
# Ensure the Airymax Gateway is running first.
```

## License

Dual-licensed under **AGPL v3 + Apache 2.0** (SPDX: `AGPL-3.0-or-later OR Apache-2.0`). See [LICENSE](LICENSE) for full text.

Copyright (c) 2025-2026 **SPHARX Ltd.** All Rights Reserved.
