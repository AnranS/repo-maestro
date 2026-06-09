# F-UI-003 — Operator console redesign (design + audit)

Status: design / awaiting review · Owner: dali direction / dafu design + implementation

This is a docs-only design/audit. No implementation code changes land with this
document. It turns dali's redesign direction into a concrete audit, a taste-skill
ruleset, an information architecture, a visual-token system, component rules, a
small-step implementation plan, and a screenshot/dogfood bar — for review before
any code.

## 0. Frame and non-goals

maestro's WebUI is a **local-first engineering operator console for multi-agent
runs**. The audience is engineers/operators. The job to be done, glanceable, is:
*can I run? what is running? where is it stuck? what is the next action?* The
visual language is **calm dev-tool / dense console / high signal** — not a
landing page, not a portfolio, not a marketing SaaS hero.

**Direction (dali):** dark graphite console — low-saturation dark, thin borders,
few shadows, no gradient orbs / glass / hero. Density high but not cramped.
Status color is semantic only (running / done / failed / approval / risk), never
decorative. System sans + mono/tabular numbers, no serif, no editorial hero.
Motion 2/10 — only cause-effect motion (running indicator, tab/expand), respect
reduced-motion.

**Stack kept (no churn):** React / Vite / Tailwind v3 / lucide. lucide is
discouraged by the taste-skills, but it is the existing global icon family;
v1 does **not** swap icon libraries — that would be churn with no operator value.

**Non-goals / hard review bar (from dali):**
- No new UI library; no framework rewrite; no API/behavior break.
- No landing-page visuals: no hero, no marketing headline/copy, no three-card
  promo row, no decorative gradients.
- No card-in-card nesting; every panel has one clear data responsibility.
- Every button / icon-button has hover / focus / disabled / loading semantics and
  is keyboard reachable.
- Mobile must not scroll horizontally; text/chips must not overflow; graph/canvas
  needs a stable container.
- No new UI surface may show raw prompt / body / log / absolute path / internal
  detail; neutral fixtures only.
- Verify at least web tsc + vite build + scan; if Rust/API is touched, run the
  standing all-targets gate; deliver desktop + mobile screenshots.

## 1. taste-skill selection (取舍)

The taste-skill pack is at the shared `~/.agents/skills`. Selection:

| Skill | Use | What we take |
|---|---|---|
| `redesign-existing-projects` | **primary — method** | Scan → Diagnose → Fix; audit-first; work with the existing stack (Tailwind v3), don't rewrite; the states/typography/color discipline; small targeted reviewable changes. |
| `ui-ux-pro-max` | **primary — checklist** | Its Quick Reference §1–§10 as the per-slice review checklist (accessibility, touch/interaction, performance/CLS, semantic tokens, layout/responsive, typography/color, animation, forms/feedback, navigation). |
| `minimalist-ui` | **assist — restraint only** | The utilitarian core: 1px borders as the primary structure, near-zero shadows, crisp ≤8–12px radius (no pill containers), color as a scarce semantic resource, mono/tabular figures, `<kbd>` for shortcuts. |
| `design-taste-frontend` | **not used** | It self-scopes to landing/portfolio and explicitly says *NOT dashboards / data tables / multi-step product UI*. maestro is exactly that, so its high-visual/anti-slop landing rules do not apply. |

**Explicitly dropped** (rules that belong to marketing/editorial, not a console):
hero imagery / `picsum` placeholders, glassmorphism, parallax / scroll-driven
reveals, serif editorial headers, macro whitespace (`py-24/32`), broken-grid
asymmetry, ambient gradient blobs, and `ui-ux-pro-max`'s mobile-native-only items
(haptics, notch safe-area, bottom tab bar — this is desktop-web).

**Net rules adopted** (the cross-section that fits a calm console):
- audit-first, fix-in-place, don't break the stack (redesign);
- semantic color tokens, never raw hex in components; color never the only signal
  (pro-max §6 `color-semantic` / `color-not-only`);
- visible focus rings on every interactive element (pro-max §1 `focus-states`);
- one consistent elevation scale; restrained borders over shadows (minimalist +
  pro-max §4 `elevation-consistent`);
- tabular/mono figures for ids, sizes, durations, counts (pro-max §6
  `number-tabular`);
- motion conveys cause-effect only, 150–300ms, transform/opacity, reduced-motion
  honored, ≤1–2 animated elements per view (pro-max §7);
- shared empty / loading / error states (redesign + pro-max §8 `empty-states`);
- no horizontal scroll, 4/8 spacing rhythm, z-index scale (pro-max §5);
- active nav highlighted; predictable back; deep-linkable tabs (pro-max §9).

## 2. Current UI audit (what we have)

**Shell:** 8 hash-routed tabs (dashboard / chat / tasks / context / memory /
architecture / codegraph / docs); header ≈52px, three-column (logo · tab pills ·
settings) + a goal/connection row. Stable, per-tab `ErrorBoundary`. Good base.

**Tokens (already calm — keep):** `tailwind.config.js` defines a correct dark
graphite palette — `bg` #0a0a0a / `bg-soft` #111 / `bg-panel` #161616 / `bg-inset`
#0d0d0d / `bg-hover` #1c1c1c; `line` #1f1f1f / `line-soft` #262626; one accent
(blue #60a5fa); `ink` #ededed / `ink-dim` / `ink-mute` / `ink-faint`. The
palette already follows the direction; the problem is **inconsistent application**,
not the palette.

**Shared status set exists but is bypassed:** `ui/StatusChip.tsx` (`statusToken`)
is one good shared set of 13 statuses — but ~40 places re-implement status colors
ad-hoc (≈250 raw `bg-emerald/red/amber/blue-*` hits): `TasksView` `laneChip`,
`FindingsPanel` hex `SEVERITY_COLOR` map, `CoordinationPanel` icon colors,
`GateBanner` amber, `OutcomePanel` risk, `Header` count chips via `graph/tokens.ts`
hex.

**The core problem — Run Inspector panel stacking:** `TasksView` stacks 11+
same-weight `<section>`s vertically (health strip → run header → gate → summary →
failure-recovery → cost → auto-actions → goal → outcome → evidence → findings →
coordination → **tasks panel last**). That is ~1500px+ of scroll before the
operator reaches the actual task list/DAG. Tasks — the point of the screen — are
buried under everything else.

**Prioritized hit-list (from the audit):**
1. **Run Inspector panel-stacking** — 11+ flat same-weight panels; tasks buried. *(HIGH)*
2. **Status-color duplication** — ~40 ad-hoc reimplementations vs one `statusToken`. *(HIGH)*
3. **No shared empty / loading / error** — each view rolls its own bare `<div>`. *(HIGH)*
4. **No focus rings** — zero `:focus-visible` across the app; keyboard-hostile. *(HIGH)*
5. **Decorative shadows + `rounded-xl`** — `shadow-2xl/xl/lg`, logo glow, 22 `rounded-xl`. *(MED)*
6. **Mobile overflow** — `CodeGraphView` absolute `w-80` panel, unaudited `min-w-0`. *(MED)*
7. **Motion noise** — `animate-pulse`, `timelineShine`, `maestroTaskPulse`. *(MED)*
8. **Findings severity as hex map** — disconnected from the status language. *(MED)*

**Already good — keep and build on:** the token palette, `StatusChip`, the
ContextView sidebar+inspector pattern, per-tab `ErrorBoundary`, the honest
F-118 readiness (no fake cached health), and the neutral-fixture discipline.

## 3. Information architecture (priority order)

Following dali's priority. The theme: re-establish hierarchy so each surface
answers one question, and the core (tasks) is never buried.

1. **App shell** — console chrome: stable, low height, always shows nav + current
   run + connection status. No per-view chrome drift. Active tab highlighted.
2. **Dashboard** — a real home that answers *can I run / what's running / what's
   next*: Readiness (real F-118 probe), Active runs, Next action — three honest
   panels, not a card pile, no fake states.
3. **Tasks / Run Inspector (core)** — collapse the 11-panel stack into a clear
   hierarchy: a compact **health strip** (status counts + gate + risk), then the
   **primary** task surface (DAG / list / lanes / timeline lenses) near the top,
   then **secondary** context (summary / findings / coordination / evidence /
   outcome / cost) as collapsible or on-demand sections — default-collapsed when
   not actionable. One primary action per panel.
4. **Task detail** — keep the 6 tabs (overview / context / findings / events /
   artifacts / logs), but unify visual density, empty/loading/error, and the
   redacted/truncated/omitted chips across all tabs.
5. **Context** — F-116 context manifest + F-121 skill inventory are the
   *explainability* surfaces: keep them as read-only inspectors (sidebar + detail),
   not editor-style full pages. (SkillDetail editor stays untouched.)

## 4. Visual tokens (the calm graphite system)

The palette is already right; this section makes its **application** consistent
and adds the missing tokens. All values are Tailwind v3 `theme.extend`.

- **Surfaces / borders (keep):** `bg / bg-soft / bg-panel / bg-inset / bg-hover`;
  `line / line-soft`. Borders are the primary structure (minimalist), not shadows.
- **Status — single source of truth:** everything status-like routes through one
  token set (extend `StatusChip`/`statusToken`): run/task status, **findings
  severity**, **risk**, **gate**. The locked mapping (§8.4): critical→failed-tone;
  high→`risk_high`/amber; medium→`blocked`/amber; low→`idle`/muted;
  gate→`awaiting_approval`. Add these as **semantic aliases** in the token source —
  business components must never hand-mix colors. Kill the `FindingsPanel` hex map,
  `laneChip`, `CoordinationPanel`/`GateBanner`/`OutcomePanel` inline colors, and chip
  colors from `graph/tokens.ts`. Always pair status color with an icon or text label
  (color-not-only).
- **Radius:** `rounded-lg` (8px) for panels; `rounded` (4–6px) for chips/inputs/
  buttons. Drop `rounded-xl` for panels and `rounded-full` for containers. Small
  status chips may stay subtly rounded.
- **Shadow:** drop decorative shadows (`shadow-lg/xl/2xl`, logo glow) on ordinary
  panels and inline expanders — structure comes from `border-line`. Keep exactly one
  restrained **overlay elevation token** for *true floating layers* (modal / drawer /
  combobox popover): a low-opacity, background-tinted shadow, plus a scrim for
  modals/drawers (40–60% black). One scale, no ad-hoc `shadow-2xl` drift.
- **Type:** system sans (current) for UI; **mono + `tabular-nums`** for all ids,
  sizes, durations, counts, token usage (prevents column jitter). No serif. Weight
  hierarchy 600 (headings) / 500 (labels) / 400 (body). Sentence case, not Title Case.
- **Motion (2/10):** transform/opacity only. The running indicator becomes one
  restrained steady pulse (or dim/bright), not multiple competing animations; drop
  `timelineShine`. Tab/expand transitions 150–200ms ease-out. Everything respects
  `prefers-reduced-motion` (reduce/disable). ≤1–2 animated elements per view.
- **Z-index scale:** define tokens (`base / sticky / dropdown / drawer / modal /
  tooltip`) and replace the ad-hoc `z-20/30/40/50`.
- **Focus:** a shared `focus-visible` ring (e.g. `ring-1 ring-accent` + offset) on
  every interactive element. Currently zero — this is an accessibility requirement.
- **Spacing:** 4/8 rhythm; tighten console section gaps (the run view's `space-y-6`
  is marketing-loose for a console) while keeping touch/scan comfort.

## 5. Component rules (the contract every panel/control follows)

- **Panel:** a semantic `<section>` = `border border-line bg-bg-panel rounded-lg`
  with **one** data responsibility and a header row (icon + label + count or single
  right-action). **No card-in-card.** A panel that has nothing actionable collapses.
- **Status / severity / risk / gate:** always via the shared token getter; never
  inline `bg-emerald/red/amber/blue`. A dedicated `RiskChip` / severity chip derives
  from the same set. Color is always backed by icon or text.
- **States:** one shared `StatePane` (or `Empty` / `Loading` / `Error` trio) with
  consistent copy/icon/tone. 404 and empty are **calm** (ink-faint, not red); a real
  error is a neutral "unavailable" line (never the raw error/path); loading is a
  skeleton or quiet "loading…" for >300ms. `redacted` / `truncated` / `omitted` are
  one shared chip set used everywhere.
- **Buttons / icon-buttons:** hover (bg/text shift) + active (≤scale .98 or bg, no
  layout shift) + disabled (`opacity .5` + cursor + `aria-disabled`) + loading
  (spinner + disabled) + `focus-visible` ring. Icon-only needs `aria-label`. Adequate
  hit area. No dead `#`/no-op buttons (disable visibly instead).
- **Privacy:** no surface renders raw prompt / body / log / absolute path / internal
  detail. Long refs truncate with a **sanitized** tooltip. Neutral fixtures only.
- **Mobile:** every flex child `min-w-0`; no fixed-px that overflows; graph/canvas in
  a stable container with an overflow guard; `w-72` sidebars collapse; the
  `CodeGraphView` absolute panel becomes a drawer on small screens.
- **Semantics:** `<nav> / <main> / <section role> / <aside>` landmarks; no div-soup
  for structural regions; headings in order.

## 6. Step-by-step implementation plan (small, reviewable slices)

Each slice is independently reviewable and shippable, lowest-risk first
(redesign skill's fix-priority: tokens/states before structure before sweep).
**Locked sequencing (dali):** Slices 0–1 are primitive/token/status migration
**only and must not change the IA**; the Run Inspector layout changes start at
Slice 2.

- **Slice 0 — token & primitive groundwork (no visual churn, no IA change):**
  z-index scale + radius/shadow tokens + a `focus-visible` utility + extend the
  status-token set with semantic aliases for severity/risk/gate (§8.4 mapping).
  Config + 1–2 shared helpers; nothing moves visually yet.
- **Slice 1 — shared primitives + status migration (no IA change):** add `StatePane`
  (empty/loading/error) + shared `redacted/truncated/omitted` chips + `RiskChip`;
  migrate `FindingsPanel` hex, `laneChip`, coordination/gate/outcome inline colors,
  and header chips → the single token getter. (Kills hit-list #2, #3, #8.)
- **Slice 2 — Run Inspector IA (biggest win):** restructure `TasksView` from 11 flat
  panels into health-strip + primary task surface (lenses up top) + collapsible
  secondary sections, one primary action per panel, per the locked matrix below.
  (Kills hit-list #1.)

  **Run Inspector default-collapse matrix (locked by dali):**

  | Tier | Panels | Default |
  |---|---|---|
  | Always visible / expanded | run health strip; run header / action row; **task lens panel** (graph/list/lanes/timeline) surfaced near the top | always shown |
  | Actionable → auto-expanded | `GateBanner` (approval/outcome gate present); `FailureRecoveryPanel` (failed/cancelled and re-runnable); `OutcomePanel` (pending outcome); `FindingsPanel` (has high/critical/refute/doctor, or severity needing action) | expanded when the condition holds |
  | Default collapsed | `CostPanel` (unless budget ≥80% or over); `AutoActionsPanel`; `GoalPanel`; `EvidencePanel`; `CoordinationPanel`; low/info-only findings | collapsed, expandable |
  | Terminal run | `EvidencePanel` may show a **compact summary** but details stay collapsed; the task panel stays above — evidence must not push it down | compact summary, details collapsed |
  | Empty / no-op | any panel with no data | not rendered, no placeholder |
- **Slice 3 — Task detail unify:** consistent density + empty/loading/error +
  redacted/truncated across the 6 inspector tabs.
- **Slice 4 — Shell + Dashboard polish:** stabilize console chrome (nav + current
  run + connection), tighten Dashboard into a real home.
- **Slice 5 — visual sweep:** radius/shadow/motion cleanup (drop decorative shadows,
  `rounded-xl`, `timelineShine`; calm the running indicator; reduced-motion), focus
  rings on all interactive elements, mobile overflow fixes (`CodeGraphView` drawer,
  `min-w-0` audit). (Kills hit-list #4, #5, #6, #7.)

Slices 0–1 and 5 are CSS/token/component-only (web gate). Slice 2–4 are structural
React with no API change (web gate). If any slice ends up touching Rust/API
(not expected for v1), it runs the standing all-targets gate.

## 7. Screenshot / dogfood bar

- Every visual slice ships **desktop (1440) + mobile (375)** neutral-fixture
  screenshots; motion changes also ship a `reduced-motion` variant.
- **Slice 2 must additionally show three neutral run states** — running,
  failed/actionable, and terminal — to prove the collapse matrix behaves across
  the run lifecycle (locked by dali).
- **Neutral fixtures only** (`billing-service` / `web-frontend` / `shared-contracts`
  / `example-workspace`); never a real path, business name, or raw body in a shot.
- **Verify per slice:** web `tsc` + `vite build` + secret-scan; standing all-targets
  gate only if Rust/API is touched.
- **A11y spot-check per slice:** focus ring visible + keyboard reachable; color never
  the only signal; primary-text contrast ≥4.5:1 (secondary ≥3:1) on the dark surface;
  `prefers-reduced-motion` honored.
- **No-regression bar:** no behavior/API change; no raw prompt/body/log/abs-path; no
  new dependency; lucide kept.

## 8. Resolved decisions (locked by dali)

These are decided. Implementation must follow them exactly; any change goes back
to dali first.

1. **lucide kept for v1.** The existing global icon family stays; new icons also
   use lucide — never mix icon sets.
2. **Panel radius = 8px.** Our own panels use `rounded-lg`; chips/inputs/buttons
   4–6px (`rounded`/`rounded-md`). Drop `rounded-xl`/`rounded-full` on containers.
   Do **not** mechanically restyle a third-party canvas/control's necessary radius
   (e.g. ReactFlow controls) — only our own surfaces.
3. **Shadow policy.** Ordinary panels/dropdowns drop decorative shadows. Keep
   exactly **one** overlay elevation token for true floating layers
   (modal / drawer / combobox popover) + a scrim. No more `shadow-2xl` drifting
   everywhere.
4. **Single status-color source.** Extend `StatusChip`/`statusToken` to cover
   severity / risk / gate with this mapping:
   - critical → failed-tone
   - high → `risk_high` / amber
   - medium → `blocked` / amber
   - low → `idle` / muted
   - gate → `awaiting_approval`

   Implement by adding **semantic aliases** to the token source; business
   components must never hand-mix colors.
5. **Run Inspector collapse strategy — locked default matrix** (see §6 Slice 2 for
   the authoritative table). Tasks surface near the top; secondary panels collapse
   unless actionable; empty/no-op panels do not render.
6. **Codename `F-UI-003` kept.**
