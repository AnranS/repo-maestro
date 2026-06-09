# F-136a1 — RuntimeProfile schema + derivation projection — contract

Status: **IMPLEMENTED** (大力 GO'd all 6 pins + 2 extra test conditions). Single
commit: `schema/runtime_profile.rs` (the `RuntimeProfile` enum + explainable
`RuntimeProfileView` {profile, reasons, advisory, unsupported} + the pure `derive`
+ `summarize`), wired into `TaskDetail.runtime_profile` (non-optional) and
`DeliveryView.runtime_profile_summary` (worst-case + per-profile counts, via
`delivery::project_view` which loads the linked run). `observed_write` extracted in
`policy_gate` (behavior-preserving) so the gate and the derivation share one
definition (branch excluded, F-126-fu). Zero behavior change — no enforcement, no
adapter/env/network/sandbox/settings/run/gate change. Pins honored: Absent →
unknown / requires_review (never review_only); shell ⇒ write_local; unknown provider
(unsupported) → requires_review; external_dir/mcp hardcoded-false → high_risk_vm /
tool_limited future-ready + a test locking they can't fake-trigger; opaque Soft dims →
`advisory` (never claimed hard); worst-case keeps the counts breakdown; permission
absent still projects an explicit label. Rust↔TS field-aligned. Tests: 16 derivation +
projection unit tests + 2 server_api summary tests (incl. soft→advisory and
branch-not-observed). Full gate green.

---

Original contract (design-only) follows. First cut of the F-136 Agent
Safe Runtime arc. **Read-only label + projection, ZERO behavior change** — no
adapter/env/network/sandbox change, no settings change, no run-behavior change, no new
enforcement (that's F-136c+). The deliverable is a deterministic **derivation table**
(`PermissionEvidence → RuntimeProfile`) + its projection onto TaskDetail and a
DeliveryView summary. The schema names are secondary; the derivation rules are the
contract.

## 0. Platform boundary (per 大力)
a1 is **cross-platform schema + projection only**. The Linux-only mechanisms (cgroup
CPU/mem/pids, rootless Docker/Podman, gVisor, Firecracker) are **deferred to the later
runner tiers (F-136c/e)** and are **not promised on macOS local**. a1 ships the *label*;
nothing in a1 changes how or where anything runs.

## 1. Input vocabulary — derive off `PermissionEvidence`, not the lossy strings
The authoritative input is `PermissionEvidence` (`src/schema/permissions.rs:15`,
`TaskState.permission: Option<…>`), NOT `TaskToolPolicy.declared_effects: Vec<String>`
(F-125, lossy summary). Six effect dimensions:
`shell · git_write · network · fs_write · external_dir · mcp`
— `requested.*` (bool, the declared capability) + `resolved.*` (`Enforcement` = `hard |
soft | unsupported | not_applicable`, how hard the provider constrains it).

**Reliability (drives the unknown / requires_review rule — NEVER silently downgrade):**
| Input | Reliable? | Rule |
|------|-----------|------|
| `requested.shell / git_write / network` | yes (when permission present) | primary tier signals |
| `requested.fs_write` | aliased from `git_write` today | treat as a write signal |
| `requested.external_dir` / `requested.mcp` | **hardcoded false today** | NEVER read as "confirmed none" — these dims are `unknown` in v1 |
| `resolved.*: Enforcement` | yes for shell/mock/codex/cursor; `unsupported` for any other provider | the hard/soft/advisory annotation |
| `TaskState.permission` itself | **often Absent** (legacy / liveness-reconstructed / pre-executor) | Absent → `unknown` (or `requires_review` w/ observed effects), NEVER `review_only` |
| observed `files_changed` / `pr_url` | reliable (write only) | cross-check, same defn as F-126 (exclude `branch`) |
| observed network / external_dir / mcp | **no signal in v1** | derive from declared+enforcement only; observed-side = unknown |

## 2. The RuntimeProfile model
`RuntimeProfile` — one mutually-exclusive label (serde snake_case):
`review_only · write_local · network_allowlisted · tool_limited · high_risk_vm ·
requires_review · unknown`. Projected as `RuntimeProfileView`:
```
RuntimeProfileView {
  profile: RuntimeProfile,            // the label
  reasons: Vec<String>,              // why this label (human-readable)
  basis: {                           // the inputs it read (audit/transparency)
    provider_id: String,
    effects: [ per-dim { name, requested: bool|unknown, enforcement: Enforcement } ],
  },
  advisory: Vec<String>,             // dims requested but only Soft-enforced (NOT hard-blocked)
}
```
`advisory` is the hard/soft honesty channel: e.g. a `shell`-provider task with
`network` requested resolves to `network` enforcement = `soft` → profile may be
`network_allowlisted` but `advisory: ["network is advisory — provider does not
hard-block egress"]`. **An opaque provider is never labelled as per-tool hard-enforced;
its Soft dims surface in `advisory` + the post-run F-126 gate remains the real check.**

## 3. The deterministic derivation table (the contract centerpiece)
Pure function `derive(permission: Option<&PermissionEvidence>, observed_write: bool) ->
RuntimeProfileView`. **Conservative-up** (assume the most capability the signals permit;
never silent-downgrade) — first match wins:

| # | Condition | → profile | reason |
|---|-----------|-----------|--------|
| 1 | `permission` is `None` (Absent) AND observed write effects exist | `requires_review` | observed effects without permission evidence (audit gap; mirrors F-126 FlagOnly) |
| 2 | `permission` is `None` (Absent), no observed effects | `unknown` | no permission evidence to derive from |
| 3 | `schema_version != "maestro.permission.v1"` | `requires_review` | unrecognized evidence (fail-closed, mirrors F-126 Gate) |
| 4 | provider enforcement all `unsupported` (unknown provider) | `requires_review` | unknown provider — enforcement undeterminable, do NOT assert safe |
| 5 | `requested.external_dir == true` | `high_risk_vm` | host-dir access ⇒ strong-isolation tier *(v1-unreachable: external_dir hardcoded false — future-ready)* |
| 6 | `requested.network == true` | `network_allowlisted` | network egress is the escalating dimension |
| 7 | `requested.git_write \|\| fs_write \|\| shell` | `write_local` | local write capability (shell is treated as write-capable — conservative) |
| 8 | `requested.mcp == true` (and none of 5–7) | `tool_limited` | narrow tools, no general shell/network *(v1-unreachable: mcp hardcoded false — future-ready)* |
| 9 | nothing requested (all false) | `review_only` | genuinely read-only |

**Honest v1 reachability:** with today's signals (`external_dir`/`mcp` hardcoded false),
rows 5 & 8 are **unreachable from real data** — v1 will emit only `review_only /
write_local / network_allowlisted / requires_review / unknown`. The 5 profile kinds stay
in the enum as **future-ready** (the rules are pinned now so later cuts that add the
`external_dir`/`mcp`/untrusted-input signals just light them up — no re-pinning). This is
documented, not a gap to hide.

## 4. Projection landing (no new fact source)
- **TaskDetail** (`src/schema/monitor.rs:679` / `web/src/types.ts:389`): add
  `runtime_profile: RuntimeProfileView` right after `tool_policy` — **non-optional**
  (matches the always-present `tool_policy`; the `unknown`/`requires_review` states live
  inside the label, not in an `Option`). Derived in `task_tool_policy`'s neighbourhood
  from the same `TaskState.permission`.
- **DeliveryView** (`src/schema/delivery.rs:509` / `web/src/types.ts:1039`): add
  `runtime_profile_summary: Option<RuntimeProfile>`. DeliveryView only exposes `run_id`
  (no task list) — so the summary loads the linked run and **aggregates the worst case**
  (any task `requires_review` → summary `requires_review`; else the highest-privilege
  tier across tasks). `run_id == None` (never executed) → `None`/`unknown`. No new fact
  source — it reads the run's existing task states.

## 5. Consistency with the F-126 gate (label, not competing enforcement)
RuntimeProfile is an **input/label**, never an enforcement. It MUST reuse F-126's
(`src/scheduler/policy_gate.rs:54`) definitions: observed write = `files_changed` +
`pr_url` (exclude `branch`); Absent ≠ allow-all (surface as `unknown`/`requires_review`,
never `review_only`); network/external_dir/mcp are unobservable → derive from
declared+enforcement and mark observed-side `unknown`. a1 does not touch the gate.

## 6. Do-not-absorb (a1)
No behavior change of any kind: no adapter/env/network/sandbox change; no settings change
(that's F-136a2); no run-behavior change; no enforcement (the profile gates nothing in
a1); no new permission *source* (derive from existing `PermissionEvidence`); no Linux
runner / cgroup / container work (F-136c+).

## 7. Test matrix
- **Derivation (pure-fn unit tests, the table rows)**: no-caps → review_only;
  shell-only (no write) → write_local; git_write → write_local; network → network_
  allowlisted; Absent (no observed) → unknown; Absent + observed write → requires_review;
  schema mismatch → requires_review; unknown provider (`unsupported`) → requires_review;
  network on a `soft`-network provider → network_allowlisted + `advisory` populated;
  external_dir/mcp synthetic-true → high_risk_vm/tool_limited (lock the future-ready rows
  even though unreachable from live data).
- **Projection**: TaskDetail carries a `runtime_profile` for present & absent permission;
  DeliveryView summary = worst-case across a multi-task run; no-run delivery → None.
- **Rust↔TS**: enum serde snake_case round-trips; `RuntimeProfileView` optionality
  matches the TS type exactly (the F-127b lesson).

## 8. Open decisions for 大力 to pin
1. **`unknown` vs `requires_review` split** — proposed: `unknown` = insufficient data
   (Absent, no run); `requires_review` = data present but not safely classifiable
   (unknown provider, schema mismatch, observed-effects-without-evidence). *(lean: this
   split.)*
2. **`shell` ⇒ `write_local`** (treat a general shell as write-capable — conservative,
   no silent downgrade) vs shell-without-declared-write ⇒ `review_only`. *(lean:
   write_local — shell can write; review_only is reserved for all-false.)*
3. **Keep `high_risk_vm` / `tool_limited` in the enum as future-ready** (rules pinned now,
   unreachable from v1 data, documented) vs omit until `external_dir`/`mcp` signals exist.
   *(lean: keep + document unreachable.)*
4. **One enum incl. `unknown`/`requires_review`** (mutually-exclusive label) vs a separate
   `status` + `Option<profile>`. *(lean: one enum + `reasons` + `advisory`.)*
5. **DeliveryView aggregate = worst-case** across the run's tasks vs representative/first.
   *(lean: worst-case = most-review-needed.)*
6. **`TaskDetail.runtime_profile` non-optional** (match `tool_policy`) vs `Option`.
   *(lean: non-optional, internal unknown state.)*

## 9. Review axes: input vocabulary/reliability → §1; model → §2; **derivation table → §3**;
projection → §4; F-126 consistency → §5; Do-not → §6; tests → §7; recommended + open
points → §8. No code until GO.
