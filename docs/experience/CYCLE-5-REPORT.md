# Cycle 5 Report - Tier 4 Interruption & Recovery

Status: closed

## TL;DR

Cycle 5 exercised process interruption paths: SIGINT, SIGKILL, and
immediate same-goal rerun after a crash. Stage 1 found 5 frictions
(4 P1, 1 P2). All 5 are fixed and Stage 3 verified.

Primary outcome: interrupted or killed `maestro work --run` no longer
silently corrupts follow-up behavior. SIGINT now cancels the targeted
run, concurrent event appends stay parseable, SIGKILL-abandoned runs
are detected by `doctor`, and same-goal rerun reports a named conflict
with `--force-new` / cleanup guidance.

## Scope

Case-book:

- `docs/cases/T4-interruption-cases.md`

Stage 1 evidence:

- `docs/cases/T4-runs/T4-20260525T070830Z/`

Stage 3 evidence:

- `docs/cases/T4-runs/T4-stage3-20260525T082018Z/`

Friction log:

- `docs/cases/T4-friction-log.md`

## Changes Shipped

| friction | commit | change |
|---|---|---|
| T4-F003 | `41be072` + `2a6d933` | Serialized run-event append with a per-file mutex and single-buffer write; added concurrent append stress coverage. |
| T4-F002 | `68a6526` | `work --run` now bridges SIGINT into a targeted cancel marker and exits 130 after graceful cancellation. |
| T4-F004 / T4-F005 | `073b0c6` | Added run-state PID persistence, liveness classification, abandoned-run doctor WARN, and same-goal preflight conflict with `--force-new`. |
| T4-F001 | `e4e2d3a` | Corrected T4 case-book probes to use real `--run` paths and current `RUN_STATE.json` naming. |
| Stage 3 | `d6db3f7` | Recorded verification artifacts and updated friction statuses. |

Supporting design docs:

- `3f4627d` - T4-F002 SIGINT propagation
- `c0ae13a` - T4-F004 / T4-F005 abandoned run handling

## Stage 3 Results

| case | Stage 1 | Stage 3 | result |
|---|---|---|---|
| T4-I1 SIGINT | `work --run` ignored SIGINT, finished `done`, exit 0 | SIGINT writes targeted cancel marker, exits 130, `RUN_STATE.json` is `cancelled` | fixed |
| T4-K1 SIGKILL | killed run left `status: running`; doctor showed only current run | doctor emits `abandoned run` WARN with dead pid and cleanup / force-new guidance | fixed |
| T4-RR1 rerun | same-goal rerun silently regenerated a new plan | rerun exits with named prior run and `--force-new` / `rm -rf` / `doctor` guidance | fixed |

Metrics delta:

| case | failure_clarity | state_integrity | recoverability | rerun_outcome |
|---|---:|---:|---:|---|
| T4-I1 | 2 -> 4 | 2 -> 5 | 3 -> 4 | restarted-clean |
| T4-K1 | 2 -> 5 | 3 -> 5 | 2 -> 4 | named-conflict |
| T4-RR1 | 1 -> 5 | 3 -> 5 | 2 -> 4 | named-conflict |

## Verification

Before pushing the product changes, local verification passed:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets` - 329 lib tests plus all integration targets passed
- `git diff --check`

CI status observed during the cycle:

- `41be072` success
- `68a6526` success
- `1178ee8` success

Final report commit will trigger the usual post-push CI.

## Notes

- The T4-F004/F005 implementation intentionally treats legacy run state
  with missing `pid` as `UnknownLegacy`, not abandoned. This avoids
  silently applying a new liveness contract to old local state.
- `--force-new` is the only rerun override added in this cycle. `--resume`
  remains out of scope because safe resume needs task-output and lock
  semantics beyond this interruption pass.
- The older channel malformed-message audit gap remains the inherited
  known limitation from Cycle 3; Cycle 5 did not reopen it.

## Next Options

1. Run a small Stage 3 re-check after final CI if desired.
2. Move to daemon / channel lifecycle edge cases.
3. Start product feature work now that the process-interruption path is
   materially safer.
