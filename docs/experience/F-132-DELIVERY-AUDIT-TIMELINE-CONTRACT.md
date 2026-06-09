# F-132 — Delivery audit timeline — contract

Status: **IMPLEMENTED** (大力 GO'd all 6 pins + 5 hard constraints). Single commit:
read-only `GET /api/deliveries/:id/timeline` → `{events:[{at,stage,by?,reason?,refs[]}]}`,
projecting `DeliverySpec.audit` enriched refs-first per node. FALLIBLE projection — an
audit/node inconsistency (Plan transition w/o plan ref, Execute w/o run_id, Accept/
verdict w/o accept, Closeout w/o closeout) → 500, never a fabricated text-only event.
Sort by explicit `(at, original_index)` (stable append order, not reliant on sort
stability). Refs-first: ids/paths/hashes/counts/uris only (asserted no RUN_STATE/tasks in
the payload). FE: a collapsible "Audit timeline" section below the stepper; a fetch
failure shows an explicit "unavailable" notice (never silent empty). Tests: 4 server_api
incl. 大力's 2 scrutiny cases (same-`at` accept→rejected keeps append order; audit/node
inconsistency → 500) + full-lifecycle ascending refs-first + three-state. 6 open pins
honored (ascending / current-node refs / dedicated endpoint / run id+link / collapsed /
absolute RFC3339). 4 light/dark screenshots (rich + minimal). Read-only — no
mutation/reopen/Feishu receipt/gate, no F-131 behavior change. Full gate green.

---

Original contract (design-only) follows. Fourth Delivery Web slice. Turn the
stage stepper (where we are) into a full chronological **audit timeline** (how we got
here): every lifecycle node — intake / spec / spec_confirm / plan / run / accept /
pm_accept / closeout / writeback — as a "what happened, who did it, when, which
run/plan/evidence it relates to" event. **READ-ONLY, refs-first.** Do-not: no mutation,
no reopen, no Feishu receipt, no new gate engine; corrupt is NEVER a silent empty
timeline.

## 1. The spine already exists — `DeliverySpec.audit`
`audit: Vec<AuditEntry { stage, at, by?, reason? }>` (`src/schema/delivery.rs`) is ALREADY
the chronological log — nearly every store action pushes a row (`delivery.rs`:
create/intake, set_spec [+ "confirmation reset"], confirm_spec, generate_plan, start_run
["linked to run X"], accept ["pm accept: Accepted" + the terminal changes_requested /
rejected row], closeout ["closed out"]). So F-132 PROJECTS `audit[]` into timeline events
and ENRICHES each with refs-first pointers from the matching node record. No new fact
source, no new audit writes.

## 2. Seam map (reuse)
| Seam | Where | Use |
|------|-------|-----|
| Audit spine | `DeliverySpec.audit: Vec<AuditEntry{stage,at,by,reason}>` | one timeline event per entry |
| Node records (refs) | `intake.source_refs` · `spec_confirm{by,at,notes}` · `plan{plan_path,plan_hash,preview_ref}` · `execute{run_id,status}` · `accept{verdict,debt,acceptance_results_ref}` · `pm_accept{by,at,notes}` · `closeout{commits,ci,reviews,doc_revisions,evidence_refs,writeback{status,doc_ref}}` | refs-first enrichment per event |
| Three-state read | `handlers/deliveries.rs::read_for_action` (bad id 400 / missing 404 / corrupt 500) | endpoint guard |
| FE primitives | `CollapsibleSection`, `StatusChip`, the run deep-link (`onOpenRun`) | timeline section |

## 3. Backend — one read-only endpoint
**`GET /api/deliveries/:id/timeline`** → `200 { events: TimelineEvent[] }`.
```
TimelineEvent { at: String, stage: DeliveryStage, by?: String, reason?: String, refs: TimelineRef[] }
TimelineRef   { kind: String, value?: String, uri?: String, summary?: String }   // refs-first — id/ref/uri/summary ONLY
```
Projection (pure, from the already-loaded `DeliverySpec`): each `AuditEntry` → a
`TimelineEvent`; `refs` are derived by the entry's `stage` from the node records, e.g.
- intake → `{kind:"source", uri/name}` per `intake.source_refs`
- plan → `{kind:"plan", value:plan_path}` (+ `plan_hash` summary, `preview_ref`)
- execute → `{kind:"run", value:run_id}` (a deep-link; live status stays F-131's
  run-status — NOT loaded here, refs-first)
- accept → `{kind:"verdict", summary:"accepted · 1 debt"}`
- closeout → `{kind:"evidence", summary:"N commit / M ci / …"}` + `{kind:"writeback", value:status, uri:doc_ref}`
**Refs only — NEVER copy RUN_STATE, the Feishu doc body, or large evidence blobs.**

## 4. Sort / dedup rules
- **Sort**: by `at` ascending (oldest→newest, reads as the story; the stepper already
  shows the current frontier). **Stable** — equal timestamps preserve `audit[]` append
  order, so a same-instant multi-row action (accept → then changes_requested/rejected)
  stays in the order it happened.
- **Dedup**: none. Each `AuditEntry` is a distinct event; an action that pushes two rows
  (e.g. accept + its terminal transition) is two events on purpose. (No collapsing —
  the audit is the source of truth.)

## 5. Three-state error (corrupt is never a silent empty timeline)
`read_for_action`: bad id → 400, missing → 404, corrupt / bad-schema / unsafe-ref
`DELIVERY.json` → 500 (same as F-128/F-136a1). A *readable* delivery always has ≥1 audit
row (create pushes the intake row), so an empty `events` only ever means a genuinely
empty record — and corruption is a 500, NEVER a silent `{events:[]}`. In the F-128 LIST,
a corrupt delivery is already a corrupt stub; the timeline is a per-id detail endpoint →
500 on corrupt.

## 6. Frontend IA
A **collapsible "Audit timeline"** `CollapsibleSection` in the Delivery detail, BELOW the
stage stepper (stepper = frontier; timeline = history). Each event row: timestamp, a
`stage` badge, `by`, the `reason`, and its refs rendered as: a run deep-link
(`onOpenRun`), a doc uri link, a plan path (mono), a verdict/evidence summary chip.
Chronological ascending. A fetch failure → an explicit "timeline unavailable" notice
(never a silent empty list — the F-131/a2 lesson). Default-collapsed (the stepper is the
at-a-glance; expand for the full history).

## 7. Test matrix
- **Backend** (`tests/server_api.rs`): a full-lifecycle delivery (intake→spec→confirm→
  plan→run→accept→closeout) → `events` in chronological order, each with the right
  stage/by/reason + refs (run_id, plan_path, verdict summary, writeback); **refs-first**:
  the payload contains NO RUN_STATE task list / no Feishu body (assert the run ref is just
  an id, not the full monitor); corrupt delivery → 500; bad id → 400 / missing → 404;
  sort stable for a same-`at` accept→rejected pair.
- **Frontend**: `pnpm -C web build`; the timeline renders the events; the unavailable
  state shows (not silent empty).

## 8. Screenshot matrix
light/dark × { a rich timeline (intake→…→closeout, multiple actors) ; a minimal one
(just intake) }. ≥4.

## 9. Do-not-absorb (v1)
No mutation / no editing the audit; no reopen (F-133); no Feishu drain/receipt (F-134);
no new gate engine; no run-state / evidence / Feishu-body COPY (refs only); no historical
node-value reconstruction (events enrich with the current node refs — the lifecycle
records are write-once, so a ref == its value at that event; the one mutable record,
spec_confirm, still has both its set + reset audit rows). No live SSE on the timeline in
v1 (it refetches with the F-131 detail tick); no per-event diff.

## 10. Open decisions for 大力 to pin
1. **Order**: ascending (oldest→newest, story order — recommended) vs descending
   (most-recent-first). *(lean: ascending.)*
2. **Ref enrichment basis**: current node refs (recommended — records are write-once, so
   accurate; simplest) vs strict per-event historical reconstruction (heavier, needs
   per-event snapshots that don't exist). *(lean: current.)*
3. **Surface**: a dedicated `GET /api/deliveries/:id/timeline` endpoint (recommended —
   keeps `DeliveryView`/the list lean) vs extend `DeliveryView`. *(lean: endpoint.)*
4. **Run ref**: just `run_id` + deep-link (refs-first — recommended) vs embed the live
   run status (would re-load RUN_STATE; that's F-131's run-status job). *(lean: id+link.)*
5. **Section default**: collapsed (recommended — the stepper is the at-a-glance) vs open.
   *(lean: collapsed.)*
6. **Timestamps**: absolute RFC3339 (recommended — audit precision) vs relative ("2h
   ago"). *(lean: absolute, maybe a relative hint.)*

## 11. Review axes: spine/seam → §1/§2; endpoint + event schema → §3; sort/dedup → §4;
three-state → §5; FE IA → §6; tests → §7; screenshots → §8; Do-not → §9; recommended +
open → §10. No code until GO.
