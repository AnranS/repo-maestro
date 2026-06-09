# Workflow runtime absorption - 2026-06

Status: arc CLOSED (F-122 → F-125 landed; enforcement deferred)
Owner: maestro
Source window: public web and GitHub survey on 2026-06-07

## Closeout — F-122 → F-125 (2026-06-08)

The read-only workflow-runtime projection/audit arc is landed and reviewed. Every
slice is read-only: no new enforcement, no RUN_STATE/events replacement, no canvas.

| slice | what | commit | CI | review |
|---|---|---|---|---|
| F-122 | pin `PLAN_PREVIEW.json` per real run | `c10bf89` | run 27097751323 ✓ | pass |
| F-123 | `RunMonitor.gates` pending review-gate projection | `5fccdab` | run 27098295279 ✓ | pass |
| F1/F2 | TaskInspector artifact-error state + row tooltips | `3921d18` | run 27098471622 ✓ | pass |
| F-124 | evidence read audit-fix (corrupt→500, run-local refs) | `3c062c1` | run 27100077565 ✓ | pass (after B1/B2) |
| F-125 | `TaskDetail.tool_policy` per-node policy projection | `9995ef1` | run 27100659168 ✓ | pass |

Deferred debt is tracked in `BACKLOG.md` → "Deferred follow-ups — workflow-runtime
arc": node-policy ENFORCEMENT (must list behavior change first), external-evidence
write point, evidence-summary path scrub, gate decided-history, `retry.max_retries`.
**Enforcement is not to be opened casually — it changes runtime behavior and needs
its own contract + risk review.**

## Position

Recent workflow projects are converging on the same shape: agents are useful
executors, but the workflow system must own durable state, reviewable plans,
gates, retries, evidence, and replay. Maestro should absorb those contracts into
its local-first multi-repo runtime. It should not become a generic visual
workflow builder.

The useful split is:

- durable workflow spine: run identity, pinned plan, node status, gates,
  retries, event stream, evidence ledger;
- agent workflow skin: LLM planning, delegation, tool use, and reviewer agents
  operating inside that spine.

## Reference sample

| project | useful signal | absorbable primitive |
|---|---|---|
| LangGraph | durable state, interrupts, streaming, resumable agent graphs | first-class gate/interrupt and checkpoint semantics |
| Mastra | typed workflows beside agent abstractions | workflow node as typed IO boundary, not just prompt text |
| Hatchet | durable task execution for agents and background jobs | retries, idempotency, and queue semantics below agent calls |
| Inngest / Trigger.dev | serverless durable step functions and AI workflow positioning | step-level observability and replayable execution history |
| Temporal | mature durable execution baseline | workflow state must survive process death and be queryable |
| n8n | visual automation adoption signal | do not start with a canvas; start with inspectable DAG/evidence |
| open-multi-agent | goal to task-DAG trend | dynamic decomposition must become a reviewable `PlanPreview` |
| OpenWOP | protocol-level standardization attempt | keep envelope/gate/evidence schemas explicit and portable |

## What Maestro already has

These are the absorption landing pads already present in the repo:

- `PLAN.yaml` task DAG and dependency-ordered scheduler.
- F-111 `PlanPreview` / `Issue` JSON contract for dry-run and validation.
- F-112 `RunMonitor` / `TaskDetail` read-only projections.
- F-115 `RunEvent` envelope plus F-120 ack/backpressure for typed streams.
- F-117 resume descriptor design for fail-closed continuation.
- F-118 runtime health and capability readiness checks.
- `MULTI-AGENT-TASK-CLOSEOUT-SOP.md` for Feishu plus botmux plus GitHub
  closeout evidence.

This means the next step is not "add a workflow engine". The next step is to
promote the existing surfaces into a coherent workflow contract.

## Absorbable primitives

| primitive | contract | Maestro mapping |
|---|---|---|
| `WorkflowRun` | durable run object with goal/spec, pinned plan, node states, gates, event cursor, evidence | `RunState` plus `PLAN.yaml`, F-112 monitor, F-115 events, future gate/evidence records |
| `PlanPreview` | compiled plan must be reviewable before execution and pinned once approved | existing F-111 contract; missing run-start snapshot |
| `WorkflowNode` | typed execution unit with dependencies, inputs, outputs, retry/idempotency hints, side-effect marker | `TaskState` plus `tasks[*]`, `workflow_outputs`, risk and approval flags |
| `ReviewGate` | interrupt point with stable id, payload, approver, decision, resume value, and audit link | current `pending_gate` / `approvals_pending`; needs structured gate record |
| `EvidenceArtifact` | proof bound to a node or run: command, CI, log, screenshot, review, doc, commit | existing evidence bundle plus SOP; needs one normalized artifact ledger |
| `ToolPolicy` | allow/deny/intercept rules at tool or node boundary | F-118 direction; risk gate is the current bespoke instance |
| Stream projection | live stream is an optimization; snapshot projection stays the source for "where are we now" | F-115/F-120 stream plus F-112 monitor |

## Absorbed in this slice

- Web API types now treat F-112 `RunMonitor` and `TaskDetail` as first-class
  frontend contracts.
- `TaskInspector` reads task-scoped findings from `/api/runs/:id/tasks/:task/detail`
  instead of re-filtering the whole run finding ledger in the browser.
- The artifacts pane now surfaces task-detail artifact refs before the computed
  diff. This makes the read-only projection visible in the operator flow without
  changing scheduler behavior.
- The closeout SOP now names `WorkflowRun`, `PlanPreview`, and
  `EvidenceArtifact` explicitly, so Feishu handoffs can use the same vocabulary
  as future product surfaces.

## Next implementation slices

### F-122 - pinned plan preview snapshot — LANDED

`PLAN_PREVIEW.json` is written into the run directory at real-run start
(`src/scheduler/executor.rs::pin_plan_preview`, right after `PLAN.yaml`). The
`maestro.plan_preview_snapshot.v1` envelope carries:

- `schema_version` + the F-111 `preview` JSON (reused verbatim from
  `config::analyze::plan_preview` — never a forked algorithm);
- `source` (`"run"` in v1 — a real run start);
- `plan_path` (`PLAN.yaml`) and `plan_hash` (`file_guard::file_hash`,
  `fnv1a64:…` — the same stable hash the resume descriptor uses, so a reader can
  detect plan drift);
- `gate` is omitted (`None`) — never a fabricated approval. The structured gate
  ref lands with F-123.

This makes dynamic decomposition reviewable: an agent may propose a DAG, but the
runtime pins exactly the compiled preview that executed.

**Dry-run decision:** dry-run does **not** pin `PLAN_PREVIEW.json`. Dry-run is
itself the preview surface (it already snapshots `PLAN.yaml` and renders the
prompts), and its run dirs are throwaway — there is no executed "runtime truth"
to audit. Only a real run pins the snapshot. The write is best-effort: a real run
never fails just because the pin could not be written (mirroring the F-117 resume
descriptor).

### F-123 - structured review gate projection — LANDED (v1)

`RunMonitor.gates: Vec<ReviewGate>` projects the run's CURRENTLY-PENDING review
gates, read-only (`src/schema/monitor.rs::review_gates`). Each `ReviewGate` has:

- `gate_id` (`run:plan` / `run:outcome` / `task:<id>`);
- `scope` (`run` | `task`) and `kind` (`plan` | `outcome` | `task_approval`);
- `status` — v1 only ever `pending` (the enum reserves
  `approved`/`rejected`/`request_changes` for later, no restructure needed);
- `summary` (one-line) and small typed `evidence` counts/refs (plan →
  project/task/dependency counts; outcome → verified + acceptance passed/total;
  task → risk level + task-scoped findings count).

`GateBanner` now reads this projection (via `/api/runs/:id/monitor`) instead of
scraping `RunState.pending_gate / verified / acceptance`. Three states: present
(`gates` non-empty) / absent (`gates: []`, success) / projection-error (RunState
or ledger unreadable → handler 500 → banner shows "gate status unavailable",
never a silent empty list, never a frontend recompute).

**Deferred (write-side first):** decided-history — who/when/decision (approve /
reject / request_changes), resume value, and a `node` scope — needs gate-decision
events the current model doesn't record. v1 stays read-only over existing state,
per "first cut read-only, do not change gate write behavior". When the write side
records gate decisions, extend `GateStatus` + add the decided fields without
breaking the v1 contract.

### F-124 - evidence artifact ledger — LANDED (audit fix; NOT a new ledger)

The machine-readable artifact ledger already existed:
`schema::artifacts::{ArtifactRef, ArtifactManifest}` (run/task-bound, kinds,
run-relative `path` OR external `uri`, `source` enum) + `RunEvidence.artifact_refs`,
persisted to `evidence/summary.json` + `evidence/artifacts.json` and served at
`/api/runs/:id/evidence`. F-124 did **not** add a parallel `evidence_artifacts.ndjson`.

What F-124 changed — **read audit semantics + ref safety** (`evidence::read_evidence_projection`,
wired into `run_evidence_handler`):

- a present-but-corrupt `evidence/summary.json` is now an explicit **500** — was
  raw-streamed straight to the client;
- a present-but-corrupt `evidence/artifacts.json` (manifest) is also an explicit
  **500** — never treated as missing/empty;
- a valid summary is returned **parsed/validated** (not raw file bytes);
- every ref read from the ledger (summary `artifact_refs`, each task's
  `artifact_refs`, and the manifest's `artifacts`) is checked run-local via
  `ArtifactRef::run_local_violation` — a valid-JSON-but-unsafe ref (absolute /
  UNC / drive-letter / `..` `path`, a `file:` `uri`, or a non-safe `task_id`) is
  a **500**, never served. The rule lives in `schema::artifacts` and
  `scheduler::events` now delegates to it (one source of truth, no drift);
- the writer no longer leaks: `build_run_evidence` emits the trajectory ref as
  run-relative `trajectories/<task>.ndjson`, not the absolute on-disk
  `trajectory_path`;
- **missing** summary still rebuilds from `RunState` (graceful, unchanged);
  **empty** is a successful `[]`.

`read_evidence_summary` stays the lenient best-effort helper for PR bodies (a
non-audit surface); only the server read path is hardened.

**Deferred follow-ups:** (1) an external-evidence WRITE point to populate
`source: External` + `uri` refs (CI / review message / Feishu·repo doc revision /
commit·branch) into the manifest — held until the SOP shape + one more dogfood
stabilize. (2) `RunEvidence`/`TaskEvidence` carry absolute
`workspace_path`/`worktree_path`/`log_path` by design (operator evidence, unlike
the scrubbed F-112 monitor) — pre-existing, out of F-124 scope; the artifact
*refs* are run-relative path or uri.

### F-125 - node-level tool policy — LANDED (v1: read-only projection/audit)

`TaskDetail.tool_policy: TaskToolPolicy` (`schema::monitor::task_tool_policy`) is a
read-only per-node projection — **no enforcement** in v1. It resolves the EXISTING
sources only (never recomputes from role defaults, which would drift from what ran):

- `status` (`present`/`absent`) from `TaskState.permission` — `absent` is an
  explicit absence, NEVER "allow-all";
- `capabilities[]` (shell/git_write/network/fs_write/external_dir/mcp) with
  `requested` + per-capability `enforcement` (Hard/Soft/Unsupported/NotApplicable)
  + `allowed_commands` (shell), straight from `PermissionEvidence`;
- `declared_effects` (side-effecting capabilities REQUESTED — `shell` is a
  capability, not auto a side effect) and `observed_effects` (changed files, PR;
  `branch` is NOT an effect — it's the executor's auto worktree branch, per
  F-126-fu);
- `retry` (`attempts`, `idempotency: "unknown"` — v1 has no per-tool model);
- `required_evidence[]` — run-local refs from `TaskState` (NOT the evidence
  summary file; F-124 run-local rules, unsafe refs dropped);
- `audit_gaps[]` — never silent: observed effects with no permission evidence, or
  a terminal side-effecting task with no run-local evidence refs;
- `pending_gate_id?` — the task's CURRENTLY-pending gate (F-123). v1 projects no
  gate HISTORY, so a completed/approved task is never flagged for an empty gate.

Errors: a corrupt `RUN_STATE` (which carries the permission evidence) is a handler
**500**, never a fallback that implies allow.

**Enforcement is deferred** (would change runtime behavior): turning the
advisory/Soft agent-tool constraints into hard interception (impossible for opaque
providers today) or adding node-boundary deny that blocks execution. Both must be
their own slice with the behavior change listed.

Earlier rationale (kept): F-125 was sequenced behind F-122/F-123/F-124 because
policy needs a pinned plan, a gate model, and evidence refs to be auditable.

## Do not absorb

- Do not start with a node-canvas builder. Maestro's first workflow surface is a
  task DAG plus evidence, not a generic automation canvas.
- Do not let "agent generated a plan" mean "runtime source of truth". A generated
  plan must be compiled into `PlanPreview` and pinned.
- Do not make MCP/tool discovery a governance model. Tool access still needs
  policy, gate, and evidence contracts.
- Do not replace `RUN_STATE.json`, `events.ndjson`, or `findings.ndjson` with a
  new workflow store in the first cut.

## Source links

- LangGraph: https://docs.langchain.com/oss/python/langgraph/overview
- LangGraph persistence: https://docs.langchain.com/oss/python/langgraph/persistence
- LangGraph interrupts: https://docs.langchain.com/oss/python/langgraph/interrupts
- Mastra workflows: https://mastra.ai/ai-workflows
- Hatchet docs: https://docs.hatchet.run/v1
- Trigger.dev docs: https://trigger.dev/docs/introduction
- Inngest durable execution: https://www.inngest.com/docs/learn/how-functions-are-executed
- OpenWOP: https://openwop.dev/
- open-multi-agent: https://github.com/open-multi-agent/open-multi-agent
- OpenAI Agents SDK guardrails: https://openai.github.io/openai-agents-python/guardrails/
- Microsoft Agent Framework durable workflows: https://devblogs.microsoft.com/dotnet/durable-workflows-in-microsoft-agent-framework/
