# AgentDock UI 视觉方案

## 1. 设计关键词

专业、安静、清晰、有生命感。

AgentDock 不做“霓虹赛博风”的炫技界面，而是以明亮、克制的工作台为基底，用少量青绿色表示运行状态，用紫色表示 Agent 智能能力，用橙色表示需要用户注意的操作。

## 2. 整体布局

```text
┌────────────────────────────────────────────────────────────────────┐
│ AgentDock · workspace       ● Connected   branch main   ⌘K  avatar │  Topbar
├──────────────┬──────────────────────────────┬─────────────────────┤
│              │                              │                     │
│  Workspace   │  Agent session / Editor      │  Inspector / Preview │
│  Navigator   │  tabs + terminal + diff      │  file / image / log  │
│              │                              │                     │
├──────────────┴──────────────────────────────┴─────────────────────┤
│  Git status · changed 4 · sync · task progress · CPU/memory         │  Statusbar
└────────────────────────────────────────────────────────────────────┘
```

- 左侧 Navigator：workspace、文件树、一级 Changes/Git 入口、Agent 会话、搜索。
- 中央 Workbench：可拖拽、可调整比例的面板画布；Agent 对话和代码是主视图，终端作为可选面板或独立 tab。
- 右侧 Inspector：文件预览、变更详情、Agent 计划、运行日志。
- 底部 Statusbar：Git 分支、修改数、任务状态、连接状态、资源使用。
- Changes/Git Diff 是一级工作区：展示变更文件列表、分组 diff、stage/unstage、discard、commit 和 commit message。
- 支持拖拽调整左右栏宽度，以及把 Agent session 拖进画布生成面板。
- 提供 `1:1`、`2:2`、`1:2:1` 等布局预设，也允许用户保存自定义布局。
- 分隔线可拖拽调整 pane 大小，并自动吸附到 `1:1`、`1:2`、`2:1` 等规则比例。
- pane 小于最小可读尺寸时自动折叠为父级 tab/stack；恢复空间后保留并恢复原有比例。
- 窄屏时右侧面板变为可滑出的 drawer，画布面板变为可切换 stack。

## 3. 品牌配色

### Light（默认）

| Token | 色值 | 用途 |
|---|---|---|
| `bg.canvas` | `#F5F7FA` | 页面底色 |
| `bg.surface` | `#FFFFFF` | 面板 |
| `bg.elevated` | `#FFFFFF` | 悬浮层、输入框 |
| `border.subtle` | `#E3E8EF` | 分隔线 |
| `text.primary` | `#17202D` | 主文字 |
| `text.secondary` | `#5E6B7D` | 次要文字 |
| `text.muted` | `#8B97A8` | 辅助信息 |
| `brand.cyan` | `#0F9F93` | 品牌色、在线状态、主操作 |
| `brand.violet` | `#7461D9` | Agent 能力、智能建议 |
| `accent.blue` | `#3978D3` | 链接、选中态 |
| `accent.orange` | `#C77916` | 警告、待确认 |
| `accent.red` | `#D94B55` | 错误、停止、删除 |
| `accent.green` | `#168A58` | 成功、已提交 |

### Dark（可选）

以 `#0F131A` 为 canvas、`#171D26` 为 surface，保留 cyan/violet 品牌色；不单独设计第二套组件，只切换 token。

## 4. 状态语义

- 运行中：cyan 点 + 轻微呼吸动画
- Agent 思考中：violet 图标 + 流动渐变
- 等待用户确认：orange 点，不使用弹窗打断主工作区
- 成功：green 短暂提示，随后回到中性状态
- 错误：red 图标 + 可展开错误详情
- 断线：灰色连接图标 + 顶部可恢复提示

颜色只表达状态，不作为唯一信息；始终配合图标、文字和 tooltip，保证可访问性。

## 5. 组件风格

- 圆角：面板 12px，按钮/输入框 8px，标签 999px。
- 阴影：只给浮层使用，面板主要依靠白色层级、浅灰边框和轻微阴影区分。
- 字体：Inter（UI）+ JetBrains Mono（代码、终端、Git）。
- 图标：Lucide，线性图标，统一 16/18/20px 尺寸。
- 动效：150–220ms，ease-out；避免大面积持续动画。
- 主按钮使用 cyan，Agent 建议使用 violet，危险操作必须使用 red。

## 5.1 Workspace canvas

Workspace canvas 是 AgentDock 的核心交互，不采用固定 IDE 三栏作为唯一布局：

- 左侧 session 列表中的 Agent 可拖入任意 pane。
- 每个 pane 有独立标题、状态、最大化和关闭操作。
- 默认布局为 `1:2:1`：左侧 Agent 对话、中间代码/产物、右侧文件预览。
- `1:1` 适合双 Agent 并行，`2:2` 适合四个工作 pane。
- Git Diff 可以独立作为 pane，也可以固定在工作区左侧；不依赖 Agent Chat 或编辑器才能访问。
- 终端不进入默认首屏，可通过 pane 类型或 Terminal tab 按需打开。
- 终端是普通 pane 类型，可以和 Agent、编辑器、预览互相替换、并排或拆分。
- “无限分裂”使用递归 split tree 表达，而不是写死三栏；每次分裂为横向或纵向两个子节点。
- pane 设置最小尺寸（建议 280×180）；空间不足时自动转为 stack/tab，不继续压缩内容。
- 折叠 pane 仍显示名称、状态和未读数，避免 Agent 或 Git 任务被隐藏。

## 6. 首屏体验

首次进入 workspace 时展示三个明确入口：

1. `Start Agent`：选择 Claude Code/Codex 和 profile。
2. `Open folder`：打开最近目录或创建 workspace。
3. `Resume session`：恢复上次未完成任务。

不要把用户带到空白编辑器；首屏应显示最近会话、Git 分支和 workspace 健康状态。

## 7. 响应式策略

- `>= 1280px`：三栏布局。
- `900–1279px`：左栏可折叠，右栏保持 drawer。
- `< 900px`：单主栏，Navigator/Inspector 通过底部导航或 drawer 切换。
- 移动端只保证查看、对话、任务确认和基础 Git 操作；完整编辑体验以桌面浏览器为准。

## 8. 前端实现建议

```text
apps/web/src/
├── app/              # 路由、主题、快捷键、全局状态
├── components/       # Shell、Sidebar、Workbench、Inspector、Statusbar
├── features/
│   ├── agents/       # 会话、provider、事件时间线
│   ├── workspace/    # 文件树、搜索、预览
│   ├── editor/       # Monaco tabs、diff
│   └── git/          # status、stage、commit、branch
└── styles/tokens.css # 颜色、间距、圆角、阴影、动效 token
```

组件先围绕工作流组织，而不是按“所有按钮放一起”组织；这样后续加入插件和更多 Agent provider 时，信息架构不会被打散。Vue 组件使用 `<script setup lang="ts">`，布局树和 pane registry 保持为独立 TypeScript 模块，避免框架耦合。
