# F-135 — Global light-theme UI sweep — scan + contract

Status: **SWEEP CLOSED — 大力 PASS on all four slices. F-135a (`5b1aa05`) + F-135b (`a9d8113`)
+ F-135c (`9b849df`+B1 `6b26d48`) + F-135d (P2, `4b73dd1`) all CLOSED. Final proof: `rg
'text-(red|amber|emerald|blue)-(50|100|200|300)' web/src` → 0; build + CI green each slice;
non-status blue → accent, destructive not weakened, dark held at the -300 shades, primary
buttons + layout untouched. Deferred (大力 non-blocking, out of this caliber): residual
`text-*-400` — icons / dots / strong-emphasis / graph-local colors — not pale-foreground debt;
own future polish if ever wanted.** The
graph pages got light/dark love; the NON-graph pages reuse the shared tokens but were never
verified in light. This scan grades the light-mode contrast debt and proposes a **token-first**
fix. **Scope: web styles/components ONLY — no Rust/API/business semantics; light is the target,
dark must NOT regress; no landing/marketing redesign.**

## 0. Slice status
- **F-135a (foundation) — DONE (`5b1aa05`)**: 4 theme-aware status FG tokens
  (`text-status-{success,warning,danger,info}`; dark == the old `-300` shade → dark unchanged,
  light == `-700`) in `index.css` + `tailwind.config.js`; shared `StatusChip` TOKENS map (15
  chips) rewired through them; the 29 `focus:border-blue-600` inputs → `focus:border-accent`
  (pure styling); status `-400` dots kept (pin 6). Proof of a clean foundation cut: the
  `rg 'text-(red|amber|emerald|blue)-(200|300)' web/src` count dropped 189 → **174** (only
  StatusChip's 15 tokenized; the ~174 scattered ad-hoc chips left for b/c/d). `pnpm -C web
  build` green; 10 light/dark shots (dashboard/tasks/deliveries-list/deliveries-detail/settings)
  — light legible, dark byte-equivalent. NO shared `StatusBadge` yet (pin 2: introduced in b).
- **F-135b (P0) — DONE (`a9d8113`)**: introduced shared `ui/StatusBadge.tsx` (`StatusTone` +
  `statusToneClasses` + `StatusBadge`) — a tone-keyed companion to the status-string-keyed
  `StatusChip`, FG via the F-135a `text-status-*` tokens. DeliveriesView: corrupt chip →
  `StatusBadge`; stage / blocked-on / danger-button ternary color triads → `statusToneClasses`;
  error panels + bare status text → `text-status-*` FG swap (each site's bg/border/padding kept
  verbatim). SettingsModal: `EnforcementBadge` tone map (hard/soft/unsupported), `GateRow`
  enabled, error panels → `text-status-*`; active model-picker item → `bg-accent/text-accent`.
  P0 residual `text-*-200/300`: **DeliveriesView 17→0, SettingsModal 7→0** (web/src 174→150).
  Primary `bg-blue-600` buttons untouched; soft stays amber "advisory"; dark byte-equivalent.
  `pnpm -C web build` green; 8 light/dark shots (deliveries blocked/bypass + settings
  matrix/gates).
- **F-135c (P1) — DONE (`9b849df`)**: 18 files (TaskRow, tasks/RunSummary, FailureRecovery/Gate/
  Outcome/Evidence/Coordination/Discussion panels, TaskInspector, TasksView, Chat{View,Input},
  MessageBubble, SessionSidebar, RunHealthStrip, RunsSidebar, GoalPanel, Dashboard). FG → the
  F-135a `text-status-*` tokens, each site's bg/border/padding kept verbatim (no forced
  StatusBadge — sizes/opacities vary). **Non-status blue → `accent` (NOT info)**: TaskRow link
  toggles (`hover:accent-soft` preserves the dark 400→300 hover), ChatView/SessionSidebar/
  CoordinationPanel selections, Dashboard "Open Run Inspector" action, MessageBubble user-avatar
  identity. Destructive buttons → `text-status-danger` (red-700 in light = stronger, not
  weakened). Per-P1-file `text-*-200/300`: all 18 → 0 (web/src 150→74). Dark stays the -300
  shades (accent-migrated selections shift blue-200→blue-400, still legible). `pnpm -C web build`
  green; 8 light/dark shots.
- **F-135d (P2 + polish) — DONE (`4b73dd1`)**: scan caliber widened to `-(50|100|200|300)`
  (the F-135c B1 lesson). Last 74 sites across 21 files (Memory/Docs/Context + LogPane/
  ActionCard/PathPicker/AddProjectModal/ErrorBoundary/ModelPicker/StarMapPanel + graph pages +
  ui/{StatePane,Chips} + Header). status/error/warning/success → `text-status-*`; brand/
  selection/link/identity blue → `text-accent` (section icons, active doc section, selected
  model, path crumb, ref/hover links); mixed-blue files (MemoryView, ProjectMemoryPanel) split
  precisely (icon/link→accent, status ternary→info); non-status other families (cyan symlink,
  purple verify-task) left per the rule; Header token-usage badge `amber-200`→`status-warning`
  (justified illegible-in-light, stays amber). **Final: `rg 'text-(red|amber|emerald|blue)-
  (50|100|200|300)' web/src` → 0** (whole sweep done). `pnpm -C web build` green; 10 light/dark
  shots (memory/docs/context/context-detail/dashboard-spotcheck).

## 1. The token system (current)
`web/src/index.css` defines CSS vars for dark (`:root`) and light (`html[data-theme="light"]`),
mapped by `tailwind.config.js` into semantic Tailwind tokens: `ink/ink-dim/ink-mute/ink-faint`,
`bg/bg-soft/bg-panel/bg-inset/bg-hover`, `line/line-soft`, `accent/accent-soft/accent-muted`.
Theme toggles via `data-theme` + `maestro-theme` localStorage + the Header Sun/Moon button.
**These semantic tokens are theme-aware and fine.** The gap is the STATUS colors — they are
NOT in the token system.

## 2. Root cause (the one thing to fix)
The shared `StatusChip` token set (`ui/StatusChip.tsx`, F-UI-001) and ~60+ ad-hoc chips use
RAW Tailwind shades for status text: `text-{red,amber,emerald,blue}-300` (and `-200`). On
DARK bg these read fine (light text on dark); on LIGHT bg a `-300` shade is ~70% pale and
becomes **illegible** (e.g. `text-amber-300`/`text-red-300` on near-white). The translucent
chip backgrounds (`bg-*-500/15`, `border-*-500/30`) are theme-neutral-ish and OK — **only the
foreground text shade is wrong for light.**

## 3. The fix — 4 theme-aware status FG tokens (minimal root fix)
Add to `index.css` + Tailwind a small status FG token family, theme-aware:
| token | light | dark |
|------|-------|------|
| `text-status-success` | emerald-700 | emerald-300 |
| `text-status-warning` | amber-700 | amber-300 |
| `text-status-danger` | red-700 | red-300 |
| `text-status-info` | blue-700 | blue-300 |
Then replace `text-{emerald,amber,red,blue}-{200,300}` → the matching `text-status-*`. Keep
the translucent `bg-*-500/15` + `border-*-500/30` (theme-neutral). This is the single-source,
"use a token not a hardcode" fix; dark is preserved (the dark value == today's shade).
Also: `focus:border-blue-600` (15+ inputs) → `focus:border-accent` (blue-600 ≠ the light
`accent`). Status DOTS (`bg-*-400`) are saturated enough — sample-check, likely keep.

## 4. Page inventory + grading (light-mode readiness — worst first)
| Page | File | Worst issues | Grade |
|------|------|-------------|:---:|
| **Deliveries** | `DeliveriesView.tsx` | 12+ `text-red/amber/emerald-300`/`-200` (corrupt/blocked/clear/run-status/timeline/writeback/gate/accept-reject) | **P0 (20%)** |
| **Settings** | `SettingsModal.tsx` | enforcement matrix `text-{amber,red,emerald,blue}-300` rows + gate badges + `bg-black/60` backdrop | **P0 (30%)** |
| **Chat** | `SessionSidebar/ChatInput/MessageBubble` | tag chip `text-blue-200`, input state `text-amber-200`/`text-emerald-200`, code-fence label | **P1 (40%)** |
| **Tasks panels** | `TaskRow` + `tasks/*Panel` | many `text-*-300` badges (GateBanner/OutcomePanel/Evidence/Coordination/FailureRecovery) | **P1** |
| **Dashboard** | `Dashboard.tsx` | `text-blue-200` "Open Run Inspector" button | **P1 (50%)** |
| **Memory** | `MemoryView.tsx` | `text-blue-300` icon/hover, `bg-blue-600 text-white` button | **P2 (60%)** |
| **Docs** | `DocsView.tsx` | active section `text-blue-300`, `bg-blue-600/80 text-white` | **P2 (70%)** |
| **Context** | `ContextView.tsx` + dialogs | mostly fine; `focus:border-blue-600`, dialog `text-red-300` errors | **P2 (70%)** |

## 5. Proposed token / component changes
1. **`index.css` + `tailwind.config.js`**: add the 4 `status-*` FG tokens (theme-aware). [foundation]
2. **`ui/StatusChip.tsx`**: rewire the TOKENS map's `text-*-300` → `text-status-*` (≈14 of 22 states; the `bg-bg-inset text-ink-faint` neutral ones already adapt). [foundation]
3. **A shared `<StatusBadge tone="success|warning|danger|info">`** (small, in `ui/`) consuming
   the same tokens — to replace the 20+ scattered ad-hoc `bg-*-500/15 text-*-300` chips so the
   sweep dedups instead of touching 60+ call sites by hand. *(open point — vs in-place token swap.)*
4. **Form inputs**: `focus:border-blue-600` → `focus:border-accent` (sweep). [foundation-adjacent]
5. Hardcoded `bg-blue-600 text-white` action buttons read OK in light (dark blue + white) —
   leave for v1, flag as polish.

## 6. Recommended slice order (each its own CI-green + reviewed commit + light/dark shots)
- **F-135a (foundation)**: the 4 status tokens + `StatusChip` rewire + `focus:border-accent`.
  Verifiable immediately (every StatusChip surface gets correct in light). Highest ROI.
- **F-135b (P0 pages)**: DeliveriesView + SettingsModal — the worst, highest-visibility.
- **F-135c (P1)**: Chat (SessionSidebar/ChatInput) + the `tasks/*Panel` badges + Dashboard,
  via the shared `StatusBadge` (if pinned).
- **F-135d (P2 + polish)**: Memory/Docs/Context + hover/faint + button-token cleanup.
*(If 大力 prefers fewer cuts, a/b can merge; c/d can merge.)*

## 7. Screenshot + verification matrix
light/dark × { Dashboard, Tasks (a run), Delivery detail, Chat, Context, Memory, Docs,
Settings } — ≥16. Per slice: shoot the pages it touches (light = the target; dark = the
non-regression check). Graph pages (Architecture/CodeGraph): SAMPLE-regress in dark only (the
foundation could shift a shared chip) — no rework. Build (`pnpm -C web build`) green each slice.

## 8. Do-not-absorb
Web styles/components ONLY — no Rust/API/schema/business-logic change; no page redesigned into
a landing/marketing style; dark must not regress (the token's dark value == today's shade); no
new feature/behavior; graph pages get sample-regression only (no big rework); don't touch the
delivery/run logic the recent slices added (only their CSS classes).

## 9. Open decisions for 大力 to pin
1. **Fix mechanism**: theme-aware `status-*` FG tokens (recommended — single source, dark-safe)
   vs Tailwind `dark:`-variant per class vs picking a both-themes shade. *(lean: status tokens.)*
2. **Shared `StatusBadge` component**: introduce it to dedup the 20+ ad-hoc chips (recommended)
   vs in-place token swap at each site (less new surface, more churn). *(lean: introduce it.)*
3. **Slice granularity**: 4 cuts (a foundation / b P0 / c P1 / d P2) (recommended) vs fewer/
   bigger. *(lean: 4; merge if you prefer.)*
4. **`focus:border-blue-600` → `accent`**: fold into the foundation (recommended) vs its own slice.
   *(lean: foundation.)*
5. **Hardcoded `bg-blue-600 text-white` buttons**: leave (read OK) (recommended) vs token-ify to
   `bg-accent`. *(lean: leave for v1.)*
6. **Status DOTS (`bg-*-400`)**: keep (saturated enough) (recommended) vs token-ify too. *(lean:
   keep, sample-check in the screenshots.)*

## 10. Review axes: token system → §1; root cause → §2; fix → §3; page grading → §4;
token/component changes → §5; slice order → §6; screenshots → §7; Do-not → §8; recommended +
open → §9. No code until GO.
