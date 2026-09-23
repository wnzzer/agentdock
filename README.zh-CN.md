# AgentDock

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img src="docs/assets/banner-light.svg" alt="AgentDock——在你自己的机器上，用一块画布驾驭官方 Claude Code 与 Codex CLI。">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/wnzzer/agentdock/actions/workflows/check.yml"><img src="https://github.com/wnzzer/agentdock/actions/workflows/check.yml/badge.svg" alt="CI 状态"></a>
  <a href="https://www.npmjs.com/package/@wnzzer/agentdock"><img src="https://img.shields.io/npm/v/%40wnzzer%2Fagentdock?style=flat-square&label=npm&color=0c8376" alt="npm 版本"></a>
  <img src="https://img.shields.io/badge/node-%E2%89%A5%2024-0c8376?style=flat-square" alt="Node.js 24 及以上">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-7760b5?style=flat-square" alt="平台：macOS 与 Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="许可证：MIT"></a>
</p>

<p align="center"><a href="README.md">English</a> · <b>简体中文</b></p>

AgentDock 是官方 **Claude Code** 与 **Codex** CLI 的浏览器工作台。它运行在代码所在的机器上，把 Agent 会话、文件、Git diff 和终端放进一块可分屏、可停靠的画布，任何浏览器（包括手机）都能访问。

AgentDock 本身不包含 Agent：Codex 通过官方 `app-server` 驱动，Claude Code 通过其 `stream-json` 接口驱动，登录、模型、hooks、`CLAUDE.md` 和权限确认都保持原生。

<p align="center">
  <img src="docs/assets/screenshot-canvas.zh-CN.png" alt="AgentDock 画布：两个 Agent 会话并排，右下是 Git 变更与 diff。" width="100%">
</p>

- [安装](#安装)
- [快速开始](#快速开始)
- [命令](#命令)
- [配置](#配置)
- [远程访问](#远程访问)
- [功能](#功能)
- [架构](#架构)
- [安全](#安全)
- [文档](#文档)
- [开发](#开发)

## 安装

要求：

- Node.js 24 及以上
- 同一台机器上已安装 [Claude Code](https://www.npmjs.com/package/@anthropic-ai/claude-code) 和/或 [Codex](https://www.npmjs.com/package/@openai/codex)

```bash
npm install -g @wnzzer/agentdock
```

每个平台对应一个预编译二进制，内含 Web 客户端和原生桥接。安装过程没有 postinstall，`--ignore-scripts` 环境也能正常安装。

| 平台  | 架构                     |
| ----- | ------------------------ |
| macOS | arm64、x64               |
| Linux | x64、arm64（静态 musl）  |
| 其他  | [从源码构建](#开发)      |

每个 [release](https://github.com/wnzzer/agentdock/releases) 也附带 tarball 和 `SHA256SUMS`。

## 快速开始

```bash
agentdock
```

该命令在后台启动服务并打印访问地址（默认 **http://127.0.0.1:28789/**）。在浏览器中：

1. **选择工作区**：宿主机上任意已有目录。
2. **复用已有登录**：进入 *端点配置 → 导入现有配置*，它直接引用本机的 `~/.claude` 或 `~/.codex`，不复制凭据。
3. **新建会话**，或用 **加载已有会话** 恢复该工作区下原生 Claude Code / Codex 的历史对话。
4. **编排画布**：把会话、文件、Git 和终端拖成分屏或页签。

## 命令

| 命令                | 说明                                   |
| ------------------- | -------------------------------------- |
| `agentdock`         | 后台启动服务，并等待它可以响应         |
| `agentdock --lan`   | 同上，但允许局域网内其他机器访问       |
| `agentdock status`  | 查看运行状态、访问地址和访问 token     |
| `agentdock logs`    | 打印后台服务的日志                     |
| `agentdock restart` | 先停止再启动                           |
| `agentdock stop`    | 停止后台服务                           |
| `agentdock serve`   | 前台运行（用于 systemd 等进程管理器）  |
| `agentdock init`    | 只创建状态目录，不启动服务             |

启动失败时会打印日志里的原因，并以非零退出码结束。

## 配置

所有状态都存放在 `~/.agentdock/`（SQLite 数据库、设置和日志），没有任何遥测。

设置从 `~/.agentdock/config.toml` 读取；同名环境变量的优先级高于该文件。

```toml
lan = true                  # 等同于 --lan
port = 28789                # 或 addr = "192.168.0.9:28789"
token = "..."               # 访问 token（AGENTDOCK_TOKEN）
allowed-origins = ["https://dock.example.com"]
```

常用环境变量：

| 变量                        | 用途                                   |
| --------------------------- | -------------------------------------- |
| `AGENTDOCK_HOME`            | 状态目录（默认 `~/.agentdock`）        |
| `AGENTDOCK_ADDR`            | 监听地址（默认 `127.0.0.1:28789`）     |
| `AGENTDOCK_TOKEN`           | 访问 token；绑定非回环地址时必填       |
| `AGENTDOCK_ALLOWED_ORIGINS` | 额外允许的浏览器来源，逗号分隔         |
| `AGENTDOCK_CLAUDE_BIN`      | `claude` 的绝对路径                    |
| `AGENTDOCK_CODEX_BIN`       | `codex` 的绝对路径                     |

自定义端点配置的 API key 只以引用形式保存，例如 `env:AGENTDOCK_SECRET_WORK`，会话启动时才由后端解析。完整列表见[配置](docs/configuration.md)和[端点配置](docs/endpoint-profiles.md)。

## 远程访问

默认只监听回环地址。从其他设备访问有三种方式：

- **SSH 端口转发**：在不受你控制的网络上推荐使用这种方式。

  ```bash
  ssh -L 28789:127.0.0.1:28789 user@server
  ```

- **`agentdock --lan`**：绑定所有网卡并签发访问 token。流量是明文 HTTP，只在可信网络中使用。
- **HTTPS 反向代理**：代理需要保留 `Host` 和 WebSocket 升级，并把它的域名加入 `AGENTDOCK_ALLOWED_ORIGINS`。

详见[安全与边界](docs/security.md#private-remote-access)。

## 功能

**画布**

- 递归横竖分屏、拖到边缘停靠、页签，以及 `1:1` / `2×2` / `1:2:1` 预设。
- 不同项目的会话可以放在同一块画布上；每个文件和 Git 面板都绑定自己的仓库。
- 关闭面板不会结束进程。布局保存在服务端，换个浏览器打开也是同样的布局。

**Agent 会话**

- 结构化对话视图：工具卡片、原生审批与提问卡片、上下文用量和轮次状态。
- 模型列表来自已安装的客户端，客户端升级后新模型会自动出现；也可以直接输入任意模型 ID。
- 思考深度和 Claude Code 的 plan 模式会转成各客户端的原生参数。
- 可按工作区恢复原生会话历史。
- **中断**与**结束会话**是两个独立的操作。输入会先保存再发送，断线后不会重放。

**文件与 Git**

- 文件树、遵循 `.gitignore` 的工作区搜索，以及输入框中的 `@路径` 补全。
- 带语法高亮的编辑器，保存时检查冲突；支持 Markdown 渲染和图片、视频、音频、PDF 预览。
- Git 面板：状态、diff、按文件暂存/取消暂存和提交。

**账号与环境**

- 导入宿主机的 Claude Code / Codex 配置，或创建相互隔离的自定义端点配置。
- 会话级环境变量，支持字面值、密钥引用和显式 unset。
- 通过各客户端的官方 API 登录官方账号、查看用量。见[官方账号](docs/official-accounts.md)。

**访问**

- 适配手机的响应式布局。
- 中英文界面。

## 架构

```text
  浏览器 ── Vue 3 画布：会话 · 文件 · Git · 终端
     │  HTTP + WebSocket，同源
     ▼
  agentdock-server (Rust) ─────────────── SQLite (WAL) · ~/.agentdock
     ├─ 工作区 / 文件 / Git 服务
     ├─ PTY 进程监管 ────────────────────▶ 宿主机 shell
     └─ 原生桥接 (Node, JSONL)
           ├─ codex app-server ──────────▶ 官方 Codex
           └─ claude stream-json ────────▶ 官方 Claude Code
```

- **服务端（Rust）**负责协议、鉴权、持久化和进程监管，不运行 Agent 循环。
- **原生桥接（Node）**把各客户端的官方接口映射为带版本的会话事件，所以 Node 是必需依赖。
- **布局引擎**把 Agent、编辑器、Git 和终端都当作面板类型来处理。

详见[架构](docs/architecture.md)与[结构化 Agent UI](docs/structured-agent-ui.md)。

## 安全

AgentDock 是给**单个受信任用户**使用的工作台，不是多租户沙箱。

- 能访问 API 的人拥有运行服务的用户的全部权限。请像保管该用户的凭据一样保管访问 token。
- 文件访问限制在工作区根目录内，拒绝访问 `.git`、`.agentdock` 和客户端配置文件，删除操作不会跟随符号链接。
- **Agent 进程不受沙箱限制。**`claude` 和 `codex` 以你的完整用户权限运行，只受它们自身权限设置的约束。
- 绑定非回环地址时，没有至少 24 个字符的 token 就不会启动，并且 Origin 和 Host 必须在白名单中。

在 localhost 之外开放 AgentDock 之前，请先阅读[安全与边界](docs/security.md)和[权限边界](docs/permission-boundary.md)。

## 文档

| 主题                 | 文档                                                                                           |
| -------------------- | ---------------------------------------------------------------------------------------------- |
| 配置、凭据与环境     | [配置](docs/configuration.md) · [端点配置](docs/endpoint-profiles.md)                          |
| 官方账号             | [官方账号](docs/official-accounts.md)                                                          |
| 会话与画布           | [会话与共享画布](docs/sessions-and-canvas.md) · [结构化 Agent UI](docs/structured-agent-ui.md) |
| 安全                 | [安全与边界](docs/security.md) · [权限边界](docs/permission-boundary.md)                       |
| HTTP API             | [API 契约](docs/api.md)                                                                        |
| 设计                 | [架构](docs/architecture.md) · [产品边界](docs/product-boundary.md) · [UI 设计](docs/ui-design.md) |
| 升级                 | [设置与更新](docs/settings-update.md)                                                          |

## 开发

要求：Rust stable 1.89+、Node.js 24+、pnpm 10.30.3。

```bash
git clone https://github.com/wnzzer/agentdock.git
cd agentdock
pnpm install --frozen-lockfile
pnpm run dev                                          # → http://127.0.0.1:5173/
```

在本地运行生产构建：

```bash
pnpm run build:web && cargo run -p agentdock-server   # → http://127.0.0.1:28789/
```

提交改动前运行检查：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm run check
```

| 路径                     | 内容                                                |
| ------------------------ | --------------------------------------------------- |
| `crates/server`          | HTTP/WebSocket API、守护进程、安全、账号、工作区 IO |
| `crates/agent-runtime`   | 进程与 PTY 监管                                     |
| `crates/persistence`     | SQLite 存储与迁移（`migrations/`）                  |
| `crates/domain`          | 共享领域模型                                        |
| `packages/native-bridge` | 连接 Claude Code 与 Codex 的 Node JSONL 桥接        |
| `packages/protocol`      | 与客户端共享的 DTO 和布局模型                       |
| `apps/web`               | Vue 3 + TypeScript 客户端与布局引擎                 |
| `npm/`                   | `@wnzzer/agentdock` 及各平台包的打包脚本            |

更多内容见[开发文档](docs/development.md)。

## 许可证

[MIT](LICENSE)。第三方内容列在[第三方声明](docs/third-party-notices.md)中。

Claude Code 和 Codex 分别是 Anthropic 和 OpenAI 的产品。AgentDock 是独立项目，与这两家公司没有隶属或背书关系。
