# Experience Gauntlet Cycle 4b Report

Date: 2026-05-25

## TL;DR

Cycle 4b ran the **Tier 3 edge-case** axis against `main` after the
Cycle 4a polish batch. We surfaced **3 friction items** (0 P0 / 1 P1 /
2 P2) around corrupted/interrupted state, partial-write behavior, and
concurrent `maestro work` invocations. All 3 were closed in-cycle.

The largest finding was T3-F002: `projects.yaml` writes were
non-atomic, and then the first atomic-write fix still allowed a
writer-vs-writer `.tmp` race. Stage 3 caught that design gap. The
final implementation uses a sibling `projects.yaml.tmp` as a
`create_new` writer gate, retries bounded writer conflicts, and cleans
up the tmp file with an RAII guard on error.

## Exit Criteria

| criterion | target | actual | met |
|---|---|---|---|
| Every Tier 3 case has a Stage 1 transcript | 3/3 | 3/3 (`T3-20260525T053649Z`) | yes |
| All P1 frictions fixed | 1/1 | T3-F002 fixed + addendum | yes |
| All P0 frictions fixed | 0 outstanding | 0 found | yes |
| P2 frictions fixed or deferred with rationale | 2/2 | T3-F001 + T3-F003 fixed | yes |
| Stage 3 re-run captured verification artifacts | 3 cases + addendum | `T3-stage3-20260525T060829Z` + `T3-stage3-addendum-20260525T064952Z` | yes |
| Regression tests added for code fixes | every product fix | projects atomic save/load, stale tmp doctor, concurrent registry | yes |

## Scope

Cases:

| id | scenario | result |
|---|---|---|
| T3-C1 | corrupted / interrupted `.maestro` state | doctor now warns on stale `projects.yaml.tmp` once older than 5s |
| T3-PN1 | disk-full / partial-write path | existing behavior stayed recoverable; failure names tmp write path and retry succeeds after cleanup |
| T3-CA1 | concurrent `maestro work --dry` invocations | same-goal and distinct-goal concurrent dry-runs now complete without empty-registry or tmp-rename errors |

Evidence:

- Stage 1 transcripts: `docs/cases/T3-runs/T3-20260525T053649Z/`
- Stage 1 metrics: `docs/cases/T3-runs/T3-20260525T053649Z/metrics.md`
- Round A design: `docs/cases/T3-F002-design.md`
- Stage 3 transcripts: `docs/cases/T3-runs/T3-stage3-20260525T060829Z/`
- Stage 3 addendum transcripts: `docs/cases/T3-runs/T3-stage3-addendum-20260525T064952Z/`
- Friction log: `docs/cases/T3-friction-log.md`

## Changes Shipped

| commit | area | summary |
|---|---|---|
| `a57e930` | docs | Open Cycle 4b Tier 3 case-book. |
| `1ca44ab` | docs | Record Stage 1 dogfood transcripts and 3 frictions. |
| `701b16c` | docs | Design T3-F002 atomic registry update. |
| `71144ad` | docs | Fix T3-CA1 case-book collision premise (T3-F003). |
| `ebc64fc` | UX fix | Atomic `projects.yaml` write via sibling `.tmp` + reader retry (T3-F002). |
| `b5f97d4` | UX fix | Doctor warns on stale `projects.yaml.tmp` (T3-F001). |
| `232e09e` | docs | Add F002 writer-vs-writer design addendum. |
| `cdd0517` | UX fix | Serialize `projects.yaml` writers via `create_new` writer gate (T3-F002 addendum). |
| `bef01e9` | tests | Cover retry-exhaustion path for `.tmp` load retry. |
| `d996dac` | docs | Record Stage 3 verification artifacts. |
| this commit | docs | Cycle 4b close-out report. |

## Fixed Findings

### T3-F001: stale `.tmp` visibility

Before: a stale `.maestro/projects.yaml.tmp` beside a valid
`projects.yaml` was invisible to `maestro doctor`, so interrupted
atomic-write residue looked healthy.

After (`b5f97d4`):

- `doctor` emits a WARN for `projects.yaml.tmp` older than 5 seconds.
- Fresh tmp files are ignored so legitimate in-flight atomic writes do
  not false-positive.
- The warning tells the user to remove the tmp file only after
  confirming `.maestro/projects.yaml` is valid.

Stage 3: `T3-stage3-20260525T060829Z/T3-C1` verified the warning path.

### T3-F002: atomic registry update under concurrency

Before: `ProjectsConfig::save` used `std::fs::write`, which truncates
`projects.yaml` before writing. A concurrent reader could observe an
empty registry and fail with misleading `no projects registered`.

Round A (`ebc64fc`):

- Save writes sibling `projects.yaml.tmp` then renames over canonical.
- Load retries an empty canonical while a sibling tmp exists.
- Empty-registry errors mention a concurrent tmp sibling when present.

Stage 3 then found a second race: two writers used the same tmp path,
so one could rename the tmp away before the other, producing
`No such file or directory` on `rename`.

Addendum (`cdd0517` + `bef01e9`):

- `save` opens `projects.yaml.tmp` with `create_new(true)` as the
  writer gate.
- Concurrent writers retry 5 x 10ms, then return a named concurrent
  writer error if the tmp remains locked.
- `TmpGuard` removes tmp on fallible paths before rename.
- The two-writer + one-reader integration test now covers the Stage 3
  regression.

Stage 3 addendum run:

- `T3-stage3-addendum-20260525T064952Z/T3-CA1`
- `ca1a_exit1=0`, `ca1a_exit2=0`, `ca1b_exitA=0`, `ca1b_exitB=0`
- regression grep for `no projects registered` and `rename projects tmp`
  is empty.

### T3-F003: case-book collision premise

Before: the case-book claimed `EXPERIENCE_RUN_ID` could force a
product run-id collision. That variable only affects dogfood
transcript paths, not Maestro's product run ids.

After (`71144ad`):

- T3-CA1 now probes a real product-level collision surface: same-goal
  concurrent invocations targeting the same deterministic plan slug.
- Distinct-goal concurrent invocations remain a separate success path.

Stage 3 addendum run verified the corrected premise with both same-goal
and distinct-goal concurrent dry-runs.

## Metrics Delta

| case | Stage 1 `failure_clarity` | Stage 3 `failure_clarity` | state integrity | recoverability |
|---|---:|---:|---:|---:|
| T3-C1 | 3 | 5 | 5 | 4 |
| T3-PN1 | 5 | 5 | 5 | 5 |
| T3-CA1 | 2 | 5 | 5 | 5 |

## Process Notes

- Direct-push workflow stayed effective for this cycle. Every code
  change had targeted local verification before push, then post-push
  CI confirmation.
- Stage 3 again paid for itself: it found the F002 writer-vs-writer
  design gap that the original Round A integration test missed.
- The F002 addendum deliberately kept on-disk layout narrow: only
  `projects.yaml.tmp` is treated as the writer gate. Other future
  atomic config files should add their own doctor coverage when they
  adopt the same protocol.

## Remaining Work

No open P0/P1/P2 from Cycle 4b.

Known limitations carried from prior cycles remain unchanged:

- Channel malformed-message parse failures still warn to stderr rather
  than writing a structured parse-failure row into a durable decisions
  log. Reopen only if future dogfood shows operators cannot recover
  from stderr + decisions-log evidence.

## Next Options

1. Re-run the full Tier 3 suite after CI on this close-out commit, if
   a fully post-report evidence pass is desired.
2. Move to the next edge axis: network failures / botmux daemon
   lifecycle / interrupted long-running executor runs.
3. Pivot from dogfood cycles back to feature work now that Tier 1,
   Tier 2, Tier 2.5, and Tier 3 have all produced fixes.

