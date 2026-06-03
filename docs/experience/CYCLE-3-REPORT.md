# Experience Gauntlet Cycle 3 Report

Date: 2026-05-25

## TL;DR

Cycle 3 ran the **Tier 2.5 cross-cut** axis (production wear) against
`main` after Cycle 2 closed flagship UX. We surfaced **3 friction items**
(0 P0 / 2 P1 / 1 P2) across `copy_files` security boundary, channel
load resilience, and planner-depth+goal-relevance scenarios. **All 3
fixed**, plus one Stage-3-found design regression patched in the same
cycle (`7cd7665`, F003 follow-up). Stage 3 metric deltas confirmed
each fix landed where intended — `failure_clarity` improved 1→5 on
PD1 and 3→5 on S1. One F002 audit-log limitation kept as a known
follow-up. All product fixes shipped via direct push to `main` per
高鹏's Cycle 3 process directive — no GitHub PRs this cycle.

**Severity rubric (unchanged from Cycle 1/2, dual-axis)**:

- `severity`: `P0` blocks the case or is exploitable · `P1` cross-cut
  workflow fails or produces silent failure on stress input · `P2`
  rough edge or setup gap
- `friction_score`: `5` quit/data-loss/security · `4` needs source
  knowledge · `3` continues after a few minutes · `2` minor · `1`
  polish
- **Fix order**: P0 → P1 by descending `friction_score` → P2 backlog

**Roles this cycle**: 大马猴 designed the T2.5 case-book + amended for
高鹏's "no back-compat" + 大力's case-picks + drafted F003 design doc +
reviewed all 3 fixes + wrote this report; 大力 dogfooded Stage 1 + Stage
3 + implemented Round A (F001, F002) + Round B (F003) including B1
visibility fix-up; 高鹏 redirected scope twice (out: back-compat axis;
in: direct-push process), reviewed final state.

## Exit Criteria

| criterion | target | actual | met |
|---|---|---|---|
| Every Tier 2.5 case has a recorded Stage 1 transcript | 3/3 | 3/3 | ✅ |
| All P1 frictions fixed | 2/2 | 2/2 (F002, F003) | ✅ |
| All P0 frictions fixed | 0 outstanding | 0 found | ✅ |
| All P2 frictions fixed or backlogged with rationale | 1/1 | F001 fixed | ✅ |
| Stage 3 re-run on post-fix `main` | 3/3 cases | `T2_5-stage3-20260525T015355Z` | ✅ |
| Each fix has targeted regression test | every fix | 10 tests added | ✅ |
| New T2.5 metrics (`failure_clarity`, `state_integrity`) recorded | all cases | recorded both stages | ✅ |
| Process discipline matches 高鹏 Cycle 3 directive (direct push, no PRs) | 5 product commits | 5/5 direct (`3e2767d`, `75b3a8a`, `db218ac`, `e3e3140`, `7cd7665`) | ✅ |

## Scope

Cases:

| id | scenario | status after fixes |
|---|---|---|
| T2_5-S1 | hostile `copy_files` (path traversal / absolute / `.env` / external secret) | guard now aggregates and reports all denied entries in one error, with rule per entry |
| T2_5-P1 | 20-task DAG synthesis + 100-msg channel poll smoke | malformed inbox line no longer aborts batch; mock + botmux history parsers tolerate per-message failures |
| T2_5-PD1 | `maestro work` plan targeting a module path | synthesis filters projects by goal-text relevance; `→ notice:` printed to stdout naming the project-level planner limitation; no more demo-project pollution |

Cases explicitly **out of scope** (and why):

- T2_5-U1 (upgrade across schema version): dropped at Stage 0 amendment
  per 高鹏 "no back-compat burden, never released externally". The
  whole upgrade / migration axis is parked until external users exist.

Evidence:

- Stage 1 dogfood transcripts: `docs/cases/T2_5-runs/T2_5-20260525T005426Z/`
- Stage 1 metrics: `docs/cases/T2_5-runs/T2_5-20260525T005426Z/metrics.md`
- Stage 3 re-run transcripts: `docs/cases/T2_5-runs/T2_5-stage3-20260525T015355Z/`
- Stage 3 metrics: `docs/cases/T2_5-runs/T2_5-stage3-20260525T015355Z/metrics.md`
- Friction log (with Stage 3 verification): `docs/cases/T2_5-friction-log.md`
- F003 design note: `docs/cases/T2_5-F003-design.md`

## Changes Shipped

| commit | landing | area | summary |
|---|---|---|---|
| `e3fe06c` | direct | docs | Open Cycle 3 with Tier 2.5 cross-cut case-book (4 cases initially). |
| `f5d81df` | direct | docs | Swap S1 + P1 to higher-signal picks per 大力 input. |
| `e27d91b` | direct | docs | Drop T2_5-U1 + back-compat axis per 高鹏. |
| `c380b6f` | direct | docs | Stage 1 dogfood transcripts + 3 confirmed frictions. |
| `406a554` | direct | docs | F003 planner-depth design note (3 options, recommend B-ii). |
| `3e2767d` | direct | UX fix | `WorktreePolicy::validate_copy_files` aggregates all denials in one error (T2_5-F001). |
| `75b3a8a` | direct | UX fix | `FileMockTransport::poll` + `BotmuxTransport::poll` history parser tolerate per-message parse failures (T2_5-F002). |
| `eaa2201` | direct | docs | Mark Round A frictions fixed. |
| `947ad26` | direct | docs | Correct Round A SHAs in friction log + log F002 N1 follow-up. |
| `db218ac` | direct | UX fix | `synthesize_file_with_intent` filters projects by goal-text relevance + writes `notice:` (T2_5-F003). |
| `a951011` | direct | docs | Mark T2_5-F003 fixed. |
| `e3e3140` | direct | UX fix | Surface `notice:` in `maestro work --dry` stdout via `Plan.notice` field + `print_plan_notice` (T2_5-F003 B1). |
| `7cd7665` | direct | UX fix | `project_contains_relative_path_case_insensitive` walks project root for case-insensitive component match; fixes Stage 3 regression where `.maestro` workspace ≠ project root broke goal-token resolution (T2_5-F003 follow-up). |
| `3fbaa3e` | direct | docs | Stage 3 verification artifacts: transcripts for all 3 cases, metrics row, friction-log fix-SHAs + verified transcript paths. |
| this commit | direct | docs | Cycle 3 report. |

Product fixes touch only:

- `3e2767d` (F001): `src/scheduler/worktree_policy.rs`
- `75b3a8a` (F002): `src/channel/transport.rs`, `src/channel/poll.rs`
- `db218ac` (F003): `src/cli/commands/plan.rs`
- `e3e3140` (F003 B1): `src/cli/commands/work.rs`, `src/config/plan.rs`,
  `tests/work_notice.rs`, plus 5 plumbing sites
  (`src/bench/{runner,score}.rs`, `src/scheduler/{evidence,replan,verify}.rs`)
- `7cd7665` (F003 Stage 3 fix): `src/cli/commands/plan.rs`

## Fixed Findings

### T2_5-F001: aggregate `copy_files` denials (`3e2767d`)

Before: hostile `projects.yaml` like
```yaml
copy_files:
  - "../../../../../etc/passwd"
  - "/etc/hosts"
  - ".env"
  - "../outside-secret.txt"
```
returned a single-entry error mentioning only `.env`. User had to fix
that line, rerun, see the next error, fix, rerun — three rounds to
discover all 4 hostile entries. The security guard caught everything
(zero leakage in any case), so this was UX cost, not a security bug.

After:

- `WorktreePolicy::validate_copy_files` collects **all** denials before
  returning. Single-entry path preserves the existing
  `DeniedPath` / `NpmrcAuthFound` error variants for back-compat with
  prior tests; multi-entry returns a new `CopyFilesDenied { message,
  denials }` variant whose `message` is one line per offending entry
  in the form `copy_files entry \`PATH\` denied: RULE`.
- Rule names exposed: `path traversal`, `absolute path`, `built-in
  pattern \`<glob>\``, `contains npm auth`.
- New test `validate_reports_all_denied_copy_files_in_one_error`
  asserts the 4-entry hostile config produces exactly 4 enumerated
  rejection lines naming every path + rule.

### T2_5-F002: tolerate malformed inbound messages (`75b3a8a`)

Before: `FileMockTransport::poll` and `BotmuxTransport::poll`'s
`parse_history` both used `?` on per-message parse errors. One bad
line at row 51 of a 100-message inbox aborted the whole batch — 100
inputs produced **0** decision rows. The same fail-fast was in the
production botmux parser path (not just mock), so a single
malformed upstream message could blackhole all subsequent inbound
traffic.

After:

- Both transports use `match` + `tracing::warn!` + `continue` on parse
  failures. Cursor advances based on the last successfully-parsed
  message (`om_100` in the 100-line regression).
- `message_is_at_or_before_cursor` extracted as a shared helper.
- New tests:
  - `file_mock_transport_skips_malformed_inbox_lines_and_continues`
  - `botmux_history_parser_skips_malformed_user_messages_and_continues`
  - `poll_inbound_continues_after_malformed_mock_inbox_line` — full
    `poll_inbound` integration: 100 lines → 99 decisions, cursor at
    `om_100`.

**Known follow-up (logged in `947ad26`)**: malformed lines emit only
`tracing::warn!` to stderr; they do **not** produce a row in
`channel_inbound_decisions.ndjson`. Operators must correlate stderr
with the decisions log to find which line failed. Defensible design
choice: forcing the transport layer to write decisions would couple
two clean layers. Recorded as N1 follow-up in `T2_5-friction-log.md`
notes column; reopen only if Stage 3 dogfood shows the audit gap
matters in practice.

### T2_5-F003: filter plan projects by goal-text relevance (`db218ac` + `e3e3140`)

Before: `maestro work "audit error handling in src/cli/commands" --dry`
emitted a 10-task plan covering 5 unrelated projects
(`T_change_demo_core`, `T_change_demo_cli`, `T_change_demo_mobile`,
`T_change_maestro`, `T_change_maestro_web`). No task targeted any
file or module under `src/cli/commands/`, and no message warned that
planning is project-level. The dogfood signal scored
`failure_clarity=1`, `output_actionability=2`.

The friction had two symptoms with one root cause: synthesizer emits
one `T_change_<project>` + one `T_verify_<project>` for every project
that passes the `--root` filter, ignoring how the goal text relates to
each candidate.

Design enumerated 3 options (`docs/cases/T2_5-F003-design.md`):

- B-i: limitation notice only (~15 LoC, doesn't fix pollution).
- B-ii: goal-text → project relevance filter + notice (~80 LoC,
  recommended).
- B-iii: real module-level task generation (≥300 LoC, feature work,
  parked).

Implementation (B-ii, `db218ac`, 263 LoC in `plan.rs`):

- `goal_path_tokens(spec)` extracts path-like tokens (containing `/`
  or a `.<ext>` shape) from the goal, lowercased, deduplicated.
- `projects_relevant_to_goal` filters the project set:
  - hit if `spec.to_lowercase().contains(&project_name.to_lowercase())`
  - hit if any path token's absolute path is a case-insensitive
    component prefix of the project's canonicalized root
  - no-match → fallback to full set (preserves generic goals like
    "clean up tests")
- Explicit `--project <name>` selection bypasses the filter.
- Notice is emitted into PLAN.yaml top-level when path tokens were
  detected, naming the detected tokens and stating planner is
  project-level.

During TDD 大力 caught and removed a basename heuristic that
false-positive-matched `demo-cli` against token `src/cli/commands`.
Replaced with real path-prefix matching.

B1 fix-up (`e3e3140`, 61 LoC across 8 files):

- `Plan.notice: Option<String>` added with `#[serde(default,
  skip_serializing_if = "Option::is_none")]` — old PLAN.yaml files
  unaffected.
- `validate_generated_plan` prints `→ notice: ...` after the validate
  line so users see the limitation in `--dry` stdout without cat-ing
  PLAN.yaml.
- New integration test `tests/work_notice.rs` runs the actual `maestro`
  binary and asserts stdout contains `→ notice:` and `planner is
  project-level`. Other 5 file touches are `notice: None` struct-init
  plumbing.

Stage 3 regression catch (`7cd7665`, 84 LoC, 1 file):

The first Stage 3 re-run of PD1 reproduced the original symptom under
a slightly different setup: when `.maestro` lives in a tmpdir
workspace and the project root is outside the workspace (e.g.
`/tmp/wk/.maestro` + project root `/tmp/repo`), the relative goal
token `src/cli/commands` was resolved against the workspace root,
yielded no match for any project, and the safe fallback re-included
the full project set — re-introducing the demo-project pollution
F003 was meant to remove. The `db218ac` implementation had silently
assumed `workspace root == project root`.

Fix:

- `project_contains_relative_path_case_insensitive` walks the
  project's own root component-by-component using `read_dir` with
  case-insensitive matching, instead of joining tokens onto the
  workspace root.
- Called from `project_matches_goal` only for relative tokens; absolute
  tokens still go through the existing path-prefix check.
- TDD test `synthesize_file_matches_relative_goal_path_under_project_root`
  builds the exact workspace/repo split that broke Stage 3 (3
  projects, only `maestro` should match `src/cli/commands`); fails
  pre-fix and passes after.

After `7cd7665`, the Stage 3 re-run confirms PD1 stdout includes the
notice and the plan contains only `T_change_maestro` /
`T_verify_maestro` with no demo or web targets.

## Post-Fix Results

| case | post-fix expectation | Stage 3 outcome |
|---|---|---|
| T2_5-S1 | one error lists all 4 hostile entries with rule names; no copy of any secret/traversal target | ✅ all 4 entries enumerated in one error; `leak-grep.txt` + `suspect-copies.txt` both empty; `failure_clarity` 3 → 5, `state_integrity` 5 → 5 |
| T2_5-P1 | 100 inbox lines → 99 decisions written; cursor advances past bad line; same in botmux history path | ✅ 100 inbox → 99 decisions; cursor at `om_load_100`; `failure_clarity` 3 → 3 (audit gap N1 by design); `state_integrity` 4 → 4 |
| T2_5-PD1 | `maestro work "..." --dry` plan contains only relevant projects; stdout shows `→ notice:` line | ✅ after `7cd7665` regression fix: stdout includes `→ notice:`; plan contains only `T_change_maestro` + `T_verify_maestro`, no `T_change_demo_*` or `T_change_maestro_web`; `failure_clarity` 1 → 5, `state_integrity` 5 → 5 |

### Stage 3 metric deltas (vs Stage 1)

| case | `failure_clarity` | `state_integrity` | `error_message_quality` | `output_actionability` | notable |
|---|---|---|---|---|---|
| T2_5-S1 | 3 → 5 (+2) | 5 → 5 | 3 → 5 (+2) | 3 → 4 (+1) | aggregated error is the single biggest UX win for security-config fixers |
| T2_5-P1 | 3 → 3 | 4 → 4 | 3 → 3 | 3 → 4 (+1) | resilience improved (99/100 vs 0/100); audit log gap (N1) caps `failure_clarity` |
| T2_5-PD1 | 1 → 5 (+4) | 5 → 5 | 1 → 5 (+4) | 2 → 4 (+2) | biggest single-case improvement of the cycle; required two fix commits (`db218ac` + `7cd7665`) |

## Verification

Targeted regression tests added this cycle:

- `validate_reports_all_denied_copy_files_in_one_error` (F001)
- `file_mock_transport_skips_malformed_inbox_lines_and_continues` (F002)
- `botmux_history_parser_skips_malformed_user_messages_and_continues` (F002)
- `poll_inbound_continues_after_malformed_mock_inbox_line` (F002)
- `synthesize_file_filters_projects_by_goal_path_token` (F003)
- `synthesize_file_falls_back_when_no_goal_token_matches` (F003)
- `synthesize_file_emits_planner_granularity_notice_when_goal_has_path_token` (F003)
- `synthesize_file_path_match_is_case_insensitive_and_matches_basename` (F003)
- `work_dry_run_prints_planner_granularity_notice` (F003 B1, integration)
- `synthesize_file_matches_relative_goal_path_under_project_root` (F003 Stage 3 regression catch)

Each fix ran the full local matrix before push:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- `git diff --check origin/main..HEAD`

GitHub CI (rust stable + web) was post-push verification (per 高鹏
Cycle 3 directive, not pre-merge gate). All product-fix commits
shipped CI green:

- `eaa2201` (Round A close, includes F001 + F002) — run `26378212294` ✅
- `a951011` (F003 land marker) — run `26378604265` ✅
- `e3e3140` (F003 B1 fix-up) — run `26378859661` ✅
- `7cd7665` (F003 Stage 3 regression fix) — run `26379153009` ✅

Local lib test count reached 309 at cycle end (+5 vs Cycle 2 close).

## Methodology Validation

Tier 2.5 worked as designed:

- **3 frictions surfaced in ~20 min of Stage 1 dogfooding** across 3
  cross-cut scenarios — signal density on par with T2 (5 in ~30 min).
  Per-axis case count of 1 was sufficient; no need to fan out further
  at Stage 0.
- The new T2.5 metrics `failure_clarity` and `state_integrity`
  successfully discriminated cases: T2_5-S1 scored high
  state_integrity (5) because the guard was strict, but mid
  failure_clarity (3) because of single-entry reporting. T2_5-P1
  scored low failure_clarity (3) before fix because the parse error
  message buried the row index that aborted the batch. T2_5-PD1
  scored failure_clarity=1 before fix because nothing warned the user
  about planner granularity at all.
- **Direct-push process worked** (per 高鹏 directive). No PR
  ceremony cost; review still happened via reading commit diffs on
  `main` and follow-up commits (B1 was a clean post-review fix-up,
  not a revert). CI being post-push instead of pre-merge cost zero
  because all 5 product commits were green on first CI run; the two
  follow-ups (`e3e3140`, `7cd7665`) were additive fixes (visibility
  gap + dogfood regression), not reverts.
- **Design-first for Round B (F003) paid off again**: 大力 picked B-ii
  from the 3-option menu, caught the basename false-positive during
  TDD, and landed under the design's LoC estimate's spirit (file
  scope unchanged at 1 file). The B1 visibility gap was a review
  miss in the design doc itself (I described "notice in PLAN.yaml"
  without specifying stdout surface). Worth folding "every notice
  must be visible in the stdout path the user runs" into future
  design checklists.
- **Stage 3 caught a real design regression** (`7cd7665`). The F003
  implementation silently assumed `workspace root == project root`,
  which only holds in self-dogfood. Stage 1 used a self-dogfood
  fixture and missed it; Stage 3's runner used a tmpdir workspace
  with the project root outside, immediately reproducing the
  original symptom. **Lesson**: Stage 1 fixtures should be at least
  one structural step removed from `cwd == .maestro-parent`; the
  Stage 3 runner's tmpdir-based isolation is what should be the
  Stage 1 default going forward.
- **The "narrower than expected" scope amendment was healthy**:
  dropping U1 mid-cycle (高鹏 directive) removed 25% of nominal work
  but didn't reduce cycle value — the remaining 3 cases were the
  high-signal ones. Cross-cut axes that have no underlying business
  reason to test are not worth running just to fill a matrix.
- **Message-bus lag persisted** (Cycle 2 §Methodology observed it
  too). Several "校准" exchanges happened where one side responded
  to outdated state. Mitigation this cycle: explicit "go silent"
  protocol — when a stale message arrives, do not respond; let the
  queue drain. Both sides converged on this independently. Worth
  proposing a structural fix in Cycle 4: have the runner script
  echo the latest origin/main HEAD before every dogfood case, so
  stale-state messages self-resolve without inter-agent ping-pong.

## Open Follow-Ups (for Cycle 4)

Non-blocking observations recorded during review (do not gate Cycle 3
closure):

- **F002 N1** (logged in `947ad26`): malformed inbound messages are
  `tracing::warn!`-only, not surfaced in
  `channel_inbound_decisions.ndjson`. Reopen if Stage 3 (or any later
  dogfood) shows operators struggle to discover parse failures from
  decisions log alone. Spec if reopened: parallel
  `channel_parse_failures.ndjson` with `{source, line/message_index,
  error}` per line + cursor re-poll regression test.
- **F003 test name**: `synthesize_file_path_match_is_case_insensitive_and_matches_basename`
  refers to a basename heuristic that was removed before merge.
  Rename to `..._is_case_insensitive_path_prefix` on next touch of
  `plan.rs`.
- **F003 design checklist**: future design docs that introduce
  user-facing notices must specify which stdout path emits them, not
  just which file they end up in. Roll into design doc template.
- **Stage-1 fixture default**: switch Stage 1 runners to a tmpdir
  workspace with project root outside the workspace by default, so
  the `workspace root ≠ project root` topology is tested at Stage 1
  instead of slipping through to Stage 3.
- **Cycle 2 leftover polish** (7 items listed in
  `CYCLE-2-REPORT.md` §Open Follow-Ups): still open. Can be a small
  batch direct-push during Cycle 4 if convenient.

## Next Round

Direction options for 高鹏:

1. **Cycle 2 + Cycle 3 leftover polish batch**: knock down the 7
   Cycle-2 follow-ups + F003 test rename + (optionally) F002 N1
   audit-log parallel file as one tight direct-push pass. ~1 hour
   total.
2. **Tier 3 edge cases**: corrupted state recovery, partial network,
   file-system-full, concurrent agent invocations (originally planned
   but moved out of Cycle 3 scope). Real cross-cut wear, larger fix
   surface.
3. **Re-run Cycle 2 Stage 3** (deferred from Cycle 2): re-dogfood the
   4 T2 cases against current `main` to confirm cycle-2 fixes still
   hold after Cycle 3 changes touched plan synthesis + channel poll.
4. **New product direction**: drop the dogfood cycle and pivot to
   feature work (e.g., real module-level planner from F003 B-iii,
   real botmux integration test, web UI updates).

---

Signed off: 大马猴 (design / review) + 大力 (implementation / runs).
Awaiting 高鹏: pick direction for Cycle 4.
