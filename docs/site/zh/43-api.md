# HTTP API

`maestro ui` 在 `127.0.0.1:7777`（可配置）暴露一个 axum server。前端只是其中一个消费者；所有 API 都可以用 `curl` 或脚本直接驱动。

所有响应体默认是 JSON；所有写请求使用 `application/json`。

## 状态 & 运行

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/state` | — | 当前 run 的 `RunState`，或 `null` |
| GET | `/api/runs` | — | `RunSummary[]` |
| GET | `/api/runs/:id` | — | `RunState` |
| GET | `/api/runs/:id/evidence` | — | `RunEvidence` |
| GET | `/api/runs/:id/replay` | — | 从事件 + evidence 复原的 `RunReplay` |
| GET | `/api/runs/:id/pr-body` | — | `text/markdown` PR 草稿 |
| POST | `/api/runs/:id/cancel` | — | 204 |
| GET | `/api/logs?task=<id>` | — | `text/plain` 流 |
| GET | `/api/events` | — | `text/event-stream` SSE：事件 `state`、`ping` |

`:id` 可以传字面量 `current` 表示当前活跃 run。

## 对话

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/chat/sessions` | — | `SessionMeta[]` |
| POST | `/api/chat/sessions` | — | `Session` |
| GET | `/api/chat/sessions/:id` | — | `Session` |
| PATCH | `/api/chat/sessions/:id` | `{title?, tags?, cursor_model?, chat_provider?}` | `Session` |
| DELETE | `/api/chat/sessions/:id` | — | 204 |
| POST | `/api/chat/current` | `{id}` | 204 |
| GET | `/api/chat/current` | — | `{id}` 或 `null` |
| POST | `/api/chat/messages` | `{text, session_id?, model?, provider?, mode?}` | SSE 流 |
| POST | `/api/chat/actions/:session_id/:action_id` | — | SSE 流 |

## 项目

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/projects` | — | `ProjectsConfig` |
| POST | `/api/projects` | `AddProjectBody` | `ProjectsConfig` |
| DELETE | `/api/projects/:name` | — | `ProjectsConfig` |
| GET | `/api/architecture` | — | `ArchitectureView`（节点 + 边） |

## 记忆 & Skills

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/memory` | — | `MemoryIndex` |
| PUT | `/api/memory/:topic/:id` | `text/markdown` | 204 |
| DELETE | `/api/memory/:topic/:id` | — | 204 |
| GET | `/api/skills` | — | `SkillsByScope` |
| PUT | `/api/skills/:scope/:id` | `text/markdown` | 204 |
| DELETE | `/api/skills/:scope/:id` | — | 204 |

## 模型 & 设置

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/models?provider=<id>` | — | `ModelInfo[]`；不传 `provider` 返回所有已知 provider |
| POST | `/api/models/refresh?provider=<id>` | — | `ModelInfo[]`；不传 `provider` 刷新 / fallback 所有已知 provider |
| GET | `/api/settings/defaults` | — | `DefaultsConfig` |
| PUT | `/api/settings/defaults` | `{agent?, agent_model?, cursor_model?, tagger_model?, branch_prefix?, max_parallel?}` | `DefaultsConfig` |

`agent_model` 是正式的任务模型字段；`cursor_model` 仍被 settings 和 project 写入接口当作旧别名接受。

## 文档

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/docs/index?lang=<code>` | — | `DocsIndex` (groups + pages) |
| GET | `/api/docs/page?file=<path>&lang=<code>` | — | `text/markdown` |

`lang` 可选；未知或缺省时回退到 `en`。

## 外部历史

| 方法 | 路径 | 请求体 | 返回 |
|---|---|---|---|
| GET | `/api/external` | — | `ExternalListing` |

## SSE 事件

`/api/events` 发两种 named event：

- `state` —— 每次 `RUN_STATE.json` 改变时触发。data 是整个 JSON，或 `null`（没有 active run）。
- `ping` —— 每 15 秒一次，保活。

`/api/chat/.../messages` 走的是 message-delta 事件，用于流式 token。
