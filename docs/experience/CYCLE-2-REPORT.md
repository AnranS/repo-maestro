# Experience Gauntlet Cycle 2 Report

Date: 2026-05-25

## TL;DR

Cycle 2 ran the Tier 2 flagship cases against `main` after Cycle 1
locked in cold-start UX. We surfaced **5 friction items** (0 P0 / 3 P1
/ 2 P2) across pnpm monorepo, self-dogfood, channel loop, and bench
warm-cache scenarios. **4 fixed, 1 deferred to Tier 2.5**. All 4
product fixes shipped via PR (per the recalibrated process discipline
adopted at end of Cycle 1).

**Severity rubric (unchanged from Cycle 1, dual-axis)**:

- `severity`: `P0` blocks the case · `P1` flagship workflow fails or
  produces misleading/non-actionable output · `P2` rough edge or setup
  gap
- `friction_score`: `5` user likely to quit · `4` needs source
  knowledge · `3` continues after a few minutes · `2` minor wording ·
  `1` polish
- **Fix order**: P0 all → P1 by descending `friction_score` → P2
  backlog

**Roles this cycle**: 大马猴 designed the T2 case-book + drafted 2 design
docs + reviewed all 4 PRs + wrote this report; 大力 dogfooded Stage 1
+ implemented Stage 2 fixes across 4 PRs + ran full local verification
on each; 高鹏 ruled GO Round A + Round B (option 2), deferred T2-F002
to Tier 2.5 planner-depth track.

## Exit Criteria

| criterion | target | actual | met |
|---|---|---|---|
| Every Tier 2 case has a recorded transcript | 4/4 | 4/4 | ✅ |
| All P1 frictions fixed or owned with rationale | 3/3 | 2 fixed + 1 deferred with track owner | ✅ |
| All P0 frictions fixed | 0 outstanding | 0 found this cycle | ✅ |
| Per-PR diff under 400 LoC for product fixes | ≤400 | 113 / 70 / 154 / 351 — biggest still well under | ✅ |
| Stage 2 product fixes ship via PR (cycle 1 carryover) | 4 PRs | 4/4 PRs (#38, #39, #40, #41) | ✅ |
| New T2 metrics (`output_actionability`, `realism_score`) recorded | all cases | recorded in `T2-20260524T163843Z/metrics.md` | ✅ |
| Deferred friction has explicit follow-up track | 1 item with track | T2-F002 → Tier 2.5 planner-depth | ✅ |

## Scope

Cases:

| id | scenario | status after fixes |
|---|---|---|
| T2-C1 | `work --root examples/monorepo-pnpm` dry-run | dry-plan no longer leaks stale `codex_lab_*` projects outside the root |
| T2-C2 | Self-dogfood: audit error handling consistency | analyzer no longer leaks scratchpad projects; planner depth deferred |
| T2-C3 | Channel loop with mock transport | now runnable end-to-end via `channels.yaml` `transport: mock` |
| T2-C4 | `bench all --json --offline` warm-cache UX | cache miss now distinct from real failure; `bench hydrate` warms cache |

Evidence:

- Stage 1 dogfood transcripts: `docs/cases/T2-runs/T2-20260524T163843Z/`
- Per-run metrics (incl. new `output_actionability` + `realism_score`):
  `docs/cases/T2-runs/T2-20260524T163843Z/metrics.md`
- Friction log: `docs/cases/T2-friction-log.md`
- Stage 1 handoff summary: `docs/experience/CYCLE-2-STAGE-1-SUMMARY.md`

## Changes Shipped

| commit | PR | area | summary |
|---|---|---|---|
| `ca64cd6` | direct | docs | Adds Tier 2 flagship case-book with 4 cases + 2 new metrics. |
| `6075dbb` | direct | docs | Stage 1 dogfood transcripts + friction log (5 confirmed). |
| `09c48b9` | direct | docs | Stage 1 handoff summary with Round A/B scope + GO/NO-GO matrix. |
| `2d9774a` | #38 | UX fix | `work --root` scopes plan synthesis to requested root; registry unmodified. |
| `30131fc` | #39 | UX fix | Discovery walker honors `.gitignore` / `.ignore` via the `ignore` crate. |
| `7994d62` | direct | docs | Marks Round A frictions fixed in friction log. |
| `76fcc38` | #40 | UX fix | First-class `FileMockTransport` selected via `channels.yaml` `transport: mock`; T2-C3 case-book updated in same PR. |
| `eb1524d` | #41 | UX fix | `bench hydrate` subcommand + `cache_miss` summary/reporting + exit-code policy. |
| this commit | direct | docs | Cycle 2 report + friction-log Round B status. |

The 4 product-fix PRs touch only:

- PR-A1 (#38): `src/cli/commands/plan.rs`, `src/cli/commands/work.rs`
- PR-A2 (#39): `src/config/discovery.rs`, `Cargo.toml`, `Cargo.lock`
- PR-B1 (#40): `src/channel/transport.rs`, `src/channel/{mod,drain,poll}.rs`,
  `src/cli/commands/channels.rs`, `src/config/channels.rs`,
  `docs/cases/T2-flagship.md`
- PR-B2 (#41): `src/cli/commands/bench.rs`, `src/cli/mod.rs`,
  `src/bench/{score,runner,report}.rs`

## Fixed Findings

### T2-F001: `--root` scope leak (PR-A1 / #38)

Before: `maestro work --root examples/monorepo-pnpm --agent mock --dry`
included tasks for stale `codex_lab_*` projects that had been
registered by earlier discovery runs against other roots.

After: `synthesize_file` accepts an optional `root_filter: Option<&Path>`
and filters registered projects to those whose path lives under the
requested root at plan synthesis time. Registry is **not** mutated, so
multi-root users keep their full project set across runs. If the filter
produces an empty set, the tool bails with
`"no registered projects under --root <path>"`.

Why filter at synthesis rather than prune the registry: `init --analyze`
should not delete user projects from other roots; `--root` is a per-call
scope, not a destructive reset.

### T2-F003: analyzer respects ignore files (PR-A2 / #39)

Before: discovery walker only skipped a hardcoded list
(`node_modules`, `target`, `dist`, `build`, `venv`, `__pycache__`) and
dotfiles. Local scratchpad dirs like `maestro-demo-*` were treated as
real projects.

After: discovery loads `ignore::WalkBuilder` (the ripgrep crate) over
the root with `require_git(false)` so that root-level `.gitignore` and
`.ignore` files take effect even outside a git working tree. The
existing hardcoded skip list is preserved as a defense-in-depth fallback.
Two-walk overhead is acceptable at `max_depth: 3` defaults.

### T2-F004: first-class mock channel transport (PR-B1 / #40)

Before: T2-C3 channel loop case was unrunnable. `maestro work --channel mock`
was an unknown argument, and `StubTransport` lived behind `#[cfg(test)]`.
A new user could not exercise the channel loop without real Feishu
credentials.

After: `FileMockTransport` is a production-grade `Transport` impl that
persists outbound replies to `.maestro/channels/<channel-name>/outbox.jsonl`
and reads inbound messages from the matching `inbox.jsonl`. Transport
selection runs through a new `select_transport(channel, entry, workspace_root)`
dispatcher keyed on `entry.transport`. `channels.yaml` remains the single
source of truth for transport configuration. Mock channels do not require
a `botmux_session_id` — `ChannelEntry::transport_session_id(channel)`
returns the channel name as session id for mocks. T2-C3 case-book updated
in the same PR to use the new configuration shape.

### T2-F005: bench hydrate + cache-miss reporting (PR-B2 / #41)

Before: `maestro bench all --json --offline` exited 1 with a misleading
`10/15 passed` summary on a fresh checkout, because 5 OSS fixtures had
not been cloned. The error message instructed users to `git clone`
each upstream manually; no discoverable command warmed the cache.

After:

- New `maestro bench hydrate [--fixture <id>] [--kind oss-replay]`
  subcommand explicitly warms OSS-replay fixtures, with per-fixture
  status (`HYDRATED` / `ALREADY-CACHED` / `SKIPPED` / `FAILED`) plus a
  summary line. Partial failure exits non-zero so CI catches it.
- `BenchResult` gains a `cache_miss: bool` field, populated by a
  classifier that recognizes the runner's offline-cache-miss bail
  message. `BenchSummary` separates `cache_miss` from `failed`;
  `pass_rate` excludes cache misses from both numerator and denominator.
- `bench all` exit code is now 0 when cache misses are the only
  problem, 1 only when real failures are present. Summary line embeds
  `"run \`maestro bench hydrate\` first"` when cache misses are
  detected.
- Runner's offline bail message updated to include the exact remediation
  command, so the raw error stream also points users at the fix.

## Deferred Findings

### T2-F002: planner depth (deferred to Tier 2.5)

Self-dogfood plan for `"audit error handling consistency across cli
commands"` stays at project level (`T_change_maestro`) instead of
naming concrete `src/cli/...` modules. This is a planner capability
gap, not a UX friction in the case-book sense.

**Owner**: deferred to a separate Tier 2.5 planner-depth track. The
case-book line item remains in `T2-friction-log.md` with
`status: deferred` to track resurfacing in the next planner cycle.

## Post-Fix Results

| case | post-fix expectation |
|---|---|
| T2-C1 | dry-plan now contains only projects under `examples/monorepo-pnpm`; `T_change_codex_lab_*` no longer present |
| T2-C2 | analyzer no longer plans for `maestro-demo-*` scratchpads; planner depth gap tracked separately |
| T2-C3 | configured `channels.yaml` + seeded `inbox.jsonl` → `channels drain --once` + `channels listen --poll --channel demo` round-trips through `outbox.jsonl` |
| T2-C4 | cold-cache `bench all --offline` reports `cache_miss=5`, exit 0, with hint pointing at `bench hydrate`; after `bench hydrate`, full suite passes |

A Stage 3 re-run on `main` post-`eb1524d` is the natural validation
step for the next cycle. It will also exercise the T2-C4 case-book
expected-output update flagged as PR-41 follow-up O2.

## Verification

Targeted regression tests added across the 4 PRs:

- `synthesize_file_root_filter_excludes_projects_outside_root` (PR-A1)
- `discover_respects_root_gitignore_for_project_dirs` (PR-A2)
- `file_mock_transport_persists_outbound_and_polls_inbound` (PR-B1)
- `select_channel_accepts_enabled_mock_without_botmux_session` (PR-B1)
- `bench_summary_separates_cache_miss_from_real_failure` (PR-B2)
- `classify_result_marks_offline_cache_miss_errors` (PR-B2)
- `bench_all_exit_code_allows_cache_miss_only_reports` (PR-B2)
- `hydrate_fixtures_reports_cached_and_skipped_fixtures` (PR-B2)
- `bench_hydrate_args_parse_fixture_and_kind_filters` (PR-B2)

Each PR ran the full local matrix before opening:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- `git diff --check origin/main..HEAD`

GitHub CI (rust stable + web) shipped green on every PR before merge.
Local test count reached ~300 lib tests + integrations at cycle end.

## Methodology Validation

The Experience Gauntlet framework continued to hold at Tier 2:

- **5 frictions surfaced in ~30 min of dogfooding** across 4 flagship
  scenarios — efficient signal density.
- The dual-axis P×F ranking made the Round A vs Round B split natural:
  Round A grouped the two frictions with clear root causes and small
  fix surface (both <120 LoC); Round B grouped the two requiring design
  decisions ahead of implementation.
- The new T2 metrics `output_actionability` and `realism_score`
  successfully discriminated cases: T2-C2 had high realism but low
  actionability (planner depth gap), while T2-C4 had low actionability
  pre-fix and high after.
- **Process discipline restored**: all 4 product-fix commits this cycle
  shipped via PR (vs the 2 direct-to-main cycle-1 commits flagged in
  `CYCLE-1-REPORT.md` line 222-225). Recalibrated LoC threshold (≤5
  direct / 6-30 PR / >30 PR+dispatch) held — every PR had a dispatch
  doc or design doc upstream.
- **Design-first split for Round B worked**: both PR-B1 and PR-B2
  matched the design docs in every dimension, no rework needed in
  review. Trade-off menu (3-4 options per friction) made the picks
  explicit and reviewable.
- One soft observation for next cycle: message-bus lag between agents
  caused several "校准" exchanges (acknowledging each other's stale
  state). Worth thinking about whether the runner script should print
  a current-`main` HEAD line so cross-fire is auto-resolvable.

## Open Follow-Ups (for Cycle 3)

Non-blocking observations recorded during review (do not gate Cycle 2
closure):

- PR-40 O1: `FileMockTransport::send` is two write syscalls; serialize
  to `String` first for safe concurrent send.
- PR-40 O2: document the channel-selection precedence when both real
  and mock channels are enabled.
- PR-40 O3: add a focused unit test for `select_transport` dispatch.
- PR-40 O4: case-book runner ergonomics for `<latest-run-dir>` substitution.
- PR-41 O1: add a doc-comment in `runner.rs` pinning the offline-cache-miss
  bail phrase to the classifier in `bench::classify_result`.
- PR-41 O2: T2-C4 case-book pass-signal language still describes the
  pre-fix expected output; refresh in a tiny docs commit.
- PR-41 O3: align-pad `bench hydrate` per-fixture status output for
  human readability.

## Next Round

Direction options for 高鹏:

1. **Tier 2 Stage 3 re-run**: cycle the 4 cases again against current
   `main` (post-eb1524d) to confirm fixes hold and to catch any
   regressions from the PR-41 cache-miss reclassification.
2. **Tier 2.5 cross-cut**: upgrade/migration · security/abuse ·
   performance/load — the deferred categories from Cycle 1
   planning. Includes the deferred T2-F002 planner-depth track.
3. **Tier 3 edge cases**: corrupted state, partial network, concurrent
   agent invocations.
4. **Cycle 2 follow-up polish**: knock down the 7 non-blocking
   observations listed above as a small batch PR.

---

Signed off: 大马猴 (design/review) + 大力 (implementation/runs).
Awaiting 高鹏: pick direction for Cycle 3.

---

## Cycle 2 → 3 transition follow-ups (deferred, recorded 2026-05-28)

dali surfaced two real architecture risks in the post-`d3c5ea6` review
that were **deliberately deferred** out of the small fmt-gate-merge
batch (commits `cbccaa1` / `a37b8f5` / `fda0fae`). Recording them here
so they're picked up next cycle rather than forgotten:

### F-001 — Abandoned-run liveness uses PID alone

`src/scheduler/liveness.rs` decides Live vs Abandoned via `kill(pid, 0)`
on `RunState.pid`. Sufficient for the current single-host local-CLI
use case; on a long-running machine or under heavy churn, PID reuse
could classify a recycled PID as a live owner and block a rerun of the
same goal.

When to fix: alongside the daemon / channel lifecycle work — that round
will already need a richer "owner identity" beyond bare PID. Drop in
a start-time fingerprint (procfs `/proc/<pid>/stat` field 22 on Linux,
`proc_pidinfo` on macOS) at that point.

### F-002 — `ProjectsConfig::load` returns empty cfg on tmp-stale

`src/config/projects.rs:235-244` retries twice when the canonical
projects.yaml is empty but a sibling `.tmp` exists; if both retries
fail the function returns `ProjectsConfig::default()`. The current
test (`src/config/projects.rs:708-720`) pins that behavior, and most
callers surface a clear "no projects + tmp exists" hint, but
architecturally the function is **conflating "concurrent write in
flight" with "no projects registered yet"**, which leaks misleading UX
into downstream commands.

When to fix: as its own focused refactor — introduce
`enum LoadOutcome { Loaded(cfg), ConcurrentWrite, StaleTmp }` and
route callers through it. Bundling it with anything else risks
spreading the API change too wide.
