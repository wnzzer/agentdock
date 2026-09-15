# 官方账号：实现、存储与限制

账号页管理的是本机官方客户端的账号目录及官方操作，不是新的 OAuth 服务、模型代理或通用 token 导入器。完整请求字段见 [API 合同](api.md#official-accounts)。这里的实现说明不代表已替用户完成真实 OAuth 或额度消费验收。

## 账号与配置的关系

`POST /api/accounts {name,provider}` 创建私密目录、`account.json` manifest 和关联的原生配置 profile；不会自动登录、启动会话或调用模型。Manifest 仅含账号 ID、名称、provider、profile ID，不包含访问令牌。

```text
<state-dir>/accounts/<account-id>/
├── account.json                 # AgentDock 身份与 profile 关联
├── config.toml                  # Codex 原生配置；使用文件式凭证存储
├── auth.json                    # 如存在，由官方 Codex 客户端管理
├── .credentials.json            # 如存在，由原生客户端管理
└── quota-reset-attempts.json     # 已授权 reset 重试键，不含 token
```

Unix 上目录为 0700，受管理 manifest、配置、凭证和 reset 元数据文件为 0600；拒绝把这些文件或账号目录替换成符号链接。原生客户端仍拥有登录缓存及历史的更新权。不要把这些目录提交到 Git、放进公开文件预览目录，或通过普通下载接口暴露。

关联 profile 固定 provider/source/directory/目录环境上下文。管理账号的 profile 不允许在普通配置页被孤立删除；不能留下一个仍存在但失去关联 profile 的账号 manifest。当前没有账号 DELETE API。导入宿主机已有配置是另一条流程：它保留原目录引用，不会把全局凭证复制到上述账号目录。

## Codex：只使用官方接口

- 浏览器和设备码登录走官方 `account/login/start`；页面显示官方 URL／设备码，登录结果由原生客户端写入自己的凭证存储。远程 Linux 上应选择合适的设备码或官方支持的回调/转发方式，不能假定浏览器机器的 localhost 就是服务器。
- 取消登录对应官方 `account/login/cancel`，仅取消该尝试；不批量结束用户 Agent 会话。
- 状态与显式 token 刷新走 `account/read`，其中 `refreshToken` 交给官方客户端处理。AgentDock 不解析 `auth.json`、交换 token 或返回 token 值。
- 注销走官方 `account/logout`，需要明确确认；不是删除账号目录、清空其他账号或结束所有会话。
- Codex 额度通过官方 `account/rateLimits/read` 获取。Claude 的额度通过用户主动点击后的官方 OAuth usage 查询获取。账户或客户端可能不提供某些窗口、套餐或重置时间；未知保持 unknown/null，不能显示成“0%”“0 剩余”或伪造套餐。

每个账号的操作串行化。会改变认证、显式刷新 token 或消费 reset credit 的操作，在该账号仍有活跃会话时返回冲突，**不会自动 stop**。维护/登录进行中也阻止冲突的新启动。只读查看缓存、普通状态查询与改变认证是不同动作。

## 额度重置不是“重置任意限制”

只在官方支持并提供可用 **earned rate-limit reset credits** 时，调用 `account/rateLimitResetCredit/consume`。这不是充值、绕过限流、刷新所有账号额度，或通过私有接口制造重置机会。

要求 `confirmed:true` 和稳定 `idempotency_key`，可选 `credit_id`。调用前重新读取官方 credit 状态，不以过期 UI 数字授权。Rust 先把此次重试键私密、持久化落盘，确认后 Node 才可消费 credit；网络结果不确定时保留同一键以查询/重试原操作，不自动换键重复消费。

仅接受官方已知终态：

| outcome | 含义 |
|---|---|
| `reset` | 官方确认本次重置 |
| `alreadyRedeemed` | 同一次操作已经兑换，不再次计为新重置 |
| `nothingToReset` | 当前没有需要重置的限制 |
| `noCredit` | 没有可用 credit |

错误、unsupported、未知 outcome 或超时不显示为成功。取消等待也不是撤销已发生消费的保证。此功能依赖具体账号资格和客户端 API，不能承诺每个账号都能重置。

## Claude Code：登录由官方客户端管理，额度支持显式查询

账号页不托管 Claude 订阅 OAuth 登录，也不替代官方客户端刷新 token。用户仍通过官方 Claude Code 完成本地配置/登录。

点击“查询用量”时，AgentDock 会从官方客户端的 native store（macOS Keychain `Claude Code-credentials`，或 Linux/兼容环境的官方 `.credentials.json`）在服务端内存中取得 OAuth access token，并请求官方 `GET https://api.anthropic.com/api/oauth/usage`。该请求参考 [CPA Usage Keeper](https://github.com/Willxup/cpa-usage-keeper) 使用的 [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) 逻辑：解析 `five_hour`、`seven_day`、模型窗口的 `utilization` 和 `resets_at`。只有用户点击时触发，不轮询、不把 token 写进数据库/日志/浏览器；凭证找不到或官方接口不可用时明确显示原因。

Claude 的 quota reset 仍不提供。CPA/CLIProxyAPI 的 `reset-quota` 主要清理其代理路由的 cooldown，不等于重置 Anthropic 订阅额度，不能直接当作 Claude 官方额度重置。

Anthropic 官方说明：未经批准，不允许第三方开发者为产品提供 claude.ai 登录或额度，包括基于 Agent SDK 的产品。这里的实现不新增登录流程、不交换 token、不代理模型请求，只在用户主动点击时读取本机官方登录上下文并调用官方 usage endpoint；遇到官方拒绝或接口变化，应明确提示 unavailable，而不是伪造数字。

## 文件维护、备份和验收

- 先确认没有该账号的活跃会话、登录或维护任务，再进行手工文件维护。不要为测试功能自动终止用户会话。
- 私密备份应同时考虑 SQLite 的 profile/session 关联和对应账号目录；数据库使用一致性备份，账号文件另做受限权限、必要时加密的备份。不要只复制 manifest 后假设登录状态也已恢复。
- `auth.json`、`.credentials.json` 的格式和迁移由官方客户端负责。AgentDock 没有原始密钥导入/导出 API，不将凭证文件内容转换成自研认证格式。
- 已运行的老服务不会因刷新前端自动获得 `official_accounts` 等 capability。现有 5173/8787 四个用户会话保持运行；新能力使用独立预览状态与端口验证。
- fixture 验证覆盖协议、隔离、取消、维护互斥和 reset 幂等边界，不等于真实 OAuth 登录、真实 credit 消费、付费模型回合或真实手机设备验收。具体证据由 [mvp-status.md](mvp-status.md) 记录。

## 官方来源

- [Codex App Server：官方账户与限额接口](https://developers.openai.com/codex/app-server/)
- [Codex Authentication：登录与凭证存储](https://developers.openai.com/codex/auth/)
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Claude Agent SDK overview：第三方登录限制](https://code.claude.com/docs/en/agent-sdk/overview)
