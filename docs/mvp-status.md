# MVP 验收记录

2026-09-09，macOS / Rust + Vue。

## 2026-09-15 更新：会话标题与 Claude 原生命名

- 会话已有持久化 `title` 字段，现在从侧边栏或会话页菜单可直接重命名；桌面使用小弹窗，侧边栏使用行内编辑。保存只 PATCH 标题，不启动/停止进程，不改变 Session ID、工作区、原生恢复 ID、历史或已打开的窗口。
- 同一个 Session 被拖入多个分屏/页签时，所有绑定窗口会同步新标题并保留 pane/layout ID；刷新后从 SQLite 恢复。
- 新建或显式重新打开的隔离 Claude Code 会话将标题透传为原生 `--name`。导入的官方配置/原生历史会话仍由 Claude Code 自己维护命名，避免改写客户端历史身份。
- 当前新增/更新 API 测试覆盖非法标题、非法 ID、身份保持、Claude 参数透传和多窗口同步。

## 2026-09-14 更新：模型选择、原生计划模式与问题选择器

- 新建会话和端点配置使用可搜索的 ModelPicker：优先展示远端/native catalog 返回的模型名称与真实 ID，同时保留手动输入未知 ID 的能力；支持桌面上下键/Enter 和手机点击。模型目录条目可带 `efforts`，不会把 Codex 未公布的推理级别猜出来。
- Claude Code 的 `low / medium / high / xhigh / max` 思考深度通过原生 `--effort` 转发；Codex 写入官方 app-server 使用的 `model_reasoning_effort` 配置。profile 可保存默认值，新会话可单独覆盖，都会进入不可变 session snapshot；原生宿主机配置引用不允许覆盖。
- Claude Code 的 `plan` 权限意图直接转发原生 `--permission-mode plan`；Codex 不伪造等价模式，选择该值会被拒绝。原生 `AskUserQuestion` / `requestUserInput` 的单选显示下拉、多选显示复选项，并支持 `Other` 自由输入，答案仍通过原生控制请求回传。
- 补齐 profile 更新和 session 创建 API 对 `effort` 的持久化/校验；SQLite schema 9 新增 profile 默认值，旧数据库自动迁移。
- 当前验证：Node **350 项**、Rust **87 项**，Vue typecheck、Vite build、Rust fmt、Clippy `-D warnings` 全部通过。5173 前端保持运行；8787 仍有 1 个用户会话运行，本次未重启后端，磁盘上的新二进制已编译但尚未替换活动进程。

## 2026-09-14 更新：Claude 手动额度查询改为官方 OAuth usage

- 修正了截图中的根因：旧实现只看 `auth status` 和可选 status-line 快照，因此登录正常也始终没有 5h/周额度。现在点击“查询用量”后，优先读取官方 native store 中的 OAuth access token，请求 `https://api.anthropic.com/api/oauth/usage`，兼容 CPA/CLIProxyAPI 使用的 `utilization`、`resets_at`、`five_hour` 和 `seven_day` 字段。
- macOS 仅在导入的默认 `~/.claude` 且原生 `CLAUDE_CONFIG_DIR` 为 unset 时查询 Keychain `Claude Code-credentials`；隔离账号不会误用宿主机 Keychain。Linux/兼容环境仅尝试账号配置目录下的官方 `.credentials.json` / `credentials.json`。凭证只在 bridge 内存中使用，不返回前端、不写日志、不写 SQLite。
- 查询仍然是显式动作，不轮询、不发模型 prompt；官方 OAuth 查询失败时才降级到 status-line 快照。Claude 的额度重置仍不提供，因为 CPA 的 reset-quota 是代理内部 cooldown，不是 Anthropic 订阅额度重置。
- 额外修正 Claude 默认登录提示：原生目录环境为 unset 时不再强制拼接 `CLAUDE_CONFIG_DIR`，避免 macOS Keychain 上下文被切换。
- 本轮新增 bridge/API fixture 覆盖 CPA payload、字符串 reset 时间、OAuth 请求头、Keychain 与隔离目录边界；Node **350 项**、Rust **87 项**、typecheck/build/fmt/Clippy 全部通过。真实 Claude usage 请求尚未替用户点击执行。

## 2026-09-11 更新：会话面板视觉收敛与键盘交互

- 移除 Agent 会话中重复的工作区栏、Session 外壳标题和常驻运行信息，标题只由画布 Tab 与聊天内容承担；三点操作菜单固定在对话区右上角。运行信息改为按需打开的只读弹窗，避免三层标题堆叠。
- 对话输入框改为紧凑字号：桌面正文 13px、标题层级 16px，手机输入保持 16px。聚焦输入时高亮整个 composer 边缘，内部 textarea 不再显示单独的矩形 outline。
- 键盘行为已接通：Enter 发送、Shift+Enter 换行、Ctrl/⌘+Enter 发送；IME 组合输入不会误提交。Esc 按优先级关闭菜单、取消结束/端点确认、取消当前回复，最后才让输入框失焦，不会误结束 Session。
- Session 外壳和 Chat/Native 子视图继续共用同一个 `session.id`；只切视图不发 HTTP、不 start/stop，不复制会话。普通 PTY 会话默认展示聊天壳，需要显式操作才打开原生终端或启用结构化聊天。工作区栏仅在文件/Git/无绑定面板显示。
- 当前验证：主页面 5173 无 alert、聊天宽度正常且无横向溢出；样例手机容器 390px 下输入字号 16px、操作按钮最小高度 44px。全量 Node 测试 **280 项**、Rust 测试 **100 项**继续通过，typecheck/build/fmt/Clippy 通过。

## 2026-09-10 更新：主页面已切换到新会话 UI

- 按用户授权，主后端 8787 已完成一次有序重启并升级到 SQLite schema 7；13 条会话记录、Session ID、工作区关系、端点快照和共享画布均保留。重启会结束由旧进程管理的运行实例，因此当前记录为 stopped；没有删除或重建会话。
- 5173 已确认连接新的 8787：`/api/health` 返回 `structured_chat`、`official_accounts`、`session_configuration` 和 `session_environment`。主页面现在将 Claude Code/Codex 会话放入统一 `SessionPane`：默认对话视图，原生终端是同一 Session ID 的备用视图；Terminal 仍始终是原生终端。
- Browser 实测主画布中的 `data-session-id`、运行信息和窗口内容使用同一个 `0aceef71-3b84-4b48-87bf-1578a25d8b63`，聊天视图和端点配置下拉均已出现。切换工作区/布局不会改写该 ID；视图切换不调用 start/stop 或创建新会话。旧 PTY 会话只有点击“启用对话界面”才转换为结构化模式，避免重启后偷偷接管旧终端。
- 官方账号入口已在主页面可见；当前没有账号数据，新增账号、OAuth、额度查询/重置仍需用户显式操作。账号目录和状态数据在独立目录受保护，不把 OAuth token 回显到 UI。
- 主页面当前没有活跃 Agent 进程（重启后的预期状态）；需要使用时从会话的“原生终端”或“启用对话界面”入口显式打开。主后端 PID 为 27770，5173 Node/Vite 正常监听。

## 最新：结构化聊天、同 CLI 端点切换与官方账号

- 新 Web Agent 会话默认结构化聊天：Codex app-server stdio、Claude 现有 CLI stream-json。标准事件驱动 Markdown 正文、折叠工具卡、原生权限/问题卡、多选回答、轮次和 token 信息。取消仅中断当前轮次，结束进程仍放在需确认的二级菜单。旧 HTTP 默认 PTY、旧运行中终端不自动接管。
- 输入框旁可以选择同 CLI 的普通端点或官方账号 Profile。显式确认且无活动轮次/待审批后关闭旧 bridge，使用新 revision 配置目录；旧 UI 记录保留，旧 native context、账号环境不转发到新端点。每次 Web 发送/手动重试固定配置 revision，另一个窗口换配置后会拒绝旧版本派发。
- 消息先保存 UUID/SHA-256 回执与用户事件，再派发；同 ID 不重复发送，JSON 202 回执可确认已裁剪的旧事件。失败/断线不会自动重放。SQLite v7 保留 2,000 事件 / 8 MiB 展示窗口，同时保护最新控制状态与最多 32 张未处理审批卡，避免长输出挤掉待确认问题；退出/重启会废弃旧审批状态。
- 原生 bridge 初始化/写入有 deadline，JSONL 单帧 192 KiB、文本 64 KiB 上限，异常明确失败。关闭前给专属进程组退出时间，再清理 TERM 忽略的后代；actor 取消也释放自己的组。最后回归修正了确定未入队的消息仍卡 busy、exit 重复记录及退出发布时序。
- 官方账号作为可复用 Profile：受保护的独立目录、`account.json` 元数据，Codex 自己维护 `auth.json`。新增 device/browser 登录、取消、状态/额度读取、手动 token 刷新、明确注销。额度重置仅消耗官方返回的 earned reset credit，需确认及持久幂等授权；持久化成功 ACK 后才发官方消费请求，不伪造重置、不调用反代。
- Claude 不提供第三方订阅 OAuth 登录/额度代理：登录仍由官方 CLI 管理；额度查询改为用户主动触发官方 OAuth usage 请求，未支持项明确显示不可用。原生 headless 跳过交互式 workspace-trust 提示的区别已在首次界面说明；工具批准仍由官方客户端请求和执行。
- **273 项 Node 测试、100 项 Rust 测试**、真实 Vue SFC 类型检查、生产构建、fmt 与 Clippy `-D warnings` 通过。Rust → 实际 Node bridge → 两种合成原生协议完成消息/审批/去重/打断/配置切换回归；账号测试覆盖凭据不泄漏、目录权限、维护竞争、官方重置已知结果、授权持久化及进程组退出。
- Browser 使用真实 `ChatSessionPane` 的开发只读预览 `http://127.0.0.1:5173/ui-preview.html`：桌面/390px 容器切换，实际窄容器宽 388px 无横向溢出，输入 16px，发送/审批按钮高度 44px。验证本地中文输入、工具卡展开、发送/审批/端点动作全部禁用；样例显式标记，不连接 Agent 或模型。这是浏览器窄容器测试，非手机真机验收。
- 独立 8788 预览升级 SQLite v7 后，实测本机 **Codex 与 Claude Code 原生初始化都返回 ready/running**；没有发送提示词、创建模型轮次或执行真实 OAuth/额度消费。只结束自己创建的两条 `Structured handshake QA · … · no prompt` 会话，记录保留为 stopped。
- 预览创建一个 `OAuth metadata QA · not signed in` 账号：目录 0700，元数据/config 文件 0600，状态 unknown，不冒充登录。生成的 Profile 不允许从普通端点页孤立删除。以上测试数据只保留在 `.agentdock/preview-v2`，未复制到主库、未读取真实凭据内容。
- **已知边界**：载入旧 native history 可继续原上下文，但旧完整正文暂不回填到聊天 UI，并在视图说明；未知原生交互 fail closed。尚未执行真实授权后的付费模型回合、实际 OAuth 登录/额度消费、Linux 实机或手机真机验收。
- **本段为重启前历史记录**：当时 8787 的 4 个用户活跃会话未被中断；2026-09-10 在用户明确授权后已完成主后端有序重启，详见本页顶部更新。新能力现已通过 5173 主页面提供；会话记录和窗口绑定保留，旧 PTY 进程按重启语义结束。

## 最新：Node 开发链、连接修复与高级环境配置

- 当前开发链已改为 Node 24+ / pnpm 10.30.3：依赖、Vite、真实 Vue SFC 类型检查、Node 测试和历史桥接一致。版本文件与 macOS/Linux CI 目标为 Node 24；本机实际验证使用 Node 26.5.0，没有降级全局 Node。旧 Bun 锁文件和版本标记仅归档留存，下方 Bun 记录属于历史验收。
- 5173 原先前端退出导致刷新 Failed to fetch，恢复后页面与文件树刷新正常；读请求与结果不明的写请求采用不同连接错误提示，不自动重放写操作。健康刷新可清理旧读连接错误。
- “一直连接中”定位到 Bun-run Vite 的 WebSocket 代理：原生进程已 running，直连 8787 可握手、经旧 Vite 超时；切 Node 后两路径都成功 open。只读取握手状态，没有发送输入、resize 或记录会话输出。额外增加实际 Vite 配置 → 临时 fixture upstream 的长期代理回归，测试结束确认停止监听。
- 会话状态区区分进程运行与输出流连接；8 秒握手超时进入可重连状态，清理旧处理器和计时器，不自动重启 Agent 或重放输入。Browser 在原 5173 页面确认“已连接 / 实时 · 原生客户端”。
- Tab 使用有色 SVG 图标，按真实 session provider 判定 Claude/Codex；Git、文本、媒体、终端有各自前景和浅色底。Browser 实测 Claude `rgb(183,91,39)`、Git `rgb(8,125,104)`，并保留既有矢量图标许可。
- 原生进程继承后端宿主机环境，不快照/回传整套宿主机变量。新增 Profile 默认环境、创建/历史载入覆盖、已有会话编辑：Literal / SecretRef / Unset；账号目录和内部变量不可覆盖，敏感值用启动时解析的引用。旧历史未记录的临时 exports 无法还原，配置优先级仍由原客户端决定。
- 新会话草稿按工作区/provider/profile、历史草稿按工作区/source 分开，保留未完成行，切账号不串值。原生 Profile 允许名称与进程级环境模板，不改源配置文件。已有会话可从侧栏齿轮直接编辑，不需要先打开/启动；运行中只读，保存不启动/停止，修改下次显式启动生效。
- SQLite v6 保存显式环境 map，默认 `{}`，不改旧快照。PATCH 必须 stopped/failed 且 runtime 非活跃，通过 operations 锁防启动竞争；历史重复载入省略环境保留既有值，不同显式值返回 409。旧后端无 `session_environment` 能力时禁用编辑、说明升级原因，不请求缺失路由或静默丢弃草稿。
- 自动验证：**186 项 Node 测试、79 项 Rust 测试**全部通过；真实 SFC typecheck、生产 build、Rust fmt、Clippy `-D warnings` 通过。Node 冻结依赖安装通过。没有执行认证后的付费模型回合，也没有宣称 Linux 实机验证或云端 CI 已执行。
- 空闲的 8788 预览后端已升级 v6（`.agentdock/preview-v2`），实测 capability、Profile 环境快照、会话完整替换、保留变量/明文敏感值拒绝及引用原样返回。保留一个 `Environment QA · blocked fixture` 配置和一个 `Environment QA · never started` 的 stopped 记录作验收样本；从未启动原生客户端、未读取真实密钥、未复制任何预览数据到主库。
- Browser 在 5173 确认旧后端环境弹窗默认展开、添加/保存禁用并显示升级说明；打开停止会话的齿轮不会启动它，关闭弹窗后原活跃窗格仍已连接，刷新无 alert。8788 的浏览器访问先前被安全设置拦截，本轮未绕过，不宣称新版预览的完整浏览器交互验收。
- **部署边界：原 PID 82687 / v5 主库已在 2026-09-10 授权重启时替换。** 当前 8787 为 schema 7 / PID 27770，13 条会话记录保留但运行中 PTY 已结束；5173 Node 前端现在连接新能力。

## 已实现并验证

- Workspace 注册/两级折叠导航；只登记现有目录，不复制项目。当前共享画布不再随工作区选择整屏切换。
- 一级 Changes 工作区，真实 Git diff、文件/全部暂存、取消暂存、commit 表单。
- 文件目录导航、文本编辑、保存冲突提示；草稿跨 Tab 和布局重建保留。
- 图片加载、视频播放元数据和媒体 Range 请求。
- 原生 Claude Code/Codex/可选 shell 会话；显式打开时连接、结束在二级菜单，按 session ID 绑定窗口。
- Endpoint Profile CRUD 和不可变 session snapshot；配置目录按 session 隔离。
- 递归 split、拖拽到边缘分裂、中心合并为 Tab、预设比例、最大化/恢复。
- 拖拽分隔线到低于最小尺寸后折叠为 Tab；保存/刷新/恢复后保留窗口和内容。
- 390px 手机视口无整页横向溢出，主工作区自动折叠为 Tab。
- SQLite WAL、无损升级早期 M0 数据库、Host/Origin/CSRF 检查、可选 token-cookie 认证。
- 前端使用 vue-tsc 校验真实 SFC，不再使用宽松 Vue 模块声明规避类型检查。

## 验收证据

- 早期验收基线为 Rust 16 项、Web 41 项；本轮扩展结果见下节。
- Rust fmt/clippy -D warnings、Vue SFC typecheck、Vite production build 通过。
- 浏览器在独立测试仓库完成：文件读取 → 编辑 → Tab 切换保留草稿 → 保存 → Diff → Stage → Commit。测试提交已由 Git log 核实。
- 浏览器原生终端键入 printf，返回 UI_TERMINAL_OK；Reconnect 后同一进程仍存在，输出回放。
- 浏览器实际拖动文件 Tab 与侧边栏 session，验证中心 docking；分隔线压缩后显示 Responsive tabs，再恢复比例。
- 原生 Codex 在隔离配置目录启动，显示官方登录界面。没有代用户登录，没有代发付费模型请求。
- PNG 自然尺寸 640×360，MP4 元数据时长 2 秒、readyState 4（本地产生的测试素材）。

## 明确没有宣称完成的部分

- 未做认证后的模型生成回合验收；需要使用者在原生客户端登录或设置自己的 API-key reference。
- PTY 在浏览器重连时继续运行，但不跨服务进程重启。原生 CLI 历史保留，服务重启后需显式重新启动/使用原生 resume。
- 未做多租户/强 OS 隔离、系统 Keychain 管理页、账号自动导入、完整权限语义统一。
- 未做 Monaco/LSP、hunk staging/discard、Git pull/push/PR、电脑桌面控制、插件市场。
- Linux CI 工作流已添加，尚未在这台 macOS 主机上宣称完成 Linux 实机部署验证。
- 临时验收仓库仅用于测试，未修改或提交用户其他项目。
- 本轮发现了正在运行的用户创建会话，保留其进程；测试仅停止了自己创建的 acceptance 会话。后台生命周期细节修正将于下次正常服务启动生效，不强制终止用户会话。

## 后续验证入口

`cargo test --workspace` 测 API/持久化/PTY；`pnpm test` 测布局、文件树、请求/草稿、连接状态机、环境编辑与历史桥接；`pnpm run build:web` 包含 vue-tsc。初始化使用 `pnpm run init`，不是 pnpm 自带的 `pnpm init`。

## 本轮：Bun、文件树、历史载入与初始化

- Bun 1.3.14：workspace、bun.lock、开发/构建脚本、测试、Vite 和原生历史桥接已切换。Rust 继续 Cargo；仅 Vue SFC 类型检查因 Node loader hook 兼容性保留 Node 24，不使用宽松 `.vue` 声明绕过检查。
- 干净临时目录中，从 `bun.lock` 冻结安装、SFC 类型检查、生产构建与 82 项 Bun 测试全部通过。不依赖原 pnpm node_modules。
- Bun Vite 开发服务在独立端口启动并返回页面，已结束该临时服务；原 5173/8787 服务未重启。
- 通过 Browser 技能在验收工作区实际验证：三级文件树懒加载 → 打开文件 → 折叠父目录 → 预览定位重新展开并选中；中英切换后新增标签同步翻译。切到已停止的 Codex 窗格不会自动启动。
- 原生历史通过官方 SDK / app-server 读取当前项目元数据；本机两来源均返回空列表，无模型请求、无自动 resume。fixture 覆盖跨工作区拒绝、显式同意、去重、Unicode流、分页边界、原始配置绑定和隐私字段。
- 在临时 `AGENTDOCK_HOME` 连续 init 两次成功：schema 3、SQLite WAL、目录 0700。未迁移用户原始数据，也未声称已提供一键下载/系统服务安装器。
- 已增加端口预留与 state/database 独占锁：重复启动先失败，不再提前重置 running 记录；init 同样持锁且不执行 reconcile。
- 原 8787 服务核验仍保留 4 个用户运行中会话。
- Rust 本轮 43 项（server 35、persistence 4、runtime 4），含启动端口冲突、数据库/状态锁、init不改运行状态、文件描述符继承后锁释放回归；连续 5 轮全量测试通过，fmt/clippy 通过。
- Browser 新建专用验收终端：创建即连接，关闭窗格后后台仍 running，重新打开可连接，二级菜单确认结束；结束后切 Tab / 整页刷新不再自动复活。仅结束该自建测试进程。

## 本轮：引入宿主机现有配置

- 新入口：端点配置 → 引入现有配置。引入成功后，新会话可选“宿主机现有 Codex 配置”或“宿主机现有 Claude Code 配置”。只保存来源引用，不拷贝账号令牌或原配置。
- SQLite v4 持久化 provider/source/canonical path/目录变量上下文，重复引入返回原 profile；可改名称或移除引用，源文件和旧会话快照保持不变。
- 原配置为共享的实时引用，不是隔离账号或配置内容快照；原生客户端仍负责登录缓存/刷新和历史读写。模型、端点、代理、权限由原客户端控制，本选项不支持这些字段的覆盖。
- 59 项 Rust 测试通过（persistence 13、runtime 4、server 42），89 项 Bun 测试通过；fmt、Clippy、SFC 类型检查和生产构建通过。
- 测试覆盖：未确认/任意路径拒绝、认证保护、两 provider 新会话启动参数、原配置字节/权限不变、不创建 session 专属配置目录、重复引入/并发去重、改名删除后引用保留、模型覆盖拒绝、目录和环境上下文变更拒绝、符号链接重定向拒绝、旧数据库升级。
- macOS 只读原生登录状态对照：Claude 默认环境已登录；强设同一路径目录变量后未登录；保持原来的 unset 状态后仍已登录。Codex 原始/保留环境的登录状态一致。仅检查官方命令报告的状态，不输出令牌、不触发登录或模型请求。
- 独立 8788 预览已升级 v4，并引入两项宿主机配置引用；二次引入返回相同 UUID，预览 session 数量仍为 0。未启动原生客户端会话。
- 原 8787 服务有用户活跃会话，未重启/迁移其数据库。8788 的 Browser 导航仍被 ERR_BLOCKED_BY_CLIENT 阻断，按 Browser 技能未绕过；新接口已实测，不宣称新预览整页交互验收。
- Browser 在可访问的 5173 页面确认“端点配置”内“引入现有配置”入口可见且说明共享边界；该页面仍连接旧后端，没有在那里执行引入。原服务最终核验保留 8 个活跃会话。

## 本轮：两级工作区导航与跨项目共享画布

- 工作区 → 会话两级展开；文件/Git/新建是工作区快捷操作。跨工作区点击/拖拽会话加入同一画布，选择工作区不清空布局。支持置顶、模糊搜索、记忆展开/折叠。
- 窗格与标签显示所属工作区；文件 ID 包含工作区与路径，Git/文件操作固定引用自己的工作区。右侧文件树可跟随焦点或固定；比例划分、拖拽和自适应 Tab 保持不变。
- SQLite v5 新增共享布局与 revision/CAS，不覆盖旧的每工作区布局。旧后端不请求新接口：明确显示“保存在此浏览器”；不支持的原生配置/历史/目录/模型发现禁用并提示升级，避免不透明的 API route not found。
- Browser 在原 5173 实测：选择 B 后 A 编辑器仍在，保存只修改 A；两个项目同名 README 同时存在、草稿互不覆盖；文件树固定 B 时聚焦 A 不切走；B 编辑器与 A Git 窗格成功并排。
- 在专用 `/tmp/agentdock-shared-canvas-qa.IvfxOP/{a,b}` 测试仓库通过 UI 验证：A 暂存后 A 为 index M，B 仍仅 worktree M；提交仅进入 A，B 的 Git log 仍为初始提交。未提交用户项目。
- 浏览器测试结束已移除测试窗格，原有 4 个窗格逐一比对保留；整页刷新后布局仍在。新增的两个 QA 工作区引用及其临时目录保留为验收样本，未删除用户数据。
- 独立 8788 的 v5 共享接口实测：首次保存 revision 1；过期写入返回 409，深层内容比较确认布局未被改动。已检查 JSON 键排序不会被误判为内容变化，并增加回归。
- 新版预览只登记测试目录/保存文件与 Git 窗格，不启动模型或原生会话；原 8787 的 8 个活跃会话保持运行。
- 最终校验：111 项 Bun 测试、68 项 Rust 测试（persistence 16、runtime 4、server 48）、类型检查、生产构建、fmt、Clippy 全通过。Browser 确认旧后端的“引入现有配置”按钮禁用并显示升级说明，不再请求该缺失接口。

## 修正：引入后新会话仍落到独立配置

- 本次实查：导入的 Claude 配置已存在，但随后创建的会话 `endpoint_profile_id` 和 `endpoint_snapshot` 仍为 null，因此实际使用独立配置目录。官方只读 `claude auth status --json` 报告宿主机原登录仍有效。
- 新会话现在优先有效的已记忆配置；无记忆时唯一宿主机引用自动选中，多个引用要求明确选择。引入成功会立即将该项加入父级列表并设为此浏览器的默认选择，避免异步刷新尚未返回时新建又落到空配置。
- 保留显式独立配置选项并说明可能需要单独登录；当前已创建会话不改绑、不停止、不复制凭证。已有独立会话在有宿主机引用可用时显示区别说明。
- 新增偏好与实际 SFC 渲染回归，覆盖默认选中、跨 provider、多个账号阻止盲选、独立配置记忆、存储异常、删除后的回退和模型输入隐藏。渲染测试不打开监听端口、不调用模型或启动 CLI。
- 本轮 Browser 对本地地址的导航被拦截，未绕过安全设置；以组件渲染和只读 API/官方状态命令核对，不宣称交互式模型登录验收。
- 最终 125 项 Bun 测试、类型检查和生产构建通过；SFC 渲染实际验证原生配置被选中、多账号未选择时创建按钮禁用、不会串到另一 provider。原运行会话及后端进程未停止或改绑。
