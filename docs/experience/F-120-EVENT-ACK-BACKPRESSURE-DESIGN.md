# F-120 - Event ack / backpressure (design)

Status: design / awaiting review - Owner: dali design / dafu implementation

Parent: [reference local agent-runtime package absorption](REFERENCE-LOCAL-AGENT-RUNTIME-PKG-ABSORPTION.md) -
borrowed direction #3. It follows [F-115](F-115-RUNTIME-EVENT-ENVELOPE-DESIGN.md)
and depends on the local-first UI / runtime groundwork from F-UI-001, F-118, and
F-119.

This is a **design document only**. It does not add feature code. It locks the
consumer ack contract, event delivery classes, local high-water storage,
backpressure behavior, privacy boundary, and implementation slices for typed run
events.

## Problem

F-115 gave Maestro a typed per-run event ledger and an additive per-run SSE
endpoint:

- durable ledger: `.maestro/runs/<run>/events.ndjson`;
- typed event: `scheduler::events::RunEvent` (`maestro.run_event.v2`);
- stream: `GET /api/runs/:id/events/stream`, using `since_seq` or
  `Last-Event-ID`;
- live trigger: the existing broadcast tick, followed by a durable ledger re-read.

That is enough for stateless replay, but not for consumer-owned delivery:

1. There is no named consumer cursor. A reconnecting UI/MCP/TUI client must keep
   its own cursor or start from `0`.
2. There is no ack high-water. The server cannot tell which seq a consumer has
   intentionally accepted.
3. There is no delivery class. A huge burst of low-value activity can compete
   with terminal, permission, or resume-critical events in the live stream.
4. There is no backpressure contract. The current stream is lossless and simple,
   but a slow consumer has no bounded "balanced" mode that can shed safe activity
   while preserving critical events.

F-120 adds a small local delivery layer on top of F-115. It does **not** replace
`events.ndjson`, does **not** trim the ledger, and does **not** become the run
state authority. It only records what a local consumer has accepted and how a
live stream may shed low-value frames under pressure.

## Goals

- Add a stable `maestro.run_event_ack.v1` consumer high-water record per run and
  consumer.
- Add a stable `maestro.run_event_gap.v1` SSE control frame for intentionally
  shed low-value seq ranges.
- Classify each `RunEventKind` into delivery classes so terminal / permission /
  resume-critical events are never shed from live delivery.
- Extend `/api/runs/:id/events/stream` additively:
  - existing clients keep working;
  - `since_seq` / `Last-Event-ID` still work;
  - named consumers may resume from stored ack when no explicit cursor is passed;
  - `delivery=lossless` remains the default;
  - `delivery=balanced` may emit gap frames for shed activity.
- Add a POST ack endpoint so WebUI, CLI, TUI, or MCP can acknowledge high-water
  after processing delivered events/gaps.
- Keep all new artifacts local-only and privacy-safe: no raw prompt, transcript,
  log, model output, path, env key/value, token, or user identity.

## Non-goals

- No cloud queue, remote daemon, websocket-to-cloud transport, SSO/RBAC, PAT,
  member invite, multi-tenant consumer registry, telemetry, or hosted ack store.
- No new event ledger. F-120 **must not** introduce `delivery_events.ndjson` or a
  second source of truth.
- No trimming or compaction of `events.ndjson` in v1. Ack high-water is consumer
  state, not retention policy.
- No run/task status authority. F-112 monitor and `RUN_STATE.json` stay the
  snapshot authorities; F-117 resume reads the event ledger directly, not ack
  files.
- No parsing assistant messages, logs, trajectories, or markdown to synthesize
  events.
- No backpressure inside event producers. F-115 producers still append the full
  durable ledger; F-120 backpressure is only a live delivery projection.
- No bot/group-chat entry. If bot collaboration is needed later, it should sit on
  the same local ack contract, not define a parallel one.

## Existing surfaces to reuse

| Surface | Reuse in F-120 |
|---|---|
| `src/scheduler/events.rs` | `RunEvent`, `RunEventKind`, `RunEventStream`, `read_events`, append invariants. |
| `src/server/handlers/runs.rs` | Existing `/api/runs/:id/events/stream` SSE endpoint and tests. |
| `src/server/mod.rs` | Adds ack route under the existing run API. |
| F-115 typed event stream | Wire compatibility: `run_event` frames keep their current JSON shape and `id=seq`. |
| F-117 resume guard | Reminder that resume-critical safety reads the durable ledger, never ack state. |
| F-UI-001 / F-UI-002 | Future UI consumers use stable status tokens and avoid event noise in primary surfaces. |
| `paths::validate_path_component` | Guard run id, consumer id, and ack file names before filesystem access. |

## Grounding notes from current code

- The real `RunEventKind` variant for wire `task.approval_required` is
  `TaskApprovalRequested`.
- The real `RunEventKind` variant for wire `evidence.captured` is
  `ReplanWritten`; there is no `EvidenceCaptured` variant in v2.
- There is no `UsageSampled` variant in v2. A future well-formed
  `"usage.sampled"` line currently reads as `Other("usage.sampled")`, and `Other`
  is classified as `critical` for forward safety. Usage can move to `activity`
  only after a real explicit variant is added.
- There is no `last_seq()` helper. Ack validation should read the ledger and use
  `events.last().map(|e| e.seq).unwrap_or(0)` or `RunEventStream::from_events`.
  A corrupt ledger already returns `Err`, so it must become `500`, never
  `last_seq=0`.
- Ack read semantics match F-119 control loading: missing ack file is `Ok(None)`;
  parse/validation failure is `Err` and surfaces as `500` for ack-aware
  consumers.
- The server broadcast channel is content-free `broadcast::Sender<()>`. Each
  typed event stream already re-reads its own run ledger on every tick. Balanced
  delivery therefore plugs into the per-stream `async_stream` loop and mostly
  affects initial backlog or large tick batches.

## Source of truth

F-120 introduces **consumer state**, not event state.

- `events.ndjson` remains the complete append-only event ledger.
- `RunEvent.seq` remains the canonical per-run sequence.
- `RUN_STATE.json` remains the current-state snapshot authority.
- `findings.ndjson` remains the durable audit authority.
- `.maestro/runs/<run>/event_ack/<consumer>.json` records only a consumer's
  acknowledged high-water and delivery preference.

If ack state is missing, corrupt, or stale, the event ledger is still readable.
The consumer can pass `since_seq` explicitly, or start at `0`. Corrupt ack state
is an error for ack-aware consumers; it is never silently treated as "all caught
up".

## Storage

Per-run ack files live under the run directory:

```text
.maestro/runs/<run>/event_ack/<consumer-id>.json
```

Rules:

- `<run>` is resolved through the existing `run_dir_from_id` path guard.
- `<consumer-id>` must pass `paths::validate_path_component("event consumer id",
  id)` before any filesystem access.
- The file is compact JSON, written atomically (`tmp + fsync + rename`) and
  validated on read/write.
- Ack files are local implementation detail. V1 exposes them only through the ack
  POST response and stream cursor behavior; there is no debug/list endpoint.
- Consumer ids are local symbols such as `webui-main`, `tui`, `cli-tail`, or a
  generated `webui-<uuid>` stored by the client. They are not user names, emails,
  hostnames, or device identifiers.

## Schema

Types should live in:

```text
src/schema/event_delivery.rs
```

`schema/mod.rs` exports:

```text
RUN_EVENT_ACK_V1 = "maestro.run_event_ack.v1"
RUN_EVENT_GAP_V1 = "maestro.run_event_gap.v1"
run_event_ack_version()
run_event_gap_version()
```

Filesystem helpers should live in:

```text
src/scheduler/event_ack.rs
```

Pure delivery planning should live in:

```text
src/scheduler/event_delivery.rs
```

### `RunEventAck`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.run_event_ack.v1` |
| `run_id` | string | path-component-safe run id |
| `consumer_id` | string | path-component-safe local consumer id |
| `high_water_seq` | u64 | largest seq the consumer has accepted, including covered gap ranges |
| `last_seen_seq` | u64 | largest seq the server saw in the ledger when this ack was written |
| `delivery` | `DeliveryMode` | `lossless` or `balanced` |
| `updated_at` | string | RFC3339, set by handler/CLI wrapper |
| `stats` | `RunEventAckStats` | counters only |

`RunEventAckStats`:

| field | type | notes |
|---|---|---|
| `acked_events` | u64 | count of concrete events acknowledged by this update, if known |
| `acked_gaps` | u64 | count of gap frames acknowledged by this update, if known |
| `shed_events` | u64 | total seq count the consumer accepted through gaps |

Validation:

- schema version exactly matches v1;
- `run_id` and `consumer_id` pass path-component validation;
- `updated_at` parses as RFC3339;
- `high_water_seq <= last_seen_seq`;
- `delivery` is a closed enum;
- stats are counters only; no labels, paths, or message text.

### `RunEventAckRequest`

HTTP body for the ack endpoint:

| field | type | notes |
|---|---|---|
| `consumer_id` | string | required; path-component-safe |
| `high_water_seq` | u64 | required; monotonic non-decreasing |
| `delivery` | `DeliveryMode?` | optional; defaults to existing ack mode or `lossless` |
| `acked_events` | u64? | optional counter |
| `acked_gaps` | u64? | optional counter |
| `shed_events` | u64? | optional counter |

The request carries no event bodies. A client that processed a gap frame may ack
the gap's `to_seq` as high-water. That means "I intentionally accept not seeing
the shed low-value events in that covered range."

### `RunEventGap`

SSE control frame emitted only by `delivery=balanced` when low-value events are
intentionally shed:

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.run_event_gap.v1` |
| `run_id` | string | same run |
| `from_seq` | u64 | inclusive |
| `to_seq` | u64 | inclusive |
| `count` | u64 | `to_seq - from_seq + 1` |
| `reason` | string | closed enum: `activity_backpressure` |
| `delivery` | string | `balanced` |
| `classes` | string[] | closed set; v1 emits only `activity` |

Validation:

- `from_seq <= to_seq`;
- `count` matches the range;
- reason and delivery are closed enums;
- classes are non-empty and from the closed delivery-class set;
- no raw messages, paths, payloads, or refs.

### Delivery enums

`DeliveryMode`:

| value | meaning |
|---|---|
| `lossless` | emit every `run_event` with `seq > cursor`; existing behavior and default |
| `balanced` | emit every critical event, emit safe activity until a cap, and cover shed activity with `run_event_gap` |

`RunEventDeliveryClass`:

| value | meaning |
|---|---|
| `critical` | never shed from live delivery |
| `normal` | retained in v1; not shed by balanced mode |
| `activity` | may be gap-covered under balanced backpressure |

## Delivery class mapping

Critical events are the events an operator must not miss live because they end a
run/task, require permission, or affect resume/security/audit interpretation.
They are never shed. The durable ledger still contains all classes.

| `RunEventKind` | class | rationale |
|---|---|---|
| `RunCompleted`, `RunFailed`, `RunCancelled` | critical | run terminal |
| `TaskSucceeded`, `TaskFailed`, `TaskCancelled`, `TaskSkipped` | critical | task terminal / resume-relevant |
| `TaskApprovalRequested`, `TaskApprovalGranted` | critical | permission gate |
| `CancelRequested` | critical | operator control |
| `FindingRecorded` | critical | durable audit projection |
| `RunCreated` | normal | lifecycle start; retained |
| `TaskQueued`, `TaskStarted` | normal | lifecycle progress; retained in v1 |
| `VerifyStarted`, `VerifyCompleted` | activity | sub-lifecycle noise; findings are carried by `finding.recorded` |
| `ReplanWritten` | normal | wire `evidence.captured`; may also represent replan written, so keep live-visible in v1 |
| `Other(_)` | critical | future/unknown kind is preserved, not shed |

If review decides `TaskStarted` can be shed, that should be a v2 change after
dogfood; v1 keeps it normal to avoid surprising live task lists.

If a real `UsageSampled` variant is introduced later, it should be classified as
`activity`. Until then, `"usage.sampled"` is reader-only `Other(_)` and remains
critical by the forward-safety rule.

## Cursor and ack semantics

### Stream start cursor

For `GET /api/runs/:id/events/stream`, cursor resolution becomes:

1. `?since_seq=N` if present;
2. `Last-Event-ID` if present;
3. stored ack `high_water_seq` if `consumer_id` is present and the ack file is
   valid;
4. `0`.

Query still wins over header and stored ack. Existing clients that do not pass
`consumer_id` see exactly the F-115 behavior.

### Ack high-water

`POST /api/runs/:id/events/ack` writes the consumer high-water. Rules:

- invalid run id or consumer id -> `400` before filesystem access;
- unknown run -> `404`;
- corrupt event ledger -> `500`, never `last_seq=0`;
- corrupt ack file -> `500` for that consumer, never silently reset;
- `high_water_seq` lower than stored value -> `409` (monotonic guard);
- `high_water_seq` greater than current ledger `last_seq` -> `409`;
- otherwise write atomically and return the validated `RunEventAck`.

Ack does not broadcast ticks and does not affect the run.

### Gap coverage

Balanced delivery may emit:

```text
event: run_event_gap
id: <to_seq>
data: <RunEventGap JSON>
```

The stream's internal cursor advances to `to_seq` after a gap frame, just as it
advances to an event's `seq` after a `run_event` frame. A client may ack
`high_water_seq = to_seq` after processing the gap. This is what permits a
consumer high-water to move across intentionally shed low-value events without
pretending those events were rendered.

Critical events never appear inside a gap range. If a range contains both
activity and critical events, the pure planner must split it so the critical
events are emitted as `run_event` frames and only contiguous activity-only spans
become gaps.

## Backpressure policy

Constants (initial values; can be tuned after dogfood):

| constant | value | meaning |
|---|---:|---|
| `MAX_LOSSLESS_EVENTS_PER_TICK` | unlimited | lossless mode keeps current behavior |
| `BALANCED_SOFT_FRAME_CAP` | 256 | target max frames per catch-up/tick batch |
| `BALANCED_ACTIVITY_KEEP_TAIL` | 64 | latest activity events retained before gap-shedding older activity |

Balanced mode planning:

1. Filter events to `seq > cursor`.
2. Classify every event.
3. If total event count is <= `BALANCED_SOFT_FRAME_CAP`, emit all as `run_event`.
4. If above cap:
   - emit all `critical` and `normal` events;
   - keep the most recent `BALANCED_ACTIVITY_KEEP_TAIL` activity events as
     concrete `run_event` frames;
   - group older activity-only contiguous spans into `run_event_gap` frames.
5. Sort final frames by sequence coverage so client high-water advances
   monotonically.

The cap is not a hard upper bound when there are many critical/normal events.
Safety wins over frame count. The only events that may be shed are activity
events, and only with explicit gap coverage.

## HTTP/API changes

### `GET /api/runs/:id/events/stream`

Existing query:

```text
?since_seq=N
```

Additive query:

```text
?consumer_id=<id>&delivery=lossless|balanced
```

Behavior:

- no `consumer_id`: existing F-115 behavior;
- `consumer_id` + no explicit cursor: starts from stored ack high-water;
- bad `consumer_id` -> `400`;
- bad `delivery` -> `400`;
- corrupt ack -> `500`;
- corrupt event ledger behavior unchanged: initial read `500`, mid-stream neutral
  `event:error` then end.

SSE frame names:

| event | data |
|---|---|
| `run_event` | existing `RunEvent` v2 JSON |
| `run_event_gap` | new `RunEventGap` v1 JSON |
| `error` | existing neutral string for mid-stream ledger corruption |

`run_event` frame shape and `id=seq` are unchanged.

### `POST /api/runs/:id/events/ack`

Body:

```json
{
  "consumer_id": "webui-main",
  "high_water_seq": 42,
  "delivery": "balanced",
  "acked_events": 10,
  "acked_gaps": 1,
  "shed_events": 80
}
```

Response: validated `RunEventAck`.

Errors:

| case | status | body |
|---|---:|---|
| invalid run or consumer id | 400 | neutral short string |
| unknown run | 404 | `run not found` |
| corrupt ledger / corrupt ack | 500 | neutral short string; detail only in server log |
| backwards ack / beyond ledger | 409 | neutral short string |

No error body should include a path, raw JSON line, event payload, env name, or
consumer file path.

## WebUI v1 adoption

WebUI can adopt F-120 without replacing the existing `/api/events` state tick:

1. Generate or load a local `eventConsumerId` from `localStorage`, prefixed with
   `webui-` and path-component-safe.
2. When the Run Inspector Events tab mounts for a run, open:

```text
/api/runs/<run>/events/stream?consumer_id=<id>&delivery=balanced
```

3. Process both `run_event` and `run_event_gap`.
4. Ack the highest contiguous covered seq after the batch has been accepted into
   component state.
5. On unmount, close the EventSource. Do not ack unprocessed frames.

The existing Dashboard / run-state flow still relies on `/api/events` and
`RUN_STATE.json`. F-120's typed event stream is for the Events tab and future
MCP/TUI consumers, not for replacing the state snapshot.

If Step 3 decides WebUI is too large for the first backend slice, ship the
backend endpoint and CLI dogfood first; WebUI can be Step 4. The schema and
server contract must still be designed with WebUI in mind.

## Privacy and security

- Ack/gap files contain only ids, sequence numbers, enum strings, counters, and
  timestamps.
- `consumer_id` must be a local symbol, not a user identifier. Generated browser
  ids are random and local-only.
- Ack APIs never return event payloads except through the existing `run_event`
  stream.
- Error bodies are neutral. Path-rich details are logged server-side only.
- Gap frames disclose only sequence ranges and the fact that activity was shed,
  never the dropped event messages/payloads/refs.
- No raw prompt, transcript, model output, log line, artifact path, env key/value,
  token, or absolute path enters the ack/gap schema.
- Ack files are not exported to public mirror dogfood artifacts.

## Implementation slices

### Step 1 - Schema + pure delivery projector

Files:

- Create `src/schema/event_delivery.rs`.
- Create `src/scheduler/event_delivery.rs`.
- Modify `src/schema/mod.rs`.
- Modify `src/scheduler/mod.rs`.

Work:

- Add `RunEventAck`, `RunEventAckRequest`, `RunEventAckStats`, `RunEventGap`,
  `DeliveryMode`, `RunEventDeliveryClass`, and validation functions.
- Add `classify_event(&RunEvent) -> RunEventDeliveryClass`.
- Add a pure `plan_delivery_frames(events, cursor, mode, options)` function that
  returns ordered frame descriptors:
  - concrete event frame;
  - gap frame.
- Keep `lossless` identical to current `event_stream_frames`.
- Add balanced tests:
  - critical events are never gapped;
  - activity-only spans gap when above cap;
  - mixed spans split so critical events are concrete;
  - `Other(_)` is critical;
  - gap JSON validates and contains no event payload/path.

Gate:

- targeted Rust tests for `event_delivery`;
- standing all-targets gate if implementation code lands.

### Step 2 - Ack storage + HTTP ack endpoint

Files:

- Create `src/scheduler/event_ack.rs`.
- Modify `src/server/handlers/runs.rs`.
- Modify `src/server/mod.rs`.
- Modify `src/scheduler/mod.rs`.

Work:

- Implement `ack_path(run_dir, consumer_id)` with path-component guard.
- Implement `read_ack`, `write_ack_atomic`, and `update_ack`.
- Add `POST /api/runs/:id/events/ack`.
- Validate against current ledger `last_seq` by reading `events.ndjson`; corrupt
  ledger is `500`, never `0`.
- Enforce monotonic high-water:
  - lower than stored -> `409`;
  - greater than ledger last seq -> `409`;
  - equal or higher within ledger -> write.
- Add handler tests:
  - success writes/returns v1 ack;
  - invalid consumer id 400 before FS;
  - unknown run 404;
  - corrupt ack 500 neutral body;
  - corrupt ledger 500 neutral body;
  - backwards/beyond-ledger 409;
  - ack file JSON contains no path/payload.

### Step 3 - Stream integration with stored ack and balanced delivery

Files:

- Modify `src/server/handlers/runs.rs`.
- Extend existing `events_stream_tests`.

Work:

- Extend `EventsStreamQuery` with `consumer_id` and `delivery`.
- Cursor resolution order:
  - query `since_seq`;
  - `Last-Event-ID`;
  - stored ack high-water for `consumer_id`;
  - `0`.
- Replace the internal `event_stream_frames` helper with the pure delivery
  planner from Step 1.
- Emit `run_event_gap` frames in balanced mode.
- Advance the server-side stream cursor by event seq or gap `to_seq`.
- Keep existing no-consumer behavior unchanged.
- Add tests:
  - no consumer remains byte-compatible enough for F-115 tests;
  - consumer resumes from stored ack when no explicit cursor;
  - query cursor wins over stored ack;
  - balanced mode emits gap frames for activity pressure;
  - critical event after a gap still emits as `run_event`;
  - mid-stream corrupt ledger still emits neutral `error` and ends;
  - invalid delivery / consumer id 400.

### Step 4 - WebUI Events tab adoption (small, optional if Step 3 grows)

Files:

- Modify `web/src/api.ts`.
- Modify `web/src/components/TaskInspector.tsx` or the current Run Inspector
  Events tab component.
- Add small shared type definitions under `web/src/types.ts` if needed.

Work:

- Generate a path-safe local consumer id once per browser profile.
- Open the run event stream in balanced mode for the Events tab.
- Render `run_event_gap` as a compact "activity collapsed" row, not as an error.
- Ack high-water only after frames are accepted into state.
- Do not show raw payload bodies in UI. Existing event message/display fields are
  short and validated; refs remain short chips / counts.
- Keep `/api/events` state tick unchanged.

Tests/gate:

- web `tsc -b`;
- `vite build`;
- manual or Playwright dogfood with a neutral run:
  - initial stored ack skips already accepted events;
  - balanced gap row appears under synthetic activity pressure;
  - terminal event still appears after pressure.

### Step 5 - Dogfood + private close

Dogfood:

- Build a neutral run with a dense event ledger:
  - many `evidence.captured` / `verify.started` activity events;
  - at least one `task.completed`;
  - one `task.approval_required` or `run.failed` critical event.
- Verify lossless stream emits all events.
- Verify balanced stream emits gap(s) for old activity, but concrete critical
  frames still appear.
- Ack through `POST /events/ack`, reconnect without `since_seq`, and verify the
  stream starts after stored high-water.
- Corrupt ack and corrupt ledger tests return neutral errors, not raw paths.

Close:

- standing Rust all-targets gate;
- web build gate if Step 4 lands;
- explicit private wordlist secret scan;
- private main push only; public mirror remains paused.

## Resolved decisions

1. WebUI uses one browser-local consumer id, not one id per run tab. Per-run ack
   files are already separated by run directory, and one browser-local id keeps
   reconnect behavior predictable.
2. `TaskStarted` remains `normal` in v1. Only `activity` is shed in the first
   cut; making task-start events droppable can be revisited after dogfood.
3. `delivery=balanced` is opt-in. Default `lossless` preserves F-115 behavior and
   keeps existing stream clients stable.
4. There is no ack debug/list endpoint in v1. The only surfaces are the stream
   query and the ack POST response.

## Acceptance criteria

- Existing F-115 stream clients continue to work without `consumer_id`.
- Ack-aware clients can reconnect without a cursor and resume from stored
  `high_water_seq`.
- Ack high-water is monotonic and cannot jump past the current ledger.
- Critical events are never dropped/gapped in balanced mode.
- Low-value activity may be covered by explicit `run_event_gap` frames.
- `events.ndjson` remains complete, append-only, and untouched by ack.
- F-117 resume and F-112 monitor do not consult ack state.
- No new artifact contains raw prompt, transcript, log, model output, path, env
  key/value, token, or user identity.

## Status — shipped

All five steps landed on private `main` (public mirror paused throughout).

- **Step 1 — schema + pure projector.** `schema/event_delivery.rs`
  (`RunEventAck`/`RunEventGap` + closed `validate_ack`/`validate_gap`) and
  `scheduler/event_delivery.rs` (`classify_event` over the real `RunEventKind`
  variants; `plan_delivery_frames` — lossless byte-identical to F-115, balanced
  sheds older `activity` into contiguous gaps that never span a kept or missing
  seq). No endpoint/FS.
- **Step 2 — ack storage + endpoint.** `scheduler/event_ack.rs`
  (`read_ack` missing→None/corrupt→Err with consumer/run identity checks;
  `write_ack_atomic`; `plan_ack` monotonic + ≤ ledger last_seq; `commit_ack`
  runs read→plan→write under a per-consumer FS lock so concurrent acks can never
  regress the persisted high-water) + `POST /api/runs/:id/events/ack`
  (400/404/409/neutral-500).
- **Step 3 — stream integration.** Cursor order `since_seq` > `Last-Event-ID` >
  stored ack > 0; balanced gap frames wired into the SSE loop; a stored ack that
  points beyond the current ledger (truncated/rewritten) is a neutral 500, never
  a silent caught-up; corrupt-ledger initial read is a neutral 500 with no path.
- **Step 4 — WebUI Events tab.** Path-safe per-browser consumer id (localStorage
  with a module-level per-session fallback); balanced `EventSource`; `run_event`
  rows (short validated fields only) and `run_event_gap` as a compact
  "activity collapsed" row; ack only after a frame is in state, never on unmount.
- **Step 5 — dogfood + close.** Token-free end-to-end against a neutral run
  (`example-workspace` / `demo-core`, 2 normal + 300 `verify.started` activity +
  `task.completed` + `run.completed`): balanced shed = 68 concrete frames + one
  `run_event_gap` `[3..238] count 236 activity_backpressure`, with the normal and
  critical/terminal events all concrete; ack high-water 250 → reconnect streams
  only 251..304; full ack → caught up; `Last-Event-ID: 200` → resumes at 201,
  bypassing the ack; corrupt ack (identity-mismatched) and corrupt/truncated
  ledger all return neutral 500s with no path. A neutral WebUI screenshot of the
  Events tab (the gap row between concrete normal events and the concrete activity
  tail) was captured for review.

Gate at close: `cargo` check + clippy `-D warnings` + test all-targets green; web
`tsc -b && vite build` green; explicit-private-wordlist release-scope secret scan
clean.
