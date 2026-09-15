# 权限边界

## 不由 AgentDock 实现的部分

- Claude Code 的工具授权和确认
- Codex 的 sandbox、命令确认和信任设置
- provider 对工具调用的具体语义
- provider 对危险命令的判断

这些能力由各自 CLI 原生实现。AgentDock 只启动 CLI、显示原生提示、透传用户输入，并保留原始事件和输出。

## 统一抽象映射

AgentDock 只提供一组最小公共语义，不试图覆盖两个 provider 的全部权限模型：

```text
PermissionMode
├── native       使用 provider 默认行为
├── interactive  尽量要求 provider 在危险操作前询问
├── trusted      使用 provider 支持的信任/自动执行模式
└── blocked      AgentDock 不启动或停止当前 session
```

`interactive`、`trusted` 和 Claude Code 专用的 `plan` 不是跨 provider 的强保证，而是启动意图。adapter 必须返回实际映射结果；Codex 对不支持的 `plan` 直接拒绝：

```text
requested: interactive
provider:  codex
effective: native-interactive
supported: true
```

如果某个 provider 不支持目标模式，AgentDock 应明确显示“已降级为 native”，不能静默伪造安全性。

运行中的确认事件也使用最小公共模型：

```text
PermissionRequest {
  tool_kind,
  summary,
  target,
  risk_hint,
  provider_payload,
}
```

UI 只需要支持 `approve_once`、`approve_session`、`deny`、`cancel` 四类动作；具体动作由 adapter 转成 Claude Code/Codex 的原生输入或配置。

## AgentDock 必须负责的部分

- workspace 根目录和路径 canonicalization
- 子进程/进程组生命周期与强制回收
- session 级环境变量和 credential 注入
- CPU、内存、磁盘和超时限制
- WebSocket 身份认证、断线恢复和审计
- 不把 secret 写入日志、事件流或前端状态

这不是第二套 Agent 权限系统，而是远程宿主机的基础运行边界。默认不自动传入 provider 的危险 bypass 选项；`trusted` 必须由用户明确选择，并显示 provider 和实际生效模式。
