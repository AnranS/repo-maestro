# F-126 — node-level tool-policy ENFORCEMENT (B4, integration-gate v1)

Status: **v1 IMPLEMENTED** — `defaults.gate_on_policy_violation` (default OFF),
pure evaluator `scheduler::policy_gate::evaluate`, gated in
`executor::handle_post_task_approval` before integration. Follows the F-122→F-125
read-only absorption arc (closed in `8ca7a37`); F-125 shipped
`TaskDetail.tool_policy` as projection/audit only, and this is the deferred
enforcement step (B4 below — the boundary that works for opaque providers).

> The design analysis below (boundary map, alternatives, risk table) is retained
> as the rationale for choosing B4. Implementation = §4 + §8 + §9.

## Closeout (v1 landed, 2026-06-08)

| what | value |
|---|---|
| commit | `c38f9e1 feat(F-126): node tool-policy violation gate (B4, opt-in, default off)` |
| range | `e7b2eff..c38f9e1` |
| CI | run 27107637362 success (web / features / rust stable — fmt, clippy, test) |
| review | 大力 PASS, no blocker (config / pure evaluator / executor gate wiring / tests / doc; adapter, web, server API untouched) |
| tests | 7 evaluator unit (full decision matrix) + 3 gate-flow integration (flag-off ignores violation; flag-on gates+approves; flag-on compliant doesn't gate) |

**F-126-fu calibration — DONE (branch downgraded).** The initial v1 treated a
task's `branch` artifact as a `git_write` signal, but a contract scan confirmed
`artifacts.branch` is the executor's per-task **worktree isolation branch**
(auto-filled at `executor.rs:2763,2898` from `worktree_guard.branch()` for every
git-backed task), never an adapter-produced push — and the schema can't tag its
source. So `branch` is **no longer a gate signal**; only `files_changed` (net
diff) and `pr_url` (adapter-produced, never auto-filled) gate. Removes the false
positive where a no-write role with an empty diff was gated on its worktree
branch.

The branch noise is cleaned on **both** sides — the gate AND the read-only
projection — for one consistent rule (`branch` ≠ write signal):

| follow-up | what | commit(s) | CI | review |
|---|---|---|---|---|
| F-126-fu | drop `branch` from the policy GATE (`scheduler::policy_gate`) | `1adace4` | 27108010252 ✓ | 大力 PASS |
| F-126-fu-2 | drop `"branch written"` from the F-125 UI projection (`schema::monitor::task_tool_policy.observed_effects`) + field doc | `f5fb338` + `1866f43` | 27108292791 ✓ | 大力 PASS |

Still deferred (their own slice): hard per-tool interception of opaque providers,
node-boundary deny.

## 1. Existing enforcement boundary map

### Already HARD (enforced today)

| boundary | mechanism | scope | when |
|---|---|---|---|
| shell command allowlist | `adapter/shell.rs::check_command` — bails on disabled shell, command not in `allowed_commands`, control operators, command substitution | **shell/verify tasks only** (not agent providers) | pre-exec |
| opaque-provider coarse sandbox | cursor `--sandbox` (`adapter/cursor.rs:184`); codex `--sandbox` + `sandbox_workspace_write.network_access=false` (`adapter/codex.rs:219,227`) | cursor/codex agent tasks | dispatch |
| worktree isolation + copy-file deny | `scheduler/worktree_policy.rs::WorktreePolicy.deny_set` (which files enter a worktree) + one git worktree per task | all git-backed tasks | dispatch/integrate |
| approval / risk gates | `plan_gate` / `outcome_gate` / `requires_approval_after` / `defaults.gate_on_high_risk` → pause via approval markers in `executor.rs::handle_post_task_approval` | run / task | **pre-integration** |

### Only SOFT / advisory

- Fine-grained agent tool limits (shell/git/network **beyond** the coarse
  sandbox) for cursor/codex — injected as **prompt policy**
  (`cursor_policy_note` / `codex_policy_note`). The agent self-polices.
- `AllowedTools` text in the agent prompt.

### Evidence-only (record, never gate)

- `PermissionEvidence` (`requested → resolved: Enforcement`), per task.
- `TaskDetail.tool_policy` (F-125 read-only projection + `audit_gaps`).

## 2. Opaque-provider (cursor/codex) hard limit

**CAN hard-control:** coarse provider sandbox (`--sandbox`), network off (codex),
workspace-write scope, worktree cwd/isolation, env, the shell adapter (verify
tasks), and **whether the change integrates** (the merge boundary).

**CANNOT hard-control:** which individual tools the agent invokes inside its
sandbox. maestro only *observes* `tool_use` events from the provider stream
post-hoc (`cursor.rs:281`, `codex.rs`) — there is **no interception hook**.

→ **Do not label prompt-advisory fine-grained limits as "enforcement".**
Per-tool deny/intercept on an opaque provider is not achievable; only the coarse
sandbox + the post-hoc integration gate are real.

## 3. Candidate enforcement boundaries + runtime-behavior change

| # | boundary | what it would enforce | runtime behavior change | feasible for opaque provider? | risk |
|---|---|---|---|---|---|
| B1 | pre-run plan/task validation (`config::analyze`) | reject a plan whose nodes declare disallowed/absent policy | a plan that would have run **fails fast** | n/a (static) | low, but needs an **authored** node-policy surface that does not exist yet |
| B2 | pre-dispatch node gate (`executor` dispatch) | block dispatch if resolved permission denies a needed capability | a task that would have dispatched is **blocked** | **no** — the needed tools aren't known before the agent runs | medium |
| B3 | adapter-level command/tool guard | hard-check capabilities at the adapter | shell: extend `check_command` to git/network; agent: only tighten the **coarse** sandbox flag | shell **yes**; agent **no** (no per-tool hook) | shell low / agent impossible |
| B4 | **post-run audit escalation = pre-integration gate** (`handle_post_task_approval`) | gate integration when **observed** effects / F-125 `audit_gaps` violate the node policy | a violating node **pauses for approval at integration** instead of auto-merging | **yes** — post-hoc on observed effects, the one place maestro controls | **low** — effect is isolated in the worktree; gating blocks it landing |

## 4. Recommended v1 — enforce at the integration boundary (B4)

Extend the existing risk gate (`handle_post_task_approval`) with a
**policy-violation gate**, behind a new opt-in flag (mirroring
`defaults.gate_on_high_risk`), **default OFF**:

> When a finished node's OBSERVED effects or F-125 `audit_gaps` violate its
> resolved policy (e.g. declared no `git_write` but changed contract files; or an
> `audit_gap`), **pause for approval before integration** instead of auto-merging.
> Corrupt policy → **fail-closed** (gate, never auto-allow). Absent policy on a
> legacy run → flag only (do NOT hard-block — see test matrix).

Why this first:
- **Lowest risk** — the change is already isolated in the per-task worktree;
  gating integration prevents it landing, with no mid-flight kill and no partial
  state.
- **Works for opaque providers** — post-hoc on observed effects, the only place
  maestro has real control.
- **Reuses existing machinery** — `handle_post_task_approval` + approval markers +
  the `gate_on_high_risk` pattern.
- **Closes the F-122→F-125 loop** — F-125 `audit_gaps` become the enforcement
  trigger; the pinned plan (F-122) + gate projection (F-123) + evidence (F-124)
  are the audit chain it gates on.
- **Reversible** — opt-in flag default off; gated runs are recoverable (approve to
  proceed). No rollback-breaking migration.

## 5. Explicit NON-goals (do NOT do in v1)

- Do **not** intercept opaque-provider individual tool calls — impossible.
- Do **not** hard-fail mid-execution inside an agent.
- Do **not** add a new sandbox/container runtime.
- Do **not** add pre-run authored node-policy validation (B1) yet — nodes carry no
  authored policy; that needs a new authoring surface = its own slice.
- Do **not** relabel the prompt-advisory fine-grained limits as "enforcement".

## 6. Alternative order (if B4 is not chosen first)

1. **B3-shell**: extend `check_command` to hard-check git_write/network for
   shell/verify tasks. Feasible + hard, but narrow (shell adapter only).
2. **B3-sandbox**: tighten the coarse provider sandbox per node policy (e.g. force
   network off for nodes that don't declare network). Provider-specific, coarse.
3. B4 last would waste the audit chain F-125 already built; hence B4 first is the
   recommendation.

## 7. Risk table

| risk | cause | mitigation |
|---|---|---|
| false-positive gate blocks a legit run | observed-effect heuristic too broad | opt-in flag default OFF; reuse the proven `files_changed`/contract-path risk classifier; approve path always available |
| legacy runs break | absent policy treated as deny | absent = flag only, never hard-block (test-locked) |
| corrupt policy silently allows | fail-open | fail-closed: corrupt → gate, never auto-allow (test-locked) |
| illusion of control on opaque providers | per-tool "enforcement" that's actually advisory | documented non-goal; only coarse sandbox + integration gate are labeled enforcement |
| dry-run side effects | enforcing on a no-effect run | enforcement is a no-op when `dry_run` |

## 8. Test matrix

| policy | effects | mode | expected |
|---|---|---|---|
| present + compliant | within policy | real | integrate normally (no behavior change) |
| present + violation | out of policy | real | **gate** at integration; approve→integrate, reject→discard |
| absent (legacy RUN_STATE) | any | real | flag only — **never hard-block** |
| corrupt | any | real | **fail-closed**: gate, never auto-allow |
| present + violation | any | **dry-run** | no-op (no real effects to gate) |
| allow / deny / intercept | — | — | allow=integrate; deny=gate; **intercept=N/A for opaque (documented, not implemented)** |

Rollback: flip the opt-in flag OFF → instant disable; no schema change that blocks
downgrade; gated runs remain approvable.

## 9. File anchors

- `src/scheduler/executor.rs::handle_post_task_approval` (gate home), `::integrate_task`
- `src/schema/monitor.rs::task_tool_policy` (F-125 `audit_gaps` → triggers)
- `src/adapter/shell.rs::check_command` (existing shell hard-guard)
- `src/adapter/{cursor,codex}.rs` (coarse sandbox flags)
- `src/config/...::defaults.gate_on_high_risk` (opt-in flag pattern to mirror)
- `src/scheduler/worktree_policy.rs` (existing deny machinery)

## 10. Decision asked of 大力

1. Confirm **B4 (integration-gate, opt-in, default OFF)** as v1, or pick an
   alternative order (§6).
2. Confirm the absent/corrupt semantics (absent=flag-only, corrupt=fail-closed).
3. Confirm the non-goals (§5), especially no opaque-provider tool interception.

No implementation until GO.
