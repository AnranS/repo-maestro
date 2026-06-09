# F-130 — Delivery Web mutation slice 2 (accept / closeout) — contract

Status: **IMPLEMENTED** (大力 GO'd all 6 leans + 7 hard constraints). Single commit:
2 POST handlers reusing the SYNC store fns, shared verdict/evidence parse helpers
(no CLI drift; `file:` evidence now routed to `uri` so it can't masquerade as a
relative path), the accept + closeout forms (tokenized textareas, verdict select,
writeback + failed-with-debt checkboxes, ActionButton inline-confirm), 12 server_api
tests + 6 light/dark screenshots. The 7 constraints are honored as: (1) no run-outcome
surfaced in accept — store-validated, 409 verbatim; (2) tokenized textareas + select +
always-shown failed-with-debt checkbox w/ "server is the final judge" copy; (3) client
disables submit until ≥1 evidence AND server keeps empty-closeout→409; (4) no
changes_requested follow-up (F-133); (5) `by` defaults web-ui and lands in the
audit/pm_accept/closeout records; (6) shared `parse_evidence_refs` helper, blanks
trimmed/dropped, unsafe paths/`file:`/`..` → store 409; (7) writeback failure → 409, no
stage advance, no `intent_emitted`.

---

Original contract (design-only) follows. Second Web-mutation slice on
top of F-129. Adds the two F-127c human-threshold actions to the Delivery detail page
— **accept** (the PM verdict) and **closeout** (evidence + write-back intent) — backed
by new POST endpoints that REUSE the SYNC store fns (`delivery::accept` /
`delivery::closeout`). NO detached spawn (neither is a run). Do-not: no
`changes_requested` rework loop (that's F-133), no spec edit, no new gate engine, no
RUN_STATE/events replacement, no auto-chain, no blocking.

## 1. Seam map (reuse)
| Seam | Where | Note |
|------|-------|------|
| Store fns | `delivery::accept(id, verdict:AcceptVerdict, by:String, notes:Option, debt:Vec, accept_failed_with_debt:bool, now) -> Result<DeliverySpec>` (SYNC; reads the run outcome via `execute.run_id`, 5-state classify); `closeout(id, commits, ci, reviews, doc_revisions, evidence_refs:Vec<DeliveryRef>, writeback:bool, now, by:Option) -> Result<DeliverySpec>` (SYNC; write-back = in-process `OutboundReply` append) | reuse verbatim |
| Handler pattern | F-129 `handlers/deliveries.rs`: `read_for_action` three-state (400/404/500) + `actor()` (default `web-ui`) + `refused(e)`→409 | mirror (no spawn) |
| verdict / evidence parsing | CLI `parse_verdict` (string→AcceptVerdict) + `parse_evidence` (http(s)→uri else path→DeliveryRef) in `cli/commands/delivery.rs` | lift/share |
| FE forms | `AddProjectModal` (input/textarea/select/Field/submit+busy/err), `tokenize()` for multi-value, `StarMapPanel` checkbox, F-129 `ActionButton` inline-confirm in `DeliveriesView` | reuse classes + idioms |
| Form slot | `DeliveryInspector` detail panel — replace the read-only accept/closeout display with a form at `stage==execute` / `stage==accept` | same panel |

## 2. Backend API increment (2 POST, sync)
- **`POST /api/deliveries/:id/accept`** body `{verdict, by?, notes?, debt?:string[], accept_failed_with_debt?:bool}` → `delivery::accept` → `200 DeliveryView`.
  verdict string→`AcceptVerdict` (invalid → 400). `by` default `web-ui`. The store does the
  5-state gate (missing/corrupt run → error; running/outcome-gated → refuse; Failed/Cancelled →
  only changes_requested/rejected; Done&&!verified → accepted needs `accept_failed_with_debt`
  + non-empty debt, partial needs non-empty debt). Refusal → **409** (message verbatim).
- **`POST /api/deliveries/:id/closeout`** body `{commits?:string[], ci?:string[], reviews?:string[], doc_revisions?:string[], evidence?:string[], writeback?:bool, by?}` → `delivery::closeout` → `200 DeliveryView`.
  evidence strings → `DeliveryRef` (http(s)→uri else path). Empty closeout → 409; no-doc-uri +
  writeback → 409. Refusal → **409**.
- Error map (both): read-first three-state (bad id 400 / missing 404 / corrupt 500) + store
  refusal → 409, identical to F-129. **No `--force`, no spawn, no RUN_STATE write.**

## 3. Button / form visibility (derived from `DeliveryView`, no new field)
| stage | accept_verdict | accept form | closeout form |
|-------|---------------|:---:|:---:|
| execute | (none) | **✓** | |
| accept | accepted / partial | | **✓** |
| accept | changes_requested / rejected | | — (landed at ChangesRequested/Rejected after accept) |
| spec / plan / closeout / bypass | — | — | — |

`accept` advances Execute→Accept (accepted/partial stay at Accept; changes_requested→
ChangesRequested; rejected→Rejected). `closeout` advances Accept→Closeout. The view's
`stage` + `accept_verdict` are sufficient — no projection change.

## 4. The forms (FE)
- **Accept form** (at stage=execute): a verdict **select** (`accepted | partial |
  changes_requested | rejected`); a **notes** textarea (optional); a **debt** textarea
  (tokenized to `string[]` on submit — one item per line/comma, the AddProjectModal pattern);
  an **accept_failed_with_debt** checkbox ("accept despite failed acceptance — records debt").
  The run outcome itself is a deep-link to Tasks (`run_id`); the store validates and 409s with a
  clear message (e.g. "failed run can only be changes_requested or rejected", "needs --debt").
- **Closeout form** (at stage=accept, verdict accepted/partial): textareas for **commits / ci /
  reviews / doc_revisions / evidence** (each tokenized to `string[]`); a **writeback** checkbox
  ("emit a Feishu/doc write-back intent"). Submit disabled until ≥1 evidence ref (client guard;
  the store is authoritative — empty → 409).
- **Submit UX:** the form's submit goes through an inline confirm (F-129 `ActionButton` idiom):
  accept confirm states it records the PM verdict (the `pm_accept` threshold); closeout confirm,
  when writeback is on, states it emits a Feishu intent (no in-process post). busy/disabled while
  POSTing; on success refetch detail+list; on error show the 409 message inline.
- No native `window.confirm`; restrained tokens (`bg-bg-inset`, `border-line`, `focus:border-blue-600`).

## 5. Error / idempotency
- Accept/closeout refusals (wrong stage, failed-run verdict, missing debt, empty closeout, no
  write-back uri, already-accepted/closed) → 409 with the store message.
- Idempotency: after accept the stage advances (Accept/ChangesRequested/Rejected) → the accept
  form disappears; after closeout (Accept→Closeout) → the closeout form disappears. A stale
  re-submit → 409 (store guards). re-closeout → 409 (stage already Closeout).

## 6. Do-not-absorb (v1)
No `changes_requested` rework reopen (F-133); no spec edit; no new gate engine; no
RUN_STATE/events replacement; no auto-chain (accept and closeout are separate explicit submits);
no blocking; no in-process Feishu post (write-back is intent only, via the store).

## 7. Backend test matrix (`tests/server_api.rs`, reusing the F-129 seed helpers + a controlled RUN_STATE)
- **accept:** clean (Done&&verified) + accepted → 200 + stage accept + pm_accepted_by recorded;
  Failed run + accepted → 409; Done&&!verified + accepted (no flag/debt) → 409, + flag+debt → 200;
  partial no debt → 409; wrong stage → 409; invalid verdict → 400; bad-id/missing/corrupt three-state.
- **closeout:** accepted delivery + ≥1 evidence → 200 + stage closeout; empty closeout → 409;
  not-accepted (wrong stage) → 409; writeback with no doc-uri source → 409; evidence ref validated
  (absolute/`file:` → 409); three-state.
- `by` recorded in the accept/closeout audit (the F-129 B1 lesson — actor provenance).

## 8. Frontend screenshot matrix
light/dark × { accept form (verdict select + debt + failed-with-debt checkbox), closeout form
(evidence textareas + writeback checkbox), a submit confirm row }. ≥6.

## 9. Open decisions for 大力 to pin
1. **Surface the run outcome in the accept form?** v1 = deep-link to Tasks + store-validates (409
   with message); the `DeliveryView` doesn't carry run status/verified — surfacing it needs a
   projection extension or a `/api/runs/:id` fetch. *(lean: deep-link + store-validates v1; surface
   later.)*
2. **Multi-value input:** a tokenized textarea (space/comma/newline split, the AddProjectModal
   pattern) vs a new chip-add component. *(lean: tokenized textarea — reuses the existing idiom, no
   new component.)*
3. **Verdict input:** `<select>` vs a segmented button group. *(lean: select — simplest; segmented
   if you prefer the visual.)*
4. **`accept_failed_with_debt`:** an always-shown checkbox vs shown only when the run is unverified
   (needs #1's run outcome). *(lean: always-shown checkbox; the store validates.)*
5. **Closeout submit guard:** client disables submit until ≥1 evidence + server 409 (both) vs
   server-only. *(lean: both — client for UX, server authoritative.)*
6. **changes_requested / rejected after accept:** v1 just records the verdict and lands at
   ChangesRequested/Rejected (terminal-for-now; reopen is F-133). Confirm no further Web action in
   this slice. *(lean: yes.)*

## 10. Review axes (大力's): API draft → §2; form/state-machine → §3/§4; error/idempotency → §5;
backend test matrix → §7; screenshot matrix → §8; Do-not-absorb → §6; recommended + open points →
§9. No code until GO.
