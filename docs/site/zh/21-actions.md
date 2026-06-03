# `maestro-action` 块

Action 协议是编排 Agent 在对话里提议具体 CLI 命令的方式。面板会把这些块解析成可点击的卡片；命令行用户看到的是同样的纯 YAML，可以直接复制使用。

这是"人在 loop 里"的关键接缝：Agent **建议**，你**确认**，`maestro` 实际**执行**。没有你的明确点击，Agent 不会动你的磁盘。

## 协议格式

一个 action 块就是一个 fence info 为 `maestro-action` 的代码块：

```yaml
verb: work
description: 扫描、计划并校验 login 改动
spec: 给 login 加 JSON 输出
root: ~/work/apps
agent: codex
```

放在 assistant 消息里的任意位置。同一条消息允许多个块。

## 支持的 verb

| Verb | 参数 | 对应到 |
|---|---|---|
| `work` | `spec: <目标>`，可选 `root`、`agent`、`out`、`run`、`dry` | `maestro work <目标> [--root <dir>]` |
| `run` | `plan: <path>` | `maestro run <path>` |
| `rerun` | `plan: <run-id>`，可选 `from: <task_id>` | `maestro rerun <id> [--from <task>]` |
| `approve` | `task: <id>` | `maestro approve <id>` |
| `status` | 无 | `maestro status` |
| `plan_validate` | `plan: <path>` | `maestro plan validate <path>` |

完整列表在 `src/chat/actions.rs::ActionVerb`。

## UI 渲染

每个 action 块会渲染成一张卡片，包含：

- verb 徽章（work / run / approve / ...）
- 一行人类可读标签（`description` 或自动合成的 verb label）
- **运行** 按钮
- **展开** 三角，展示原始 YAML
- **输出面板** —— 命令运行时实时流式接收 stdout/stderr，diff/grep/状态行带语义高亮

执行后卡片会切到：

- ✓ ok —— 绿色边框
- ✗ failed —— 红色边框，输出保留可查
- ↻ running —— 加载图标（多数 action 都是秒级，少见）

## 为什么用 YAML 不用 function call

三个理由：

1. **人可读、可改**。任意 markdown 编辑器都能改这个块。
2. **跨工具**。Cursor 不是所有模型都支持稳定的 tool call 协议，但所有模型都能输出 YAML。
3. **可回放**。聊天历史里的 action 块本身就是一份"Agent 提议过什么、你什么时候点了同意"的审计日志。

## 自己写一段

你可以直接在对话输入框里粘一个 action 块——`maestro` 会同样把它渲染成可点的卡片：

````markdown
```maestro-action
verb: work
spec: 给 CLI 加 JSON 输出
root: ~/work/monorepo
agent: codex
```
````

Skill 也可以用同样的块让用户确认一次 workflow。
