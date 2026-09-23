# AgentDock

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img src="docs/assets/banner-light.svg" alt="AgentDock — one canvas for the official Claude Code and Codex CLIs, on your own machine.">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/wnzzer/agentdock/actions/workflows/check.yml"><img src="https://github.com/wnzzer/agentdock/actions/workflows/check.yml/badge.svg" alt="CI status"></a>
  <a href="https://www.npmjs.com/package/@wnzzer/agentdock"><img src="https://img.shields.io/npm/v/%40wnzzer%2Fagentdock?style=flat-square&label=npm&color=0c8376" alt="npm version"></a>
  <img src="https://img.shields.io/badge/node-%E2%89%A5%2024-0c8376?style=flat-square" alt="Node.js 24 or newer">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-7760b5?style=flat-square" alt="Platforms: macOS and Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="License: MIT"></a>
</p>

<p align="center"><b>English</b> · <a href="README.zh-CN.md">简体中文</a></p>

AgentDock is a browser workspace for the official **Claude Code** and **Codex** CLIs. It runs on the machine where your code lives and puts agent sessions, files, Git diffs and terminals on one canvas you can split and dock, reachable from any browser, including a phone.

It does not have its own agent. Codex is driven through its official `app-server` and Claude Code through its `stream-json` interface, so logins, models, hooks, `CLAUDE.md` and permission prompts all stay native.

<p align="center">
  <img src="docs/assets/screenshot-canvas.png" alt="The AgentDock canvas: two agent sessions side by side, and Git changes with a diff bottom right." width="100%">
</p>

- [Install](#install)
- [Quick start](#quick-start)
- [Commands](#commands)
- [Configuration](#configuration)
- [Remote access](#remote-access)
- [Features](#features)
- [Architecture](#architecture)
- [Security](#security)
- [Documentation](#documentation)
- [Development](#development)

## Install

Requirements:

- Node.js 24 or newer
- [Claude Code](https://www.npmjs.com/package/@anthropic-ai/claude-code) and/or [Codex](https://www.npmjs.com/package/@openai/codex), installed on the same machine

```bash
npm install -g @wnzzer/agentdock
```

The package ships one prebuilt binary per platform, with the web client and native bridge inside it. It has no postinstall step, so `--ignore-scripts` works.

| Platform | Architectures             |
| -------- | ------------------------- |
| macOS    | arm64, x64                |
| Linux    | x64, arm64 (static musl)  |
| Other    | [Build from source](#development) |

Tarballs with `SHA256SUMS` are also attached to every [release](https://github.com/wnzzer/agentdock/releases).

## Quick start

```bash
agentdock
```

This starts the server in the background and prints its URL (by default **http://127.0.0.1:28789/**). In the browser:

1. **Pick a workspace**, which can be any existing directory on the host.
2. **Reuse your sign-in.** Go to *Endpoint profiles → Import existing configuration*. It points at your existing `~/.claude` or `~/.codex` without copying credentials.
3. **Create a session**, or use **Load existing session** to resume a native Claude Code or Codex conversation from this workspace.
4. **Arrange the canvas** by dragging sessions, files, Git and terminals into splits and tabs.

## Commands

| Command             | Description                                                    |
| ------------------- | -------------------------------------------------------------- |
| `agentdock`         | Start the server in the background and wait until it responds  |
| `agentdock --lan`   | Same, but reachable from other machines on the network         |
| `agentdock status`  | Show whether it is running, its URL and the access token       |
| `agentdock logs`    | Print the background server's log                              |
| `agentdock restart` | Stop, then start again                                         |
| `agentdock stop`    | Stop the background server                                     |
| `agentdock serve`   | Run in the foreground (for systemd or another supervisor)      |
| `agentdock init`    | Create the state directory without starting anything           |

If a start fails, the command prints the reason from the log and exits with a non-zero code.

## Configuration

All state is stored in `~/.agentdock/` (SQLite database, settings, logs). There is no telemetry.

Settings are read from `~/.agentdock/config.toml`. An environment variable with the same meaning takes precedence over the file.

```toml
lan = true                  # same as --lan
port = 28789                # or addr = "192.168.0.9:28789"
token = "..."               # access token (AGENTDOCK_TOKEN)
allowed-origins = ["https://dock.example.com"]
```

Commonly used environment variables:

| Variable                    | Purpose                                            |
| --------------------------- | -------------------------------------------------- |
| `AGENTDOCK_HOME`            | State directory (default `~/.agentdock`)           |
| `AGENTDOCK_ADDR`            | Listen address (default `127.0.0.1:28789`)         |
| `AGENTDOCK_TOKEN`           | Access token; required for non-loopback binds      |
| `AGENTDOCK_ALLOWED_ORIGINS` | Extra allowed browser origins, comma-separated     |
| `AGENTDOCK_CLAUDE_BIN`      | Absolute path to `claude`                          |
| `AGENTDOCK_CODEX_BIN`       | Absolute path to `codex`                           |

API keys for custom endpoint profiles are stored only as references such as `env:AGENTDOCK_SECRET_WORK`, and the backend resolves them when a session launches. For the full list, see [Configuration](docs/configuration.md) and [Endpoint profiles](docs/endpoint-profiles.md).

## Remote access

By default AgentDock listens only on loopback. There are three ways to reach it from another device:

- **SSH port forwarding** is the recommended option on networks you do not control:

  ```bash
  ssh -L 28789:127.0.0.1:28789 user@server
  ```

- **`agentdock --lan`** binds every interface and issues an access token. Traffic is plain HTTP, so use it only on a trusted network.
- **An HTTPS reverse proxy** must preserve `Host` and WebSocket upgrades, and its hostname must be listed in `AGENTDOCK_ALLOWED_ORIGINS`.

See [Security and boundaries](docs/security.md#private-remote-access) for details.

## Features

**Canvas**

- Recursive horizontal and vertical splits, drag-to-edge docking, tabs, and `1:1` / `2×2` / `1:2:1` presets.
- Sessions from different projects can share one canvas. Each file and Git pane stays bound to its own repository.
- Closing a pane never ends its process. Layouts are saved on the server, so another browser opens the same layout.

**Agent sessions**

- Structured conversation view with tool cards, native approval and question cards, a context-usage indicator and turn status.
- The model list comes from the installed client, so new models appear when you upgrade it. You can also type any model ID.
- Reasoning effort and Claude Code plan mode are passed to each client's native flags.
- Native session history can be resumed per workspace.
- **Interrupt** and **End session** are separate actions. Input is saved before it is sent and is never replayed after a disconnect.

**Files and Git**

- File tree, workspace search that respects `.gitignore`, and `@path` completion in the composer.
- Syntax-highlighted editing with conflict-checked saves, plus rendered Markdown and image, video, audio and PDF previews.
- Git pane with status, diff, per-file stage/unstage and commit.

**Accounts and environment**

- Import the host's Claude Code or Codex configuration, or create isolated custom endpoint profiles.
- Per-session environment variables, supporting literal values, secret references and explicit unset.
- Official account login and usage through each client's official API. See [Official accounts](docs/official-accounts.md).

**Access**

- Responsive layout that works on phones.
- English and Chinese UI.

## Architecture

```text
  Browser ── Vue 3 canvas: sessions · files · Git · terminals
     │  HTTP + WebSocket, same origin
     ▼
  agentdock-server (Rust) ─────────────── SQLite (WAL) · ~/.agentdock
     ├─ workspace / file / Git services
     ├─ PTY supervisor ──────────────────▶ host shell
     └─ native bridge (Node, JSONL)
           ├─ codex app-server ──────────▶ official Codex
           └─ claude stream-json ────────▶ official Claude Code
```

- **Server (Rust)** handles the protocol, authentication, persistence and process supervision. It does not run an agent loop.
- **Native bridge (Node)** maps each client's official interface to versioned session events, which is why Node is required.
- **Layout engine** treats agents, editors, Git and terminals as pane types.

See [Architecture](docs/architecture.md) and [Structured agent UI](docs/structured-agent-ui.md).

## Security

AgentDock is a workspace for **a single trusted user**. It is not a multi-tenant sandbox.

- Anyone who can reach the API has the permissions of the user running the server. Protect the access token like that user's credentials.
- File access is limited to workspace roots. `.git`, `.agentdock` and client configuration files are refused, and deletion never follows symlinks.
- **Agent processes are not sandboxed.** `claude` and `codex` run with your full user permissions, limited only by their own permission settings.
- A non-loopback bind won't start without a token of at least 24 characters, and Origin and Host must be on the allow-list.

Read [Security and boundaries](docs/security.md) and [Permission boundary](docs/permission-boundary.md) before exposing AgentDock beyond localhost.

## Documentation

| Topic                                  | Documents                                                                                          |
| -------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Configuration, credentials, environment | [Configuration](docs/configuration.md) · [Endpoint profiles](docs/endpoint-profiles.md)            |
| Official accounts                      | [Official accounts](docs/official-accounts.md)                                                     |
| Sessions and the canvas                | [Sessions and the shared canvas](docs/sessions-and-canvas.md) · [Structured agent UI](docs/structured-agent-ui.md) |
| Security                               | [Security and boundaries](docs/security.md) · [Permission boundary](docs/permission-boundary.md)   |
| HTTP API                               | [API contract](docs/api.md)                                                                        |
| Design                                 | [Architecture](docs/architecture.md) · [Product boundary](docs/product-boundary.md) · [UI design](docs/ui-design.md) |
| Upgrading                              | [Settings and update](docs/settings-update.md)                                                     |

## Development

Requirements: Rust stable 1.89+, Node.js 24+, pnpm 10.30.3.

```bash
git clone https://github.com/wnzzer/agentdock.git
cd agentdock
pnpm install --frozen-lockfile
pnpm run dev                                          # → http://127.0.0.1:5173/
```

Run a production build locally:

```bash
pnpm run build:web && cargo run -p agentdock-server   # → http://127.0.0.1:28789/
```

Run the checks before submitting changes:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm run check
```

| Path                     | Contents                                                     |
| ------------------------ | ------------------------------------------------------------ |
| `crates/server`          | HTTP/WebSocket API, daemon, security, accounts, workspace IO |
| `crates/agent-runtime`   | Process and PTY supervision                                  |
| `crates/persistence`     | SQLite store and migrations (`migrations/`)                  |
| `crates/domain`          | Shared domain model                                          |
| `packages/native-bridge` | Node JSONL bridge to Claude Code and Codex                   |
| `packages/protocol`      | DTOs and layout model shared with the client                 |
| `apps/web`               | Vue 3 + TypeScript client and layout engine                  |
| `npm/`                   | Packaging for `@wnzzer/agentdock` and platform packages      |

See [Development](docs/development.md) for more.

## License

[MIT](LICENSE). Third-party material is listed in [third-party notices](docs/third-party-notices.md).

Claude Code and Codex are products of Anthropic and OpenAI respectively. AgentDock is an independent project and is not affiliated with or endorsed by either company.
