# F-131 — Delivery Web live refresh + inline run status — contract

Status: **IMPLEMENTED** (大力 GO'd, one slice). Single commit: read-only `GET
/api/deliveries/:id/run-status` → slim `RunStatusLite` (resolves the linked run via
`execute.run_id`, else the in-flight run via the `RunState.delivery_id` back-ref;
linked-run missing/corrupt → 500, never silent idle; no run → `{run:null}`). The
watcher now also watches `.maestro/deliveries/`; the DeliveriesView opens the `/api/events`
SSE as a refetch tick (+ 7s poll fallback) and silently refetches list+detail+run-status;
the inline run status shows StatusChip + progress + linked/in-flight + Tasks deep-link
(a fetch failure shows an "unavailable" notice, never silent idle); Start-run is hidden
while a run is in flight (the server still 409s a duplicate launch); after Start-run the
page stays on the Delivery detail (no force-jump). 5 server_api tests (linked / in-flight
unlinked / no run / corrupt 500 / bad-id+missing) + 6 light/dark screenshots
(idle / in-flight / linked-done). Read-only — no audit timeline / reopen / Feishu receipt,
no runner behavior change. Full gate green.

---

Original contract (design-only) follows. Third Web slice on the Delivery
detail (after F-129 forward actions + F-130 accept/closeout). Two coupled goals:
**(A) live refresh** — the list + open detail auto-update (no manual refetch) as
actions land and a run progresses; **(B) inline run status** — show a delivery's run
status (running/done/failed + progress) inline in the detail, including a **detached
run that is still in flight**, instead of only a deep-link to Tasks. Do-not: no run
*control* from the delivery page (cancel/approve/gates stay in Tasks), no WebSocket, no
DeliveryView projection change, no run-lifecycle change.

## 1. Seam map (reuse)
| Seam | Where | Note |
|------|-------|------|
| SSE tick | `GET /api/events` (`server/mod.rs:255`) emits `state` events on ANY `.maestro/runs/` change (fs-watcher `watch_state`, runs dir only) | use as a **refetch tick**, not for its payload |
| SSE consumer pattern | `web/src/hooks/useRunState.ts` (EventSource + `connected` + 7s poll fallback) | mirror into a delivery hook |
| Run status (light) | `GET /api/runs/:id/monitor` → `RunMonitor {status, progress{total,done,failed,running,…}, started_at, ended_at}` (`handlers/runs.rs:173`) | the inline-status source |
| Delivery↔run back-ref | `RunState.delivery_id` (`scheduler/state.rs:220`, serde-default, set by F-127b delivery runs) | the ONLY way to find an in-flight run before link write-back |
| Status pill | `web/src/components/ui/StatusChip.tsx` (F-UI-001 shared tokens) | reuse for the inline badge |
| Detached-run gap | `start_run` writes `execute.run_id` + stage=execute only AFTER the run finishes (DeliveriesView comment); during the run the delivery is at **stage=plan, run_id=null** | the crux F-131 solves |

## 2. The crux — the detached run is invisible on the delivery while it runs
F-129's run is detached (`maestro delivery run <id>` subprocess). `start_run` runs the
plan to completion, THEN stamps `execute.run_id` + advances to `execute`. So mid-run the
delivery sits at **plan with no run_id** — and the F-129 "Start run" button is still
showing (a 2nd click 409s on the `.run-launch` marker, but the UI looks idle). The run
*does* exist with `RunState.delivery_id == <id>` + status=running. F-131 surfaces it.

## 3. Backend increment — one read-only resolver endpoint
**`GET /api/deliveries/:id/run-status`** → `200 { run: null | RunStatusLite }` where
`RunStatusLite = { run_id, status, progress{done,failed,running,pending,total}, started_at, ended_at?, linked: bool }`.
Resolution:
1. read_for_action(id) three-state (bad id 400 / missing 404 / corrupt 500) — same as F-129/F-130.
2. If `spec.execute.run_id` present → load that run's monitor, `linked: true`.
3. Else scan `.maestro/runs/*/RUN_STATE.json` for `delivery_id == id`; pick the most
   recent by `started_at` → load its monitor, `linked: false` (the in-flight case).
4. None found → `{ run: null }`.
Read-only; no mutation, no run control. (Projection unchanged — `DeliveryView` can't
carry the in-flight run_id anyway, since the spec doesn't have it yet; the resolver is
the only thing that can.)

## 4. Live refresh mechanism
A `useDeliveriesLive` hook (mirrors `useRunState`): subscribes to `/api/events`; on each
tick refetches **list + open detail + the open delivery's run-status**; tracks
`connected`; falls back to a 7s poll when SSE is down (same as `useRunState`). The SSE
tick fires on run-dir changes (covers run progress); a slow safety poll (~10–15s) +
post-action refetch cover delivery-file changes. **Recommended**: also extend the
watcher to `.maestro/deliveries/` (one line) so delivery edits push live too — else the
slow poll catches them.

## 5. Frontend rendering
- **Inline run status** in the detail: when run-status `run` is non-null, render a
  `StatusChip` + a compact progress (`3/5 · 1 failed`) and the run deep-link. Live via
  the hook.
- **Gate the "Start run" button on no in-flight run**: if the resolver returns an
  in-flight run (`linked:false`, status running), HIDE the F-129 Start-run button and
  show the live status instead — closes the F-129 "looks idle mid-run" gap. (A 2nd run
  is already 409'd server-side; this makes the UI honest.)
- **After Start run, don't force-jump to Tasks** (current `onRunStarted` jumps): stay on
  the delivery and let the inline live status appear; keep the Tasks deep-link for the
  full view.

## 6. Do-not-absorb (v1)
No run control on the delivery page (cancel/approve/gate-resume stay in Tasks); no
DeliveryView projection change; no WebSocket; no new run lifecycle; no auto-advance; the
resolver is read-only and never starts/links a run.

## 7. Test matrix
- **Backend (`tests/server_api.rs`)**: run-status three-state (bad id 400 / missing 404 /
  corrupt 500); no run → `{run:null}`; **linked** run (execute.run_id present) → status
  from monitor + `linked:true`; **in-flight** run (RUN_STATE with `delivery_id==id`,
  status running, delivery still at plan, no execute) → resolved + `linked:false`;
  most-recent-wins when ≥2 runs back-ref the same delivery.
- **Frontend**: `pnpm -C web build`; the inline status renders running/done/failed; the
  Start-run button is hidden while an in-flight run exists.

## 8. Screenshot matrix
light/dark × { inline status = running (in-flight, Start hidden), inline status = done
(linked), inline status = failed }. ≥4–6.

## 9. Open decisions for 大力 to pin
1. **Scope as one slice vs split** F-131a (live refresh) + F-131b (inline run status +
   resolver)? *(lean: one slice — they're coupled; inline status is only useful live.)*
2. **In-flight resolution**: include the `delivery_id` back-ref scan (recommended — the
   whole point of "live") vs v1 only shows status once `run_id` is linked (post-finish).
   *(lean: include the scan; note it's a `.maestro/runs/*` scan — fine at local scale, an
   index is a later optimization.)*
3. **Live mechanism**: SSE-tick refetch + extend watcher to `.maestro/deliveries/`
   (recommended, truly event-driven) vs SSE-tick + slow-poll-only (no backend watcher
   change). *(lean: extend the watcher.)*
4. **Start-run UX**: stop force-jumping to Tasks + show inline live status (recommended)
   vs keep the jump. *(lean: stop force-jump; keep the deep-link.)*
5. **Gate Start-run on in-flight run**: hide the button while an in-flight run exists
   (recommended) vs leave it (server 409s a dup). *(lean: hide — honest UI.)*
6. **run-status shape**: slim `RunStatusLite` (recommended) vs return the full
   `RunMonitor`. *(lean: slim — the detail only needs status + progress + timestamps.)*

## 10. Review axes: resolver/endpoint → §3; live mechanism → §4; rendering/UX → §5;
Do-not-absorb → §6; test matrix → §7; screenshots → §8; recommended + open points → §9.
No code until GO.
