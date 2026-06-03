# HTTP API

`maestro ui` exposes an axum server on `127.0.0.1:7777` (configurable). The frontend is just one consumer; everything is also driveable from `curl` or a script.

All response bodies are JSON unless noted; all writes use `application/json`.

## State & runs

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/state` | — | `RunState` of current run, or `null` |
| GET | `/api/runs` | — | `RunSummary[]` |
| GET | `/api/runs/:id` | — | `RunState` |
| GET | `/api/runs/:id/evidence` | — | `RunEvidence` |
| GET | `/api/runs/:id/replay` | — | `RunReplay` reconstructed from events + evidence |
| GET | `/api/runs/:id/pr-body` | — | `text/markdown` draft PR body |
| POST | `/api/runs/:id/cancel` | — | 204 |
| GET | `/api/logs?task=<id>` | — | `text/plain`, streamed |
| GET | `/api/events` | — | `text/event-stream` SSE: events `state`, `ping` |

`:id` accepts the literal string `current` to refer to the active run.

## Chat

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/chat/sessions` | — | `SessionMeta[]` |
| POST | `/api/chat/sessions` | — | `Session` |
| GET | `/api/chat/sessions/:id` | — | `Session` |
| PATCH | `/api/chat/sessions/:id` | `{title?, tags?, cursor_model?, chat_provider?}` | `Session` |
| DELETE | `/api/chat/sessions/:id` | — | 204 |
| POST | `/api/chat/current` | `{id}` | 204 |
| GET | `/api/chat/current` | — | `{id}` or `null` |
| POST | `/api/chat/messages` | `{text, session_id?, model?, provider?, mode?}` | streamed `text/event-stream` |
| POST | `/api/chat/actions/:session_id/:action_id` | — | streamed `text/event-stream` |

## Projects

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/projects` | — | `ProjectsConfig` |
| POST | `/api/projects` | `AddProjectBody` | `ProjectsConfig` |
| DELETE | `/api/projects/:name` | — | `ProjectsConfig` |
| GET | `/api/architecture` | — | `ArchitectureView` (nodes + edges) |

## Memory & skills

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/memory` | — | `MemoryIndex` |
| PUT | `/api/memory/:topic/:id` | `text/markdown` | 204 |
| DELETE | `/api/memory/:topic/:id` | — | 204 |
| GET | `/api/skills` | — | `SkillsByScope` |
| PUT | `/api/skills/:scope/:id` | `text/markdown` | 204 |
| DELETE | `/api/skills/:scope/:id` | — | 204 |

## Models & settings

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/models?provider=<id>` | — | `ModelInfo[]`; omit `provider` for all known providers |
| POST | `/api/models/refresh?provider=<id>` | — | `ModelInfo[]`; omit `provider` to refresh/fallback all known providers |
| GET | `/api/settings/defaults` | — | `DefaultsConfig` |
| PUT | `/api/settings/defaults` | `{agent?, agent_model?, cursor_model?, tagger_model?, branch_prefix?, max_parallel?}` | `DefaultsConfig` |

`agent_model` is the canonical task model field. `cursor_model` is still accepted by settings and project writes as a legacy alias.

## Docs

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/docs/index` | — | `DocsIndex` (groups, pages) |
| GET | `/api/docs/page?file=<path>` | — | `text/markdown` |

## External history

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/external` | — | `ExternalListing` |

## SSE events

`/api/events` emits two named events:

- `state` — fires after every `RUN_STATE.json` change. Data is the full JSON of the file, or `null` if there's no current run.
- `ping` — every 15s for keep-alive.

`/api/chat/.../messages` emits message-delta events for streaming.
