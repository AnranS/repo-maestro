# F-136a2 — Settings 5-block information architecture — contract

Status: **CLOSED** (大力 PASS, range `b33ceb5..e1642ab` + B1 fix `e1642ab`, CI `27139924291` green). B1: the Providers matrix could silently vanish on a fetch failure — fixed to a three-state (loading / unavailable / table) so the security info surface is never hidden. — Implemented in a single commit:
SettingsModal restructured into 5 `CollapsibleSection` blocks (Runtime+Providers open by
default); language hoisted above as a UI preference; gate flags (`gate_on_*` / `auto_pr`)
shown as READ-ONLY enabled/disabled badges (not toggles); `max_total_tasks` read-only,
distinguished from `max_parallel`; Network&Secrets + Evidence&Audit are honest
placeholders. The Providers block adds the per-provider enforcement matrix via a new
read-only `GET /api/providers/profiles` (projecting `provider_permission_profile` — no TS
hand-copy); Soft is rendered amber as "advisory", legend "soft, NOT hard-blocked",
unsupported red (fail-closed), n/a gray. Additive — `DefaultsPut`/PUT unchanged, nothing
newly writable, no behavior change. Tests: 1 server_api endpoint test + 2 Rust matrix unit
tests (incl. unknown-provider fail-closed, kept out of the API). 6 light/dark screenshots
(overview / providers matrix / review-gates). Full gate green.

---

Original contract (design-only) follows. Second F-136 cut. Restructure
the Settings modal from a flat parameter list into the 5 risk-legible blocks —
**Runtime & Isolation / Network & Secrets / Review Gates / Providers / Evidence &
Audit** — so an operator can read the safety posture, not just toggle knobs. **ADDITIVE
only: no existing control is deleted, no setting becomes newly writable, NO security /
runtime behavior changes.** a2 reorganizes + adds READ-ONLY explanation/labels; the
editable surface is unchanged.

## 0. What exists today (the honest starting point)
`SettingsModal.tsx` is a flat modal (gear in Header) with 7 controls — language,
default agent (select), agent_model + tagger_model (ModelInput), branch_prefix, max_
parallel, model-refresh. It writes only 6 `Defaults` fields via PUT
`/api/settings/defaults` (`DefaultsPut`: agent, branch_prefix, max_parallel,
agent_model, cursor_model, tagger_model). The Rust `Defaults` struct ALSO holds
`gate_on_high_risk`, `gate_on_policy_violation`, `refute_on_high_risk`, `auto_pr`,
`max_total_tasks`, … which GET returns but the web neither models nor edits. The
per-provider enforcement matrix lives ONLY in Rust (`provider_permission_profile`,
`permissions.rs:97`) — no endpoint. `CollapsibleSection` (`web/src/components/ui/`) is a
reusable block primitive. The 5 blocks have very uneven real content — a2 must be honest
about that, not fake-fill.

## 1. The 5-block IA (what each block shows; ✎=existing editable, 👁=read-only display)
| Block | Content | a2 source |
|------|---------|-----------|
| **Runtime & Isolation** | ✎ max_parallel · ✎ branch_prefix · 👁 max_total_tasks (run task ceiling) · 👁 note: "task worktree isolation is today's boundary; OS/container tiers (rootless/gVisor/microVM) are **deferred to F-136c/e, Linux-only — not available on macOS local**" | GET defaults + static note |
| **Network & Secrets** | 👁 honest placeholder: "no global network/secret settings — egress + secrets are governed per-task (mode) and per-provider today; a default egress policy + secret broker arrive in **F-136d**" | static note |
| **Review Gates** | 👁 current gate config as on/off badges: `gate_on_high_risk`, `gate_on_policy_violation` (F-126), `refute_on_high_risk` (F-106), `auto_pr` — read-only (config lives in projects.yaml; web editing is a later cut) | GET defaults (extend the TS type, **read-only**) |
| **Providers** | ✎ default agent · ✎ agent_model · ✎ tagger_model · ✎ model-refresh · 👁 **per-provider enforcement matrix** (shell/codex/cursor/mock × shell/git_write/network/fs_write/external_dir/mcp → Hard/Soft/Unsupported/NotApplicable), **Soft labelled "advisory — not hard-blocked", never disguised as hard** (the doc's honesty centerpiece) | GET defaults + the matrix (see §3 decision) |
| **Evidence & Audit** | 👁 pointer: evidence + F-110 findings are kept per-run under the run dir; recent policy-gate verdicts + RuntimeProfile labels are on the task/delivery detail (link). No retention/export setting today | static note + deep-link |

**Language** stays a top-level UI preference ABOVE the 5 blocks (it isn't a security
setting — honest placement). Every existing editable control keeps its exact control +
save path (no PUT/API change).

## 2. What does NOT change (additive guarantee)
- `PUT /api/settings/defaults` + `DefaultsPut` unchanged — no setting becomes newly
  writable. The gate flags are DISPLAY-only in a2.
- No `Defaults` field removed; no existing control removed or relocated out of reach.
- No security/runtime behavior change; a2 touches presentation + read-only labels only.

## 3. The one backend question — the Providers enforcement matrix (anti-drift)
The matrix is Rust-only (`provider_permission_profile`). To SHOW it honestly without a
drifting hand-copy in TS (the F-130/a1 lesson), the clean source is a tiny **read-only**
`GET /api/providers/profiles` → `[{ provider_id, enforcement:{shell,git_write,network,
fs_write,external_dir,mcp} }]`, projecting `provider_permission_profile` for the known
providers. Read-only, no behavior change, single source of truth. (Alternative: embed the
matrix in TS — frontend-only but it drifts from Rust. Lean: the endpoint.)

## 4. Structure / reuse
Reuse `CollapsibleSection` for the 5 blocks (first 1–2 open, rest collapsed) + the
existing `Field` primitive for controls. A small `RuntimeProfile`/enforcement badge
reuses the F-UI-001 `StatusChip` tokens. en/zh i18n under `settings.*`. No new editable
form state — read-only blocks render from GET defaults + (the matrix endpoint).

## 5. Do-not-absorb (a2)
No new WRITABLE security settings (gate toggles stay read-only — making them editable
changes the security surface, that's a later cut); no `DefaultsPut`/API write change; no
behavior change; no deletion/hiding of existing controls; no Linux runner/cgroup/
isolation settings (they don't exist — F-136c/e); no change to the provider enforcement
itself (the matrix is a read-only mirror).

## 6. Test + screenshot matrix
- **Frontend**: `pnpm -C web build` green; the 5 blocks render; the existing settings
  still save (no PUT change — a save round-trip test stays green); the Providers matrix
  shows Soft as advisory.
- **Backend** (only if §3 endpoint): a server_api test for `GET /api/providers/profiles`
  (known providers return their matrix; an unknown provider → all `unsupported`).
- **Screenshots**: light/dark of the restructured modal — the 5 blocks, the Providers
  enforcement matrix (advisory labelling), the read-only Review-Gates badges. ≥4–6.

## 7. Open decisions for 大力 to pin
1. **Providers matrix source**: read-only `GET /api/providers/profiles` endpoint
   (single source, no drift — recommended) vs embed the matrix in TS (frontend-only a2,
   but drifts). *(lean: endpoint.)*
2. **Review Gates**: read-only badges of the current gate config (no behavior change —
   recommended) vs make the gate flags editable now (changes the security surface —
   beyond "additive IA"). *(lean: read-only; editable is a later cut.)*
3. **Empty future blocks** (Network & Secrets): show with honest "governed elsewhere /
   deferred to F-136d" messaging (the IA is the point — recommended) vs hide until they
   have content. *(lean: show with messaging.)*
4. **Language**: top-level UI pref above the 5 security blocks (recommended) vs inside a
   block. *(lean: top-level.)*
5. **Collapse**: reuse `CollapsibleSection`, first 1–2 open (recommended) vs all
   always-expanded. *(lean: collapsible.)*
6. **max_total_tasks**: surface read-only in Runtime & Isolation (recommended) vs leave
   config-only. *(lean: surface read-only.)*

## 8. Review axes: 5-block IA → §1; additive guarantee → §2; providers matrix/endpoint →
§3; structure/reuse → §4; Do-not → §5; tests/screenshots → §6; recommended + open points
→ §7. No code until GO.
