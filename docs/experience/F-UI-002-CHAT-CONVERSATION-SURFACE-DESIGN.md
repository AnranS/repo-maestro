# F-UI-002 — Chat Conversation Surface (design)

Status: design / awaiting review · Owner: dali design / dafu drafting

Parent: [reference UI/UX absorption](REFERENCE-UI-UX-ABSORPTION.md) ·
sibling: [F-UI-001 operator console](F-UI-001-OPERATOR-CONSOLE-REDESIGN-DESIGN.md).

Design only — no UI code this round. It locks how maestro's **Chat** becomes a
disciplined **Conversation surface** (the Conversation type from the F-UI-001
taxonomy): how a human and the local agent talk, how activity is shown without
flooding, and how the next step is taken. It changes presentation/IA only — it
adds no runtime behavior, and it does NOT make Chat a second task system.

## Goals

- Make Chat a calm three-pane Conversation surface, not a scroll of mixed state.
- Keep the message stream readable: user/assistant are primary; tool/run events
  collapse into Activity by default.
- Header-ize status (workspace/session/provider/model/runtime) so nothing floats.
- A new session is an onboarding entry point, not a blank canvas — without faking
  health.
- A stable, sticky composer; clean session-switch reset.
- Executable work always lands on Run/Task/Finding/Event; Chat only links + summarizes.

## Non-goals

- No bot / group-chat / message **entry** (no inbound channel UI); Chat is the
  single-operator ↔ local-agent surface only.
- No cloud collaboration, member presence, multi-tenant threads, or remote
  transcript store.
- No parallel status/state model in Chat — trackable state stays in Run/Task/
  Finding/Event.
- No new runtime/executor behavior; no decorative redesign.

## Surface placement

Chat is the **Conversation** surface from the F-UI-001 taxonomy — reachable from
the top nav, but **outside** the four operator slots (Dashboard / Run Inspector /
Setup-Config / Timeline). It does not count against the four-slot budget and may
not grow operator panels; operator work is launched from Chat into those slots.

## 1. Three-pane layout

| Pane | Content | Default |
|---|---|---|
| **Left** | session / history list (existing SessionSidebar) | visible |
| **Center** | the message stream (user / assistant) | visible |
| **Right** | collapsible **Activity / Inspector** (tool + run events, run/task links for this conversation) | **collapsed** |

The center stays the conversation; the right pane holds the noise. On narrow
screens the right pane is a sheet, the left is a drawer (mobile rule from
F-UI-001), never a horizontally-crammed three-column.

## 2. Header-ized status

A fixed-height Chat header band (no floating overlays):

- **Identity:** workspace · session title (editable) · session id (short).
- **Engine:** provider · model (shared chip style).
- **State:** a single runtime/session status via the shared status token set —
  `idle` / `streaming` / `reconnecting` / `error`. Loading and streaming live
  **in the header or as a message-group affordance**, never as a floating spinner
  over the stream.

## 3. Message + event layering

Messages are typed and layered, not flattened:

- **user** / **assistant** — the primary stream, full width, readable density.
- **tool-event** (tool_use / tool_result) and **run-event** (run/task lifecycle)
  — **Activity**: collapsed by default into one `Activity · N events` affordance
  per assistant turn (and/or routed to the right pane). Expanding reveals a
  compact, grouped, stable-line-height list — never a raw wall.
- **action** (approval / decision) — an inline actionable card with **one primary
  action** (Approve / Reject), using the shared tokens; resolved actions collapse
  to a one-line result.

Event noise must never push the conversation off-screen; the default view reads
as a human↔assistant dialog with activity tucked away.

### Activity v1 — real data sources only, no synthesized events

The current Chat model has only `messages` / `actions` / `thinking` — there is no
typed tool/run event stream in the session. So **Activity v1 consumes ONLY
existing data**:

- message **actions** (approval/decision cards);
- the **streaming / thinking** affordance (in-flight assistant turn);
- a **run/task link card** for a run explicitly linked to this session (via
  `session_id` / `run_id`).

If there is no explicit run link, Activity shows an **empty/collapsed** state. It
must NOT fabricate "events" by parsing assistant markdown, a trajectory log, or
message text. F-115 typed run events are a later wiring — and only ever keyed off
an explicit run link, never inferred. (Same honesty rule as Dashboard readiness:
no synthesized data.)

## 4. Empty state → onboarding (no fake health)

A new/empty session is not a blank canvas. It shows a small onboarding card with
lightweight entries that link to the right surface:

- pick a profile (→ Setup/Config catalog)
- check readiness (→ Dashboard readiness)
- run a plan preview (→ plan preview)
- start a goal (focus the composer)

These are **links/launchers**, not status. Readiness shown here obeys the same
honesty rule as the Dashboard: items without a data source read "not wired yet"
(until F-118) — the onboarding never fakes a green check.

**No dead launchers (v1).** The WebUI does not yet have a full Setup/Config
catalog or a plan-preview page, so a launcher only renders as a real link when its
target surface exists. Anything not yet wired (e.g. pick-profile catalog,
plan-preview) is a **disabled chip + "not wired yet"**, never a clickable no-op —
the same discipline as the Dashboard next-action set. `start a goal` is always
live: it simply **focuses the composer** (no navigation). As surfaces land, the
disabled chips become real links one at a time.

## 5. Composer

- **Sticky** at the bottom of the center pane; it never scrolls away.
- Fixed control positions: attach · tool · model · send — their layout does not
  reflow when streaming starts/stops (no jump; a disabled/streaming state changes
  affordance, not position).
- **Mobile:** the input is prioritized and never obscured by a bottom bar; the
  send affordance stays reachable above the keyboard.

## 6. Session-switch cleanup

Switching session (or the linked run) must fully reset transient state so one
conversation never bleeds into another:

- abort/clear any in-flight stream and its **pending reply** buffer;
- drop **draft activity** (un-committed tool/run-event accumulation);
- clear **temporary attachment** state and any optimistic pending user message.

The invariant: after a switch, the center/right panes reflect only the newly
selected session — no residual stream, draft, or attachment from the previous one.

## 7. Chat does not replace the task system

When a conversation turns into executable work it **generates or links** a
Run/Task; it does not maintain its own status:

- the chat renders a compact **run/task link card** (id + status chip + a jump to
  the Run Inspector), driven by the existing session↔run linkage;
- the card shows an **entry + summary** (status token + short line), never a
  parallel progress model;
- all trackable state — progress, findings, events, resume — lives in Run / Task /
  Finding / Event and is viewed in the operator slots, not re-implemented in Chat.

## Layout & scroll constraints (acceptance)

The three-pane desktop layout must implement these as hard rules:

- the **center** message pane is the only vertically-scrolling region; the header
  and composer are **fixed** (header pinned top, composer pinned bottom);
- nothing produces **horizontal** scroll at any width;
- on narrow screens the **right Activity** pane is a sheet / drawer or a hidden
  affordance (not a squeezed third column), and the **left** history is a drawer;
- **composer control positions do not move** when streaming starts/stops (state
  changes the affordance, not the layout);
- new components keep the F-UI-001 rules: **no card-in-card**, shared status
  tokens, grouped/collapsible long lists, sanitized-tooltip long refs.

## Privacy boundary

No reference-project name, screenshot, internal URL, raw product copy, or raw
transcript in git or chat. Wireframes use neutral placeholder text. The
Conversation renders the operator's own messages (expected), but Activity/event
rows still obey the standing rule: provenance + short refs, never a raw
prompt/body/secret/absolute path; run/task cards show ids + status, not bodies.

## Anti-patterns to remove from Chat

1. **State stuffed into the message stream** — run status, counters, health mixed
   inline. → Header for status, right pane for activity, run/task link cards for
   work.
2. **Floating loading/streaming** — a spinner hovering over the stream. → In the
   header or the message group.
3. **Event-noise flood** — every tool/run event as its own stream bubble. →
   Collapsed Activity by default.
4. **Blank empty state** — a big empty canvas on a new session. → Onboarding
   launchers (no fake health).
5. **Composer reflow** — controls jumping when streaming toggles. → Fixed
   positions; state changes affordance, not layout.
6. **Cross-session bleed** — A's stream/draft showing under B. → Full switch
   cleanup.
7. **Parallel status in Chat** — a second progress model. → Link to Run/Task.

## UI review checklist (in addition to F-UI-001's)

- [ ] Right surface = Conversation; outside the four operator slots.
- [ ] Status is in the header / message group, never floating.
- [ ] Tool/run events default-collapsed to Activity; stream stays user↔assistant.
- [ ] Empty state is onboarding launchers, no fake health.
- [ ] Composer sticky + fixed control positions; no reflow on streaming.
- [ ] Session switch clears pending reply / draft activity / temp attachments.
- [ ] Executable work links a Run/Task; no parallel status model.
- [ ] Privacy: no raw body/secret/absolute path; shared status tokens; long refs
  → sanitized tooltip; no card-in-card.

## Acceptance criteria

- Chat renders as three panes (left history / center stream / right collapsible
  Activity), Conversation surface, outside the four operator slots.
- The header shows workspace/session/provider/model + a single shared-token
  runtime state; no floating loading.
- The stream shows only user/assistant by default; tool/run events are one
  collapsed Activity affordance per turn; actions are one-primary-action cards.
- **Activity v1 consumes only real data** (actions / streaming-thinking / an
  explicit run-link card); with no run link it is empty/collapsed; it never
  synthesizes events by parsing markdown/trajectory/text.
- A new session shows onboarding launchers (no fake readiness); **unwired
  launchers are disabled chips ("not wired yet"), never clickable no-ops**;
  `start a goal` focuses the composer.
- **Center pane owns scroll; header + composer fixed; no horizontal scroll at any
  width; narrow screens use a sheet/drawer for the side panes; composer controls
  do not move on streaming; no card-in-card.**
- The composer is sticky with fixed control positions and does not reflow on
  stream start/stop.
- Switching session leaves no residual stream/draft/attachment from the previous.
- Executable work appears as a run/task link card; Chat holds no parallel status.
- Privacy + the UI review checklist pass.

## Wireframe 1 — three-pane Chat

```text
┌─ chat ─────────────────────────────────────────────────────────────────────┐
│ WORKSPACE example-workspace · session "contract upgrade"  ⌁ idle            │
│ provider · model                                          [Activity ▸]      │
├──────────────┬──────────────────────────────────────────────┬───────────── ┤
│ SESSIONS     │  ● you                                        │ ACTIVITY     │
│ ▸ contract.. │   upgrade the shared contract across the …    │ (collapsed)  │
│   refactor.. │  ◍ assistant                                  │ 3 tool · 2   │
│   spike ..   │   planned 6 tasks. Activity · 5 events ▸      │ run events   │
│              │   ┌ run 20260605-..  ◍ running 4/7  → open ┐  │ [expand]     │
│              │   └────────────────────────────────────────┘  │              │
│              │  ⏸ action: approve T_review?  [Approve][Reject]│              │
│              ├──────────────────────────────────────────────┤              │
│ [+ new]      │ [📎][tools][model ▾]  message…           [send]│              │
└──────────────┴──────────────────────────────────────────────┴───────────── ┘
   stream = user↔assistant; events tucked into Activity; work = a run link card
```

## Wireframe 2 — new-session onboarding (no fake health)

```text
┌─ chat · new session ───────────────────────────────────────────────────────┐
│ WORKSPACE example-workspace · new session                ⌁ idle             │
├──────────────┬──────────────────────────────────────────────────────────── ┤
│ SESSIONS     │   Start here                                                  │
│ ▸ (new)      │    ▸ Pick a profile          → Setup                          │
│              │    ▸ Check readiness         → Dashboard (some not wired yet) │
│              │    ▸ Run a plan preview      → preview                        │
│              │    ▸ Start a goal            (type below)                     │
│              ├──────────────────────────────────────────────────────────────┤
│ [+ new]      │ [📎][tools][model ▾]  describe a goal…                 [send] │
└──────────────┴──────────────────────────────────────────────────────────── ┘
   onboarding launchers, not a blank canvas; readiness is honest, never faked
```

## Implementation slices (small steps, after design review)

1. **Header + status** — a fixed Chat header (identity/engine/state via shared
   tokens); move streaming/loading out of the stream into the header/group.
2. **Activity (real data only)** — collapse the existing actions + streaming/
   thinking + an explicit run-link card into the Activity affordance / right pane;
   keep user/assistant primary. No synthesized events from markdown/trajectory/text.
3. **Composer stability + session cleanup** — sticky composer with fixed control
   positions; full transient-state reset on session switch.
4. **Onboarding empty state + run/task link card** — onboarding launchers (no
   fake health); render executable work as a run/task link, not a parallel status.

Each slice: web tsc + vite build + explicit-wordlist scan; no Rust unless a route
is touched (then standing all-targets). Private-only; neutral dogfood screenshots.
