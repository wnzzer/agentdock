# 产品边界：编排壳，而不是自研 Agent 客户端

## AgentDock 自己做什么

- 远程 workspace 和宿主机进程编排
- Claude Code/Codex session 创建、绑定、恢复和布局
- Endpoint Profile 与会话级配置
- PTY/WebSocket 远程访问
- 文件树、预览、编辑和 Git 工作流
- 可拖拽 pane、无限 split、通知和状态聚合
- Workspace Layout Engine：比例、最小尺寸、Stack/Tab、布局持久化和恢复
- 连接恢复、日志、审计和安装部署

## AgentDock 不重复实现什么

- Agent 推理循环
- Prompt/context 管理的完整实现
- 工具调用执行器
- Claude Code/Codex 的权限确认语义
- 模型路由和供应商协议的全部细节
- 官方客户端已有的终端交互能力

## 适配原则

每个 provider adapter 只完成四件事：

1. 检测 CLI 版本和能力。
2. 根据 Endpoint Profile 生成 session-scoped 启动配置。
3. 启动、连接、停止和恢复官方 CLI 进程。
4. 将 PTY/结构化输出映射成 AgentDock 的通用 session 事件。

当官方客户端新增功能时，优先通过 adapter 透传，而不是在 AgentDock 里复制一套实现。

## 核心产品判断

AgentDock 的长期扩展性来自 Workspace Layout Engine：只要新能力能注册成 pane，就能进入现有工作区，不需要重新设计整套页面。比例划分解决空间组织，Tab/Stack 解决空间不足和状态恢复，Pane Registry 解决功能扩展。

## 权限策略

AgentDock 提供最小公共策略：`native`、`interactive`、`trusted`、`plan`、`blocked`。这些是用户意图，不是统一的底层权限实现。`plan` 目前只对 Claude Code 暴露，直接转发其原生 plan 模式；Codex 不支持时拒绝，而不是伪造等价能力。每个 adapter 负责将意图映射到 provider 原生能力，并返回实际生效结果；不支持时必须显示降级状态。
