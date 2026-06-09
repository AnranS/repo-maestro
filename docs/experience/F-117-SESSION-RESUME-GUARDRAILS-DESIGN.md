# F-117 — Session / resume guardrails (design)

Status: design / awaiting review · Owner: dali design / dafu implementation

Parent: [reference agent-runtime absorption](REFERENCE-AGENT-RUNTIME-ABSORPTION.md) —
borrowed direction #3

This is a **design document only**. It does not add feature code. It locks the
resume descriptor, validation rules, privacy boundary, and implementation slices
for making `maestro resume` fail closed instead of blindly trusting a stale
on-disk run.

## Problem

`maestro resume` already exists. Today it:

1. resolves a target run directory (`current` by default);
2. loads `RUN_STATE.json`;
3. refuses a run that still appears live unless `--force`;
4. loads the run's `PLAN.yaml` snapshot;
5. starts a **new** run with `skip = done task ids` and
   `seed_skipped_from = previous RunState`.

That model is pragmatic and should stay. The unsafe part is that the command
trusts the previous run state too much. A run may have a stale or partially
updated state, a modified plan snapshot, a corrupt event ledger, missing
workflow-output snapshots for tasks that will be seeded, or an old schema the
current binary no longer understands. Resuming from that state can silently skip
work that was never safely completed.

F-117 adds a small, local resume descriptor plus validation gates before
`maestro resume` seeds any completed task. Resume remains local-first and
single-host; it becomes auditable and fail-closed.

## Goals

- Keep the existing resume shape: create a new run and seed completed tasks from
  a prior run. Do not try to take over a dead scheduler process in place.
- Record a compact `RESUME.json` descriptor for each run: run identity, plan
  integrity, event-ledger cursor, and completed-task checkpoint metadata.
- Validate descriptor + state + plan + events before resume. Refuse unsafe
  resume with actionable messages.
- Treat `--force` narrowly: it may override process-liveness uncertainty, but it
  does not bypass corrupt artifacts, plan drift, or missing seeded outputs.
- Keep all artifacts privacy-safe: no raw prompt, log, transcript, model output,
  absolute path, environment key, or internal workspace name.

## Non-goals

- No cross-host migration or remote session store.
- No in-place process resurrection. F-117 does not attach to a dead agent or
  continue an unfinished adapter invocation.
- No provider chat-session replay beyond existing adapter support
  (`resume_chat_id` remains an adapter concern, not the run-level resume guard).
- No new planner behavior, no DAG rewrite, no rerun replacement.
- No raw artifact body in `RESUME.json`.
- No public mirror sync for this round. Current policy is private-only.

## Existing surfaces to reuse

| Existing surface | Reuse |
|---|---|
| `maestro resume` | Keep command and user-facing semantics; add validation before `run_plan`. |
| `RunState` / `RUN_STATE.json` | Current-state snapshot to seed from, after validation. |
| `PLAN.yaml` snapshot | Plan to resume from; guarded by size + `file_guard::file_hash`. |
| F-115 `events.ndjson` | Source for `last_event_seq` and `last_settled_seq`; corrupt ledger is a hard error. |
| F-112 progress vocabulary | Task statuses map to the same pending/running/awaiting/done/failed/cancelled/skipped buckets. |
| `scheduler::liveness::classify_run` | Existing live/dead/legacy process check. |
| `file_guard` | Existing stable hash format (`fnv1a64:*`) for local integrity guards. |
| `workflow_outputs` snapshots | Seeded done tasks must point only to existing, safe run-relative snapshots. |
| `doctor` | Can later surface descriptor drift; v1 only needs minimal checks. |

## Source of truth

F-117 does **not** replace any existing authority:

- `RUN_STATE.json` remains the authoritative current snapshot of a run.
- `events.ndjson` remains the typed per-run event ledger (F-115).
- `findings.ndjson` remains the durable audit ledger (F-110).
- `RESUME.json` is a **guard descriptor**. It is not a second state file. If it
  is missing, stale, or invalid, resume refuses by default.

The descriptor answers one question: "Is this prior run safe to seed from?"

## Storage

Write one descriptor per run:

```text
.maestro/runs/<run-id>/RESUME.json
```

Use an atomic write with fsync before rename, mirroring `RunState::write_atomic`
discipline. Descriptor write failure must **not** fail the active run; it logs a
warning. The consequence is simple: a later `maestro resume` refuses because the
guard artifact is missing or stale.

## Schema

Types should live in:

```text
src/schema/resume.rs
```

Filesystem helpers should live in:

```text
src/scheduler/resume.rs
```

`schema/mod.rs` exports:

```text
RESUME_DESCRIPTOR_V1 = "maestro.resume_descriptor.v1"
```

### `ResumeDescriptor`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.resume_descriptor.v1` |
| `run_id` | string | parent run id; must match `RUN_STATE.json` and directory id |
| `created_at` | string | RFC3339 |
| `updated_at` | string | RFC3339 |
| `writer_pid` | u32 | pid of the process that last wrote the descriptor |
| `plan` | `ResumePlanRef` | run-relative plan snapshot integrity |
| `event_ledger` | `ResumeEventCursor` | F-115 event ledger cursor |
| `state` | `ResumeStateSummary` | compact status/task summary |
| `seeded_tasks` | `ResumeTaskSeed[]` | done tasks that may be seeded on resume |

### `ResumePlanRef`

| field | type | notes |
|---|---|---|
| `path` | string | must be exactly `PLAN.yaml` in v1 |
| `bytes` | u64 | byte size of the plan snapshot |
| `hash` | string | `file_guard::file_hash` value, e.g. `fnv1a64:*` |

### `ResumeEventCursor`

| field | type | notes |
|---|---|---|
| `schema_version` | string | observed current event schema, expected `maestro.run_event.v2` |
| `last_seq` | u64 | last event sequence in `events.ndjson`, or 0 if no events |
| `last_settled_seq` | u64 | last event seq whose kind settled a task or terminal run |

`last_settled_seq` is computed from F-115 events:

- task-settling kinds: `task.completed`, `task.failed`, `task.cancelled`,
  `task.skipped`;
- run-terminal kinds: `run.completed`, `run.failed`, `run.cancelled`;
- other kinds do not update it.

### `ResumeStateSummary`

| field | type | notes |
|---|---|---|
| `run_status` | string | `running/done/failed/cancelled` |
| `task_total` | u64 | number of tasks in `RUN_STATE.json` |
| `done` | u64 | done task count |
| `failed` | u64 | failed task count |
| `cancelled` | u64 | cancelled task count |
| `skipped` | u64 | skipped task count |
| `awaiting_approval` | u64 | awaiting-approval task count |
| `running` | u64 | running task count |
| `pending` | u64 | pending task count |

The seven task buckets must add up to `task_total`.

### `ResumeTaskSeed`

| field | type | notes |
|---|---|---|
| `task_id` | string | safe task id |
| `project` | string | project id as stored in state |
| `ended_at` | string? | RFC3339 if present |
| `workflow_outputs` | u64 | number of output snapshots available for this task |

Only `Done` tasks are listed. F-117 does not seed failed/cancelled/skipped tasks.

## Validation rules

Validation should be split into a pure report and a CLI wrapper:

```text
ResumeValidationReport {
  run_id,
  can_resume: bool,
  reusable_done_tasks: Vec<String>,
  issues: Vec<ResumeIssue>,
}
```

`ResumeIssue` uses a flat code table and severity:

| code | severity | effect |
|---|---|---|
| `resume.missing_descriptor` | high | refuse unless explicit legacy override is added later |
| `resume.schema_mismatch` | high | refuse |
| `resume.run_id_mismatch` | high | refuse |
| `resume.live_run` | high | refuse unless `--force` |
| `resume.legacy_liveness_unknown` | medium | refuse unless `--force` |
| `resume.terminal_run` | medium | refuse; use `maestro rerun` |
| `resume.plan_drift` | high | refuse |
| `resume.event_ledger_corrupt` | high | refuse |
| `resume.event_cursor_stale` | high | refuse |
| `resume.task_set_mismatch` | high | refuse |
| `resume.seed_output_missing` | high | refuse |
| `resume.seed_output_unsafe_path` | high | refuse |
| `resume.already_complete` | info | no-op |

### Liveness

- `Live` -> refuse unless `--force`.
- `Abandoned` -> allowed if all other validation passes.
- `UnknownLegacy` (`pid == 0`) -> refuse unless `--force`. Unknown liveness is
  not silently treated as abandoned anymore.

`--force` only affects liveness-related issues. It does **not** override corrupt
JSON, missing snapshots, plan drift, or schema mismatch.

### Terminal runs

`resume` is for interrupted runs. In v1:

- `RunStatus::Done` + all tasks done -> no-op, same as today.
- `RunStatus::Failed` or `RunStatus::Cancelled` -> refuse and point to
  `maestro rerun`. A terminal failed/cancelled run is no longer an interrupted
  process; resuming it would blur `resume` and `rerun`.

### Plan integrity

The descriptor's `plan.path` must be `PLAN.yaml`. Recompute
`file_guard::file_hash(run_dir/PLAN.yaml)` and file length on resume:

- hash/bytes match -> ok;
- mismatch -> refuse (`resume.plan_drift`).

No plan content is stored in the descriptor.

### Event ledger integrity

Read `events.ndjson` with the existing F-115 reader and apply an **append-only
compatibility** check (NOT exact cursor match):

- corrupt line, or a sequence that is not strictly contiguous `1..=N` **in
  file-read order** (gap / duplicate / reorder) -> refuse
  (`resume.event_ledger_corrupt`);
- `recomputed.last_seq < descriptor.last_seq` -> refuse
  (`resume.event_cursor_stale`, detail: ledger truncated/rewritten);
- `recomputed.last_settled_seq != descriptor.last_settled_seq` -> refuse
  (`resume.event_cursor_stale`, detail: a task/run settled after the descriptor);
- otherwise (`recomputed.last_seq >= descriptor.last_seq` **and** settled seqs
  equal) -> allow.

The descriptor is refreshed at task **settle**, so a run killed mid-task
legitimately has a **non-settling tail** beyond the descriptor (`task.started`,
approval, `verify.*` sub-lifecycle — none of which advance `last_settled_seq`).
That tail is normal in-flight progress and stays resumable; resume is for exactly
these interrupted runs. The real safety boundary is the **settled** cursor (the
seedable done-set) plus an append-only (never-shrinking) ledger — not every
event. An exact `last_seq` match would (wrongly) reject the common mid-task
crash. `verify.completed` after the descriptor stays resumable (F-115 N5).

### State / task integrity

- `RUN_STATE.json` must parse.
- state `run_id` must match descriptor and run directory id.
- task ids in `RUN_STATE.json` must match the `PLAN.yaml` snapshot after load.
- every `ResumeTaskSeed` must correspond to a `Done` task.
- every `Done` task in state must appear in `seeded_tasks`.
- done tasks with `workflow_outputs` must have every snapshot path validate as a
  real output snapshot under the run directory:
  - non-empty;
  - a snapshot **may be stored as an absolute path** (`capture_task_outputs`
    writes snapshots under `run_dir/outputs/<task>/`), but it must normalize —
    by stripping the `run_dir` prefix — to a **safe run-relative path** before
    validation; a path that does not normalize under `run_dir` (an external
    absolute path, a leading-slash/UNC/drive-letter path, or a `..` traversal) is
    rejected as an escape;
  - the normalized target must exist and be a **regular file** — a directory or a
    symlink (which could point outside the run directory) is rejected
    (`symlink_metadata`, not following links);
  - the descriptor itself stores only the output **count**, never any snapshot
    path — paths are validated against `RUN_STATE.json` at build/resume time and
    never copied into `RESUME.json`.

If a done task's output snapshot is missing, resume refuses. Skipping such a task
would be a silent downstream data loss.

## Descriptor production

Descriptor updates should be cheap and deterministic:

1. On fresh run creation, after `PLAN.yaml` snapshot and initial `RUN_STATE.json`
   are written, write an initial descriptor.
2. After each durable state transition that can affect resume eligibility
   (task status, workflow outputs, run status, event seq), update the descriptor.
3. Descriptor update is best-effort warn-only during the live run.
4. On resume, descriptor validation is strict.

This gives the active executor forward progress, while keeping resume
fail-closed.

## CLI behavior

`maestro resume` should keep the current shape:

```text
maestro resume [--run <id>] [--max-parallel N] [--force]
```

Behavior changes:

- Before loading current `projects.yaml` or starting a new run, run
  `validate_resume_target`.
- On validation failure, print concise actionable issues and exit non-zero.
- On success, print:

```text
→ resume guard ok for <run-id> — reusing <done> completed task(s), continuing <remaining>
```

- Then call `run_plan` exactly as today: `skip = done task ids`,
  `seed_skipped_from = Some(previous_state)`.

No raw paths are printed except existing user-facing run directory hints. Issue
messages should prefer run id + code + suggested command.

## Privacy boundary

`RESUME.json` may contain task ids, project ids, counts, timestamps, a local plan
hash, and run-relative artifact refs. It must not contain:

- raw prompt / prompt layer / skill / role / memory / mailbox body;
- raw log, transcript, model output, or adapter trajectory body;
- absolute path or `..` path;
- environment key, token, or secret-like value;
- provider chat transcript or provider session payload.

The descriptor is a local run artifact, not a public report. Tests should still
use neutral fixtures only.

## Implementation slices

### Step 1 — Schema + helpers

Files:

- Create `src/schema/resume.rs`
- Create `src/scheduler/resume.rs`
- Modify `src/schema/mod.rs`
- Modify `src/scheduler/mod.rs`

Scope:

- `ResumeDescriptor` schema + validation.
- `build_descriptor(run_dir, state, plan, events)` pure-ish builder; filesystem
  work only for plan hash/output existence in scheduler helper.
- Atomic `write_descriptor`.
- `read_descriptor`: missing -> `Ok(None)`, corrupt/invalid -> `Err`.

Tests:

- serde round-trip with all fields;
- derived task buckets must sum to total;
- bad schema version / bad plan path / bad timestamp rejects;
- unsafe snapshot path rejects;
- event seq and settled seq computation is deterministic.

### Step 2 — Producer updates

Files:

- Modify `src/scheduler/executor.rs`
- Modify `src/scheduler/state.rs` only if a small hook is cleaner

Scope:

- Write descriptor after initial run setup.
- Update descriptor after task settlement / workflow-output capture / run
  terminal status.
- Descriptor write failure logs a warning and does not fail the active run.

Tests:

- fresh run writes `RESUME.json`;
- done task with workflow output appears in `seeded_tasks`;
- descriptor is updated after task completes;
- descriptor write failure does not fail a task.

### Step 3 — Resume validation + CLI

Files:

- Modify `src/cli/commands/run.rs`
- Add/modify integration tests under `tests/`

Scope:

- Add `validate_resume_target(run_dir, force)` before `run_plan`.
- Refuse missing/stale/corrupt descriptor, plan drift, corrupt events, stale
  event cursor, missing output snapshot, task-set mismatch, terminal failed or
  cancelled runs.
- Preserve existing happy path for an abandoned running run with valid
  descriptor.

Tests:

- happy path: abandoned running run with one done task resumes and skips it;
- live run refuses without `--force`;
- `pid == 0` refuses without `--force`;
- plan hash drift refuses;
- corrupt `events.ndjson` refuses;
- stale descriptor cursor refuses;
- done task missing workflow output refuses;
- terminal failed/cancelled run refuses with `maestro rerun` guidance;
- complete run remains a no-op.

### Step 4 — Minimal surface + private收口

Files:

- Optionally modify `src/cli/commands/doctor.rs`
- Optionally modify WebUI only if a small status label is justified

Scope:

- Keep v1 small. A doctor check for descriptor drift is useful but optional.
- No public mirror sync. Push private main only.
- Neutral dogfood: create a run, kill/abandon it after one done task, validate
  descriptor, run `maestro resume`, confirm done task is seeded and remaining
  task runs.

Gate:

```text
cargo check --features embeddings,codegraph --all-targets
cargo test --all-targets --features codegraph
cargo clippy --all-targets --features codegraph -- -D warnings
MAESTRO_PRIVATE_WORDLIST=~/.maestro-private-wordlist.txt SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 ./scripts/secret-scan.sh
```

Report only private HEAD + gate + neutral dogfood result.

## Open review points

1. **Missing descriptor behavior.** Recommended: refuse by default. Do not add a
   legacy bypass in v1; users can use `maestro rerun` for old runs.
2. **Terminal failed/cancelled runs.** Recommended: refuse `resume` and point to
   `rerun`; keep `resume` for interrupted running runs only.
3. **Doctor check in v1.** Recommended: add a small warning-only check if cheap,
   but do not block Step 3 on dashboard/UI work.

