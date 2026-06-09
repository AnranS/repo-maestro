# F-133 — Delivery reopen loop (rework) — contract

Status: **IMPLEMENTED** (大力 GO'd all 6 pins + 6 hard constraints). Single commit:
`delivery::reopen` (only from ChangesRequested → Spec) — archive the prior round into the
new `superseded_rounds[]` via `.take()` (archive-before-reset in one step, all in-memory
before a single `save`, so a write failure leaves the prior record intact), reset the
live `{plan,execute,accept,pm_accept,closeout}` + `spec_confirm` so the new round MUST
re-confirm/re-plan/re-run, audited reopen row. `ref_violation` extended to the archive
(F-127a). DeliveryView gains `round`/`superseded_count`. CLI `delivery reopen` + `POST
/reopen` + a Reopen ActionButton @ changes_requested + a "round N" indicator.
**Reconcile**: the F-132 timeline is made ROUND-AWARE (a node-producing row of round R
resolves its node from `superseded_rounds[R-1]` or live), so a reopened delivery's old
Plan/Execute rows are NOT false inconsistencies — old run shows from the archive, new run
from live, both in the timeline, no 500. Tests: 5 server_api incl. 大力's focus
(reset+archive keeps old run_id; rework loop → new run_id, old preserved; wrong-stage +
reopen-twice → 409; round-aware timeline shows both runs) + three-state. 4 light/dark
screenshots. Full gate green.

---

Original contract (design-only) follows. Fifth Delivery slice. After a PM
verdict of `changes_requested`, explicitly **reopen** the delivery back to a
re-spec/re-plan/re-run-able state — closing the rework loop. **The prior round is
SUPERSEDED (archived + kept by reference), never silently deleted; reopen is an audited
event; the new round must re-confirm-spec + re-plan/run (no auto-crossing the thresholds).**
Do-not: no botmux runtime, no event-ledger overhaul, no new gate.

## 0. Headline — the state machine already allows it
`DeliveryStage::can_transition_to` ALREADY permits `ChangesRequested → Spec` (and
`→ Clarify`) — F-127a/c anticipated reopen. So F-133 is NOT a state-machine change; it's
the **reopen operation** (archive the prior round, reset the live fields, audit the
event) + the CLI/API/UI surfaces. v1 reopens **only** `ChangesRequested → Spec`.

## 1. State machine (v1 scope)
| from | reopen? | to | why |
|------|:---:|----|------|
| **ChangesRequested** | ✓ | Spec | the PM asked for changes — rework |
| Rejected / Cancelled / Duplicate / Parked | ✗ (409) | — | terminal bypass; un-rejecting is a bigger policy call — deferred |
| Closeout / Accept / Execute / … (forward) | ✗ (409) | — | not a rework state; edit the live round instead |

Reopen target = **Spec** (so re-confirm is required). `ChangesRequested → Clarify` is
also legal — kept as a future flag if the rework needs more clarification first.

## 2. Field retention / cleanup (the crux — supersede, never silent-delete)
On reopen (round N → N+1):
- **ARCHIVE** (refs-first, NOT delete): snapshot the prior round's `{plan, execute,
  accept, pm_accept, closeout}` into `superseded_rounds` (see §3). The old `run_id`,
  `plan_path`, verdict, closeout refs are PRESERVED here — they are NOT lost when the new
  round later overwrites `execute.run_id` etc.
- **RESET** the live fields for the new round: `plan = None`, `execute = None`,
  `accept = None`, `pm_accept = None`, `closeout = None`, **`spec_confirm = None`**.
- **KEEP**: `intake`, `spec` (the prd/acceptance — editable for the rework), `clarify`,
  and `audit` (append-only — the full history stays).

The reset is what enforces the discipline (no auto-crossing thresholds, F-127b lesson):
`spec_confirm = None` → the `plan` guard refuses until **re-confirm-spec**; `plan = None`
→ a fresh re-plan; `execute = None` → `start_run`'s idempotency no longer blocks, so a
**fresh run** (new `run_id`) — the stale `PlanRef`/run linkage can't be misused.

## 3. Superseded representation (new schema)
```
SupersededRound {
  round: u32,                  // 1-based prior round number
  superseded_at: String,
  by?: String,
  reason?: String,             // the reopen reason
  plan?: PlanRef,              // refs only: plan_path / plan_hash / preview_ref
  execute?: ExecuteRef,        // run_id / status
  accept?: Accept,             // verdict / debt / acceptance_results_ref
  pm_accept?: PmAccept,
  closeout?: Closeout,         // commits/ci/reviews/doc_revisions refs + writeback
}
```
+ `DeliverySpec.superseded_rounds: Vec<SupersededRound>`. These structs are ALREADY
refs-first (ids / paths / hashes / counts / uris) — no RUN_STATE, task list, Feishu body,
or evidence blob is copied.

## 4. Audit
Reopen pushes an `AuditEntry { stage: Spec, at, by, reason: "reopened for rework (round
N→N+1)[: <reason>]" }` via `update_stage` (the transition is already legal). The F-132
timeline surfaces it automatically; the prior round's audit rows stay in `audit[]`.

## 5. CLI / API / UI
- **CLI**: `maestro delivery reopen <id> --by <who> [--reason <text>]`.
- **API**: `POST /api/deliveries/:id/reopen` `{by?, reason?}` → `200 DeliveryView` | `409`
  (not in `ChangesRequested`) | three-state (bad id 400 / missing 404 / corrupt 500),
  same shape as F-129/F-130. `by` defaults `web-ui`.
- **UI**: a **Reopen** `ActionButton` in `DeliveriesView`, shown when
  `stage == "changes_requested"` (pure derivation, like the F-129/F-130 actions); inline
  confirm whose copy states it supersedes the prior round and requires re-confirm + a new
  run. After reopen the detail refetches (the F-131 tick) → it lands at Spec with the
  re-confirm action. A small "superseded rounds: N" indicator (count) in the detail.

## 6. Three-state error
`read_for_action` (bad id 400 / missing 404 / corrupt 500) + store refusal (stage ≠
ChangesRequested) → 409 with the message. Never a silent no-op.

## 7. Test matrix
- reopen from ChangesRequested → stage `spec`, `spec_confirm` reset, live `plan/execute/
  accept/pm_accept/closeout` cleared, `superseded_rounds` has 1 round carrying the OLD
  `plan_path` + `run_id` + verdict (the old run_id is preserved, not lost).
- reopen pushes a `spec` audit row with `by` + the reopen reason.
- reopen from a non-ChangesRequested stage (Accept / Closeout / Spec / Rejected) → 409.
- **rework loop end-to-end**: reopen → re-confirm-spec → re-plan (fresh) → re-run (a NEW
  `run_id` ≠ the superseded one) → re-accept; `superseded_rounds.len() == 1`.
- reopen twice (after the first, stage=Spec) → 409 (idempotency / wrong-stage).
- DeliveryView projects the superseded count / current round.
- three-state (bad id / missing / corrupt).

## 8. Do-not-absorb (v1)
No botmux runtime / no event-ledger overhaul (the audit + superseded archive are enough);
no auto-crossing `spec_confirm`/`pm_accept` (re-confirm + re-plan/run are manual); no
silent field deletion (archive); no reopen from terminal states beyond ChangesRequested;
no auto re-plan/re-run (the PM drives the new round); no editing/deleting audit history;
no new gate engine; no change to F-131/F-132 behavior.

## 9. Open decisions for 大力 to pin
1. **Allowed source states**: ChangesRequested-only (recommended — v1) vs also allow
   `Rejected`/`Closeout` reopen. *(lean: ChangesRequested-only.)*
2. **Reopen target**: `Spec` (recommended — re-confirm required) vs `Clarify` (also legal,
   for rework needing more clarification — a future `--to-clarify` flag). *(lean: Spec.)*
3. **Superseded archive**: `superseded_rounds: Vec<SupersededRound>` (recommended — keeps
   structured refs incl. the old run_id) vs rely on `audit[]` alone (loses the structured
   refs). *(lean: the archive.)*
4. **UI of superseded rounds**: a count indicator now + the full superseded-round detail
   deferred (recommended) vs full display now. *(lean: count now, detail later.)*
5. **Spec on reopen**: keep the prd/acceptance (editable for the rework) + reset
   confirmation (recommended) vs wipe the spec for a fresh shape. *(lean: keep + reset
   confirmation.)*
6. **Projection**: add a small `round` / `superseded_count` to `DeliveryView` (recommended,
   so the list/detail show "round 2") vs only in the timeline. *(lean: add to the view.)*

## 10. Review axes: state machine → §1; retention/cleanup → §2; superseded schema → §3;
audit → §4; CLI/API/UI → §5; three-state → §6; tests → §7; Do-not → §8; recommended +
open → §9. No code until GO.
