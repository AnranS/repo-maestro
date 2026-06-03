# 共享记忆：L1 事实 + L2 决策

`maestro` 与"在 N 个目录里分别调用某个 agent CLI"最大的区别是：它维护一个**所有 Agent 之间共享的知识层**。每个任务在 prompt 真正送给模型之前，会先被拼上一份对应切片的前言。

记忆分为两层：

| 层 | 路径 | 寿命 | 谁写 |
|---|---|---|---|
| **L1 事实** | `.maestro/memory/l1_facts/<topic>/*.md` | 永久，手动维护 | 你（或 Agent 通过 `maestro-action`） |
| **L2 决策** | `.maestro/memory/l2_decisions/<project>/*.md` | 永久，仅追加 | reports 模块在每次 run 之后自动写入 |

## L1 事实

按主题划分的、可持续复用的知识，比如：

- `api/auth-scheme.md` —— "我们用 HS256，每个环境一份轮转密钥"
- `design/typography.md` —— "Inter 用于 UI，JetBrains Mono 用于代码"
- `schema/openapi-conventions.md` —— "每个 endpoint 都要明确定义 400 和 401"

```text
.maestro/memory/
└── l1_facts/
    ├── api/
    │   └── auth-scheme.md
    ├── design/
    │   └── typography.md
    └── schema/
        └── openapi-conventions.md
```

项目通过 `memory_scope` 订阅主题：

```yaml
projects:
  login-api:
    memory_scope: [api, schema]
  login-web:
    memory_scope: [design]
```

`login-api` 的任务被派发时，`maestro` 会读 `l1_facts/api/` 和 `l1_facts/schema/` 下所有 `.md` 文件，作为 system 前言拼到 prompt 顶部。

### CLI

```bash
maestro memory ls
maestro memory put api/auth-scheme < notes.md
maestro memory get api/auth-scheme
maestro memory rm api/auth-scheme
```

### 在 UI 里改

**上下文** tab 有一个按主题分组的事实列表。在浏览器里 CodeMirror 改完直接保存就是落盘。

## L2 决策

每次 `maestro run` 之后，reports 模块会给每个被改动的项目追加一条 markdown 决策条目，总结这次发生了什么。

```text
.maestro/memory/
└── l2_decisions/
    ├── login-api/
    │   └── 20260516-101205-add-jwt-login.md
    └── login-web/
        └── 20260516-101205-add-jwt-login.md
```

决策条目里包含：

- run id 和时间戳
- 这个项目下涉及的任务 id
- 一段简短摘要（从任务输出 + 聊天会话里自动提取）
- 你在聊天里写下的"为什么决定 X / 因为 Y"

L2 也会被自动注入——按时间倒序，每个项目最多注入最近 5 条，避免 prompt 无限膨胀。

## 为什么要两层？

| | L1 | L2 |
|---|---|---|
| 用途 | 不随单一功能变化的参考资料 | "什么时候决定了什么"的痕迹 |
| 可编辑 | 手动维护，结构由你定 | 仅追加，结构由 `maestro` 决定 |
| 注入策略 | 受 `memory_scope` 过滤 | 永远按项目 + 最近 5 条 |

如果你发现自己反复在改某个 L2 文件——它其实应该是 L1 事实。把它升级一下。

## 隐私

两层都在本地。除非你把它们喂给 Agent（按定义就是会发到模型 API），否则没有任何东西离开你的机器。会话自动打标签时只看**用户侧**消息——不会把 assistant 的输出也送过去。
