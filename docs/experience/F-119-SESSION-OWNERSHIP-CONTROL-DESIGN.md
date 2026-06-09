# F-119 - Session ownership / control (design)

Status: design / awaiting review - Owner: dali design / dafu implementation

Parent: [reference local agent-runtime package absorption](REFERENCE-LOCAL-AGENT-RUNTIME-PKG-ABSORPTION.md) -
borrowed direction #2. It follows [F-117](F-117-SESSION-RESUME-GUARDRAILS-DESIGN.md)
and depends on the Chat cleanup/Conversation surface from
[F-UI-002](F-UI-002-CHAT-CONVERSATION-SURFACE-DESIGN.md).

This is a **design document only**. It does not add feature code. It locks the
local chat-session ownership contract, session-reuse signature, busy-session
guard, first-turn idempotency rule, privacy boundary, and implementation slices.

## Problem

Maestro has a local Chat surface and durable chat-session JSON files. A Chat turn
currently works roughly like this:

1. `POST /api/chat/messages` loads or creates a `Session`;
2. `send_streaming_with_options` appends the user message and saves the session;
3. it builds either the full first-turn prelude or a follow-up recap;
4. it streams through the selected provider;
5. on completion it appends the assistant message and saves the session;
6. approved actions run subcommands with `MAESTRO_SESSION_ID`, so the resulting
   run can link back to the originating chat session.

This is functional, but it has no local ownership guard. Two clients or two tabs
can start turns against the same session. A UI-side abort closes the browser
stream, but the backend task may still be running. A provider session id
(`cursor_chat_id` today; other providers may return a session id later) can be
reused even if the local prompt context that created it has drifted. A retry of
the first turn can append the same user request again and resend the expensive
first-turn prelude.

F-119 adds a small local control layer around Chat. It does not change the run
scheduler, does not replace F-117 resume, and does not introduce cloud/session
infrastructure. It answers three questions:

- is this provider session safe to reuse for this local chat session?
- who owns the session right now?
- has this first user turn already been accepted, so a retry must not resend it?

## Goals

- Compute a privacy-safe session **reuse signature** from local context that
  affects provider-session reuse: workspace fingerprint, provider, model,
  mode, prompt-context contract, project registry, memory-topic index, and skill
  index. If the signature changes, open a fresh provider session instead of
  silently resuming a drifted one.
- Add a local **busy ownership** guard so only one chat turn or one approved
  action can own a session at a time.
- Add **first-turn idempotency** so a client retry / reconnect does not append
  the same first user message or resend the full first-turn prelude.
- Keep Chat a Conversation surface: executable work still links to Run/Task;
  run safety remains with F-117 resume.
- Keep all new artifacts local-only and privacy-safe: no raw prompt, transcript,
  provider stdout/stderr, absolute path, env key/value, token, or raw skill body.

## Non-goals

- No cloud session store, remote daemon, websocket control plane, SSO/RBAC, PAT,
  member invite, multi-tenant state, or telemetry.
- No bot / group-chat / message entry. Chat remains a single local operator
  surface; bot work follows a dedicated collaboration reference if needed.
- No provider transcript migration or cross-host chat-session replay.
- No in-place scheduler takeover. F-117 continues to own interrupted run resume.
- No new run/task status model inside Chat. Run/Task/Finding/Event remain the
  authorities for executable work.
- No raw prompt body in any new descriptor, even hashed descriptors.

## Existing surfaces to reuse

| Surface | Reuse in F-119 |
|---|---|
| `src/chat/sessions.rs` | Existing `Session`, `Message`, `cursor_chat_id`, save/load/list behavior. |
| `src/chat/stream.rs` | Current turn assembly, provider resolution, first-turn prelude vs follow-up recap. |
| `src/chat/actions.rs` | Approved actions already pass `MAESTRO_SESSION_ID` into `work/run/rerun`. |
| `src/server/handlers/chat.rs` | Chat message/action endpoints; v1 ownership checks live at this boundary. |
| F-UI-002 Chat cleanup | UI already aborts/clears transient state on session switch; F-119 adds server-side ownership. |
| F-117 resume guard | Separate run-level safety gate; F-119 must not treat run resume as chat-session reuse. |
| F-115 events | Run events are linked by explicit run/session id only; no parsing assistant text. |
| `paths::validate_path_component` | Path-component guard for session ids, action ids, turn ids, and control files. |
| `scheduler::liveness::classify_pid` | Local pid liveness for stale owner recovery. |
| `file_guard::stable_hash_bytes` | Stable `fnv1a64:*` hashes for signatures without storing body content. |

## Source of truth

F-119 is a control guard, not a new transcript or run ledger:

- chat `Session` JSON remains the transcript and user-visible message history;
- provider-specific session ids remain adapter/provider details;
- F-117 `RESUME.json` remains the run resume guard;
- F-115 `events.ndjson` remains the run event ledger;
- the F-119 control file records only ownership, hashes, and turn receipts.

If the control file is missing for an old session, the first new turn creates it.
Missing control state does **not** block reading an existing chat transcript. It
only controls whether a new turn/action may start and whether a provider session
id may be reused.

## Storage

Add a local control directory under `.maestro/chat/`:

```text
.maestro/chat/control/<session-id>.json
.maestro/chat/locks/<session-id>.lock
```

`<session-id>` must pass `paths::validate_path_component("session id", id)`.
Existing chat endpoints should start validating URL/body session ids before
touching the filesystem; invalid ids return `400`, not `404` or a path-derived
error.

The control JSON is written atomically (`tmp + fsync + rename`) and validated on
read/write. The lock file is acquired with an atomic create/open-exclusive style
operation. Holding the lock is short: prepare ownership, write control/session
metadata, then release. Long-running provider/action work is represented by the
`active_owner` record and periodically refreshed; the filesystem lock is not
held for the whole model stream.

## Schema

Types should live in:

```text
src/schema/session_control.rs
```

Filesystem/control helpers should live in:

```text
src/chat/control.rs
```

`schema/mod.rs` exports:

```text
SESSION_CONTROL_V1 = "maestro.session_control.v1"
session_control_version()
```

### `SessionControl`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.session_control.v1` |
| `session_id` | string | safe path component; must match filename/session |
| `created_at` | string | RFC3339 |
| `updated_at` | string | RFC3339 |
| `signature` | `SessionReuseSignature?` | last signature that safely owns provider-session reuse |
| `active_owner` | `SessionOwner?` | current owner, if any |
| `turns` | `SessionTurnReceipt[]` | bounded recent receipts, newest last; no content |

Keep `turns` bounded (for example last 64 receipts) so control files do not grow
with transcript length. The first-turn receipt is special: once present it is
retained for the life of the session, because it is the guard that prevents a
retry from resending the expensive first-turn prelude. Older non-first receipts
can be dropped once their dedup window is not useful.

### `SessionReuseSignature`

| field | type | notes |
|---|---|---|
| `hash` | string | `fnv1a64:*` over the canonical signature payload |
| `provider` | string | `cursor/codex/claude/...`; symbol only |
| `model` | string? | effective model id, symbol-ish single-line value |
| `mode` | string | `plan/exec` |
| `workspace_hash` | string | hash of normalized workspace identity, not path |
| `prompt_contract_hash` | string | hash of the prompt assembly contract/template version only |
| `project_registry_hash` | string | hash of project names/types/scopes, not raw paths |
| `memory_topic_hash` | string | hash of topic names only |
| `skill_index_hash` | string | hash of skill name/scope/trigger metadata only, not skill body |
| `created_at` | string | RFC3339 |

The canonical payload is deterministic JSON with sorted keys/lists. It may use
hashes of local inputs, but it must not store raw workspace path, raw prompt,
raw skill body, raw `projects.yaml`, raw memory content, env names/values, or
provider stdout/stderr.

`prompt_contract_hash` covers the **shape** of prompt assembly: prelude template
version, section ordering, first-turn vs follow-up contract, and mode-prefix
contract. Project registry, memory topics, and skill metadata are deliberately
separate hashes. They are not double-counted into `prompt_contract_hash`; this
keeps drift reasons inspectable and avoids accidental signature gaps.

`status_snippet` / current run state is **not** part of the provider-session
reuse signature. It changes every turn and is intentionally resent as fresh
turn context. Including it would force a new provider session on every run
tick and defeat reuse.

### `SessionOwner`

| field | type | notes |
|---|---|---|
| `owner_id` | string | safe component; turn id or action id |
| `kind` | string | `chat_turn` or `action` |
| `pid` | u32 | local process that owns the stream/action |
| `started_at` | string | RFC3339 |
| `heartbeat_at` | string | RFC3339 |
| `session_signature_hash` | string? | hash observed when owner started |
| `message_id` | string? | assistant message id for chat_turn |
| `action_id` | string? | action id for action |
| `run_id` | string? | set when an action launches/links a run |

Only `kind` values above are written. Future owner kinds require a v2 schema.

### `SessionTurnReceipt`

| field | type | notes |
|---|---|---|
| `turn_id` | string | client-provided idempotency key, safe component |
| `client_nonce_hash` | string? | optional hash if client gives a nonce separate from turn id |
| `role` | string | v1 only `user` |
| `phase` | string | `accepted/running/done/failed/abandoned` |
| `first_turn` | bool | true when session had no prior messages when accepted |
| `user_message_id` | string | persisted user message id |
| `assistant_message_id` | string? | reserved/created before provider call |
| `request_hash` | string | hash of sanitized request envelope (text hash, mode, provider, model) |
| `signature_hash` | string | signature used for this turn |
| `accepted_at` | string | RFC3339 |
| `settled_at` | string? | RFC3339 |

The text hash is enough for dedup and auditing. The raw text is already in the
regular `Session` transcript because the operator typed it; duplicating it in
control artifacts is forbidden.

## Validation rules

Validation lives in `schema/session_control.rs` and is called before every
control write and after every control read.

Required checks:

- `schema_version == maestro.session_control.v1`;
- RFC3339 timestamps;
- `session_id`, `owner_id`, `turn_id`, `message_id`, `action_id`, and `run_id`
  are safe single path components when present;
- all enum-like strings are closed (`chat_turn/action`,
  `accepted/running/done/failed/abandoned`, `plan/exec`, `user`);
- hashes use `fnv1a64:` + 16 lowercase hex characters;
- `active_owner.owner_id` references either the running turn receipt or action;
- turn ids are unique within the bounded receipt list;
- `first_turn=true` appears at most once and that receipt is never dropped by
  bounded-history compaction;
- no string contains newline, absolute/drive/UNC/file URI, `..`, or an env-like
  key/value token.

Reading corrupt control JSON is an error. Starting a new turn with corrupt
control refuses with a neutral `session.control_corrupt` error instead of
silently ignoring the guard.

## Session signature semantics

Before a provider call, compute the effective signature from current local
inputs. Compare it to `SessionControl.signature`:

- **same signature** -> provider session id may be reused, if the provider
  supports reuse and the `Session` has the provider id;
- **different signature** -> do not pass the old provider session id to the
  provider. Write the new signature after the turn is accepted. Keep the chat
  transcript; only provider-session reuse is reset.

Provider-specific v1 behavior:

| provider | current behavior | F-119 behavior |
|---|---|---|
| `cursor` | `Session.cursor_chat_id` is passed via `--resume` when present | pass it only when signature matches; clear/ignore it on signature drift |
| `claude` | provider returns a session id but v1 does not resume it | store signature but do not enable resume unless provider code adds explicit support |
| `codex` | stateless for chat provider-session reuse | signature still recorded for consistency; no provider-session id to pass |

Signature drift is not a user-facing error. It is a safe reopen. The UI may show
a small Activity row later ("provider session reopened") but v1 does not need a
new panel.

## Ownership / busy semantics

Starting a chat turn or executing an approved action calls:

```text
prepare_session_owner(session_id, owner_kind, owner_id, signature, request_hash)
```

Rules:

1. validate `session_id` and owner ids before filesystem access;
2. acquire the short filesystem lock;
3. load or initialize `SessionControl`;
4. if `active_owner` is present and `pid` is live -> refuse with
   `409 session.busy`;
5. if `active_owner` is present but its pid is abandoned **or** its heartbeat is
   stale -> mark it `abandoned`, clear it, and continue;
6. write the new `active_owner` and turn receipt/action marker;
7. release filesystem lock.

Heartbeat must not depend on model deltas. Slow providers can be silent for a
long time, so progress-driven heartbeat would produce false abandonment. V1 uses
a fixed interval:

- heartbeat interval: 30 seconds while a chat turn/action is active;
- stale threshold: 3 minutes since `heartbeat_at`;
- each heartbeat briefly re-acquires the session lock, reloads the control file,
  verifies `active_owner.owner_id` still matches, updates only `heartbeat_at`,
  writes atomically, and releases the lock;
- if another owner replaced it, the heartbeat stops without modifying the file.

The same fixed heartbeat runs for chat streaming and action execution. Deltas,
thinking chunks, and action stdout are not required for heartbeat progress.

Completion clears `active_owner` only if the owner id still matches. This avoids
a late finisher clearing a newer owner that started after stale recovery.

Busy is a local guard, not a queue. V1 refuses rather than enqueues. The WebUI
can show "session is busy in another turn/action"; the user may wait, stop the
current turn in that client, or open a new session.

## First-turn idempotency / dedup

The client should send a `turn_id` idempotency key with each chat message. The
WebUI generates it per composer submission and keeps it stable across retry
until the turn reaches done/failed. The CLI/TUI can do the same with a UUID.

Server behavior:

- If `turn_id` is new: accept it, persist the user message once, and continue.
- If `turn_id` already exists and is `running`: return `409 session.busy` with a
  neutral message; v1 does not attach to the existing stream.
- If `turn_id` already exists and is `done`: first compare the current
  `request_hash` with the receipt's `request_hash`. If it matches, emit the
  persisted assistant message as a no-op SSE `done` response (no provider call,
  no new user message). If it differs, refuse with
  `409 session.turn_payload_mismatch`.
- If `turn_id` already exists and is `failed/abandoned`: refuse with
  `409 session.turn_not_retriable`; the operator can start a new turn or new
  session. Do not silently resend the first-turn prelude.

This is intentionally conservative. The goal is not to make provider execution
exactly-once; that is impossible across process crashes without provider-level
transactions. The goal is to prevent Maestro from duplicating its own first-turn
write/prompt and to fail visibly when the state is ambiguous.

### Why first-turn matters

The first turn carries the full system prelude: project registry, memory topics,
skill metadata, action protocol, and current status. Sending it twice can create
two provider-side branches and two copies of the user's first instruction. A
follow-up turn carries a smaller recap. F-119 must not let a reconnect change a
first-turn retry into a follow-up that sends the same user text again with a
different prompt shape.

## Error semantics

Errors returned to HTTP clients are short and neutral:

| code | status | meaning |
|---|---|---|
| `session.invalid_id` | 400 | session/turn/action id failed path-component validation |
| `session.control_corrupt` | 500 | control file parsed/validated as corrupt |
| `session.busy` | 409 | live owner already controls the session |
| `session.turn_duplicate` | 200/SSE done | duplicate done turn replayed from transcript |
| `session.turn_running` | 409 | same turn id is already running |
| `session.turn_payload_mismatch` | 409 | same turn id was reused with different request hash |
| `session.turn_not_retriable` | 409 | failed/abandoned turn id must not be resent |
| `session.signature_drift` | 200 | safe provider-session reopen; not an error |

Server logs may include detailed validation context after redaction. HTTP bodies
must not contain raw paths, env keys, command output, or prompt bodies.

## Interaction with existing features

### F-117 resume

F-117 answers "is a previous run safe to seed from?" F-119 answers "is a chat
session safe to reuse / who owns it now?" They are separate:

- `maestro resume` may be launched from a chat action and still carries
  `MAESTRO_SESSION_ID`;
- run resume validation does not bypass session busy ownership;
- session signature drift does not make a run unsafe to resume;
- F-119 never seeds task outputs or reads `RESUME.json` except through existing
  action/run flows.

### F-115 events

F-119 does not add a new event ledger. In v1, it may write small Activity/link
rows through existing Chat message/action state. Any later typed event wiring
must key off explicit `session_id`/`run_id`, not assistant text parsing.

### F-UI-002 cleanup

UI cleanup remains necessary but insufficient. Aborting a fetch clears the
browser-side stream; F-119 prevents the server-side session from accepting a
second owner until the first owner settles or is proven abandoned.

### Actions and run ownership

Approved actions should acquire session ownership before execution. `work`,
`run`, and `rerun` actions already set `MAESTRO_SESSION_ID`; F-119 records
`run_id` when available but does not invent run state. If an action is running,
a new chat turn against the same session receives `session.busy`.

## Privacy boundary

New artifacts may include:

- session id, turn id, action id, run id;
- provider id and model id;
- `fnv1a64:*` hashes;
- counts and enum-like state;
- timestamps and pid.

New artifacts must not include:

- raw prompt/user text beyond the existing `Session` transcript;
- assistant body, thinking trace, provider stdout/stderr;
- raw system prelude, raw skill body, raw memory content;
- absolute path, workspace root, binary path, env key/value;
- token/secret/cookie/header values;
- package/reference names from absorption materials.

All examples use neutral fixtures such as `billing-service`, `web-frontend`, and
`example-workspace`.

## Implementation slices

### Step 1 - schema + control helpers (no stream/action wiring)

Files:

- create `src/schema/session_control.rs`;
- create `src/chat/control.rs`;
- export `SESSION_CONTROL_V1`;
- add path-component validation for `chat::sessions::session_path` callers or a
  shared `validate_session_id`.

Tests:

- serde round-trip;
- validate rejects corrupt schema, bad enum, bad hash, path-like strings,
  duplicate turn ids, and unsafe ids;
- `session_path("../x")` / URL session ids return guard errors before
  filesystem access;
- atomic write/read: missing -> `None`, corrupt -> `Err`.

### Step 2 - busy ownership guard

Wire `prepare_session_owner` / `clear_session_owner` into a shared turn/action
entrypoint, not only HTTP handlers. Current callers include the server handler,
CLI chat send path, and TUI chat send path; all must pass through the same
ownership/dedup logic or F-119 would only protect WebUI.

Proposed shape:

```text
start_chat_turn(ChatTurnRequest) -> StreamEvent receiver / result
run_chat_action_with_owner(session_id, action_id, decision) -> Action
```

The wrappers call the existing `send_streaming_with_options` /
`execute_action_with_session` after ownership is prepared. The existing lower
functions should either become private to the wrapper or clearly documented as
test-only internals, so future callers do not bypass the guard.

Callers to route through the shared entrypoint:

- `POST /api/chat/messages`;
- `POST /api/chat/actions/:session_id/:action_id`;
- `cli/util.rs` chat stream path;
- `cli/commands/chat_tui.rs` chat stream/action path.

`chat_messages_post` currently returns `Sse<...>` directly. To return `400/409`
before a stream starts, Step 2 should change it to return `Response`: normal
paths wrap the same SSE stream; guard failures return a small neutral body with
the code below. This is an API-shape change but additive for successful calls.

Tests:

- live owner blocks a second chat turn with `409 session.busy`;
- abandoned owner is cleared and a new owner can start;
- stale heartbeat owner is marked abandoned and a new owner can start;
- slow provider with periodic heartbeat is not considered abandoned while
  heartbeat updates continue;
- late finisher cannot clear a newer owner;
- action running blocks chat turn and vice versa;
- WebUI, CLI, and TUI all hit the same guard path;
- invalid session/action ids return 400.

### Step 3 - signature-based provider-session reuse

Compute signature before provider call. Use it to decide whether provider session
ids are safe to pass:

- cursor `--resume` only on signature match;
- signature drift clears/ignores provider-session reuse but keeps transcript;
- stateless providers record the signature but do not pretend to reuse.

Tests:

- same workspace/provider/model/prelude metadata reuses cursor session id;
- model/provider/project-registry/skill-index drift causes safe reopen;
- status/run changes alone do not force reopen;
- signature JSON never contains raw path, raw prompt, raw skill body, or env
  values.

### Step 4 - first-turn idempotency + UI client turn id

Extend `POST /api/chat/messages` and `streamMessage` with `turn_id`.

Tests:

- first turn accepted once: duplicate done turn replays persisted assistant and
  does not append another user message;
- duplicate done turn with the same `turn_id` but different `request_hash`
  returns `409 session.turn_payload_mismatch` and does not replay;
- duplicate running turn returns `409`;
- failed/abandoned duplicate refuses and does not resend provider prompt;
- retry after browser abort does not create a second first user message;
- session switch cleanup from F-UI-002 still clears transient state and does not
  alter persisted receipts.

### Step 5 - neutral dogfood + private closure

Dogfood:

- start a new chat session and send first turn;
- force a duplicate same `turn_id` and verify no duplicated user message;
- start a second tab/turn while first owner is live and verify busy refusal;
- change provider/model or project registry and verify provider session safe
  reopen rather than reuse;
- approve a `work/run` action and verify chat/run linkage remains explicit.

Gate:

- all-targets Rust check/test/clippy;
- web `tsc -b` and `vite build` if UI changed;
- explicit private wordlist release-scope scan;
- private main only, public mirror not pushed.

## Resolved decisions

1. **Duplicate completed turn behavior** -> replay persisted assistant as SSE
   `done` with no provider call **only when `request_hash` matches**. Mismatch is
   `409 session.turn_payload_mismatch`.
2. **Stale running turn recovery** -> stale pid or stale heartbeat clears the
   owner so a new turn can start, but the old `running` receipt becomes
   `abandoned`; the same `turn_id` is not retriable.
3. **Control storage location** -> separate `.maestro/chat/control/` and
   `.maestro/chat/locks/` files. Do not embed control metadata in `Session` JSON;
   that would couple transcript wire, compaction, and bounded control history.

## Acceptance criteria

- Session ids are path-guarded before filesystem access in Chat endpoints and
  helpers.
- A live busy session cannot be taken by another chat turn/action.
- An abandoned owner can be recovered locally without deleting transcript.
- Provider-session reuse is signature-gated; drift opens a fresh provider
  session and does not lose chat history.
- Duplicate first-turn retries do not append the user message again and do not
  resend the first-turn prelude.
- F-117 resume, F-115 events, and F-UI-002 cleanup remain separate and keep their
  existing authorities.
- New artifacts are hash/count/id-only and pass the privacy boundary.
- Tests cover busy, stale owner, signature drift, first-turn dedup, invalid ids,
  and no raw path/prompt/body leakage.

## Status — shipped

All five slices landed (private-only; public mirror not pushed):

- **Step 1** — `schema/session_control.rs` (`maestro.session_control.v1`) +
  `chat/control.rs` helpers + the `session_path` path-component guard.
- **Step 2** — busy ownership guard via the shared `start_chat_turn` /
  `run_chat_action_with_owner` entrypoints (every surface — HTTP/CLI/TUI — routes
  through them); short FS lock, fixed 30s/3min heartbeat (lock contention is a
  tri-state `Contended`, not a stop), pid-abandoned-OR-heartbeat-stale recovery,
  late-finisher guard.
- **Step 3** — signature-based provider-session reuse: cursor `--resume` only on a
  signature match; a drift opens a fresh provider session (full prelude, not a
  recap), persisting the cursor-id clear to disk before the call; the signature is
  hash-only (workspace/registry/topics/skills+description/contract version), never a
  raw path/prompt/body.
- **Step 4** — first-turn idempotency: a client `turn_id` makes a turn idempotent;
  a duplicate `done` replays the persisted assistant ONLY when `request_hash`
  matches, otherwise (mismatch / running / failed / abandoned) a neutral 409.
- **Step 5** — neutral dogfood + private 收口.

Step-5 dogfood (token-free; the guards return before any provider call) confirmed
end-to-end through the binary: invalid session id → 400; a live busy owner → 409;
a duplicate running turn_id → 409; a failed turn_id → 409 (not retriable); a done
turn with the same payload → SSE `done` replay of the persisted assistant with NO
new user message; a done turn with a different payload → 409 (payload mismatch).
Standing gate (cargo check/clippy/test all-targets + secret-scan + web tsc/vite
build) green at closure. F-117 resume, F-115 events, and F-UI-002 cleanup keep their
authorities; all new control artifacts are id/hash/count/enum/timestamp/pid only.
