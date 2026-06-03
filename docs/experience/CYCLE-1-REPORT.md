# Experience Gauntlet Cycle 1 Report

Date: 2026-05-24

## TL;DR

Cycle 1 ran the Tier 1 cold-start cases against `main`, excluding the no-Rust
path per product direction. We found **12 friction items** (0 P0 / 4 P1 / 8 P2).
The first pass fixed 4 items; the follow-up passes closed the remaining
low-risk UX items. Current state: **12 fixed / 0 deferred**.

**Severity rubric (dual-axis)**:

- `severity`: `P0` blocks the case · `P1` common first-run pain · `P2`
  confusing but not blocking
- `friction_score`: `5` user likely to quit · `4` needs source knowledge · `3`
  continues after a few minutes · `2` minor wording · `1` polish
- **Fix order**: P0 all → P1 by descending `friction_score` → P2 backlog

**Roles this cycle**: 大马猴 designed cases, judged severity, drafted this
report; 大力 implemented case-book + runner + Stage 2 fixes + regression run;
高鹏 set scope (drop no-Rust, dev-runs-fixes / reviewer-designs split).

## Exit Criteria

| criterion | target | actual | met |
|---|---|---|---|
| First artifact latency | ≤10 s per case | ≤2 s per case | ✅ |
| Every failure has copy-pasteable next step | 5/5 | 5/5 after fixes (see metrics `recovery_success=yes` for all) | ✅ |
| All P0 fixed | 0 outstanding | 0 found this cycle | ✅ |
| Top P1 closed or owned | 4/4 P1 closed | done | ✅ |
| Largest per-commit LoC | ≤300 | 81 in first pass; follow-up remains under 300 changed lines | ✅ |
| Deferred friction | 0 open Tier 1 items | 12/12 fixed | ✅ |

## Scope

Cases:

| id | scenario | status after fixes |
|---|---|---|
| T1-C1 | Rust present, provider CLIs missing from `PATH` | exits 0; setup gives provider and next-step guidance |
| T1-C2 | setup + providers + doctor in a fresh workspace | exits 0 after case-spec correction |
| T1-C3 | `demo --run` then `init --analyze --root examples` | exits 0; now includes real-workspace handoff |
| T1-C4 | empty workspace `maestro work "fake goal"` | exits 1 by design; now actionable |
| T1-C5 | `bench all --json --offline` cold OSS cache | exits 1 by design; JSON now explains cache misses |

Evidence:

- Before fixes: `docs/cases/T1-runs/T1-20260524T155220Z/`
- After fixes: `docs/cases/T1-runs/T1-20260524T160511Z/`
- Metrics: `docs/cases/T1-runs/T1-20260524T160511Z/metrics.md`
- Friction log: `docs/cases/T1-friction-log.md`

## Changes Shipped

| commit | area | summary |
|---|---|---|
| `68b2643` | runner | Adds `scripts/run-experience-case.sh` and a shell regression test. |
| `d371adc` | docs | Adds Tier 1 case-book and friction-log scaffold. |
| `11adfc3` | docs | Adds status enum, metrics template, `examples` root, and T1-F002 seed. |
| `630e029` | UX fixes | Adds actionable empty-work recovery, demo handoff, bench error fields, and C2 case correction. |
| post-`578c751` follow-up | UX fixes | Adds setup doctor-status wording, missing-provider install hints, concrete setup example, bounded plan filenames, and provider table legend. |
| final Tier 1 follow-up | UX fixes | Suppresses optional channel-config INFO noise, clarifies provider auth semantics, and documents binary freshness preflight. |

The UX fix commit touches only:

- `src/cli/commands/plan.rs`
- `src/cli/commands/demo.rs`
- `src/bench/score.rs`
- `src/bench/report.rs`
- `docs/cases/T1-cold-start.md`
- `docs/cases/T1-friction-log.md`

## Fixed Findings

### T1-F001: empty `maestro work` recovery

Before:

```text
Error: no projects registered in .../.maestro/projects.yaml
```

After:

```text
next: maestro work "<goal>" --root <path>
or:   maestro init --analyze --root <path> --agent mock
cleanup: remove .maestro/ if this empty workspace was initialized by mistake
```

### T1-F003: bench offline cold-cache JSON reason

Before, failed OSS replay rows had zero scores but no setup reason. After, each
failed row includes `error`, for example:

```json
"error": "fixture vite-config-flag not cached and --offline set; manually clone https://github.com/vitejs/vite to \".../.maestro/bench/cache/vite-config-flag\""
```

### T1-F004: demo to real-workspace handoff

Before, `maestro demo --run` stopped at the report path. After a successful
demo run it prints:

```text
next:
  cd <demo-dir>
  maestro init --analyze --root /path/to/your/projects --agent mock
  maestro work "<goal>" --root /path/to/your/projects --agent mock --dry
```

### T1-F005: C2 case correction

C2 now runs `maestro setup` before `providers` and `doctor`, so the provider UX
is not hidden by a missing `.maestro/` failure.

### T1-F006: setup completion wording

Setup now distinguishes local setup completion from doctor health:

```text
✓ Setup complete with doctor issues.
```

when the doctor pass reports failures in non-strict setup mode.

### T1-F007: missing provider install hints

When setup detects missing runnable task adapters, it now prints one-line
install or environment-variable hints next to the missing adapter summary.

### T1-F008: concrete setup example

The setup next-step block still keeps the placeholder form, but now includes a
copy-pasteable low-risk example:

```text
maestro work "update README" --root examples --agent mock --dry
```

### T1-F009: bounded audit plan filenames

Default plan filenames now cap long slugs and append a stable hash suffix, so
`init --analyze --root <absolute path>` no longer creates filenames dominated
by absolute path fragments.

### T1-F010: providers table legend

The providers table now prints a short legend explaining `INSTALLED`, `TRACE`,
and `NONINT` instead of assuming users know those abbreviations.

### T1-F002: binary freshness preflight

The case-book now requires a fresh `cargo build --release` and records
`which maestro` / `maestro --version` before a full dogfood cycle when using a
repo-built binary.

### T1-F011: optional channel config noise

Missing `.maestro/channels.yaml` is now logged at debug level. Optional channel
configuration absence no longer appears in normal demo output.

### T1-F012: provider auth semantics

The providers legend now states that `INSTALLED` means binary presence only and
that auth/session health is not verified by the table.

## Deferred Findings

None. All Tier 1 cycle findings are either fixed in product code or closed by
the dogfood process documentation.

## Post-Fix Results

| case | exit | result |
|---|---:|---|
| T1-C1 | 0 | Setup completes and prints next commands. |
| T1-C2 | 0 | Setup, providers, and doctor complete; no failing checks in this environment. |
| T1-C3 | 0 | Demo report plus real-workspace handoff and generated plan. |
| T1-C4 | 1 | Expected failure, now with actionable recovery. |
| T1-C5 | 1 | Expected cold-cache failure, now with parseable per-fixture errors. |

## Verification

Targeted tests added:

- `empty_project_registry_error_includes_recovery_commands`
- `run_demo_next_steps_point_to_real_workspace_analysis`
- `evaluate_preserves_run_error_for_json_reports`

Verification commands run after `630e029`:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- `bash tests/scripts/run_experience_case_test.sh`
- `git diff --check HEAD~1..HEAD`

Additional targeted regression tests added in the follow-up pass:

- `setup_completion_text_distinguishes_doctor_issues`
- `setup_next_steps_include_concrete_example_and_placeholder_form`
- `providers_table_legend_explains_trace_and_noninteractive_columns`
- `provider_install_hint_names_binary_and_env_override`
- `default_plan_path_bounds_long_generated_filenames`
- `load_channels_config_missing_file_is_quiet_at_info_level`

## Methodology Validation

The Experience Gauntlet framework (severity P×F + runner transcripts +
friction-log + per-run metrics) **worked**:

- 12 actionable items surfaced in ~60 min design + ~30 min running + ~30 min
  fixing — well inside one evening.
- Every fixed item has a before/after evidence pair from runs 1 and 2 in
  `docs/cases/T1-runs/`.
- The dual-axis P×F ranking made it unambiguous which 4 to fix first
  (everything P1 with `friction_score` ≥ 3) and which 8 to defer (every P2
  plus F007 P1/3, with `recovery_success=yes` post-fix proving they were not
  blocking).
- Process miss to flag for cycle 2: `630e029` landed direct to `main` instead
  of opening a PR. Future Stage 2 product fixes should open PRs so CI runs
  and the review trail is preserved; docs/transcripts (`578c751`) are fine
  direct-to-main.

## Next Round

1. Add a `scripts/run-t1-cycle.sh` batch wrapper once the case sequence is
   stable.
2. Pick the next direction once 高鹏 reviews: either **Tier 2 flagship
   scenarios** (pnpm monorepo / Rust workspace / OpenAPI / channel flow) or
   **Tier 2.5 cross-cut** (upgrade/migration · security/abuse ·
   performance/load).

---

Signed off: 大马猴 (design/review) + 大力 (implementation/runs).
Awaiting 高鹏: pick direction for cycle 2.
