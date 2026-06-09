# Reference UI/UX absorption — Round 0 (docs-only)

Status: Round 0 / docs-only · Owner: dali design / dafu drafting

Documentation only — no UI code. It records the **UI/UX discipline** worth
borrowing to reshape maestro's surface, read as interaction + information-
architecture patterns, not as a visual copy. It is the input to
[F-UI-001 — operator console redesign](F-UI-001-OPERATOR-CONSOLE-REDESIGN-DESIGN.md).

## Source

A reference local-agent-runtime **web design system** — its surface taxonomy,
typical pages, and usage guide — read **only** as UX patterns. This document names
no project/package, shows no screenshot, and quotes no internal business copy.
The patterns below are generic and reimplementable from this text alone.

## The core insight

What makes the reference feel calm is **not** decorative styling — it is strong
**information-architecture discipline**. maestro feels busy because capability was
added one panel at a time: each feature grew its own surface. The reference does
the opposite — it fixes a small set of **surfaces** first, assigns every page to
one, and then lets complex data expand progressively inside that fixed slot.

The borrowed direction: stop being a *collection of function panels* and become a
**local multi-repo operator console** with a fixed surface taxonomy. (This matches
the house aesthetic: restrained dev-tool, density over ornament, motion only when
it carries meaning.)

## Primitives to absorb

### 1. Define surfaces first, then place features

The reference classifies every page into one of a few **surface types**, and a
new page picks a surface before it picks a layout — so layout converges by
construction:

- **Conversation** — ephemeral exploration (chat-like).
- **Canvas** — a spatial/graph view (nodes, dependencies, a board).
- **Inspector** — an object list/graph on the left, a detail panel + tabs on the
  right.
- **Catalog** — a browsable, read-only inventory of configured things.
- **Console** — an operational status + action surface; detail (logs, events)
  lives *inside* a console view, not as its own page.

Discipline: a new capability does not get a new page; it picks an existing surface.

### 2. One app shell — no card-in-card

Only the outermost content shell has any floating/elevated feel. **Inside** a
page, grouping comes from headers, thin borders, spacing, and list-item rows —
not from nesting cards inside cards. Stacked large cards add visual weight and
read as clutter; they are disallowed.

### 3. workspace-first / run-first, not feature-first

The first screen stays organized around the *current workflow* of the workspace,
not around feature names. For maestro that means run/operator-first: can it run
now, which run is active, which task is stuck, what to do next — the user should
never have to hunt between "findings / context / events / resume" feature labels
to orient.

### 4. Inspector mode (list/graph + detail tabs)

Object→detail screens are uniformly: left = object list or graph, right = the
selected object's detail with tabs. It scales to any per-object capability by
adding a tab, never a new panel.

### 5. Neutral canvas + small semantic signals

Large areas are neutral gray/white. Status is carried by **small** chips, dots,
or thin lines — color answers a status question and is never decorative. A single
**status token set** is shared across the whole UI: `running`, `done`, `failed`,
`blocked`, `awaiting approval`, `skipped`, `cancelled`, `resume-unsafe`, `risk`
each map to one chip + one color everywhere, instead of every panel inventing its
own palette.

### 6. Dense but calm

Tool UIs use restrained type: small base sizes, **tabular** figures, stable line
height, fixed-height toolbars. Density is controlled with grouped headers,
metadata chips, and collapse — so a long list (context layers, findings, events)
reads as a structured table, not a "debug log wall".

### 7. Issue-first, not data-first

Error/empty copy says **what happened + what to do next** before it shows fields.
"resume blocked: plan drift → `rerun` / inspect PLAN" comes first; the event seq /
finding id / context ref are secondary evidence the user can open.

### 8. Mobile does not crop the desktop layout

Narrow screens get a *different* layout, not a horizontally-squeezed desktop one:
a graph/board becomes a grouped list, a side detail becomes a sheet. (maestro need
not ship mobile now, but the rule is recorded so the desktop layout is never hard-
crammed into narrow widths later.)

## Review checklist (borrowed)

Every UI change is gated against a short, repeatable checklist — the same one the
reference applies — so the IA discipline holds over time. See F-UI-001 for the
maestro-specific checklist; the borrowed shape is: right surface? no new top-level
panel? no card-in-card? issue before data? shared status token? density
controlled? long refs deferred? privacy preserved?

## Non-goals

- No cloud / SSO / RBAC / multi-tenant console; no remote-device or member views.
- No bot/chat **operator** entry; Conversation stays ephemeral exploration,
  distinct from trackable Tasks/Runs. (If bot/group collaboration is ever wanted,
  it is a separate effort against a dedicated bot-collaboration reference.)
- No decorative / flashy redesign; the win is fewer, calmer, more legible
  surfaces.

## Privacy boundary

No project/package name, internal URL, screenshot, or internal business copy in
git or chat — every borrowed pattern is described generically. Any surface that
renders run data still obeys the standing rule: provenance + counts + short refs,
never a raw prompt / body / secret / absolute path. Wireframes and demos use
neutral fixtures only (`billing-service`, `web-frontend`, `shared-contracts`,
`example-workspace`).
