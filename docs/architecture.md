# AgentDock 架构草案

```text
Browser (React)
   │ HTTPS + WebSocket
   ▼
Rust API/Session Gateway
   ├── Workspace Service ── Host workspace + process supervisor
   ├── Agent Runtime ────── PTY ── official Claude Code / Codex CLI
   ├── Filesystem Service ── file preview/edit
   ├── Git Service ──────── status/diff/stage/commit/branch
   └── Persistence ──────── SQLite (default) / PostgreSQL (scale-out)
```

## 关键边界

- API 层只负责协议、鉴权和请求编排，不直接执行任意命令。
- Agent Runtime 通过 `AgentProvider` trait 接入不同 CLI，统一输出为版本化事件。
- Workspace Service 为每个 workspace 提供独立目录、环境变量、进程组和资源限制。默认直接使用宿主机真实开发环境；需要更强隔离时切换 runtime。
- Filesystem/Git Service 所有路径必须 canonicalize，并限制在 workspace 根目录内。
- 前端只依赖 `packages/protocol` 定义的 DTO 和事件，不耦合 Rust 内部模型。
- SQLite 只存元数据、事件索引和可恢复状态；workspace 文件、预览产物和大日志放在宿主机目录，密钥放在 secret store。

## 建议的核心 trait

```rust
pub trait AgentProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn start(&self, request: StartAgentRequest) -> Result<AgentSession>;
    async fn stop(&self, session_id: SessionId) -> Result<()>;
}
```

第一版不追求微服务化。保持单个 Rust server + 宿主机 workspace，等出现多节点调度需求后再拆 control plane 和 worker。AgentDock 是编排壳，不复制官方 CLI 的 Agent loop、权限系统或工具实现。

## SQLite storage policy

v0.1 使用单个 SQLite 数据库文件，开启 WAL、foreign keys 和 busy timeout。写入通过 persistence 层集中处理，避免多个 runtime 直接竞争写锁；读取可以使用短连接或只读连接。

核心表：

```text
workspaces
endpoint_profiles
sessions
session_events
layouts / layout_nodes
audit_logs
settings
```

不把 API key 写进数据库明文；数据库只保存 `secret_ref`。文件内容、视频和大附件不进入 SQLite，只保存路径、hash、大小和 MIME 元数据。session event 需要保留可恢复的序号，并提供按 workspace/session 的保留策略。

备份使用 SQLite Online Backup/一致性快照，不直接复制正在写入的数据库文件；默认备份数据库、布局、session 元数据和审计日志，不备份可重新生成的缓存。

## Workspace runtime

运行时通过统一接口抽象，默认实现 `HostRuntime`：

```text
HostRuntime (default)
  ├── workspace root directory
  ├── process group / PTY
  ├── env and credential profile
  └── optional systemd-run/bwrap limits
```

`ContainerRuntime` 和 `MicroVmRuntime` 是可选实现，不参与核心业务模型。宿主机模式下不要求构建或维护完整镜像，Agent 使用服务器已安装的工具链、SSH、Git 和项目依赖。

## Workspace layout model

Workspace Layout Engine 是 AgentDock 的核心产品层。Agent provider、Terminal、Git 和 Preview 都只是被布局引擎承载的 pane，不应反过来决定工作区结构。

可编排工作区使用持久化布局树：

```text
Split(horizontal)
├── Pane(AgentChat, session_id=...)
└── Split(vertical)
    ├── Pane(Editor, file=...)
    └── Pane(Terminal, shell=...)
```

每个 `Pane` 都可以再次 split、替换类型、最大化或关闭。布局引擎负责最小尺寸约束和比例调整：拖拽分隔线时支持像素调整，并吸附到 `1:1`、`1:2`、`2:1` 等规则比例。每个 split 节点保存方向、比例、最小尺寸和折叠状态。

当某个 pane 被挤压到最小可读尺寸以下时，不继续缩小内容，而是将它转换成父级的 `Stack`/tab；恢复空间或点击展开后，恢复原来的比例和 pane 状态。终端、Agent Chat、Editor、Preview、Git Diff 都只是 pane 类型，不应在核心模型中拥有特殊的固定位置。

```text
Split(horizontal, ratio=1:2)
├── Pane(AgentChat, min=280x180)
└── Stack(min=280x180)
    ├── Pane(Editor)
    └── Pane(Terminal, collapsed=true)
```

布局树需要版本化持久化，并支持迁移；前端操作统一抽象为 `split`、`resize`、`move`、`replace`、`collapse`、`restore`、`maximize` 和 `close`。新增功能优先注册新的 pane kind，而不是修改核心布局模型。

```text
PaneRegistry
├── agent-chat
├── editor
├── terminal
├── git-diff
├── file-preview
└── future plugin panes
```

Git Changes 是工作区的一等入口，不应只作为某个编辑器 tab 的附属功能。Git service 需要返回文件级和 hunk 级 diff，并支持 stage、unstage、discard、commit 前检查；UI 可以把 Changes 固定为导航项，也可以把它编排成 pane。
