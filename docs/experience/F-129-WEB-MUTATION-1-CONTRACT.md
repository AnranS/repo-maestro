# F-129 — Delivery Web mutation slice 1 (confirm-spec / plan / run) — contract (design-only, await GO)

Status: **GO'd + implemented** (大力 pinned all 6 + 2 extra guards). Pinned: full preflight
extracted to `delivery::run_preflight` (handler runs it before the 202; the detached child
re-runs it anti-TOCTOU); error map = read-first 400/404/500 + refusal→409; `by` body default
`web-ui`; no `--force` in the UI; run→Tasks deep-link (execute.run_id lands post-run); inline
confirm (not native). Extra guards SHIPPED: confirm-spec refuses a 2nd confirm (409); a durable
run-launch marker (`.run-launch`, pid + 6h-age staleness, acquired by the handler, released by
the run) makes a 2nd run POST 409 while a run is in flight — verified (no orphan/duplicate run).
First Web-mutation slice on top
of the read-only F-128. Adds THREE forward actions to the Delivery detail page —
**confirm-spec → plan → run** — backed by new POST endpoints that REUSE the existing
store fns (`delivery::confirm_spec` / `generate_plan` / `start_run`). Low-risk forward
only: NO spec editing (spec content stays CLI / later UI), NO accept/closeout, NO
crossing `pm_accept`, NO one-click full-auto, NO new gate engine, NO RUN_STATE/events
replacement. Every button explicit-confirms; the run button states it really starts a run.

## 1. Seam map (reuse, do not rebuild)

| Seam | Where | Note |
|------|-------|------|
| Store fns | `src/delivery.rs` `confirm_spec(id, by:Option, now) -> Result<DeliverySpec>` (SYNC); `generate_plan(id, now, by, force) -> Result<(DeliverySpec, PathBuf)>` (SYNC); `start_run(id, now, by) -> Result<DeliverySpec>` (ASYNC, **blocks on run_plan().await**) | reuse verbatim |
| POST handler pattern | `handlers/projects.rs` projects_post (409 conflict); `handlers/runs.rs` run_approve (`Json<Body>` + 204), run_cancel (204), **run_rerun (DETACHED `Command::spawn`, returns immediately + pid)** | mirror |
| Router | `server/mod.rs` `.route(..., axum::routing::post(...))` ; `ServerState{tx: broadcast::Sender<()>}` (SSE only, no run capability) | add 3 POST routes |
| Live updates | `server/mod.rs` fs-watcher → broadcast → `/api/events` SSE; `web/src/hooks/useRunState.ts` EventSource. A new run's RUN_STATE.json is auto-picked-up by the Tasks view. | a spawned delivery run shows live in Tasks |
| FE mutation | `web/src/api.ts` `addProject`/`runApprove` (`fetch{method:POST, JSON body}`, throw on !ok); confirm+busy button `components/tasks/GateBanner.tsx` (`busy` state, `disabled`, confirm-on-action) | mirror |
| Button visibility source | the existing `DeliveryView` (stage + blocked_on) is SUFFICIENT — no new field | see §4 |

## 2. Backend API increment (3 POST routes, new `handlers/deliveries.rs` fns)
- **`POST /api/deliveries/:id/confirm-spec`** body `{by?:string}` → `delivery::confirm_spec` (sync, fast) → `200 DeliveryView`.
- **`POST /api/deliveries/:id/plan`** body `{by?:string}` → `delivery::generate_plan(id, now, by, force=false)` (sync; synthesize is fast) → `200 DeliveryView`. (force omitted in v1 — §9.4.)
- **`POST /api/deliveries/:id/run`** body `{by?:string}` → starts the run DETACHED (§3) → `202 {ok:true, pid}` ("run starting").

**Error mapping (consistent + typed-enough):** the handler FIRST does `delivery::read(id)` for the
F-128 three-state (bad id → **400**, missing → **404**, corrupt/bad-schema → **500**); THEN calls the
action. A store refusal (anyhow `bail!`: not-confirmed / spec-incomplete / already-has-plan /
already-linked / illegal-stage) → **409 Conflict** with the message verbatim (user-actionable). This
keeps corrupt/IO as 500 (caught by the read) and refusals as 409, without a store-error refactor.

## 3. The `run` action over HTTP — KEY constraint
maestro has **no server-side run-start**; `start_run` is CLI-blocking (`run_plan().await` — could be
minutes). The existing `run_rerun` handler spawns a **detached subprocess** and returns at once.
- **Recommendation:** `POST …/run` spawns `maestro delivery run <id>` as a detached child (stdio
  null, like run_rerun) → returns `202 {ok,pid}` immediately. The detached CLI runs the F-127b
  preflight + run + linkage; the fs-watcher → SSE surfaces the run live in the **Tasks** view; the
  delivery detail shows `execute.run_id` on refresh once linked. **The HTTP request never blocks on
  the run.**
- **Immediate feedback:** the handler runs the cheap non-mutating guard synchronously BEFORE spawning
  — `stage == Plan` + `plan` present + no `execute` → else **409** at once. (Deep preflight —
  hash/project/cap — still runs in the detached process; its failure shows as "stayed at Plan" + the
  run's own error. §9.1 asks whether to extract the full preflight into the handler for richer
  immediate feedback.)

## 4. State machine / button visibility (derived from `DeliveryView`, no new field)
| stage | blocked_on | confirm-spec | plan | run |
|-------|-----------|:---:|:---:|:---:|
| spec | `spec_confirm` (not confirmed) | **✓** | | |
| spec | `nothing` (confirmed) | | **✓** | |
| plan | `nothing` | | | **✓** |
| intake / clarify | — | — (spec not shaped — needs CLI `delivery spec`; F-129 doesn't edit spec) | — | — |
| execute / accept / closeout / bypass | — | — | — | — (already run / past) |

`confirm-spec` keeps stage at Spec (sets `spec_confirm`; `blocked_on` flips spec_confirm→nothing →
plan appears). `plan` advances Spec→Plan. `run` advances Plan→Execute (then no buttons).

## 5. Confirm / error / idempotency / loading
- **Explicit confirm on every action** (a small inline confirm or modal). The **run** confirm states
  plainly: "this starts a real run that executes the generated PLAN".
- **loading:** button `disabled` + "working…" while the POST is in-flight (GateBanner pattern).
- **success:** refetch the delivery detail (+ list) so the new stage/buttons reflect immediately; for
  `run`, also offer "open run" deep-link to Tasks (the live run).
- **error:** surface the 409 message inline (e.g. "spec incomplete: …", "already linked to run …").
- **idempotency:** the store guards make re-clicks safe — after an action the stage advances and the
  button disappears; a stale double-click → 409 (no double effect). `confirm-spec` re-validates each
  time (re-confirm of an already-complete spec is a no-op-ish re-set, harmless).

## 6. Do-not-absorb (v1)
No spec-editing UI (prd/target_projects/acceptance via CLI / later slice); no accept/closeout
buttons; no crossing `pm_accept`; no one-click chain (each action is a separate explicit click); no
new gate engine; no RUN_STATE/events replacement; no blocking the HTTP request on a run; no auth
system (`by` is a body field / default). Mutations beyond these three are F-130+.

## 7. Backend test matrix (`tests/server_api.rs`)
- **confirm-spec:** complete confirmable spec → 200 + `spec_confirm` set (stage stays spec, blocked_on
  flips to nothing); incomplete spec → 409 + gap message; wrong stage → 409; missing → 404; corrupt →
  500; bad id → 400.
- **plan:** confirmed → 200 + stage plan + PlanRef; not confirmed → 409; wrong stage → 409; missing 404
  / corrupt 500 / bad id 400.
- **run:** stage=plan + plan present + no execute → 202 (guard passes; the spawn is the action);
  wrong stage / already execute → 409; missing 404 / corrupt 500 / bad id 400. (NOTE: the detached
  subprocess actually executing is NOT asserted in server_api — that path is covered by F-127b's
  `delivery_cli` run-linkage tests; server_api asserts the GUARD + the 202/409 decision. §9 flags how
  far to test the spawn.)

## 8. Frontend screenshot matrix
light/dark × { spec→confirm-spec button, confirmed→plan button, plan→run button + the run confirm
dialog }. ≥6; the run confirm dialog (the "this really starts a run" copy) is the key shot.

## 9. Open decisions for 大力 to pin
1. **run preflight depth in handler:** cheap guard only (stage/plan/no-execute → 409; deep preflight
   async in the detached process) vs extract the full F-127b non-mutating preflight (drift/project/cap)
   into a shared fn and run it in the handler before spawning (richer immediate error). *(lean: extract
   the full preflight — immediate, honest feedback before "starting"; the detached run re-checks.)*
2. **error mapping:** read-first three-state (404/500/400) + action-Err→409 (no store refactor) vs a
   typed `DeliveryActionError` enum on the store fns. *(lean: read-first + 409 v1.)*
3. **`by` identity:** body `{by?}` default `"web-ui"` (recorded in audit / spec_confirm.by) vs null vs a
   configured identity. *(lean: optional `{by}`, default `"web-ui"`.)*
4. **plan `--force` (regenerate) in the UI:** omit in v1 (plan button only at stage=spec, first-gen) vs
   expose a "regenerate plan" at stage=plan with a strong confirm. *(lean: omit v1 — forward-only; regen
   is an edge case for a later slice.)*
5. **run post-action:** deep-link to the Tasks view (live run) vs stay on the delivery + poll for the
   execute linkage. *(lean: deep-link to Tasks — reuses the existing run-open pattern; the delivery
   detail picks up execute.run_id on next visit.)*
6. **confirm UX:** a styled inline confirm/modal vs native `window.confirm`. *(lean: a small inline
   confirm row for the restrained look; native acceptable as a fallback.)*

## 10. Review axes (大力's): API draft → §2; state-machine/button-visibility → §4; error/idempotency →
§5; side-effect preflight → §3; backend test matrix → §7; screenshot matrix → §8; Do-not-absorb → §6;
recommended approach + open points → §9. No code until GO.
