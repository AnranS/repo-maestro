# Skills 行动手册

一个 **skill** 就是一个可复用的 Agent 行动手册，以"YAML frontmatter + markdown"的形式存在。Skill 关注的是某一类重复出现的事**怎么做**："我们怎么写一个新 API endpoint"、"iOS 上的 e2e 测试怎么跑"、"怎么清理一个旧分支"。

它们故意保持小而易读。编排 Agent 在派发任务时，如果任务的文本**触发**了某个 skill 的 trigger，就会把这条 skill 拼到任务 prompt 里。

## 结构

```markdown
---
description: 给 FastAPI 路由写 pytest 测试
trigger: write tests, add coverage, pytest, route test
scope: login-api          # 或 "_global" 表示全局
---

# pytest 行动手册

1. 用 `fastapi.testclient.TestClient`。
2. 外部服务一律用 `respx` mock，绝不在测试里打真实 URL。
3. 每个路由至少一个 happy-path 用例，加一个 auth-failure 用例。

```python
from fastapi.testclient import TestClient

def test_login_happy_path(client: TestClient):
    r = client.post("/login", json={"username": "x", "password": "y"})
    assert r.status_code == 200
    assert "token" in r.json()
```
```

frontmatter 字段：

| 字段 | 必填 | 含义 |
|---|---|---|
| `description` | 是 | 一行摘要，列表里展示用。 |
| `trigger` | 是 | 用逗号分隔的若干短语；与任务 prompt 大小写不敏感地做子串匹配，任意一个命中就触发。 |
| `scope` | 是 | `_global` 表示全局，或者填项目名表示仓库本地。 |

Skill 文件路径：`.maestro/skills/<scope>/<id>.md`。

## CLI

```bash
maestro skill ls                              # 按 scope 分组列出
maestro skill new login-api pytest-playbook   # 新建一个空模板
maestro skill show login-api pytest-playbook
maestro skill update [qa-web-flow] [--force]  # 安装/刷新内置 skills
maestro skill sync                            # 镜像到 .cursor/rules + .claude/skills
```

`maestro skill update` 会从当前 `maestro` 二进制里读取内置 skill 目录。默认只补齐缺失的内置
skill，并跳过已有的本地编辑；如果你确实要用内置版本覆盖本地文件，再加 `--force`。

`maestro skill sync` 是关键命令：每条 maestro skill 都会被同步到：

- `.cursor/rules/<scope>__<id>.mdc` —— Cursor IDE 下次 reload 时自动识别
- `.claude/skills/<scope>__<id>/SKILL.md` —— Claude Code 也会自动识别

所以一条 skill 写一次，等于在你用的每个 AI 工具里都生效。保存 skill 时面板会自动触发 sync。

## Trigger 匹配规则

agent 任务被派发时，编排器会注入两类可见 skill（全局 + 当前任务的项目 scope）：

- `tasks[*].skills` 显式声明的依赖 skill。这些是必需项；名字写错会在派发 adapter 前让任务失败。
- `trigger` 命中任务 prompt 的 skill。

这些 skill 会作为一段清晰标记的 Maestro skills 小节进入 prompt。

匹配规则：

- 大小写不敏感的子串匹配
- 竖线分隔的多个 trigger 取并集（OR），例如 `a | b | c`
- 空 `trigger` 等于禁用这条 skill（你可以借此让它只作为文档而存在）

新的 `maestro init` 工作区会自带几条 workflow 护栏 skill，可在 PLAN 里显式引用：

- `workflow-task-guardrails`
- `contract-first`
- `verify-before-done`
- `qa-web-flow` —— 要求 Web QA flow 用真实 headless 浏览器证据验收

## 为什么值得写

没有 skill 时，每次 Cursor 会话都要从零学一遍你的约定。Skill 让你：

1. **固化**一个约定一次，自动被引用到所有相关任务。
2. **跨工具**：sync 到 `.cursor/rules/`，IDE 也能读到。
3. **作用域可控**：全局规则不会污染不相关的仓库。

一个典型工作区会沉淀十来条 skill，覆盖：lint、test、契约变更、错误处理、设计 token、可访问性、运维手册等等。
