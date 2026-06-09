# F-128 — Delivery Web UI (read-only v1) — contract (design-only, await GO)

Status: **GO'd + implemented** (大力 pinned all 6 decisions). Resolved: list is an
**envelope** array (`{delivery_id, status:"ok", view:DeliveryView}` | `{…, status:
"corrupt", error}`) — same `DeliveryView`, no 2nd projection; corrupt rows are visible
stubs (only a dir-read failure → list 500); v1 timeline is a stage stepper (no audit);
run linkage is a deep-link (App `setSelectedRunId`+`setTab("tasks")`); nav `Package`
after codegraph; `delivery_id` asc. Makes the closed F-127
PM→Delivery chain visible in the Web UI. **v1 is strictly read-only**: list +
detail of deliveries; no edit/execute buttons, no mutation endpoints, no Web
crossing `spec_confirm`/`pm_accept`, no new gate engine, no RUN_STATE/events
replacement. Reuses the existing `DeliveryView` projection, the read-only HTTP
handler pattern, and the restrained graph-token / light-dark UI.

## 1. Seam map (reuse, do not rebuild)

| Seam | Where | Note |
|------|-------|------|
| Read store | `src/delivery.rs:72` `read(id) -> Result<Option<DeliverySpec>>` (missing→`Ok(None)`, corrupt/bad-schema→`Err`), `list() -> Result<Vec<String>>` | already exists |
| Projection | `src/schema/delivery.rs` `DeliveryView::project(&DeliverySpec)` — pure; fields: `schema_version, delivery_id, stage, blocked_on, source_refs, run_id?, plan_path?, accept_verdict?, pm_accepted_by?, closeout_summary?, writeback_status?` | already exists (incl. F-127c B1 fields) |
| HTTP handler pattern | `src/server/handlers/runs.rs` `run_handler` (404 missing) / `run_monitor_handler` (500 corrupt) / `task_context_handler` (400 bad id); router `src/server/mod.rs:44` | mirror for deliveries |
| **GAP** delivery route | no `/api/deliver*` route exists | F-128 adds it |
| API client | `web/src/api.ts` `json()` helper (throws on !ok) + `architecture()/runs()/run(id)` shape | add `deliveries()/delivery(id)` |
| Types | `web/src/types.ts` read-only projection types (snake_case, optionals) | add `DeliveryView` + `BlockedOn` |
| Routing/nav | `web/src/hooks/useHashTab.ts` `Tab` union + `tabFromHash`; `web/src/components/Header.tsx` `TabButton`s; `web/src/i18n/{en,zh}.ts` | add `deliveries` tab |
| List+detail + tokens | sidebar-list + detail-panel pattern; `web/src/index.css` tokens (`bg-bg-panel`, `border-line`, `text-ink*`), light/dark vars | reuse, restrained |

## 2. Backend API increment

Two read-only GET routes (new `src/server/handlers/deliveries.rs`, registered in
`src/server/mod.rs` after the runs routes, `pub mod deliveries;` in `handlers/mod.rs`):

- **`GET /api/deliveries`** → list. Empty registry / no `.maestro/deliveries/` → `200 []`
  (not an error). See §11.1/§11.2 for the list read-model + corrupt-entry choice.
- **`GET /api/deliveries/:id`** → the `DeliveryView` projection. Three-state mirrors `runs.rs`:
  - bad id (path traversal etc.) → **400** (`paths::validate_path_component` first).
  - missing (`read`→`Ok(None)`) → **404** `"delivery not found"`.
  - corrupt / bad-schema / unsafe-ref (`read`→`Err`) → **500** (never projected as empty).
  - ok → **200** `application/json` `DeliveryView`.

No POST/PUT/PATCH/DELETE in v1 (mutations are a later slice).

## 3. Rust↔TS projection shape (must align exactly)
`BlockedOn` is a serde **externally-tagged** enum — verified against real `show --json`:
unit variants serialize to a STRING, the struct variant to an object. The TS type MUST be:
```ts
type BlockedOn =
  | "spec_confirm" | "pm_accept" | "nothing"
  | { open_clarify_questions: { count: number } }
```
```ts
type DeliveryStage = "intake"|"clarify"|"spec"|"plan"|"execute"|"accept"|"closeout"
  |"rejected"|"parked"|"duplicate"|"changes_requested"|"cancelled"
type AcceptVerdict = "pending"|"accepted"|"changes_requested"|"partial"|"rejected"
type WritebackStatus = "skipped"|"intent_emitted"
interface DeliveryRef { kind: string; path?: string|null; uri?: string|null; name?: string|null }
interface DeliveryView {
  schema_version: string; delivery_id: string; stage: DeliveryStage; blocked_on: BlockedOn
  source_refs: DeliveryRef[]; run_id?: string|null; plan_path?: string|null
  accept_verdict?: AcceptVerdict|null; pm_accepted_by?: string|null
  closeout_summary?: string|null; writeback_status?: WritebackStatus|null
}
```
Every Rust `Option<>` ↔ TS `?`/`|null`; snake_case throughout. (This is the alignment
that bit F-127b — call it out in the test matrix.)

## 4. Frontend information architecture
- New tab `#deliveries` (`Tab` union + `tabFromHash`); Header `TabButton` (lucide `Package`
  icon) after `codegraph`, before `docs`; i18n `tab.deliveries` (en/zh).
- **`DeliveriesView`** (new): left sidebar list (delivery_id + stage chip + blocked_on dot) →
  right **DeliveryInspector** detail:
  - a **stage stepper** (the 7 forward stages intake→…→closeout, marking reached/current/
    pending from `stage`; bypass stages rejected/parked/duplicate/changes_requested/cancelled
    render as a terminal chip) — derived from `stage` alone (see §11.3).
  - **blocked_on** chip (open clarify count / awaiting spec_confirm / awaiting pm_accept / clear).
  - **source_refs** (kind + name + uri link if remote).
  - **plan/run linkage**: `plan_path`, `run_id` (+ deep-link to the run; §11.4).
  - **accept**: `accept_verdict` + `pm_accepted_by`.
  - **closeout**: `closeout_summary` + `writeback_status` chip.
- Fetch-on-mount (list) + fetch-on-select (detail); no SSE/live in v1 (manual refresh; §9).

## 5. Empty / error / loading states
- loading → `t("common.loading")` centered.
- list empty → restrained empty card: `deliveries.emptyTitle` / `emptyBody` ("created via the
  CLI; they appear here once recorded").
- detail not-selected → `deliveries.selectOne`.
- fetch error (list 500 / detail 500/404) → error card (`common.error` + message); a corrupt
  delivery's detail surfaces the 500 reason, never a blank "ok" panel.

## 6. Light/dark visual acceptance points
Reuse the existing tokens (`bg-bg-panel`, `border-line`, `text-ink/-dim/-faint`, status colors).
Acceptance: stepper, chips, sidebar, and detail readable in BOTH themes; no marketing gradients;
hover/selection only change color/shadow (no layout shift); chip backgrounds track the theme;
controls/labels legible. (Same bar as F-UI-graph-taste.)

## 7. Do-not-absorb (v1)
No edit/execute/mutation buttons; no `POST /api/deliveries*`; no Web action that crosses
`spec_confirm`/`pm_accept` or starts a run; no new gate engine; no RUN_STATE/events replacement;
no audit-write; no AI; no SSE/live stream (v1). Mutations (spec/confirm/plan/run/accept/closeout
from the Web) are explicitly a LATER slice.

## 8. Test matrix
- **Backend** (`tests/server_api.rs`): `/api/deliveries` empty → `200 []`; with N deliveries → N
  `DeliveryView`s; `/api/deliveries/:id` present → 200 + correct projection (incl. blocked_on
  shape, pm_accepted_by, writeback_status); missing → 404; **corrupt DELIVERY.json → 500** (never
  empty); bad/traversal id → 400; corrupt-in-list behaves per §11.2.
- **Frontend**: `pnpm -C web build` (tsc + vite) green; the `DeliveryView`/`BlockedOn` TS type
  matches the Rust JSON (a small fixture-decode test or a typed parse); view renders list+detail,
  empty, and error.
- Rust↔TS optionality alignment explicitly checked (the F-127b lesson).

## 9. Screenshot matrix
light/dark × { empty state, populated list+detail }, plus one detail at a late stage (closeout
with writeback chip) to show the milestone fields — captured against a seeded workspace with a few
deliveries at different stages (intake / spec-blocked / execute / closeout). Mechanism = the
F-UI-graph-taste playwright setup (vite dev + `MAESTRO_API_TARGET`, localStorage `maestro-theme`).

## 10. First-slice (F-128a) scope
Backend 2 routes + handler + tests; frontend types + api methods + tab/nav + `DeliveriesView`
(list + detail stepper + the projection fields) + i18n + empty/error states; light/dark
screenshots; build green. **Read-only only.** No mutations, no SSE, no audit timeline.

## 11. Open decisions for 大力 to pin
1. **List read-model:** reuse full `DeliveryView` per item, or a lighter `DeliveryListItem`
   `{delivery_id, stage, blocked_on, accept_verdict?, has_run}` (smaller payload, sidebar only needs
   a little)? *(lean: a light list-item summary — sidebar doesn't need source_refs/closeout; detail
   fetches the full view.)*
2. **Corrupt entry in the LIST:** (a) whole list → 500 if any DELIVERY.json is corrupt; (b) list
   omits corrupt entries (silent); (c) list includes a flagged stub `{delivery_id, corrupt:true}`
   so it's visible + clickable (detail → 500 with reason). *(lean: c — honest, no silent omission,
   no whole-list break.)*
3. **Detail timeline:** v1 **stage stepper derived from `stage`** (no audit), or add `audit[]`
   (who/when per transition) to the projection for a real timeline? *(lean: stepper v1; exposing
   `audit` + a full timeline is a fast-follow — keeps the projection contract stable.)*
4. **Run linkage:** show `run_id` + a deep-link to the existing run/tasks view, or also fetch
   `/api/runs/:run_id` to show live run status inline? *(lean: deep-link only in v1; status fetch
   later.)*
5. **Nav placement/icon:** `#deliveries` after `codegraph` / before `docs`, lucide `Package`.
   *(lean: yes; trivial to move.)*
6. **List ordering:** by delivery_id (stable) vs by created_at (recency)? `DeliveryView` doesn't
   carry `created_at`; recency would need it added to the list item. *(lean: delivery_id asc v1;
   add created_at to the list item if recency is wanted.)*

## 12. Review axes (大力's): backend API increment + three-state → §2; FE info architecture → §4;
light/dark acceptance → §6; empty/error → §5; test matrix → §8; screenshot matrix → §9;
Do-not-absorb → §7; first-slice scope → §10. No code until GO.
