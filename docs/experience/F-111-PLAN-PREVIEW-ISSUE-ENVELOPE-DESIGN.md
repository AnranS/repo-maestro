# F-111 — Plan preview + issue envelope (design)

Status: design (awaiting review before implementation) · Reviewer: cross-review
Parent: [acpus absorption plan](ACPUS-ABSORPTION.md) — borrowed direction #1

## Problem

`maestro work --dry` and `maestro plan validate` already do the right work —
discover, synthesize the DAG, validate, classify blast radius — but they
communicate it as **human stdout text** (`goal matched 3/65`, `plan validate ok ·
6 task(s)`, `T_change_x → 1 downstream`). A CC skill, MCP tool, or the WebUI that
wants to "dry-first" today has to **scrape stdout**, which is brittle. acpus's
lesson: make preview a **stable machine-readable contract**, and express problems
as **structured issues** (code / path / suggestion) rather than prose.

This is a **contract** change. v1 adds a `--json` channel to the two existing
dry/validate commands and a shared `Issue` envelope. It does NOT touch runtime,
preserves the default (non-`--json`) human output's key information, and does NOT
add a workflow engine.

## `PlanPreview` JSON contract

Emitted by `work --dry --json` and `plan validate --json`. One JSON object:

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.plan_preview.v1` |
| `goal` | string? | the dry-run spec, when previewing a synthesized plan |
| `project_count` | u32 | projects in the (narrowed) plan |
| `task_count` | u32 | tasks in the plan |
| `goal_matched` | `{matched, total}`? | for `work --dry` goal-relevance narrowing |
| `dependency_edges` | `Edge[]` | `{ from, to, kind }` — the plan DAG |
| `blast_radius` | `BlastEntry[]` | `{ task, project, downstream: string[] }` — contract + downstream impact |
| `warnings` | `Issue[]` | non-blocking |
| `errors` | `Issue[]` | blocking; non-empty ⇒ plan is not runnable |

`errors` empty ⇒ the plan validates. The shape is identical whether the plan was
just synthesized (`work --dry`) or read from `PLAN.yaml` (`plan validate`); only
`goal`/`goal_matched` differ in presence.

**No `findings_summary` in v1.** Plan preview is a dry/validate contract with no
run context; the F-110 finding ledger is a run-time / HOTL surface. Folding it in
would make a dry preview depend on a run ledger — scope creep. If a monitor view
later needs aggregated findings, that lands in **F-112** (run-monitor
projection), not here.

## `Issue` envelope

The generic structured-problem type, shared by warnings + errors (and reusable
later by F-112/F-113):

| field | type | notes |
|---|---|---|
| `code` | string | stable machine code, e.g. `plan.cycle`, `plan.dangling_contract`, `plan.unknown_project` |
| `severity` | enum | `info` / `warning` / `error` |
| `path` | string? | where it applies — a task id, project id, or `PLAN.yaml` pointer |
| `message` | string | one-line human-readable; neutral, no internal names |
| `suggestions` | string[] | actionable fixes (e.g. "add `--project x`", "declare a producer for `shared-contracts`") |
| `docs` | string? | a docs-site anchor for the code |

`code` is a closed, documented set in v1. The same codes a human sees in text are
the codes a machine reads.

### v1 `code` set

| code | severity | when |
|---|---|---|
| `plan.parse_failed` | error | the `PLAN.yaml` / spec cannot be parsed |
| `plan.unknown_project` | error | a task references a project not in the registry |
| `plan.cycle` | error | the dependency graph has a cycle |
| `plan.self_dependency` | error | a task depends on itself |
| `plan.task_id_traversal` | error | a task id escapes its run dir (`..` / absolute) |
| `plan.dangling_contract` | warning | a consumed contract has no producer |
| `plan.shell_syntax` | warning | a task command fails a shell-syntax check |
| `plan.size_warning` | warning | task/project count crosses the audit-size advice threshold |
| `plan.invalid` | error | generic fallback for a structural validate failure with no more specific code yet (duplicate id, unknown dep, empty prompt, …) |

Extended deliberately as the existing `analyze` / validate findings are folded in.

## Integration points

Recommendation (lock at review): do **both**, since they share one serializer
and one `Issue` set, so the marginal cost is two flag wirings.

- `maestro work --dry --json` — primary; previews a freshly synthesized plan.
- `maestro plan validate --json` — previews an existing `PLAN.yaml`.

If `plan validate`'s wiring proves non-trivial, ship `work --dry --json` first
and fast-follow `plan validate --json` — but the `PlanPreview` / `Issue` types
land once, shared.

Explicitly NOT wired in v1: runtime / `run` / monitor (that's F-112), recover
(F-113). The preview is dry-first only.

## stdout-compat strategy

- **Without `--json`**: the human stdout contract is preserved — no existing key
  information (the goal-match line, task count, blast-radius lines, validation
  result) is removed or restructured. Tests lock those *key lines*, not a
  byte-for-byte golden (which would be brittle to harmless formatting tweaks).
- **With `--json`**: stdout carries the `PlanPreview` JSON and *nothing else* —
  all progress / log / human prose goes to stderr — so a caller can
  `JSON.parse(stdout)` safely.
- **`--json` error path**: even when the YAML can't be parsed, the workspace
  fails to load, or validation fails, stdout MUST still be a single valid
  `PlanPreview` JSON with the problem(s) in `errors: Issue[]`. stdout never emits
  anyhow prose; any human detail goes to stderr. When the plan is unparseable,
  `task_count` / `project_count` are `0` and `dependency_edges` / `blast_radius`
  are empty — the envelope is always well-formed. The process may still exit
  non-zero, but callers read `errors`, not the exit code.

## Test bar

- serde round-trip for `PlanPreview` and `Issue` (all optional fields
  present/absent; enum values).
- a `work --dry --json` over a fixture workspace emits a `PlanPreview` whose
  `task_count` / `project_count` / `dependency_edges` match the known fixture.
- an invalid plan (e.g. a cycle, or a dangling contract) emits the problem as an
  `errors: Issue[]` entry with the expected `code` — not as stdout prose.
- `--json` stdout is valid JSON (assert it parses, and that no human text leaks
  onto stdout).
- `--json` on an **unparseable** plan still emits a valid `PlanPreview` with the
  problem in `errors`, counts `0`, and empty edges / blast_radius — no prose on
  stdout.
- default (no `--json`) output keeps its **key lines** (assert the goal-match /
  task-count / validation-result lines remain) — not a byte-for-byte golden.

## Resolved decisions (review)

1. **Both commands in v1** — `work --dry --json` + `plan validate --json`. May
   land in two commits, but F-111 is NOT closed until both go through the *same*
   serializer.
2. **`findings_summary` deferred** — removed from v1; a future F-112 monitor
   projection owns findings aggregation.
3. **`code` namespace** — flat dotted strings (`plan.cycle`), per the v1 table
   above.
4. **Types location** — `src/schema/preview.rs`: `PlanPreview` + `Issue` + the
   enums + the `maestro.plan_preview.v1` version, in one file (don't split yet).

Implementation order (after review): schema / serializer + `plan validate --json`
first, then `work --dry --json`; each a small commit.
