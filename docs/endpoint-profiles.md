# Endpoint Profiles 与会话隔离

## 目标

AgentDock 的端点设置不应该是全局单例。每个 Agent session 在启动时绑定一个 `EndpointProfile`，并保存一份不可变的 effective config snapshot。

```text
EndpointProfile
  ├── provider: claude_code | codex
  ├── endpoint_url
  ├── protocol: anthropic_messages | openai_responses | openai_chat
  ├── model
  ├── auth_ref          # 只保存 secret store 引用，不保存明文
  ├── extra_headers     # 加密保存或引用 secret
  └── capability_flags
  └── permission_mode: native | interactive | trusted | plan (Claude Code) | blocked

AgentSession
  ├── provider
  ├── endpoint_profile_id
  ├── config_snapshot
  ├── workspace_id
  └── runtime_handle
```

## 为什么不能只做一个统一 URL

Claude Code 和 Codex 都是 CLI Agent，但它们的配置模型、认证方式、协议和模型能力不完全相同：

- Claude Code profile 通常需要 Anthropic 侧的 base URL、认证环境变量或第三方云凭证。
- Codex profile 需要 model provider、wire API、base URL、模型和认证策略；其 CLI 也支持通过配置覆盖启动参数。
- 某个网关可能同时暴露 Anthropic Messages 和 OpenAI Responses 兼容入口，但这不是默认保证，不能只靠 URL 猜测。

因此 UI 可以统一，但 provider adapter 必须分别生成启动配置：

```text
ClaudeCodeAdapter  -> session env / temporary settings
CodexAdapter       -> temporary CODEX_HOME + config.toml / -c overrides
```

## 会话隔离策略

1. 用户在启动会话时选择 `Provider + Endpoint Profile + Model`。
2. AgentDock 将 profile 解析成 effective snapshot，写入 session metadata。
3. 启动子进程时只注入当前 session 的环境和临时配置目录。
4. 不修改宿主机全局 `~/.claude` 或 `~/.codex/config.toml`。
5. profile 修改只影响新会话；已有会话需要明确“重启并应用”。

## Provider 原生权限

AgentDock 不重新实现 Claude Code 或 Codex 的工具授权、确认、sandbox 和 trust 语义。启动 session 时由对应 adapter 透传 provider 支持的参数和配置；运行中将 CLI 的确认提示作为事件或 PTY 输出展示，并把用户的确认/拒绝输入回传给原生 CLI。

诸如跳过权限检查的危险启动选项不应被 AgentDock 默默启用；可以作为明确标记的透传选项显示，但不能被产品默认策略替换。AgentDock 自己只负责非语义性的宿主机边界：工作目录、进程组、资源限制、凭证注入和生命周期。

Codex 建议每个 session 使用独立的 `CODEX_HOME`；Claude Code 也使用 session-scoped env/settings 文件，避免并发会话互相覆盖。

## UI 形态

启动 Agent 时显示一个紧凑的配置条：

```text
[Claude Code] [Personal Anthropic] [claude-sonnet] [Connected]  ▾
```

点击后可查看或切换：端点、模型、认证引用、协议、超时、代理和能力开关。密钥只显示名称和末四位，不回显明文。

## 兼容性分级

- L1：官方 CLI + 官方 endpoint，完整支持。
- L2：协议兼容网关，支持基础对话、工具调用和流式输出。
- L3：仅文本兼容或自定义网关，显示能力缺失，不承诺完整 Agent 行为。

AgentDock 应在启动前做 capability probe，并明确告诉用户当前 endpoint 是否支持 tools、vision、streaming、structured output 和长上下文。

## 成本与风险

- Profile CRUD、session 绑定和启动配置生成：低到中等成本。
- 安全存储、并发会话、日志脱敏和配置快照：中等成本，必须早做。
- 让同一个自定义 endpoint 同时兼容 Claude Code 和 Codex：中到高成本，取决于网关是否实现两套协议和工具语义。

首版建议只保证“一个 provider 对应一个 profile”，不要承诺任意 endpoint 双向兼容。
