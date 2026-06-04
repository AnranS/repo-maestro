# F-112 — Run monitor projection (design)

Status: design (awaiting implementation) · Owner: dali design / dafu
implementation

Parent: [acpus absorption plan](ACPUS-ABSORPTION.md) — borrowed direction #2

## Problem

Maestro now has three machine-readable building blocks:

- F-110 `findings.ndjson` records run-time findings.
- F-111 `PlanPreview` / `Issue` provides a stable dry-run and validate envelope.
- F-114 specialist profiles record writer / reviewer provenance on each task.

The local dashboard still reads mostly raw `RunState` plus several adjacent
endpoints (`/evidence`, `/findings`, `/replay`, `/diff`). That is good enough
for one UI, but it makes every surface independently answer the same questions:

- Is this run active, done, blocked, waiting for approval, or failed?
- Which task is currently actionable?
- Which tasks are blocked by dependencies or human approval?
- What findings matter right now?
- Which specialist profile handled a task?
- What should an MCP client or TUI render without learning `RunState` internals?

F-112 adds a **read-only projection contract** for monitor surfaces. It does not
replace `RUN_STATE.json`; it summarizes it into stable JSON that WebUI, TUI, and
future MCP tools can consume without scraping or coupling to scheduler internals.

## Goals

- Add a stable `RunMonitor` projection for run-level status, progress, approvals,
  blocked work, active work, findings summary, and budget/usage.
- Add a stable `TaskDetail` projection for task-level status, dependencies,
  artifacts, risk, findings, approval state, and F-114 specialist provenance.
- Keep the projection **pure and read-only**: no state mutation, no event writes,
  no runtime side effects.
- Reuse existing F-110 finding types and F-111 issue style where applicable.
- Keep v1 small enough to land in reviewable commits and to support current
  WebUI/TUI needs.

## Non-goals

- No new workflow engine or scheduler behavior.
- No replacement of `RUN_STATE.json`, `events.ndjson`, `findings.ndjson`, or
  evidence artifacts.
- No write APIs, approval decisions, retry controls, profile editing, or training
  UI.
- No graph redesign or dashboard relayout in v1.
- No cross-run analytics, retention policy, or historical aggregation.
- No internal project names, real paths, transcripts, screenshots, or private
  examples in docs/tests.

## API surface

Add two endpoints:

| method | path | response |
|---|---|---|
| `GET` | `/api/runs/:id/monitor` | `RunMonitor` |
| `GET` | `/api/runs/:id/tasks/:task/detail` | `TaskDetail` |

Both endpoints are pure reads:

1. resolve `:id` with the same `current` semantics as existing run handlers;
2. load `RUN_STATE.json`;
3. read F-110 findings with `read_findings` (empty if none);
4. project into schema structs;
5. return JSON.

Errors follow existing dashboard conventions:

- unknown run -> `404`;
- invalid run/task id -> `400`;
- parse/read failure -> `500`;
- unknown task in a known run -> `404`.

## Schema

Types should live in a new module, recommended:

```text
src/schema/monitor.rs
```

`schema/mod.rs` exposes `RUN_MONITOR_V1` / `TASK_DETAIL_V1` constants.

### `RunMonitor`

One JSON object:

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.run_monitor.v1` |
| `run_id` | string | from `RunState.run_id` |
| `status` | string | existing run status string |
| `spec` | string | existing run spec; already user-provided |
| `started_at` | string | existing RFC3339 string |
| `ended_at` | string? | existing RFC3339 string when present |
| `progress` | `RunProgress` | task counts and settled count |
| `active_tasks` | `MonitorTaskRef[]` | running tasks |
| `blocked_tasks` | `MonitorTaskRef[]` | pending tasks whose deps failed / are not done |
| `approvals_pending` | `MonitorTaskRef[]` | pending human approval tasks |
| `findings_summary` | `FindingSummary[]` | count by kind + severity |
| `usage` | existing usage object? | copy through current usage shape |
| `budget_tokens` | u64? | current run budget |
| `updated_at` | string? | projection time, RFC3339; optional for deterministic tests |

`RunProgress`:

| field | type | notes |
|---|---|---|
| `total` | u32 | number of tasks |
| `done` | u32 | status `done` |
| `failed` | u32 | status `failed` |
| `running` | u32 | status `running` |
| `pending` | u32 | status `pending` |
| `cancelled` | u32 | status `cancelled` |
| `settled` | u32 | done + failed + cancelled |

`MonitorTaskRef`:

| field | type | notes |
|---|---|---|
| `task_id` | string | task id |
| `project` | string | project id |
| `status` | string | task status |
| `kind` | string | task kind (`agent` / `verify`) |
| `title` | string? | task prompt / command summary, if already in state |
| `resolved_agent_profile` | string? | F-114 writer provenance |
| `resolved_review_profile` | string? | F-114 reviewer provenance |
| `risk_level` | string? | existing task risk |

`FindingSummary`:

| field | type | notes |
|---|---|---|
| `kind` | string | F-110 finding kind |
| `severity` | string | F-110 severity |
| `count` | u32 | number of matching findings |

No finding details in `RunMonitor` v1. Detailed finding rows already exist at
`/api/runs/:id/findings`; the monitor summary is only for triage badges.

### `TaskDetail`

One JSON object:

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.task_detail.v1` |
| `run_id` | string | parent run |
| `task_id` | string | requested task |
| `project` | string | task project |
| `status` | string | task status |
| `kind` | string | task kind |
| `agent` | string | task agent backend |
| `role` | string? | task role used at dispatch |
| `resolved_agent_profile` | string? | F-114 writer provenance |
| `resolved_review_profile` | string? | F-114 reviewer provenance |
| `depends_on` | string[] | upstream task ids |
| `downstream` | string[] | computed from the plan/order/state where available |
| `risk_level` | string? | existing task risk |
| `attempts` | u32 | existing attempts |
| `started_at` | string? | existing task timestamp |
| `ended_at` | string? | existing task timestamp |
| `approval` | `TaskApprovalState?` | pending / approved / rejected when known |
| `artifacts` | `TaskArtifactSummary[]` | log / report / diff / trajectory refs only |
| `findings` | `Finding[]` | F-110 findings for this task id |
| `last_error` | string? | one-line task error, sanitized / as stored |

`TaskArtifactSummary` is a small link description, not file contents:

| field | type | notes |
|---|---|---|
| `kind` | string | `log`, `trajectory`, `diff`, `report`, `pr_body`, etc. |
| `path` | string? | run-relative artifact ref only |
| `available` | bool | whether handler can serve it |

V1 must not return absolute paths. If the underlying task state stores absolute
`workspace_path` / `worktree_path`, omit them from `TaskDetail` v1 or replace
with booleans such as `has_worktree`.

## Projection rules

### Active tasks

`active_tasks` are tasks with status `running`.

### Approval pending

Use `RunState.approvals_pending` as the source of truth and project each task id
to a `MonitorTaskRef` when the task exists. Missing task ids are ignored and
should be covered by a projector unit test.

### Blocked tasks

V1 uses a deterministic, local rule:

- task status is `pending`;
- at least one dependency is in `failed` or `cancelled`; OR
- dependency exists but is not `done` while the run itself is terminal.

Do not infer "blocked" from wall-clock age in v1. Abandoned / stale run detection
remains doctor / liveness logic.

### Findings summary

Read all F-110 findings for the run and group by `(kind, severity)`. The grouping
must be deterministic: sort by kind, then severity.

`TaskDetail.findings` includes only findings whose `task_id == requested task`.
Run-level findings stay out of task detail unless v2 adds a separate
`run_findings` section.

### Privacy

- Artifact refs must be run-relative (`logs/T0.log`, `REPORT.md`) or endpoint
  URLs; never absolute paths.
- Do not include `workspace_path`, `worktree_path`, raw prompt transcript, raw
  logs, or filesystem roots in v1 projection.
- `last_error` is allowed because it is already in `RUN_STATE.json`, but tests
  should cover that projection does not synthesize new absolute paths.

## Implementation order

### Step 1 — Schema + pure projector

Add `src/schema/monitor.rs`:

- schema constants;
- `RunMonitor`, `TaskDetail`, and helper structs;
- pure builders:
  - `RunMonitor::from_state_and_findings(state, findings, updated_at)`;
  - `TaskDetail::from_state_and_findings(state, task_id, findings)`.

No server code in Step 1.

Tests:

- serde round-trip for monitor and task detail;
- done / running / approval-pending / blocked fixture projections;
- findings summary count / sort is deterministic;
- no absolute paths in artifact refs.

### Step 2 — Backend handlers

Wire:

- `/api/runs/:id/monitor`;
- `/api/runs/:id/tasks/:task/detail`.

Keep handlers thin: resolve run dir, load state, read findings, call the pure
projector, return JSON.

Tests:

- known run returns schema version + counts;
- unknown run returns 404;
- unknown task returns 404;
- task detail returns only task-scoped findings.

### Step 3 — Minimal surface wiring

V1 UI scope is intentionally small:

- WebUI may call `/monitor` for run-level badges/counts instead of hand-rolling
  them where convenient.
- WebUI may call `/tasks/:task/detail` from the task expander if it removes
  duplicated direct state reads.
- TUI may use the same projector internally, but no redesign.

Do not add new editing controls, training UI, or profile management UI.

Tests:

- `pnpm -C web build`;
- existing Rust full gate;
- one neutral screenshot after dogfood if requested.

### Step 4 — Dogfood

Use neutral fixture names only:

- `billing-service`;
- `web-frontend`;
- `shared-contracts`.

Dogfood cases:

1. done run with findings summary;
2. running run with one active task;
3. approval-pending run;
4. blocked run with failed dependency;
5. task detail showing F-114 specialist / reviewer provenance.

Report only category/count data. No real workspace names, paths, transcripts, or
screenshots containing private content.

### Step 5 — Public sync

After private CI/gate is green and dogfood passes:

1. clean export tracked files only;
2. run release-scope secret scan with private wordlist;
3. public mirror receives a normal additive commit, not force-push;
4. paste public HEAD + CI run.

## Resolved decisions

1. **Type location** — `src/schema/monitor.rs`, exported from `schema/mod.rs`.
2. **F-110 reuse** — task detail returns actual `Finding` rows; run monitor only
   returns grouped counts.
3. **No absolute paths** — artifact refs are run-relative or omitted.
4. **No state writes** — F-112 is read-only. It must not append events/findings,
   modify `RUN_STATE.json`, or touch control markers.
5. **V1 backend first** — schema/projector + handlers land before any UI wiring.
6. **MCP/CLI monitor command deferred** — useful, but belongs to F-112b after
   the HTTP projection is stable.

## Review checklist

- Does the projection answer "what needs attention now" without exposing raw
  scheduler internals?
- Are all fields either existing state, F-110 findings, or deterministic
  derivations?
- Does the schema avoid absolute paths and raw transcript/log content?
- Can WebUI/TUI consume it without needing to know `RunState` internals?
- Are tests fixture-only and neutral?
