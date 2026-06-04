# F-115 — Runtime Event Envelope + Stream Normalization (design)

Status: design / awaiting review · Owner: maestro · Reviewer: 大力 · Implements: Round 0 reference-runtime candidate F-115 (see [REFERENCE-AGENT-RUNTIME-ABSORPTION.md](REFERENCE-AGENT-RUNTIME-ABSORPTION.md))

This is a **design document only** — no feature code. It locks the schema,
taxonomy, privacy rules, SSE migration, and implementation slices for normalizing
maestro's existing per-run event ledger into a typed, projectable envelope.

> Correction absorbed from Round 0 review: maestro **already has** a per-run
> event ledger — `scheduler::events::RunEvent` written to `events.ndjson`
> (`paths::RUN_EVENTS_FILE`). F-115 is **not** a second ledger and introduces no
> `runtime_events.ndjson`. It normalizes and evolves the existing `RunEvent`
> schema / producers / consumers, and adds a typed projection for SSE/consumers.

## 0. What exists today (ground truth)

| Surface | Today | File / type |
|---|---|---|
| Per-run event ledger | append-only NDJSON, monotonic `seq` under an append lock, `read_events().last().seq + 1` | `events.ndjson` · `scheduler::events::RunEvent` |
| Current-state snapshot (authority) | full run state, polled | `RUN_STATE.json` · `RunState` |
| Durable audit ledger (authority) | append-only findings | `findings.ndjson` · F-110 `Finding` |
| Aggregate projection (read) | counts / progress buckets / task refs, computed on read | F-112 `RunMonitor` / `TaskDetail` |
| Live web signal | **opaque** "state changed, re-read" tick | SSE `/api/events` → `read_current_state()` |

`RunEvent` v1 fields already present: `schema_version` (`maestro.run_event.v1`),
`event_id` (`{run_id}-{seq}`), `run_id`, `seq`, `timestamp`, `kind`, `task_id?`,
`message?`, `payload` (capped at `MAX_EVENT_PAYLOAD_BYTES = 1024`, large data →
`refs`), `refs: BTreeMap<String, ArtifactRef>`.

Two gaps F-115 closes:
1. **The wire taxonomy is lossy.** `RunEventKind::as_str` collapses distinct
   internal events into fewer wire strings — `TaskApprovalGranted` and
   `VerifyCompleted` both serialize to `task.completed`; `TaskSkipped` →
   `task.cancelled`; `RunCancelled` → `run.failed`. There is no wire kind for a
   usage tick or a finding projection. A consumer cannot tell "approved" from
   "succeeded".
2. **No typed live projection.** `/api/events` emits an opaque tick; clients
   re-fetch the whole snapshot. There is no per-event, seq-cursored stream.

## 1. Source of truth (locked)

- `RUN_STATE.json` stays the **authoritative current-state snapshot**. F-115 never
  writes it and never becomes a parallel state store.
- `findings.ndjson` (F-110) stays the **durable audit ledger**. Events may carry a
  *reference* to a finding; an event is never the audit record of one.
- `events.ndjson` is the **existing per-run event ledger**. F-115 normalizes its
  schema/producers/consumers in place. **No new ledger file is introduced.**
- Direction is one-way and explicit: `finding (durable) → finding.recorded (event
  projection)`. Never the reverse. An event is live transport; a finding is the
  system of record.

## 2. Schema — recommend `maestro.run_event.v2` (additive superset)

**Recommendation: bump to `maestro.run_event.v2`.** Rationale, grounded in the
existing doctor guard:

`doctor::schema_version_checks` already **warns** when it reads a ledger whose
`schema_version` is *newer* than the binary knows (test
`schema_version_checks_warn_on_future_versions` asserts a `maestro.run_event.v2`
ledger produces a "migration" warning). That guard only fires if the version
string changes. F-115 **expands the closed `kind` set** (new kinds:
`task.approval_granted`, `verify.*`, `finding.recorded`, `run.cancelled`,
`task.queued`, `task.skipped`, plus a reserved `usage.sampled`). A pre-F-115 binary's strict
`RunEventKind::from_wire` returns `None` on an unknown kind, which makes
`read_events` **error the entire ledger**. Therefore expanding the taxonomy *must*
bump the version, so the doctor guard fires instead of an old binary silently
choking.

Additive optional *fields* alone would not require a bump — but the taxonomy
change does, so we bump once and add the fields in the same step.

**Rejected alternative — "stay v1 + tolerant reader":** keep `...v1`, add new
kinds, ship a tolerant `Other` fallback. Rejected because a v1 ledger that
secretly contains v2-era kinds defeats the doctor version guard — a pre-F-115
binary would hard-fail with no migration warning. The version string must tell the
truth about content.

### 2.1 Compatibility strategy (required for the bump)

- **v1 read-compat (new binary reads old ledgers):** v2 reader reads v1 lines
  unchanged. v1 has no `status`/`severity`/`display` → serde `default` fills them
  as `None`. v1 wire kinds are a subset of v2; the existing `from_wire` legacy
  aliases stay. No migration of on-disk v1 ledgers is needed.
- **Forward-proofing (v2 reader, future v3 kinds):** add a tolerant
  `RunEventKind::Other(String)` fallback so an unknown future kind degrades to an
  opaque, preserved value instead of erroring the whole ledger. This is the one
  reader-hardening shipped in Step 1; it makes the taxonomy "closed-known + tolerated-unknown".
  **N3 — `Other` is reader tolerance only, never a writer escape hatch.** The
  **writer set stays closed**: producers may only emit kinds from the §3 table —
  there is no code path that writes `Other`. `Other(String)` exists *solely* so a
  reader catches a well-formed *future* kind without erroring the ledger. A
  structurally **corrupt** line is still a hard `read_events` error (it is not an
  `Other`). This split is part of the Step-1 test bar (§6).
- **Writer:** always writes `...v2` after F-115. `RUN_EVENT_V1` const → add
  `RUN_EVENT_V2`; `run_event_version()` returns v2; the doctor's notion of
  "current" becomes v2 (its warn-on-future test shifts to assert against a v3
  literal, or asserts the mechanism generically).
- **Channel outbound** (`reply_to_run_events` filters by wire-kind string) keeps
  working: v2 is a superset; existing filters match the unchanged kinds; new kinds
  are simply available to subscribe to.

### 2.2 v2 envelope fields

Existing fields unchanged. Added fields are all `Option`, `#[serde(default,
skip_serializing_if)]` → forward/backward compatible:

| field | type | meaning |
|---|---|---|
| `status?` | **new** `RunEventStatus` enum (see N1) | post-transition lifecycle status; strict 7-value string set below |
| `severity?` | `Severity` (reuse F-110: Info/Low/Medium/High/Critical) | set on `finding.recorded` |
| `display?` | `DisplayMeta { label, tone?, icon? }` | short structured display hint; **no free-text bodies** |

**N1 — `status` is a NEW enum, not the existing `RunStatus`.** The repo's
`RunStatus` has only four *run-level* values (`running/done/failed/cancelled`); the
F-112 7 buckets come from *task progress*, not `RunStatus`. F-115 introduces a
dedicated event-lifecycle enum — **`RunEventStatus`** — whose serialized strings are
*exactly* the F-112 7-bucket vocabulary, and does **not** reuse `RunStatus`:

```
pending · running · awaiting_approval · done · failed · cancelled · skipped
```

This keeps a stream folded into a snapshot and a fresh F-112 projection from ever
disagreeing, without overloading the narrower run-level `RunStatus`.

**Message-field hardening (privacy) — locked.** `message?` is currently *uncapped*.
v2 caps it at **`MAX_EVENT_MESSAGE_BYTES = 256`** and requires **single-line**:
both `message` and `display.label` **reject `\n` / `\r`** and are a short
structured summary — never raw model output / prompt / transcript. Large or raw
content goes only to a run-relative artifact ref.

## 3. Closed kind taxonomy (v2)

No open strings: producers pick from this table; readers tolerate unknowns as
`Other` (§2.1). "Projects→finding" = a producer *may also* write a `Finding`;
"Monitor" = whether the kind changes a task/run status bucket (the stream↔snapshot
agreement F-112 relies on).

| wire kind | group | producer | status implied | payload cap | projects→finding | affects monitor |
|---|---|---|---|---|---|---|
| `run.started` | run lifecycle | executor | running | 1KB / refs | no | yes |
| `run.completed` | run lifecycle | executor | done | 1KB | no | yes |
| `run.failed` *(new variant `RunFailed`, Step 2)* | run lifecycle | executor | failed | 1KB | may (run-level) | yes |
| `run.cancelled` *(new)* | run lifecycle | executor / cancel | cancelled | 1KB | no | yes |
| `run.cancel_requested` *(new, `CancelRequested`)* | run lifecycle (control) | executor / cancel | — (request, not terminal) | 1KB | no | no |
| `task.queued` *(new)* | task lifecycle | executor | pending | 1KB | no | yes |
| `task.started` | task lifecycle | executor | running | 1KB | no | yes |
| `task.completed` | task lifecycle | executor | done | 1KB | no | yes |
| `task.failed` | task lifecycle | executor | failed | 1KB | may (risk/doctor) | yes |
| `task.cancelled` | task lifecycle | executor | cancelled | 1KB | no | yes |
| `task.skipped` *(new)* | task lifecycle | executor | skipped | 1KB | no | yes |
| `verify.started` *(new)* | task sub-lifecycle | executor | — (omitted; see N5) | 1KB | no | no |
| `verify.completed` *(new)* | task sub-lifecycle | executor | — (omitted; see N5) | 1KB | may | no |
| `task.approval_required` | approval | executor | awaiting_approval | 1KB | no | yes |
| `task.approval_granted` *(new)* | approval | executor / approve | running | 1KB | no | yes |
| `finding.recorded` *(new)* | finding projection | risk / refute / doctor | — (carries `severity`) | ref to finding only | **is** the projection | yes (finding counts) |
| `evidence.captured` | tool/log/artifact ref | executor / evidence | — | refs only | no | no |
| `usage.sampled` *(reserved — deferred, see Q4)* | usage | — (no first-cut writer) | — | small numeric only | no | no |

Notes:
- **N2 — keep the canonical wire kind `evidence.captured`; do NOT rename to
  `artifact.captured` in the first cut.** Existing channel filters may already
  subscribe to `evidence.captured` (`reply_to_run_events`); changing the wire kind
  the writer emits would silently break them. `display`/`group` metadata MAY label
  it "artifact", but the on-wire `kind` string stays `evidence.captured`. Any
  rename is a *separate* migration, out of F-115's first cut.
- **N5 — `verify.*` are sub-lifecycle events and carry no `status`.** During
  verify the task bucket is unchanged (`running`); the authoritative bucket
  transitions are carried by the `task.*` events. `verify.*` therefore **omit**
  `status` rather than invent a value — never a non-vocabulary string like
  `running→done`. Monitor remains authoritative via `RUN_STATE` / F-112.
- **N4 — `finding.recorded` ordering & failure semantics.** The durable
  `findings.ndjson` write happens **first**; only on its success is the
  `finding.recorded` event written, **best-effort** (a failed event write logs a
  `warn`, does **not** roll back the finding, and does **not** interrupt the run).
  If the finding write **fails**, no projection event is written. The event carries
  **only** `finding ref` + `severity` + `kind` + `task_id` — never the finding body.
  It is the live mirror of an already-durably-written F-110 finding.
- **Q4 — `usage.sampled` is reserved but deferred.** It is *not* in the v2
  first-cut **writer** set (no producer emits it yet). It is listed only so the
  taxonomy/version is forward-stable; a real producer waits on the provider-adapter
  cost accounting. The reader tolerates it (it is a known reserved kind), but the
  first cut ships lifecycle / approval / finding / evidence de-collapsing only.
- Tool-level granular kinds (`tool.invoked`/`tool.completed`) are **deferred** —
  `evidence.captured` covers tool/log/diff via `refs` for v2.
- **Run terminal de-collapse (Step 2).** A failed run now emits `run.failed`
  (`RunFailed`, status `failed`) instead of the v1 `run.completed` + Failed
  payload, and the cancel *request* gets its own `run.cancel_requested`
  (`CancelRequested`) distinct from the terminal `run.cancelled`. Since the writer
  string changes, `subscription_keys()` carries legacy aliases so pre-F-115 channel
  subscriptions don't miss events (reply/title stay canonical):
  `RunFailed → run.failed + run.completed`; `RunCancelled → run.cancelled + run.failed`;
  `CancelRequested → run.cancel_requested + run.failed` (read alias `cancel_requested`).

## 4. Privacy & security (locked, reuse existing discipline)

- **No absolute paths.** Large content travels only as `ArtifactRef` with a
  **run-relative** `path` (same rule F-112 enforces). Step 1 adds a validation
  that rejects absolute / `..`-escaping ref paths at append time.
- **No raw transcript / prompt / model output / secret / env** in any field.
  `payload` stays ≤ 1KB (existing `MAX_EVENT_PAYLOAD_BYTES`); `message` and
  `display.label` are short and capped (§2.2). Reuse the existing redaction pass
  (`src/schema/redaction.rs`) over text fields before append.
- **Bounded payload.** The existing 1KB cap stays; `append_event` already errors
  on oversize ("store large data in refs"). v2 keeps that and adds the message cap.
- **Append discipline reused.** Keep the per-path append lock + `read-last-seq`
  sequence allocation (already in `events.rs`) — unchanged.
- **Corrupt ledger is never silently swallowed.** `read_events` already returns a
  hard `Err` on a malformed line. Consumers (SSE projector, monitor fold, CLI)
  must surface that error, never `unwrap_or_default()` it — same rule as the F-112
  Step-2 fix. The tolerant `Other` kind (§2.1) covers *unknown-but-well-formed*
  kinds only; it does **not** swallow structurally corrupt lines.

## 5. SSE migration (additive — does not break the WebUI)

Today `/api/events` emits an opaque `state` tick off `RUN_STATE.json` changes.
F-115 v1 is **purely additive**: the existing tick stays; typed events are added
alongside.

**Decision (Q3, locked by review): Option B.** Add a new **per-run** endpoint
`/api/runs/:id/events/stream` that tails `events.ndjson` from a `seq` cursor and is
resumable via **`Last-Event-ID` / `since_seq`**. The existing `/api/events` state
tick is **untouched** (it stays the global "current run changed" signal). Option A
(overloading `/api/events` with a second event name) is rejected — it has no clean
per-run resume cursor.

A pure projector `RunEventStream::from_events(&[RunEvent], since_seq)` (no FS, no
clock — same purity rule as F-112 builders) backs the endpoint and is the
unit-tested core.

Consumer adoption order:
- **WebUI run-detail timeline** reads typed events first (Step 3).
- **F-112 monitor** is unchanged — clients still call `/api/runs/:id/monitor` for
  the aggregate "where are we now"; the event stream answers "what just happened".
  Monitor stays the snapshot; events are the deltas it could fold from.
- **TUI** can tail `events.ndjson` locally (already on-host) — no new transport
  needed; it adopts the typed kinds opportunistically.
- **MCP/CLI** adopt the projector later (out of scope for v2 step set).

## 6. Implementation slices

1. **Schema + compat projector (tests only, no behavior change).** `RUN_EVENT_V2`
   constant; new `RunEventStatus` enum (N1, the 7 strict strings — *not* `RunStatus`);
   add `status?`/`severity?`/`display?` (optional, defaulted); de-collapse the kind
   taxonomy (§3) keeping `evidence.captured` (N2) + reader-only `Other` fallback
   (N3); `MAX_EVENT_MESSAGE_BYTES = 256` + single-line (reject `\n`/`\r`) cap +
   ref-path validation; `RunEventStream::from_events` pure projector; doctor
   "current version" → v2. Unit-test bar: v1 line reads under v2 (fields default);
   new kinds round-trip; **reader** maps unknown well-formed kind → `Other` (no
   ledger error); **writer set is closed — a producer cannot emit `Other`** (N3);
   structurally corrupt line → hard error (not `Other`); oversize/multi-line
   message + oversize payload rejected; abs/`..` ref path rejected; `verify.*` carry
   no `status` (N5); projector folds a fixed event list to the expected stream +
   7-bucket status agreement vs F-112.
2. **Producers populate typed payload (no UI change).** `executor::record_event`
   + `evidence.rs` set `status` on lifecycle kinds, emit the de-collapsed kinds
   (`task.approval_granted`, `verify.*` without `status`, `task.skipped`,
   `run.cancelled`). Wherever a finding is written, follow N4 ordering: durable
   `findings.ndjson` write first → on success best-effort `finding.recorded`
   (warn-only on event failure, no rollback, no run interruption); on finding-write
   failure, no projection event. **`usage.sampled` is NOT emitted in this cut** (Q4
   defer). Channel outbound filters still pass (superset; `evidence.captured`
   unchanged).
3. **SSE typed projection (Option B, §5).** Implement the per-run
   `/api/runs/:id/events/stream` (`Last-Event-ID`/`since_seq`) over
   `RunEventStream::from_events`; wire the WebUI run-detail timeline. Monitor +
   existing `/api/events` `state` tick untouched.
4. **Neutral dogfood + scan + public sync.** Run against `example-workspace`
   fixtures; release-scope secret scan; public clean-export sync.

## 7. Resolved decisions (locked by review)

1. **v1 vs v2 → v2.** Taxonomy expansion forces the bump so the doctor
   warn-on-future guard stays honest; the v2 reader is v1-compatible.
2. **`message` / `display.label` cap → 256 bytes, single-line.** Both reject
   `\n` / `\r`; never carry raw output. (`MAX_EVENT_MESSAGE_BYTES = 256`.)
3. **SSE → Option B.** New per-run `/api/runs/:id/events/stream` with
   `Last-Event-ID` / `since_seq`; existing `/api/events` `state` tick untouched.
4. **`usage.sampled` → deferred.** First cut ships lifecycle / approval / finding /
   evidence de-collapsing only; `usage.sampled` is a reserved kind with no first-cut
   writer (it waits on provider-adapter cost accounting).

## 8. Prerequisite: F-SCAN-001 (scanner hardening, before Step 1)

The privacy gate must not rely on manual `grep -i` backstops. **Before** F-115
Step 1, a small pre-fix `F-SCAN-001` hardens `scripts/secret-scan.sh`:

- Private wordlist supports an explicit **mode prefix**: `ci:<regex>`
  (case-insensitive), `cs:<regex>` (case-sensitive). **No prefix keeps today's
  case-sensitive behavior** — so existing wordlists do not suddenly explode with
  false positives.
- Clarify the script/wordlist comments: matching is **not** implicitly
  case-insensitive; entries needing case folding use `ci:`.
- A minimal self-test asserts: `ci:` catches Titlecase/PascalCase; a short `cs:`
  token does **not** collide with camelCase identifiers (e.g. `offsetTop`);
  CJK / non-ASCII patterns are not broken by word-boundary handling.
- The operator-local wordlist switches the case-fold-needing entries to `ci:`
  (wordlist contents are never committed).

This avoids the global-`-i` regression (short tokens colliding with camelCase) while
closing the Titlecase-brand-leak hole that the F-115 design doc itself tripped on.
