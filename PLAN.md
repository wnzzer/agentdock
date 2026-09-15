# AgentDock 产品与工程计划

> 这是产品路线图，不是完成清单。当前可运行范围与实测边界见 [MVP 验收记录](docs/mvp-status.md) 和 [README](README.md)。

## 1. 产品定位

AgentDock 是一个浏览器优先的远程多 Agent 编排壳：用户在远程 Linux workspace 中运行 Claude Code、Codex 等官方客户端，通过统一 UI 管理会话、workspace、文件预览、编辑、Git、布局和任务状态。AgentDock 不重造 Agent 客户端本身。

核心承诺：打开浏览器即可进入一个可恢复、可隔离、可视化的 Agent 开发环境。

核心产品层：Workspace Layout Engine。通过递归比例划分、可恢复的 Tab/Stack 和可注册 Pane，让 Agent、Terminal、Git、Preview 以及未来插件共享同一个可扩展工作区。

## 2. v0.1 目标

- 支持 Linux 服务器上的宿主机原生 workspace
- 支持 Claude Code、Codex 的适配器和可切换 profile
- 支持 Endpoint Profile 与会话级配置快照，每个 session 独立端点、模型和认证引用
- 复用 Claude Code/Codex 原生权限与确认流程，AgentDock 只做透传、展示和输入回传
- 支持官方 CLI 的 PTY/Web 交互透传、停止/恢复任务
- 支持文件树、代码编辑、图片/视频/Markdown/日志预览
- 支持可拖拽 workspace canvas，提供 `1:1`、`2:2`、`1:2:1` 布局预设
- 支持递归 pane split；终端、Agent Chat、编辑器、预览和 Git Diff 都可作为独立 pane 编排
- 支持 pane 比例调整、规则比例吸附和最小尺寸下自动折叠为 tab/stack
- 支持 Git status、diff、stage、commit、branch
- Git Changes/Diff 作为一级工作区，支持文件级/块级 stage、discard、commit 和变更回溯
- 支持断线重连、任务恢复、基础认证和 workspace 权限
- 前端响应式布局，桌面浏览器优先，平板可用

## 3. v0.1 非目标

- 不做任意桌面电脑控制
- 不做多云 Kubernetes 编排
- 不承诺绕过模型供应商的配额、登录或服务条款
- 不做完整 IDE/LSP 生态，不重做 Claude Code/Codex 的聊天、工具审批和 Agent 推理界面
- 终端不是默认主视图，作为可选 pane/tab 支持
- pane 设置最小尺寸，空间不足时自动转为 stack/tab
- 不在首版同时支持 Windows/macOS 服务器

## 4. 技术基线

- Rust：stable toolchain、Cargo workspace、Tokio、Axum、Serde、Tracing
- 前端：TypeScript、Vue 3、Vite、Pinia、VueUse、Tailwind、shadcn-vue
- 编辑器/终端：Monaco Editor、xterm.js
- 实时通信：WebSocket（终端/Agent 事件），HTTP/REST（资源和命令）
- 持久化：SQLite（v0.1 默认，WAL 模式）；PostgreSQL 作为后续多节点/团队部署选项
- workspace：宿主机目录 + 进程 supervisor；PTY 使用 portable-pty
- Git：优先调用系统 git，并由 Rust service 做安全参数校验
- 部署：单个 Rust server + systemd 用户服务起步；后续可选接入 Podman/microVM

## 5. 目录设计

```text
AgentDock/
├── apps/
│   ├── web/                 # Vue 3 Web UI
│   └── desktop/             # 后续 Tauri 壳，不阻塞 Web MVP
├── crates/
│   ├── api/                 # HTTP/WebSocket 路由、鉴权中间件
│   ├── domain/              # 领域模型、事件、错误类型（无基础设施依赖）
│   ├── agent-runtime/       # Agent 生命周期、PTY、任务恢复、事件总线
│   ├── agent-providers/     # Claude Code/Codex 适配器；未来可插件化
│   ├── workspace/           # workspace 创建、容器、目录和权限
│   ├── filesystem/          # 文件树、读写、上传下载、预览元数据
│   ├── git-service/         # status/diff/stage/commit/branch
│   ├── persistence/         # SQLite/PostgreSQL、迁移、repositories
│   └── server/              # 二进制入口、配置、启动装配
├── packages/
│   ├── ui/                  # 共享 UI 组件和主题
│   ├── protocol/            # 前后端共享事件/DTO/schema
│   └── config/              # TS 配置、lint、构建共享配置
├── migrations/              # 数据库迁移
├── runtimes/
│   ├── host/                # 宿主机原生进程运行时（默认）
│   ├── container/           # 可选 Podman/Docker 运行时
│   └── microvm/             # 后续可选 Firecracker 等强隔离运行时
├── deploy/
│   ├── systemd/             # Linux 用户服务和安装脚本
│   └── compose/              # 可选的服务依赖部署
├── docs/
│   ├── architecture.md      # 架构和边界
│   ├── security.md          # 凭证、沙箱、权限模型
│   └── adr/                 # 重要技术决策记录
├── tests/
│   ├── integration/         # Rust/API/容器集成测试
│   └── e2e/                 # Playwright 端到端测试
├── Cargo.toml
├── package.json             # Bun workspaces and scripts
├── bun.lock
├── rust-toolchain.toml
└── README.md
```

目录边界原则：`domain` 不依赖数据库和 Web 框架；Agent provider 只负责启动和适配官方 CLI；前后端通过 `packages/protocol` 的版本化 schema 通信；Agent 工具审批、聊天交互和推理展示交给 provider 原生客户端，AgentDock 只负责 PTY/事件透传、workspace 编排和宿主机生命周期边界。

## 6. 里程碑

### M0：骨架与技术验证

- 建立 Cargo/Bun monorepo、CI、格式化和 lint
- 打通一个宿主机 workspace、进程 supervisor 和 PTY
- WebSocket 端到端显示终端输出

### M1：可用的远程工作台

- 登录、workspace 列表、启动/停止/重连
- 文件树、Monaco、基础预览
- Git 状态和 diff

### M2：多 Agent 能力

- Claude Code/Codex provider
- profile、会话隔离、任务事件时间线
- 停止、重试、恢复和失败诊断

### M3：产品化

- 权限和审计日志
- 资源限制、健康检查、备份
- 公开部署文档和演示 workspace

## 7. 首要风险

1. Agent CLI 的认证、参数和输出格式可能变化，必须隔离在 provider adapter 中。
2. 远程执行 shell 是高风险面；工具审批复用 provider 原生机制，AgentDock 仍需提供 workspace 目录边界、进程回收、资源限制和审计。
3. 大文件和视频预览会消耗带宽，需流式读取、大小限制和缩略图策略。
4. 实时连接断开后状态恢复是核心体验，事件必须持久化而不是只存在内存。
5. “全平台”优先通过浏览器实现；原生桌面端应等 Web 工作流稳定后再做。

## 8. 第一批需要确定的决策

- 单用户自托管，还是从第一天支持团队多租户
- 何时从 SQLite 切换 PostgreSQL（单节点先用 SQLite，多节点/高并发再切换）
- Claude/Codex 凭证仅用户自带，还是允许服务器托管加密凭证
- workspace 是一次性容器，还是支持长期持久化开发环境
- 是否首版支持自定义兼容网关，还是先支持官方 endpoint

默认建议：先做单用户/小团队自托管、SQLite 持久化、可替换 repository、持久化宿主机 workspace、用户自带凭证；隔离能力通过 runtime 插件按需开启。Endpoint 首版按 provider 分开适配，允许 profile 绑定到 session，但不承诺 Claude Code/Codex 任意双协议兼容。
