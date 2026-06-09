# Backlog — open follow-ups

Tracked, intentionally-deferred work. Each entry says **why** it's deferred and
**how to resume**, with file anchors. Items are removed from here when landed.

_Last updated: 2026-06-08 (F-126 node-policy enforcement v1 landed — opt-in integration gate; arc F-122→F-125 + F-126 closed)._

## Workflow runtime absorption

> #42 (pin `PlanPreview` into each run dir) **landed as F-122** — `PLAN_PREVIEW.json`
> is now pinned at real-run start in `src/scheduler/executor.rs::pin_plan_preview`.
>
> #43 (project structured review gates) **landed as F-123** — `RunMonitor.gates`
> projects currently-pending plan/outcome/task-approval gates
> (`src/schema/monitor.rs::review_gates`); `GateBanner` reads it instead of
> scraping `RunState`. **Follow-up (deferred):** decided-history (approver /
> timestamp / approve·reject·request_changes) needs write-side gate-decision
> events the current model doesn't record — the `GateStatus` enum already reserves
> the variants so it can be added without restructuring the contract.

### #44 — normalize closeout evidence as `EvidenceArtifact` — landed as F-124 (audit fix only)

> **Landed as F-124, NOT a new ledger.** The machine-readable artifact ledger
> already existed (`schema::artifacts::{ArtifactRef, ArtifactManifest}` +
> `RunEvidence.artifact_refs`, persisted to `evidence/summary.json` +
> `evidence/artifacts.json`, served at `/api/runs/:id/evidence`). F-124 only
> hardened the **read** audit semantics: a present-but-corrupt summary or
> manifest is now an explicit 500 (`evidence::read_evidence_projection`), never a
> raw passthrough or silent rebuild; missing still rebuilds, empty is `[]`.
>
> **Follow-up (deferred):** an external-evidence WRITE point — populating
> `source: External` + `uri` refs (CI run id, review message id, Feishu/repo doc
> revision, commit/branch) into the existing manifest. Deferred until the SOP
> shape + one more dogfood closeout stabilize, per the original "don't overfit"
> rationale. Also pre-existing & out of F-124 scope: `RunEvidence`/`TaskEvidence`
> carry absolute `workspace_path`/`worktree_path`/`log_path` (operator evidence,
> unlike the scrubbed F-112 monitor) — the artifact *refs* themselves stay
> run-relative path or external uri.

> F-125 (node-level tool policy) **landed (v1: read-only projection/audit)** —
> `TaskDetail.tool_policy` (`src/schema/monitor.rs::task_tool_policy`) resolves the
> existing `PermissionEvidence` + observed artifacts per node; no enforcement.

### Deferred follow-ups — workflow-runtime arc (F-122 → F-125)

These are intentionally out of the read-only-projection scope of this arc. Each
is its own future slice; **enforcement must list its behavior change first.**

- **F-125-fu: node-level tool-policy ENFORCEMENT — landed as F-126** (`c38f9e1`,
  review PASS). B4 integration-gate: `defaults.gate_on_policy_violation` (default
  off) gates a finished node before integration when observed effects fall outside
  its requested policy (`scheduler::policy_gate`). Read-only-projection arc stays
  read-only; this is the opt-in enforcement step. See
  `docs/experience/F-126-NODE-POLICY-ENFORCEMENT-DESIGN.md`.
- **F-126-fu: branch/PR observed-signal calibration — landed (both sides).** Contract
  scan confirmed `artifacts.branch` is the executor's auto worktree isolation branch
  (not a real push, source un-taggable). `branch` removed as a signal on **both**
  sides for one consistent rule (only `files_changed` + `pr_url` count): the policy
  GATE (`scheduler::policy_gate`, `1adace4`, CI 27108010252, PASS) and the F-125 UI
  projection (`schema::monitor::task_tool_policy.observed_effects` + field doc,
  `f5fb338`+`1866f43`, CI 27108292791, PASS). Fixes the false positive where a
  no-write role with an empty diff was gated/flagged on its worktree branch.
  **Still deferred (their own slice):** hard per-tool interception of opaque
  providers (impossible today) and node-boundary deny that blocks execution.
- **F-124-fu: external-evidence WRITE point.** Populate `source: External` + `uri`
  refs (CI run id, review message id, Feishu/repo doc revision, commit/branch) into
  the existing manifest. Await SOP shape + one more dogfood.
- **F-124-fu: evidence summary path scrub.** `RunEvidence`/`TaskEvidence` carry
  absolute operator paths (`workspace_path`/`worktree_path`/`log_path`); a future
  slice could scrub them like the F-112 monitor (artifact *refs* are already
  run-local). Separate change.
- **F-123-fu: gate decided-history** (approver/timestamp/decision) — needs
  write-side gate-decision events; `GateStatus` already reserves the variants.
- **F-125-fu (minor): `retry.max_retries`** is `None` in v1 (per-run/config plumbing
  deferred); `idempotency` has no per-tool model yet (`"unknown"`).
- **F-112-fu (minor): `TaskArtifactSummary.available`** is unused by the frontend
  (projection always sets `true`); surface it once `available:false` can occur.

## Channel

### #8 — `BotmuxTransport.poll` doesn't paginate with `--since-message-id`

- **Where:** `src/channel/transport.rs::poll` (invokes `botmux history --limit N`).
- **Problem:** the fetch is bounded by a fixed recent-N window + a local filter.
  If more than N messages arrive between two polls, the ones below the window
  are never returned, and the cursor advances past them (unrecoverable).
- **Why deferred:** the only correct fix is a **server-side** cursor
  (`--since-message-id`), but whether the installed `botmux` supports that flag
  is unverified — shipping it blind (even behind a flag) could break the channel.
- **How to resume:** confirm `botmux history --since-message-id <id>` exists;
  thread the newest cursor id into the invocation and paginate until botmux
  reports nothing newer than the cursor. Keep the local boundary seen-set filter
  (already landed, see #9) as the safety net. If support is version-dependent,
  gate it behind a channel-config opt-in (default off).

## Server

### #30 — `run_outcome` runs git twice per task

- **Where:** `src/server/handlers/runs.rs::run_outcome`.
- **Problem:** `changed_files_with_status` is invoked per task in the main loop
  and again in the contract-drift block (~`changed_projects`).
- **Why deferred:** cosmetic — this is a cold, human-triggered endpoint, not a
  hot poll. The naive dedup is **behavior-changing**: a safe version must reuse
  the main-loop `files` set AND still iterate tasks that have `workspace_path`
  but no `worktree_path` (the in-place worktree-create-failure fallback), or it
  silently regresses contract-drift detection for in-place runs.
- **How to resume:** collect changed projects into a `HashSet` during the main
  loop when `!files.is_empty()`, then run `changed_files_with_status` only for
  the remaining `workspace_path`-but-no-`worktree_path` tasks — do not drop the
  second pass wholesale.

## Scheduler

### #20 (remainder) — decompose `dispatch_one_task`'s worktree/spawn phase

- **Where:** `src/scheduler/executor.rs::dispatch_one_task`.
- **Done so far:** the memory fan-in (`assemble_memory_context`) and the prompt
  prelude (`assemble_prompt_prelude`) are extracted as clean synchronous phases.
- **Remaining:** the worktree decision/policy/integration/guard-creation and the
  spawned-run closure. These are **entangled** with run-state locks, the
  `fail_task_inline` early-returns, and async spawning.
- **Why deferred:** unlike the extracted phases, this part interleaves
  `state.lock().await`, `.await`, and `LoopFlow::Break/Proceed` early returns, so
  a careless extraction can change lock scope or control flow in ways tests may
  not catch. Extract only with explicit attention to those.

## Discovery / contracts

### #41 — cross-workspace contract consumers never link (0-consumer on real monorepos)

- **Where:** `src/config/discovery/contracts.rs` (provider/consumer matching) +
  `src/config/analyze.rs::producer_projects_for`.
- **Problem:** contract matching is scoped to a single workspace root. On a real
  multi-monorepo dogfood, discovery promoted the in-tree contract **providers**
  but found **0 consumers** — the services consuming those contracts live in a
  **separate sibling repo** outside the scanned root, so `provides`/`consumes`
  never link across the boundary. Surfaced repeatedly via the
  `RUST_LOG=debug` "saw generated-client subdirs but no registered producer
  matched" near-miss logs.
- **Why deferred:** this is **structural**, not a bug — a correct fix needs a
  **cross-workspace provider index** (let maestro recognize a producer that
  lives under a different root), which is a real feature with its own design
  questions (how multiple roots are declared/registered, how stale a remote
  contract snapshot may be, security of reading a sibling tree). It deserves a
  dedicated design round, not a dogfood-tail patch.
- **How to resume:** write a design note for a multi-root provider index
  (likely a `.maestro/workspaces.yaml` listing sibling roots, or an explicit
  `contracts.provides_repo` pointer on the consumer); decide whether matching is
  by registered name or by contract-file fingerprint across roots; keep it
  opt-in so a single-root workspace is unchanged. The near-miss debug log is the
  ready-made signal to validate against.

## Self-evolution (`src/learn`) — future hardening

From the design review of the learning subsystem
([docs/site/15-learning.md](../site/15-learning.md)). Not bugs; hardening so the
opt-in loop can't quietly degrade.

- **Live-skill outcome audit.** Record, per run, which promoted-via-`learn` skill
  triggered and whether that run passed, so a reviewer can flag a
  guardrail/playbook that stopped helping ("skill X triggered in 6 runs, 3
  failed"). This is the missing decay signal that otherwise lets an accepted
  skill become evidence for synthesizing more skills (the reinforcement loop).
- **Bound synthesized skills.** A per-scope cap, and have the refinement path
  MERGE into an existing skill rather than allow a near-duplicate trigger.

---

## Recently resolved (for traceability)

Landed on `main` from the 2026-06 review sweep + follow-ups: discovery.rs split
(#19/#35), `read_json`/`read_yaml` dedup (#33), contract-injection producer
fallback (#40), same-timestamp boundary seen-set dedup (#9), `channels`
listen/drain SIGINT handling (#37), and the bulk of the 48-finding review (40
fixes). See the git log and `CHANGELOG.md`.
