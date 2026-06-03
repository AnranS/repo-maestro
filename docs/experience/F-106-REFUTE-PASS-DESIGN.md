# F-106 — adversarial "refute pass" before integration

> **Scope.** Design note for the second Dynamic-Workflows borrow item
> (#1 refute pass). **Rules + decisions only**, no raw monorepo content.
> Owner: 街溜子-大福 (drafting, implementation) · 大力 (review). Status:
> design — direction approved + 3 questions resolved (§9); two review
> amendments folded in (§10). Implementation goes after dali's recheck
> of this fixup.

## 1. Problem

maestro verifies a task two ways today:

- **`verify` tasks / acceptance checks** — run the *writer's own* tests.
  They encode what the author already thought to check.
- **`review_by: <role>`** — a single reviewer persona reads the work and
  emits PASS / REJECT; a REJECT feeds the existing
  attribution → retry → circuit-breaker recovery path
  (`handle_task_review` + `run_review` in `executor.rs`).

Both miss the same class of bug: **what the author didn't think of.**
Anthropic's Dynamic Workflows handle this by having agents *try to
refute* each other's findings and iterating until convergence
("results are checked before they are folded in"). maestro has no
adversarial step — nothing actively attacks a change looking for the
missed call site, the un-migrated consumer, the contract field that
broke a downstream, the edge case no test covers.

This matters most for **contract-touching / high-risk changes**, which
are exactly the ones the existing `gate_on_high_risk` already flags as
dangerous.

## 2. Key insight — this is mostly reuse, not new machinery

The adversarial step has the *same shape* as `review_by`: a read-only
agent runs over a finished task, emits a verdict, and a fail feeds the
retry loop. So F-106 does **not** build a parallel review engine,
iteration loop, or escalation path. It reuses:

| Existing piece | Reused for |
|---|---|
| `review_by` + `run_review` + `handle_task_review` | runs the refuter read-only, REJECT → retry |
| retry counter + circuit breaker | bounded "convergence" — a refuter that keeps failing the same way escalates to a human instead of looping |
| `crate::roles` builtin bundle (`src/roles/builtin/*.md`) | ships the adversarial persona as embedded markdown |
| risk classification (`contract_paths_for` + `classify_change_risk`) from `gate_on_high_risk` | decides *which* tasks auto-get a refuter (via a shared helper — see §4.2 ordering correction) |

The genuinely new surface is therefore small (see §4). The 1–2 week
estimate was for a from-scratch build; reuse brings it to ~2–3 days.

## 3. Goals and non-goals

**Goals:**
- Add an adversarial verification step that actively hunts for what the
  writer missed, scoped to risky changes by default.
- Reuse the `review_by` machinery; do not duplicate it.
- Be conservative about false positives — a refuter that cries wolf
  blocks every task. Concrete-evidence requirement + circuit breaker.

**Non-goals:**
- Not a replacement for tests or `review_by`. It's an *additional* lens.
- No new iteration/convergence engine — the retry loop is the loop.
- No blanket application — cost-scoped to high-risk by default.
- Not multi-agent voting (that's `maestro compare`, already shipped).
- **Reviewer/refuter token accounting is out of scope for v1 (dali
  N1-B).** `run_review` runs the adapter directly and does **not**
  write usage into `TaskState.usage` / `RunState.usage`, so the
  `--max-tokens` budget gate cannot see reviewer tokens today. The
  refute pass therefore does *not* claim to respect the budget. Wiring
  review usage into the budget gate is a separate follow-up; this note
  must not promise it.

## 4. Design

### 4.1 The refuter role (new)

Ship `src/roles/builtin/refuter.md` — an adversarial reviewer persona.
Unlike `qa.md` (which checks "does it work?"), the refuter's mandate is
"**find what's wrong**": missed call sites, un-migrated consumers,
contract-field changes that break downstream, untested edge cases,
silent behavior changes. It runs read-only and must end with
`VERDICT: pass|fail`. A `fail` must cite **concrete evidence**
(file:line, a specific broken consumer, a concrete failing scenario) —
a vague "could be better" is defined as a `pass`. This is the primary
false-positive guard.

Because `review_by: <role>` already accepts any role name, a user can
opt in per-task **today** with `review_by: refuter` once the role
exists. That's step one and needs no scheduler change.

### 4.2 Risk-driven auto-attach (new toggle)

`defaults.refute_on_high_risk: bool` (default **false**). Parallels
`gate_on_high_risk`. When true, `handle_task_review` — for a task that
has **no explicit `review_by`** — determines whether the task is
high-risk and, if so, runs the builtin `refuter` role as the review
step. Explicit `review_by` always wins (no override).

> **Ordering correction (dali N1-A).** `handle_task_review` runs
> *before* `handle_post_task_approval` in the executor loop (lines
> ~2487 vs ~2522), and `TaskState.risk_level` is currently computed and
> written **inside the approval gate** (~line 988). So auto-refute must
> **not** read the stored `risk_level` — at review time it is still
> `None`, and every high-risk task would silently skip the refuter.
>
> Fix: extract a shared helper
> `compute_and_store_risk(ctx, projects, task_id) -> bool` that
> classifies risk from `artifacts.files_changed` +
> `contract_paths_for(project)` (the exact logic currently inlined in
> the approval gate), writes `risk_level` into `TaskState` if absent,
> and returns whether it's high. `handle_task_review` calls it to decide
> auto-attach; `handle_post_task_approval` is refactored to reuse the
> same helper instead of its inline block. This removes the duplication
> *and* fixes the ordering bug in one move.

So the trigger matrix:

| Task state | Refuter runs? |
|---|---|
| `review_by: refuter` (explicit) | yes (works today once role ships) |
| `review_by: <other>` | no — user chose a different reviewer |
| no `review_by`, `refute_on_high_risk: true`, risk=High | yes (auto) |
| no `review_by`, `refute_on_high_risk: true`, risk≠High | no |
| no `review_by`, `refute_on_high_risk: false` | no (default) |

### 4.3 Convergence = the existing retry loop

A refuter `fail` is already handled by `handle_task_review`: it feeds
the refutation text as the diagnosis into the retry, the task re-runs
addressing it, and the **circuit breaker** stops an identical-failure
loop and escalates to a human. That *is* "iterate until convergence,
bounded." No new code path.

## 5. False-positive guards (the hard constraints)

| Guard | What it stops |
|---|---|
| `fail` requires concrete evidence (file:line / named consumer / scenario) | a refuter that vaguely blocks everything |
| Circuit breaker on repeated identical refutation | infinite refute→retry→refute loop |
| Default off (`refute_on_high_risk: false`) | surprise cost / latency on every run |
| Risk-scoped when auto | doubling agent calls on trivial tasks |
| Un-runnable refuter → PASS (same as `review_by`) | a broken refuter blocking a good task |

> **Note:** the `--max-tokens` budget does **not** cover the refute pass
> in v1 (see Non-goals / dali N1-B). Cost is contained by the
> default-off + risk-scoped guards above, not by the budget gate.

## 6. What gets logged

- The auto-attach decision behind `RUST_LOG=debug`: "task T high-risk +
  refute_on_high_risk → attaching builtin refuter".
- A refuter `fail` already surfaces through the existing review path
  (event + retry reason + REPORT.md). No new surface; it reuses the
  reviewer-rejected ledger entry, tagged so the dashboard can tell a
  refuter-reject from a normal reviewer-reject.

## 7. Tests we'll require before merge

**Helper / unit:**
- `refuter_role_loads_from_builtin_bundle`
- `auto_attach_picks_refuter_only_when_high_risk_and_toggle_on`
- `explicit_review_by_wins_over_auto_refute`
- `auto_attach_noop_when_toggle_off`
- **`auto_attach_computes_risk_before_approval_gate` (dali N1-A
  regression)** — `refute_on_high_risk=true`, `gate_on_high_risk=false`,
  no explicit `review_by`, a high-risk diff → the refuter is still
  auto-attached. This fails if the implementation reads the stored
  `risk_level` (which is `None` at review time) instead of computing it
  via the shared helper.

**Integration (scheduler, mock adapter):**
- `high_risk_task_gets_refuted_then_retries_on_fail` — mock refuter
  returns `fail` with evidence → task retries → second pass → integrates.
- `refuter_pass_integrates_without_retry`
- `repeated_refute_fail_trips_circuit_breaker_and_escalates`

Reviewer/refuter token-budget accounting is **out of scope for v1**
(N1-B) — no test required this round; if a later follow-up wires review
usage into the budget gate, that follow-up adds the budget-trigger test.

## 8. Rollout boundaries

- Direct push to main + post-push CI + dali review (current flow).
- Implementation order: (1) ship `refuter.md` builtin role (usable
  immediately via `review_by: refuter`); (2) add
  `defaults.refute_on_high_risk` + auto-attach in `handle_task_review`;
  (3) debug logging + the test bar in §7; (4) docs for the toggle +
  the role in `docs/site/`.
- Default stays **off**. This is opt-in capability, not a behavior
  change for existing users.

## 9. Decisions (resolved with dali)

1. **Reuse `review_by`; no `refute_by` field.** Plan schema unchanged.
   Auto-attach is gated purely on the `refute_on_high_risk` toggle +
   computed risk.
2. **Refuter runs before human approval.** When
   `refute_on_high_risk=true` **and** `gate_on_high_risk=true`: refuter
   first; on pass, proceed into the approval gate; on fail, retry —
   never hand a human a result the refuter already rejected. Showing the
   refutation on the approval card is a dashboard follow-up, not v1.
3. **v1 is a single refuter.** Multi-refuter / voting later reuses
   `maestro compare`; no fan-out this round.

## 10. Implementation amendments from review (must hold)

- **N1-A — compute risk at review time, don't read stored `risk_level`.**
  Extract `compute_and_store_risk(ctx, projects, task_id) -> bool`,
  call it from `handle_task_review` for the auto-attach decision, and
  refactor `handle_post_task_approval` to reuse it. Stored `risk_level`
  is `None` at review time (the gate writes it later), so reading it
  would skip refute on every high-risk task. Regression test:
  `auto_attach_computes_risk_before_approval_gate`.
- **N1-B — do not claim `--max-tokens` covers the refute pass.** Review
  usage isn't accounted into the budget gate today; cost is contained by
  default-off + risk-scoping only. Budget coverage is a separate
  follow-up.
