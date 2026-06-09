# F-UI-001 — operator console redesign (design)

Status: design / awaiting review · Owner: dali design / dafu drafting

Parent: [reference UI/UX absorption](REFERENCE-UI-UX-ABSORPTION.md).

Design only — no UI code. It locks maestro's new **information architecture** on
the borrowed discipline (surface taxonomy → fixed slots → progressive disclosure),
the app-shell and status-token rules, the density and mobile rules, the
anti-patterns to remove, a reusable **UI review checklist**, the WebUI/TUI
alignment, the privacy boundary, and acceptance criteria. It does **not** change
any executor/runtime behavior — it re-homes existing data (F-110 findings, F-112
monitor, F-114 profiles, F-115 events, F-116 context, F-117 resume, codegraph,
memory) into a smaller, calmer surface.

## Goals

- Answer on the first screen: can it run, what's running, what next.
- Fix a small surface taxonomy and four top-level slots; new capability lands
  inside a slot, never as a new top-level panel.
- Issue-first; one shared status token set; calm density; no card-in-card.
- Preserve every existing capability — this is re-homing, not removal.

## Non-goals

- No new runtime/executor behavior and no new data source (F-118 fills the
  Dashboard readiness source *after* this IA lands).
- No cloud / SSO / multi-tenant / remote-device / member surfaces.
- No bot/chat operator entry; Conversation stays ephemeral exploration.
- No decorative redesign.

## Surface taxonomy → maestro

Each maestro destination is assigned exactly one surface type (so its layout
converges by construction):

| maestro destination | Surface | Notes |
|---|---|---|
| Dashboard | **Console** | status + actions; the operator home |
| Run DAG / task list | **Canvas** | spatial dependency view |
| Task detail | **Inspector** | left list/graph + right detail tabs |
| Profiles / Skills / Memory | **Catalog** | read-only inventory of what agents can see |
| Logs / Events | **Console detail** | inside a view (a tab), never their own page |
| Chat | **Conversation** | exploration only, outside the four operator slots |

## Four top-level slots

Exactly four operator destinations. Everything an operator does maps to one:

| Slot | Surface | Answers | Absorbs (today's surfaces) |
|---|---|---|---|
| **Dashboard** | Console | can it run / what's running / what next | health roll-up (doctor), active-run summary, next-action launcher |
| **Run Inspector** | Inspector + Canvas | what is this run doing, where is it stuck | tasks (graph/list), monitor F-112, findings F-110, context F-116, events F-115, trajectory/logs, diff, approvals, resume F-117 |
| **Setup / Config** | Catalog + Console | is my env ready, what can agents see | doctor checklist, projects/architecture, profiles F-114, skills, memory, capability/health F-118 (later) |
| **Timeline** | Console detail | what happened over time | run timeline, history |

Secondary (reachable, not operator slots, not counted against the four-slot
budget): **Chat** (Conversation) and **Docs / code map** (reference).

### Slot 1 — Dashboard (Console, first screen)

Three stacked bands. The constraint is **first-viewport**, not "never scroll":
the first viewport must always surface the readiness line, the active summary, and
the next-action set; the page may scroll below that for overflow. Readability is
never sacrificed to fit everything above the fold.

1. **Readiness** — a compact line of the Setup checklist items (provider, skills,
   profiles, workspace, resume-guard), each pass / warn / fail with a one-line
   reason; warns/fails bubble to the top.
2. **Active** — current run(s): status, progress, blocked count, pending
   approvals, top risk. One row per run; the list is **capped (e.g. 3 rows) with a
   "view all"** so many concurrent runs can't blow out the first viewport; click a
   row → Run Inspector.
3. **Next action** — a fixed set: Run · Resume · Doctor · Configure · View last
   failure; enabled from live state, the most useful one highlighted (Resume when
   an abandoned resumable run exists; Doctor when readiness has a fail).

### Slot 2 — Run Inspector (Inspector + Canvas)

- **Top — run health strip:** status · progress · blocked · approval ·
  **resume-safe** · risk, each a shared status chip; a non-green item is the
  run-level issue line and links down to its evidence.
- **Left — DAG / task list (Canvas):** dependency-ordered tasks with status
  glyphs; the selection drives the right column. Graph and list are two lenses on
  one set, not two top-level tabs.
- **Right — selected task inspector (Inspector), tabs:** Overview · Context
  (F-116) · Findings (F-110) · Events (F-115) · Artifacts (diff/outputs) · Logs.
  Overview carries **at most one primary action** (e.g. Approve when awaiting).

This re-homes F-110/F-112/F-115/F-116/F-117 + diff as tabs/strip elements — none
of them grows a separate panel.

### Slot 3 — Setup / Config (Catalog + Console)

One linear readiness checklist replaces state scattered across doctor / profile /
run-detail:

- Workspace detected
- Provider available
- Profiles valid
- Skills visible
- Plan preview valid
- Resume guard ready

Each item: pass / warn / fail + a reason + a fix action (copyable CLI command or
a jump). Profiles, skills, memory, projects/architecture sit underneath as the
Catalog of "what agents can see"; F-118 becomes the live source for the provider
and resume-guard items.

### Slot 4 — Timeline (Console detail)

The existing run timeline / history — a chronological lens, not a new panel
family.

## Status tokens (one shared set)

A single component renders every status, with one color per state, used in the
health strip, task list, dashboard, and every tab:

| Token | Meaning | Signal | Group |
|---|---|---|---|
| `running` | in progress | neutral-accent dot/chip | active |
| `done` | settled ok | success chip | ok |
| `failed` | settled error | error chip | needs action |
| `blocked` | dep/mail blocked | info/warn chip | needs action |
| `awaiting_approval` | gate paused | warn chip | needs action |
| `resume_unsafe` | resume would refuse | warn/error chip | needs action |
| `risk_high` | high change-risk | warn chip | needs action |
| `skipped` / `cancelled` | not run | faint chip | idle |

`blocked` is a **needs-action** state (it wants diagnosis / unblocking), so it is
NOT a muted chip — it reads at the same weight as failed / awaiting-approval, and
groups under "needs action". Only the genuinely-inert `skipped` / `cancelled`
states are faint/muted. Panels may not invent their own status colors; color
answers a status question only, never decoration. The three groups —
**needs action** (loud) / **active** (neutral-accent) / **ok**+**idle** (quiet) —
drive both color weight and issue-first ordering.

## Density rules

- Restrained type: small base sizes (tool-UI scale), **tabular** figures for
  counts/durations/bytes, stable line height, fixed-height toolbars.
- Long lists (context layers, findings, events) render as grouped tables: a group
  header + metadata chips + collapse, never a flat wall of lines.
- Default density is low: health + progress + next action + top issue; detail is
  one interaction away.

## Issue-first model

Every surface leads with issues, not fields:

1. **Issue** — a short typed line: `resume blocked: plan drift`,
   `provider unavailable`, `approval waiting`, `task failed`, `high risk`.
2. **Evidence** — links to proof: event seq, finding id, context layer, log ref
   (never the raw body — a ref to open).
3. **Action** — exactly one obvious next step: a copyable CLI command
   (`maestro resume …`, `maestro rerun …`, `maestro doctor`) or a jump to the
   right slot/tab.

A clean run shows a calm "no issues" state, not an empty grid of fields.

## Mobile / narrow-screen rules

Narrow screens get a different layout, never a cropped desktop one:

- Run DAG (Canvas) → a **grouped task list** (by status / dependency tier).
- Task inspector (right column) → a **sheet** over the list, not a squeezed
  side-by-side.
- The health strip wraps to stacked chips; toolbars collapse into an overflow
  menu.

(Not implemented now; recorded so the desktop layout is never hard-crammed.)

## Anti-patterns to remove from maestro

1. **A new panel per feature** — every capability sprouting its own top-level
   surface. → It must land in one of the four slots / as a Run Inspector tab.
2. **Card-in-card** — large elevated cards nested inside a page. → Group with
   header + thin border + spacing + list rows; only the app shell elevates.
3. **Ad-hoc status colors** — each panel coloring statuses its own way. → One
   shared status token set.
4. **Data-first** — raw fields/logs shown before the issue + action. → Issue and
   suggested action first; evidence second.
5. **Long refs breaking layout** — a memory topic / skill name / path-like id
   widening a row. → Chip + truncate + tooltip / side detail.

## UI review checklist (gate every WebUI change)

Each WebUI change is reviewed against this before merge:

- [ ] **Right surface** — does it fit an existing surface type (Console / Canvas /
  Inspector / Catalog / Conversation)?
- [ ] **No new top-level panel** — it lands in one of the four slots or as a Run
  Inspector tab.
- [ ] **No card-in-card** — grouping via header/border/spacing, not nested cards.
- [ ] **Issue before data** — problem + action precede raw fields.
- [ ] **Shared status token** — uses the one status set, no bespoke colors.
- [ ] **Density controlled** — stable line height, tabular numbers, grouped +
  collapsible long lists.
- [ ] **Long refs deferred** — chips truncate; only the **sanitized** run-relative /
  symbolic ref in tooltip/side detail (never a raw path/body/secret on hover).
- [ ] **Privacy preserved** — no raw prompt/body/secret/absolute path; logs
  capped + redacted; artifacts workspace-scoped + previewable.
- [ ] **WebUI/TUI parity** — same IA + issue-first ordering in both.

## WebUI / TUI alignment

The four-slot IA, the status token set, the issue-first ordering, and the privacy
rules are identical in WebUI and TUI; only rendering differs (Dashboard → status
header + action keys; Run Inspector → list/detail split with tab keys; Setup →
the checklist; Timeline → a scrollable log). A fix discovered in one surface reads
the same in the other.

## Privacy boundary

Enforced in every slot/tab: never render a raw prompt, layer/skill/role/memory
body, transcript, model output, secret, or absolute path; show provenance +
counts + short refs (long refs → tooltip/side detail); logs line/byte-capped +
redacted; artifacts workspace-scoped + size-capped + previewable-only. Wireframes
and demos use neutral fixtures.

**Tooltip / side-detail is NOT a privacy exception.** A long ref that truncates to
a chip may only reveal, on hover or in a side detail, the **same already-sanitized
run-relative / symbolic ref** — never the raw absolute path, prompt/body, secret,
or any value that the truncation removed. Hover restores width, not redaction: if
a value isn't safe to show inline, it isn't safe to show in a tooltip.

## Acceptance criteria

- Exactly four top-level operator slots; every destination maps to one surface
  type; no other top-level operator panel.
- Dashboard first viewport surfaces readiness + active summary + next-action; the
  active list is capped with "view all"; the most useful next action is
  highlighted from live state (readability is not sacrificed to avoid scroll).
- Run Inspector = health strip + DAG/task list + an inspector with exactly six
  tabs (Overview / Context / Findings / Events / Artifacts / Logs); each tab
  re-homes its capability with no loss.
- Setup shows the six-item checklist with pass/warn/fail + one fix action each.
- One shared status token set across the whole UI; no card-in-card anywhere.
- Issue-first ordering on every surface; each issue carries one action.
- Density rules met (stable line height, tabular numbers, grouped/collapsible
  long lists).
- Privacy: no raw body/secret/absolute path; long refs truncate to tooltip.
- WebUI/TUI render the same IA; narrow screens follow the mobile rules.
- No regression: F-110 / F-112 / F-114 / F-115 / F-116 / F-117 all reachable
  inside the four slots.
- The UI review checklist passes.

## Wireframe 1 — Dashboard first screen (Console)

```text
┌─ maestro ───────────────────────────  [Run] [Resume] [Doctor] [Configure] ──┐
│                                                                             │
│  READINESS                                                                  │
│   ● provider   ● skills   ⚠ profiles (1)   ● workspace   ● resume guard     │
│   1 warning                                  → open Setup to resolve         │
│                                                                             │
│  ACTIVE  (1 run)                                                            │
│   ▸ 20260605-..  example-workspace   ◍ running 4/7   ⏸ 1 approval   ⚑ low   │
│       blocked 0                                   → open Run Inspector       │
│                                                                             │
│  NEXT                                                                        │
│   ▸ Resume  20260601-..  abandoned, resumable — reuses 3 done tasks          │
│     Run a new goal · Doctor · Configure · View last failure                  │
│                                                                             │
└──────────────────────────────────────────────────────────────────────────  ┘
   issue-first: the one ⚠ and the resumable run surface above everything else
```

## Wireframe 2 — Run Detail inspector (Inspector + Canvas)

```text
┌─ run 20260605-..  example-workspace ───────────────────────────────────────┐
│ HEALTH  ◍ running 4/7 · blocked 0 · ⏸ 1 approval · resume-safe ✓ · ⚑ low    │
├──────────────────────────┬──────────────────────────────────────────────── ┤
│ TASKS (dep order)        │ T_change_billing   billing-service · agent       │
│  ✓ T_contract     done   │ [Overview][Context][Findings][Events][Artifacts] │
│  ◍ T_change_billing run  │ [Logs]                                           │
│  ⏸ T_review   approval   │ ── Overview ───────────────────────────────────  │
│  · T_web      pending    │  ◍ running · 1m12s · role backend · risk ▴ low   │
│  · T_verify   pending    │  ▶ primary action: (none — task running)         │
│                          │                                                  │
│                          │ ── Context (F-116) ────────────────────────────  │
│                          │  5 layers · ~1.5k tok · 6.0 KB                    │
│                          │  0 memory.prompt_similarity  1× 0.2KB  topic:…    │
│                          │  3 skills.section            3× 2.6KB  skill:…+2  │
│                          │  refs truncate → tooltip · no raw body            │
└──────────────────────────┴──────────────────────────────────────────────── ┘
   issue-first: a non-green health chip (e.g. "approval waiting") links to its tab
```

## Migration (incremental, no capability loss)

Land the IA as a thin re-home of existing components: route today's tabs into the
four slots, move findings/context/events/trajectory/logs/diff into the Run
Inspector tabs, fold doctor + profiles + skills + memory into Setup, and
introduce the shared status token + the no-card-in-card rule as the first visual
pass. Nothing is deleted; each panel becomes a tab or a strip element. F-118 then
arrives as the Dashboard's live readiness source. Each later slice (F-119…F-122)
lands within this IA, gated by the UI review checklist.
