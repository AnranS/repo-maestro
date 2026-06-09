# F-127c — accept + closeout + Feishu write-back (contract, design-only, await GO)

Status: **GO'd + implemented** (大力 pinned all 6 decisions + 2 extra guards). One
enabler added beyond the original draft: `delivery intake --source-uri <url>` records
the source doc's remote uri (the write-back target) — without it `--writeback` could
never locate a doc to address. Final slice of F-127
([[maestro-pm-to-delivery-f127]]); closes the PM "提出 → 落地" loop on top of the
CLOSED F-127a (data spine) + F-127b (PLAN→run linkage). Reads a finished run's
acceptance outcome, records an explicit human **PM-accept verdict**, writes a
**closeout** (refs/summary only), and emits a **Feishu write-back intent**.
Refs-first, recoverable, reuses existing gates + F-124 evidence validators. **No new
gate engine, no auto-accept, no in-process Feishu write.**

## 1. Seam map (reuse, do not rebuild)

| Seam | Where | Shape |
|------|-------|-------|
| Run outcome | `scheduler/state.rs:380` `RunState::load(run_dir:&Path)->Result<Self>`; `status: RunStatus{running\|done\|failed\|cancelled}`; `verified: bool` (true iff every acceptance passed AND DAG ok); `pending_gate: Option<String>` (`"outcome"` when paused); `acceptance_results: Vec<AcceptanceResult{describe,check,passed,exit_code?,output,started_at,ended_at}>` | read via `execute.run_id` |
| Outcome gate (F-123) | `executor.rs` `GATE_OUTCOME="__gate_outcome__"`, `pending_gate=Some("outcome")`; cleared by the generic `maestro approve` marker. **No existing "PM-accept" action** — F-127c adds it. |
| External write-back | **maestro has NO in-process Feishu write.** It appends `OutboundReply{channel,run_id,event_kind,title,body,attachments:Vec<ArtifactRef>}` (`channel/outbound.rs:7`) to `outbound_replies.ndjson` (`channel/subscribe.rs:10`); an external skill (`lark-*`) drains it. A failed append is an error, not a silent loss. |
| Report artifact | `reports/mod.rs:18` `write_run_report` → run-dir `REPORT.md` (+ `plans/*.report.md`); evidence at `evidence/summary.json`. |
| SOP + evidence | `docs/experience/MULTI-AGENT-TASK-CLOSEOUT-SOP.md` (WorkflowRun / EvidenceArtifact / DebtItem vocab + closeout template). `schema/artifacts.rs:83` `run_local_violation()` + `ref_path_is_unsafe`/`ref_uri_is_unsafe` (F-124; reuse for evidence refs). |
| Accept/closeout slots | `schema/delivery.rs` `Accept{verdict:AcceptVerdict, acceptance_results_ref:Option<String>, debt:Vec<String>}`, `AcceptVerdict{pending\|accepted\|changes_requested\|partial\|rejected}`, `PmAccept{by?,at?,notes?}`, `Closeout{commits,ci,reviews,doc_revisions,evidence_refs:Vec<DeliveryRef>}` — F-127a defined, F-127c FILLS. |
| Stage rules | `Execute→Accept` (forward), `Accept→Closeout` (forward), `Accept→ChangesRequested` (changes), `Accept→Rejected` (bypass), `ChangesRequested→Clarify/Spec` (changes). |

## 2. Accept-source classification (大力's pin — 5 states from one RunState)

`delivery accept` loads the run via `execute.run_id` and classifies BEFORE recording:

| State | Detected by | Outcome of `delivery accept` |
|-------|-------------|------------------------------|
| missing run | `execute.run_id` is `None` OR `run_dir_for_id` missing | explicit error (nothing to accept) |
| corrupt run state | `RunState::load` parse error | explicit error (never silently "accepted") |
| still running | `status == Running` | REFUSE ("run not finished") |
| paused at outcome gate | `pending_gate == Some("outcome")` | REFUSE ("run paused at the outcome gate; approve/cancel it first") |
| run failed (DAG) | `status == Failed` | readable — surface failure; only `rejected`/`changes_requested` make sense |
| acceptance failed | `status == Done && !verified` (with `acceptance_results`) | readable — surface failing checks; `partial`/`changes_requested`/`rejected` |
| clean pass | `status == Done && verified` | readable — any verdict |

The run outcome is SURFACED; the verdict is the **human's** — `verified==true` never auto-accepts (大力's "CI 绿 ≠ 自动 closeout").

## 3. The actions (input / output / gate reuse)

### 3a. `maestro delivery accept <id> --verdict <v> --by <who> [--notes ..] [--debt ..]`
- **Guard:** `stage == Execute`. Else REFUSE (accept only a linked, finished run).
- **Reads** the run outcome (§2). missing/corrupt → error; running/gated → refuse.
- **Records** `Accept{verdict, acceptance_results_ref, debt}` + `PmAccept{by, at, notes}` — the
  PM verdict IS the explicit human action (the `pm_accept` threshold; `--by` required, never
  inferred). `acceptance_results_ref` = run-relative ref (`RUN_STATE.json`), resolved WITH
  `execute.run_id` (same pattern as `preview_ref`); validated run-local (F-124).
- **Stage by verdict:**
  `accepted`/`partial` → `Execute→Accept`;
  `changes_requested` → `Execute→Accept → ChangesRequested`;
  `rejected` → `Execute→Accept → Rejected`.
- **`accepted` with `!verified`:** see §11 open-decision (require `--accept-failed-with-debt`?).

### 3b. `maestro delivery closeout <id> [--commit ..] [--ci ..] [--review ..] [--doc-revision ..] [--evidence <ref> ..] [--writeback]`
- **Guard:** `stage == Accept && pm_accept present && verdict ∈ {accepted, partial}`. Else REFUSE
  ("not PM-accepted"). pm_accept is the hard threshold — CI-green alone never reaches here.
- **Records** `Closeout{commits, ci, reviews, doc_revisions, evidence_refs}` — refs/ids/summary
  ONLY, never a RUN_STATE or Feishu-body copy. `evidence_refs` validated run-local (F-124);
  `commits`/`ci`/`reviews`/`doc_revisions` are opaque external ids/urls (not file refs).
- **Advances** `Accept→Closeout`.
- **`--writeback`:** emit a closeout `OutboundReply` intent (§7) to the intake source doc; record
  its status. NOT a precondition for the local closeout record.

## 4. PM-accept threshold + verdict transition rules
- `pm_accept` is written ONLY by `delivery accept` (explicit `--by`). No path auto-sets it; CI-green
  / `verified==true` never advance the delivery on their own.
- `accepted`/`partial` → eligible for closeout. `partial` carries `debt[]` (non-blocking follow-ups,
  SOP DebtItem semantics).
- `changes_requested` → lands at `ChangesRequested` (PM wants rework). The rework loop
  (`ChangesRequested→Spec/Clarify` + re-plan/re-run) is **deferred** (§10/§11) — F-127c records the
  verdict, it does not re-open the cycle.
- `rejected` → lands at `Rejected` (terminal bypass).

## 5. Closeout artifact (reuse SOP + F-124 + linkage)
Reuses the SOP closeout vocabulary (commits / CI / verification / review / docs / debt) as the
`Closeout` fields, the F-124 run-local validators for `evidence_refs`, and the F-127b linkage
(`execute.run_id` + `plan.plan_hash`) as the provenance. The delivery record stays the single
machine-readable closeout; `REPORT.md` etc. remain run-local refs, never inlined.

## 6. Three-state / explicit errors
- missing `execute.run_id` / run dir → error. corrupt `RUN_STATE.json` → error.
- running / gated run → refuse. None silently becomes `accepted` or `closed`.
- `delivery closeout` without pm_accept / wrong verdict → refuse.
- incomplete closeout (no evidence at all) → see §11 open-decision (warn vs refuse).
- write-back enqueue failure → explicit error (§7), never recorded as "written".

## 7. Feishu write-back — the KEY constraint
**maestro cannot write Feishu in-process.** `--writeback` therefore EMITS an `OutboundReply` intent
(reusing the existing outbound channel) addressed to the intake `source_refs` (the original doc);
an external `lark-*` skill drains the queue and actually posts. F-127c records
`closeout.writeback = {status, at, ref}` where:
- `intent_emitted` — the NDJSON append succeeded (the post is async/external; we do NOT claim
  "Feishu written").
- `skipped` — no `--writeback`.
- enqueue FAILURE → explicit error from `delivery closeout` (don't record a closeout that lies about
  write-back). The local closeout record is the truth; the doc is closeout-OUT, not runtime truth.

**Options for 大力 (§11.2):** A) emit intent + record status (reuse outbound channel); B) no in-tree
write-back — record only the source doc ref, write-back is a separate skill step. *(lean: A.)*

## 8. Recoverability / idempotency
- Re-`accept`: blocked by the `stage == Execute` guard (once accepted/changes/rejected, stage moved)
  → re-accept REFUSES. (No silent re-verdict.)
- Re-`closeout`: stage already `Closeout` → REFUSE ("already closed out"). §11.3: refuse vs `--amend`.
- Re-`--writeback`: re-emitting an intent is allowed (idempotent at the skill side via run_id +
  event_kind); records the latest emit. §11 open if we want a once-only guard.
- All writes go through F-127a `save()` + transition-validated `update_stage` (illegal → error).

## 9. Explicitly NOT doing
No auto PM-accept (verdict is always an explicit human `--by`). No new gate engine (reuses
stage thresholds + the existing outcome gate). No in-process Feishu write (intent only). No
RUN_STATE/events replacement (reads run-local, never copies). No UI editor / canvas. No AI
verdict. The `ChangesRequested` rework cycle (re-spec→re-plan→re-run) is deferred.

## 10. Schema increment
- FILLS existing `Accept` / `PmAccept` / `Closeout` (no struct change needed for the core).
- ADD `Closeout.writeback: Option<Writeback>` where `Writeback{ status: WritebackStatus, at:
  Option<String>, ref: Option<DeliveryRef> }`, `WritebackStatus{ skipped | intent_emitted }`
  (`#[serde(default, skip_serializing_if)]` — backward compatible).
- `DeliveryView` projection grows: `accept_verdict` (exists) + `pm_accepted_by`, `closeout_summary`
  (exists) + `writeback_status`. Rust↔TS optionality aligned if the TS projection is touched.
- `DeliverySpec::ref_violation` extended to validate `accept.acceptance_results_ref` (already) +
  `closeout.writeback.ref` (new).

## 11. Open decisions for 大力 to pin
1. **accept vs pm_accept** — one action (`delivery accept --verdict --by` sets BOTH `Accept` +
   `PmAccept`) vs two separate actions? *(lean: one — the verdict IS the human threshold.)*
2. **Feishu write-back** — §7 Option A (emit intent + status) vs B (record source ref only). *(lean: A.)*
3. **closeout idempotency** — re-closeout REFUSE vs `--amend` (append evidence). *(lean: refuse.)*
4. **`accepted` on a failed-acceptance run** (`verified==false`) — allow only with an explicit
   `--accept-failed-with-debt` (records the gap as `debt[]`), or restrict `accepted` to
   `verified==true` and force `partial` otherwise? *(lean: require the explicit flag — PM judgment
   can override, but never silently.)*
5. **closeout evidence** — explicit flags only (deterministic, v1) vs auto-pull commits/CI from the
   run `REPORT.md`? *(lean: explicit flags; auto-pull later.)*
6. **rework loop** — defer `ChangesRequested→Spec/Clarify` re-open entirely (F-127c records the
   verdict only) vs include a thin `delivery reopen` that clears `plan`/`execute` and returns to
   Spec? *(lean: defer — keep F-127c to accept + closeout + write-back.)*

## 12. CLI draft
```
maestro delivery accept <id> --verdict accepted|partial|changes_requested|rejected --by <who> [--notes ..] [--debt ..] [--accept-failed-with-debt]
maestro delivery closeout <id> [--commit <sha> ..] [--ci <url> ..] [--review <ref> ..] [--doc-revision <id> ..] [--evidence <relpath|uri> ..] [--writeback]
maestro delivery show <id>   # projection grows: accept verdict, pm_accepted_by, closeout summary, writeback status
```

## 13. Test matrix
- accept-source: missing run / corrupt RUN_STATE → error; running / outcome-gated → refuse; failed /
  acceptance-failed / clean each readable and verdict-recordable.
- `accept` records `Accept` + `PmAccept`; verdict→stage (accepted/partial→Accept,
  changes_requested→ChangesRequested, rejected→Rejected); `acceptance_results_ref` run-local.
- no auto-accept: a clean `verified==true` run does NOT advance without `delivery accept`.
- `accepted` on `!verified` without the flag → refuse; with `--accept-failed-with-debt` → records debt.
- `closeout` guard: refuse before pm_accept / wrong verdict; success → `Accept→Closeout`, evidence
  refs validated (absolute/`file:` rejected).
- write-back: `--writeback` emits an OutboundReply (assert the NDJSON line) + records
  `intent_emitted`; enqueue failure → error, no closeout lie; no `--writeback` → `skipped`.
- recoverability: re-accept refused (stage), re-closeout refused.
- schema serde round-trip incl. `Closeout.writeback`; legacy record without `writeback` → `None`.

## 14. Risk table
| Risk | Mitigation |
|------|------------|
| "closed out" implies Feishu updated | write-back is `intent_emitted` only; never claims "written"; enqueue failure is an explicit error |
| PM accepts a red run by mistake | run outcome surfaced; `accepted` on `!verified` needs an explicit debt flag (§11.4) |
| rework loop half-built | deferred (§11.6) — `changes_requested` records the verdict, no partial re-open |
| evidence ref escapes the run | F-124 `run_local_violation` on every ref (read + write) |
| re-closeout double-counts | stage guard refuses re-closeout |

## 15. Review axes (大力's): state transition table → §2/§4; input/output artifacts → §3/§5;
CLI draft → §12; schema increment → §10; test matrix → §13; risk table → §14. No code until GO.
