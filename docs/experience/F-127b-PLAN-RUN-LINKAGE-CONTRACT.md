# F-127b — PLAN generation + delivery↔run linkage (contract, design-only, await GO)

Status: **GO'd + implemented** (大力 pinned all 4 decisions). Second slice of F-127
([[maestro-pm-to-delivery-f127]]). Connects a confirmed `DeliverySpec` to Maestro's
existing execution chain: generate a PLAN (reusing `plan::synthesize` + Goal/
Acceptance — **no new planner**), start/link a run, and record the linkage
(`PlanRef`/`ExecuteRef`) back into the record, advancing `Spec → Plan → Execute`
under the **`spec_confirm` hard guard**. Refs-first, recoverable, reuses existing
gates. Precondition for F-127c (accept/closeout/Feishu), which is OUT of scope here.

CLI shipped: `delivery spec` (shape PRD/target_projects/acceptance, Intake→Spec) ·
`delivery confirm-spec` (the ONLY writer of `spec_confirm`) · `delivery plan`
(synthesize + record `PlanRef`, Spec→Plan) · `delivery run` (start + link a run,
Plan→Execute). Pinned: `Spec.target_projects` added + validated against
`projects.yaml`; `RunState.delivery_id` + `ExecConfig.delivery_id` (serde-default,
legacy → `None`); PLAN at `plans/delivery-<id>.yaml`.

## 1. Seam map — what exists (reuse, do not rebuild)

| Seam | Where | Shape | Status |
|------|-------|-------|--------|
| PLAN synth | `cli/commands/plan.rs:113` `synthesize_file(spec:&str, out:Option<PathBuf>, selected:Vec<String>, root_filter:Option<&Path>) -> Result<PathBuf>` (`pub(crate)`) | input = goal **string** + selected projects; writes PLAN.yaml; the goal block is hard-coded (`description=spec`, one synthesized acceptance check, plan.rs:847-854) | ✓ reuse |
| Plan/Goal schema | `config/plan.rs` `Plan{spec,tasks,goal:Option<Goal>,…}`, `Goal{description, acceptance:Vec<Acceptance>}`, `Acceptance{describe, check}` | `Acceptance.check` is a **required** shell string | ✓ reuse |
| Goal→run | `scheduler/state.rs:344` `goal: plan.goal.clone()`; results in `RunState.acceptance_results` after the DAG | acceptance checks run post-DAG (F-123) | ✓ reuse |
| Run start | `scheduler/executor.rs:2951` `run_plan(plan:Plan, projects:ProjectsConfig, cfg:ExecConfig) -> Result<RunState>`; `generate_run_id()` `YYYYmmdd-HHMMSS_<uuid8>` (:369) | `ExecConfig{run_id:Option<String>, plan_gate:bool, outcome_gate:bool, …}` | ✓ reuse |
| Plan snapshot | `executor.rs:2973` writes run-dir `PLAN.yaml` via **`serde_yaml::to_string(&plan)`** (re-serialized, NOT a byte copy) | — | ✓ (hash implication below) |
| F-122 pin | `executor.rs:2934` `pin_plan_preview` → `PLAN_PREVIEW.json`, `plan_hash = file_guard::file_hash(run_dir/PLAN.yaml)` = `stable_hash_bytes(serde_yaml::to_string(&plan))` | best-effort, at run start | ✓ reuse |
| Landing slots | `schema/delivery.rs` `PlanRef{plan_path, plan_hash, preview_ref:Option}`, `ExecuteRef{run_id:Option, status:Option}`, `DeliverySpec.plan/.execute` | already validated run-local (F-127a) | ✓ fill |
| **GAP** run→delivery | `RunState` / `ExecConfig` have **no** `delivery_id`/owner field (only `session_id`) | — | ✗ §3 decision |

## 2. The two transitions (input / output / gate reuse)

### 2a. `Spec → Plan` — `maestro delivery plan <id>`
- **Hard guard:** `stage == Spec && spec_confirm.confirmed == true`. Otherwise REFUSE
  (explicit error, exit ≠ 0, points to `confirm-spec`). This is a guard, NOT a clarify.
- **Inputs derived from the record:** synth spec string = `spec.prd` (fallback
  `intake.objective`); `selected` = `spec.target_projects` (NEW field, §5.2);
  `Goal.acceptance` ← `spec.acceptance` (`AcceptanceCriterion{describe, check?}`).
- **Insufficient fields → BLOCKING clarify (never silent):** missing objective/prd,
  empty `target_projects`, or ANY acceptance criterion with `check == None` →
  append a blocking `clarify.question`, do NOT advance, do NOT fabricate a check
  (a runnable `config::Acceptance` requires a `check`).
- **Reuse (no new planner):** `synthesize_file(spec_str, Some(out), target_projects, None)`
  writes the task DAG; then load it (`Plan::read_only`), replace `plan.goal` with a
  `Goal{description, acceptance}` built from the record (reusing `config::plan::{Goal,
  Acceptance}`), re-serialize.
- **Output:** PLAN.yaml at `plans/delivery-<id>.yaml` (workspace-relative → run-local-safe).
  Record `PlanRef{plan_path, plan_hash, preview_ref:None}`. Advance `Spec→Plan` via the
  F-127a transition-validated `update_stage`.
- **`plan_hash` (大力's pin — generation timing + same-source):** computed at the Plan
  step as the **run-normalized** hash `stable_hash_bytes(serde_yaml::to_string(Plan::load(plan_path)))`,
  i.e. identical to the F-122 pin the run will write (because `run_plan` snapshots via the
  same `serde_yaml::to_string`). **NOT** the raw source-file `file_hash`, which would
  diverge from the run's pin. `preview_ref` is `None` until execute (no run yet).

### 2b. `Plan → Execute` — `maestro delivery run <id>`
- **Guard:** `stage == Plan && plan.is_some()`. Otherwise REFUSE.
- **PREFLIGHT — all before any run is created** (so a footgun yields no run / no side
  effects / no stamped `delivery_id` / no orphan run, only an explicit error):
  1. drift guard (B1): `normalized_plan_hash(Plan::load(plan_path)) == PlanRef.plan_hash`.
  2. the normal `maestro run` non-mutating defenses (B2 — `delivery run` must not bypass
     them): `check_plan_project_drift` (reused `pub(crate)` helper), `enforce_task_cap`,
     and bail on `config::analyze(..).has_errors()`.
  - `ExecConfig.max_parallel = projects.defaults.max_parallel.max(1)` (the configured
    default, NOT the bare `ExecConfig::default()` of 4).
- **Reuse:** `Plan::load(plan.plan_path)` → `run_plan(plan, projects, ExecConfig{run_id:None,
  plan_gate, outcome_gate, delivery_id:Some(id) [§3], …})` → `RunState`; `run_id` read from
  the returned state.
- **Gate reuse (no auto-cross):** F-127b passes `plan_gate`/`outcome_gate` through as
  configured and does NOT bypass them; if a gate halts the run, F-127b stops at the gate.
- **`preview_ref` set NOW** to the run-relative `PLAN_PREVIEW.json`; the post-run check still
  **verifies** the F-122-pinned `plan_hash == PlanRef.plan_hash` (proves the pin is same-source;
  after the preflight passes this is guaranteed, kept as defense-in-depth).
- **Output:** `ExecuteRef{run_id, status}` (status = the run's status string; **NO RUN_STATE
  copy**). Update `plan.preview_ref`. Advance `Plan→Execute`.

## 3. `delivery_id` back-reference on the run side (SEPARATE decision — behavior + migration)
- **Behavior (if GO):** add `RunState.delivery_id: Option<String>` + `ExecConfig.delivery_id:
  Option<String>`; `run_plan` stamps it so a run knows its owning delivery (enables F-127c
  reverse lookup + a UI "this run delivers X").
- **Migration risk:** additive `Option` with `#[serde(default, skip_serializing_if)]` → old
  `RUN_STATE.json` without the field deserialize to `None`; no migration step; zero effect on
  non-delivery runs. **Low.**
- **Alternative (no run-side change):** one-way linkage (`delivery.execute.run_id → run`);
  reverse lookup = scan deliveries (O(deliveries)).
- **Lean:** add the optional field (low-risk, unblocks F-127c reverse queries). 大力 to GO/defer.

## 4. Recoverability (no overwrite of confirmed/plan)
- `spec_confirm` is written ONLY by the explicit human action `delivery confirm-spec`
  (which confirms only a complete spec). `delivery plan` / `delivery run` NEVER write
  it — they read it as a hard guard. (`confirm-spec` records the gaps as blocking
  clarify questions + refuses on an incomplete spec, so there is never a
  "confirmed-but-unplannable" record; `delivery plan` re-checks completeness as
  anti-tamper.)
- **Editing a confirmed spec RESETS the confirmation (B3):** `spec_confirm` guards a
  SPECIFIC spec version. A `delivery spec` re-edit (still pre-plan) clears `spec_confirm`
  + audits `"spec changed after confirmation; confirmation reset"`, so `delivery plan`
  refuses until a fresh `confirm-spec`. A stale confirmation can never authorize a changed
  spec.
- Re-run `delivery plan` when `plan` already present → **REFUSE** by default ("plan already
  generated at <path>; `--force` to regenerate"). `--force` permitted only while `stage == Plan`
  (not yet Execute); once Execute, the plan is frozen. (Silent regen would diverge `plan_hash`
  from the run's pin.)
- Re-run `delivery run` when `execute` already present → **REFUSE / idempotent** ("delivery
  already linked to run <run_id>"); returns the existing linkage, never starts a second run.
- All writes go through F-127a `save()` + `update_stage` (transition-validated, refs validated);
  illegal transition → error. Reuses F-127a's no-silent-overwrite discipline (blocker 3).

## 5. Open decisions for 大力 to pin
1. **Spec population in-slice?** Include a minimal `delivery spec <id>` (record prd/acceptance/
   target_projects, `Clarify→Spec`) + `delivery confirm-spec <id> --by <who>` (record
   `spec_confirm`) so the guard is reachable + the slice is testable end-to-end? *(lean: yes,
   minimal versions — otherwise the hard guard can't be exercised.)*
2. **`Spec.target_projects: Vec<String>`** — add it as the `synthesize` `selected` input; empty
   → blocking clarify. *(lean: yes.)*
3. **`RunState.delivery_id`** — GO (add optional field) or defer (one-way linkage)? *(lean: add.)*
4. **PLAN location** — `plans/delivery-<id>.yaml` (reuses `plans/` + existing run tooling) vs
   `.maestro/deliveries/<id>/PLAN.yaml` (delivery-scoped). *(lean: `plans/`.)*

## 6. Explicitly NOT doing
No auto-cross of any plan/outcome gate (respects F-122/F-126; halts, never bypasses). No auto
PM-accept. No Feishu closeout write. No RUN_STATE/events replacement (`ExecuteRef` binds
`run_id`/`status` only). No UI editor / canvas / workflow builder. No new planner (reuses
`synthesize` + Goal/Acceptance). No reading `acceptance_results` back into `delivery.accept`
(that is F-127c).

## 7. Three-state / explicit errors
- `spec_confirm` absent/false → explicit refuse (exit ≠ 0), points to `confirm-spec`.
- `plan`/`execute` already present → explicit refuse (recoverable/idempotent), never overwrite.
- corrupt `DELIVERY.json` / unsupported schema / unsafe ref → F-127a `validate()` (explicit error).
- `plan_hash` drift (run pin ≠ `PlanRef`) → explicit error.
- `synthesize` failure / no projects → propagated explicit error.

## 8. Test matrix
- guard: unconfirmed → `plan` refused; confirmed → plan generated + `Spec→Plan`.
- insufficient: no objective / empty `target_projects` / acceptance missing `check` → BLOCKING
  clarify, stage NOT advanced, no fabricated check.
- plan gen: reuses `synthesize_file`; `Goal.acceptance` injected from `spec.acceptance`;
  `PlanRef.plan_hash` == run-normalized hash (assert equals the value `run_plan` would pin).
- linkage: `delivery run` → run starts, `ExecuteRef.run_id` recorded, `Plan→Execute`,
  `preview_ref` set, same-source hash verified.
- recoverability: re-run `plan` → refuse (unless `--force` pre-Execute); re-run `run` →
  idempotent refuse, no second run.
- `delivery_id` back-ref (if GO): `RunState.delivery_id` stamped; legacy `RUN_STATE.json`
  without the field → `None` (serde-default migration test).
- three-state errors each have a binary repro (mirrors F-127a style).
- schema serde round-trip; Rust↔TS optionality alignment if the projection grows.

## 9. Review axes (大力's): input/output → §2; gate reuse → §2/§6; recoverability → §4;
three-state errors → §7; test matrix → §8. No implementation until GO.
