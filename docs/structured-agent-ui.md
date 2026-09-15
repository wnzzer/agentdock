# 结构化 Agent 对话 UI：实现与边界

2026-09-09 更新。当前代码已实现“结构化消息流 + 折叠工具调用”的适配层；旧 PTY 保留，不把终端画面解析成伪聊天。具体运行环境和验收结果以 [mvp-status.md](mvp-status.md) 为准。

## 当前结构

Web 对话窗格显示消息正文、可折叠工具卡片、原生审批和问题表单、输入框、用量及轮次状态。窄屏使用同一套响应式组件，不另造一个移动端 Agent。普通 Terminal 和既有 PTY 会话继续使用 xterm.js。

浏览器通过鉴权后的 Rust HTTP/WebSocket 接口读写。Rust 负责会话生命周期、独立进程组、命令队列和持久化；Node 桥只转换官方客户端的 JSONL/RPC 消息。推理、工具执行和权限规则仍由官方客户端负责，不实现新的 Agent loop，不向外暴露无鉴权的原生 app-server。

## 原生接口

| 客户端 | 已实现接入 | UI 消费 |
|---|---|---|
| Codex | `codex app-server` 的 stdio JSONL / 双向 RPC | `item/agentMessage/delta`、`item/started`/`completed`、`turn/completed`、原生 approval requests |
| Claude Code | 用户已安装 CLI 的 `--input-format stream-json` / `--output-format stream-json` 与 stdio 控制协议 | assistant/stream_event、tool_use/tool_result、原生 can_use_tool / AskUserQuestion、slash command metadata、interrupt 控制请求 |
| 普通 Terminal | 现有 PTY | 原始字节流，不包装成聊天 |

Codex app-server 官方用于富客户端，包括认证、历史、审批、流式事件。本实现使用本地 stdio；初始化执行 `initialize` / `initialized`，不注入 prompt。首条明确的用户消息才触发 `thread/start` 或 `thread/resume`，然后 `turn/start`。原 `-c` 配置保留，模型参数映射给官方接口。原生 app-server 的 WebSocket 监听仍有 experimental/production 限制，不应据此直接暴露到公网。

Claude 使用原 CLI 程序、工作目录和已构建的进程环境，默认加载 user/project/local 设置来源，通过 `--permission-prompt-tool stdio` 往返原生控制请求；不调用 SDK `query()`，也不提供 Claude 订阅 OAuth 代理或 token exchange。切换成 CLI 不是绕过 Anthropic 限制的理由：遇到不支持的认证或交互时明确报错。客户端本身的启动 hooks/plugins 仍按原生设置运行；桥不主动发送模型请求。

官方 `--print` 模式会跳过交互式 workspace-trust 弹窗，只应用于用户信任的工作区。工具权限模式仍遵循原生设置，桥不加入 bypass 参数，不把默认规则重实现为自己的风控引擎。

## 交互与兼容

- 支持该能力的后端上，新 Web Agent 会话请求 `interaction_mode: structured`。旧 HTTP 调用省略字段时仍默认 `pty`；普通 Terminal 不支持 structured。
- 已运行的 PTY 不接管、不注入控制协议、不无损迁移。用户明确结束旧进程后，才可选择结构化入口。旧后端缺少 capability 时继续兼容原界面，不靠反复调用缺失接口探测。
- 消息、工具、审批、问题、轮次、用量和错误是统一显示事件，不等同于两个客户端拥有相同能力。assistant delta 按 ID 追加，final 替换全文；工具文字和用量是快照，不能重复相加。
- 审批仅提供当前请求支持的 accept/decline/cancel。问题答案按原生 question ID/文本转换；单选显示为下拉选项，多选显示为勾选项，`isOther` 保留自由输入。这样 Claude 的 AskUserQuestion、Codex 的 requestUserInput 以及 plan 类交互可以在 Web 中选择后原样回传；不静默批准，不写入“永久允许”规则。未知 MCP/dialog 等交互明确拒绝或提示使用原生客户端。
- Claude Code 启动时公布的 slash commands 会进入会话 `ready` 事件，输入 `/` 时只展示客户端实际公布的命令并支持键盘补全。Codex app-server 没有 slash-command 协议，因此 AgentDock 不把 Codex 的 `/...` 静默发送给模型，而是提示用户切换原生终端。
- **Interrupt** 转发当前轮次打断，不是结束会话或暂停/恢复整个进程。真正 **End session** 是单独的二级确认操作；关闭 pane 和断开 WebSocket 不结束会话。
- 进程运行与浏览器输出连接是两个状态；会话运行中不意味着输出流已连接。连接失败不自动重试模型请求或重放输入。

## 持久化、限额和配置切换

SQLite schema 9 保存单调递增 `seq` 的标准事件和 endpoint reasoning defaults。最近展示窗口为 2,000 条事件 / 8 MiB，另保留最新控制状态及最多 32 个未解决审批，重连时按 `seq` 合回。超出展示窗口会标记 truncated，不能声称这是完整永久原始日志。用户消息提交回执保存 UUID 与内容 SHA-256，不重复永久保留一份明文 prompt；展示裁剪不会删除幂等回执而导致旧请求重放。

先事务保存用户消息和回执，再发送给原生客户端。202 表示接受请求，不是模型完成确认。重连、重启和失败恢复都不会自动重发旧 prompt。服务器停止/重启后没有活进程恢复保证，但已保存的标准事件仍可读取。

202 响应带 JSON 回执；即使旧展示事件已裁剪，也可用原 UUID 手动确认是否已接受，不需要重放模型请求。Web 的发送/重试同时固定 `configuration_revision`，跨窗口切换配置后，旧版本输入会在派发前被拒绝，不能误发给另一个端点。

桥文本上限为 64 KiB，超过时附截断标记；最终序列化 JSONL 单帧上限为 192 KiB，Rust 还检查自己的 256 KiB 接收上限。未知/超大事件和持久化失败不会被伪装成成功。stderr 不直接转给浏览器，也不通过诊断事件返回进程环境或原始 credential 文件；对话本身仍可能包含敏感用户输入，应保护 SQLite 与备份。

同一 CLI 内切换配置必须明确确认，且当前轮次与审批均已结束。旧 bridge 先关闭，然后清除旧 native conversation/source 关联、递增 `configuration_revision`，使用新 profile 的环境默认值，不沿用旧账号的进程覆盖。生成式隔离配置写入新的 revision 目录，避免复用旧登录缓存。引用宿主机配置时，引用仍指向该账号自己的原生目录，而不是复制出一个不可变文件快照。

此前 UI 历史和回执保留；它们不会作为新 prompt 被发送到另一个端点。后续首条明确消息创建新的 native context。单独的环境编辑仍只允许 stopped/failed 会话，保存本身不启动或停止进程。

## 尚未覆盖的部分

- 已导入 native history 当前用于 CLI resume 上下文，**结构化 Web UI 不回填其更早的完整消息原文**。首次显示为空不代表原生上下文为空。
- 官方只读历史接口可用于未来有界回填，但需要独立导入标记、来源/消息 ID 去重和超限提示；当前没有把此方案当作已实现功能。
- 不提供自研 Agent loop、全能力跨客户端同义映射、第三方 Claude 订阅登录/额度代理，或对任意网关的兼容保证。Claude 额度仅支持用户主动触发的官方 OAuth usage 查询，不能据此扩展出第三方登录或自动额度服务。
- 未据自动化 fixture 声称真实 OAuth、付费模型回合或真实手机触控已验收。

开发路由 `http://127.0.0.1:5173/ui-preview.html` 使用真实组件与明确标注的本地 snapshot fixtures，不调用用户会话。390px 容器只是窄屏预览，不是真机测试。现有 5173/8787 的四个用户会话不能为了新功能被重启；独立 8788 预览的实际验证由主验收记录单独记载。

## 历史只读验证记录（保留）

此前在临时隔离配置目录中完成 initialize 和 model/list 验证，返回 5 个模型条目（当时曾用 Bun 1.3.14 重验）。未发送 thread/turn 提示或模型推理请求，未读取/修改用户全局凭证。开发链现已统一 Node，当前命令为 `node scripts/probe-codex-app-server.mjs`；旧运行时记录仅保留验证历史，不代表仍需安装或使用 Bun。

端点设置中的“原生 Codex 模型目录”现已利用 model/list；它说明目录条目可被发现，不保证当前账号都具有模型使用权。

## 来源

- [OpenAI Codex App Server](https://developers.openai.com/codex/app-server/)
- [OpenAI Codex Authentication](https://developers.openai.com/codex/auth/)
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Claude Agent SDK overview / authentication restriction](https://code.claude.com/docs/en/agent-sdk/overview)
- [Claude Code programmatic usage](https://code.claude.com/docs/en/headless)
- [Claude Agent SDK TypeScript](https://code.claude.com/docs/en/agent-sdk/typescript)
- [Claude network / proxy configuration](https://code.claude.com/docs/en/network-config)

上面的历史核实不替代本轮端到端验收；实现范围见本文，真实运行记录见 [mvp-status.md](mvp-status.md)。
