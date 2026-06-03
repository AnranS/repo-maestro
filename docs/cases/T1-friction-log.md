# Tier 1 Friction Log

Use this file to turn dogfood observations into fix work. Keep each row short;
link to the transcript folder for evidence.

## Rating

- severity:
  - P0: blocks the case or can corrupt user state.
  - P1: common first-run pain with a clear fix.
  - P2: confusing or rough but not blocking.
- friction_score:
  - 5: user is likely to quit.
  - 4: user needs help or source-code knowledge.
  - 3: user can continue after a few minutes.
  - 2: minor wording or ordering issue.
  - 1: polish.

Fix order: P0 first, then P1 by descending `friction_score`, then P2 backlog.

## Status Values

- `observed-manual`: observed by a human without a runner transcript.
- `confirmed`: reproduced and linked to a runner transcript.
- `in-fix`: a fix commit or PR is in progress.
- `fixed`: fixed on `main`.
- `deferred`: moved to backlog or a later tier with an explicit reason.

## Cycle 1 Log

| id | case | severity | friction_score | transcript | observation | expected fix | owner | status |
|---|---|---:|---:|---|---|---|---|---|
| T1-F001 | T1-C4 | P1 | 4 | `T1-20260524T155220Z/T1-C4`, `T1-20260524T160511Z/T1-C4` | Empty `maestro work "fake goal"` can create `.maestro/` and then fail with `no projects registered` without a clear next command. | Add next-step guidance: `maestro work --root <path>` or `maestro init --analyze --root <path>`, plus cleanup guidance if initialization happened. | 大力 | fixed |
| T1-F002 | T1-C1 | P2 | 2 | pending | `./target/release/maestro` can be stale unless the user runs `cargo build --release` first. | Make cold-start docs prefer the installed `maestro` binary or explicitly require a fresh release build before using `target/release/maestro`. | 大力 | fixed |
| T1-F003 | T1-C5 | P1 | 4 | `T1-20260524T155220Z/T1-C5`, `T1-20260524T160511Z/T1-C5` | `maestro bench all --json --offline` exits 1 with valid JSON, but failed OSS replay rows do not include the cache/offline error reason. | Preserve fixture setup errors in each `BenchResult.error` so JSON consumers and humans can distinguish cold-cache setup from planner regressions. | 大力 | fixed |
| T1-F004 | T1-C3 | P1 | 3 | `T1-20260524T155220Z/T1-C3`, `T1-20260524T160511Z/T1-C3` | `maestro demo --run` prints a report path but no next command for moving from the demo workspace to real project analysis. | Print next-step commands after successful demo run: `init --analyze --root ...` and `work ... --dry`. | 大力 | fixed |
| T1-F005 | T1-C2 | P2 | 2 | `T1-20260524T155220Z/T1-C2`, `T1-20260524T160511Z/T1-C2` | The C2 case definition ran `providers && doctor` in a fresh workspace, so `doctor` failed on missing `.maestro/` before provider/auth UX could be judged. | Include `maestro setup` before `providers`/`doctor` in the case definition. | 大力 | fixed |
| T1-F006 | T1-C1 | P2 | 3 | `T1-20260524T155220Z/T1-C1`, `T1-20260524T160511Z/T1-C1` | `setup` prints `✓ Setup complete.` even when doctor reported failing checks. | Separate local setup completion from health status, or print `setup complete with doctor issues`. | 大力 | fixed |
| T1-F007 | T1-C1 | P1 | 3 | `T1-20260524T155220Z/T1-C1`, `T1-20260524T160511Z/T1-C1` | Setup lists missing runnable adapter binaries but does not put install commands or URLs in the provider summary. | Add one-line install/auth hints for missing runnable adapters, or link directly to provider setup docs. | 大力 | fixed |
| T1-F008 | T1-C1 | P2 | 2 | `T1-20260524T155220Z/T1-C1`, `T1-20260524T160511Z/T1-C1` | Setup final hints use literal placeholders like `<goal>` and `/path/to/your/projects`. | Add a concrete example command near the placeholder form. | 大力 | fixed |
| T1-F009 | T1-C3 | P2 | 2 | `T1-20260524T155220Z/T1-C3`, `T1-20260524T160511Z/T1-C3` | `init --analyze --root <absolute path>` generates a very long plan filename containing absolute path fragments. | Shorten generated audit plan filenames by using root basename plus hash. | 大力 | fixed |
| T1-F010 | T1-C2 | P2 | 2 | `T1-20260524T155220Z/T1-C2`, `T1-20260524T160511Z/T1-C2` | `providers` table columns `TRACE` and `NONINT` have no in-table legend. | Add a short legend after the table. | 大力 | fixed |
| T1-F011 | T1-C3 | P2 | 2 | `T1-20260524T160511Z/T1-C3` | Demo run prints internal `INFO channels config not found ...` even though channels are optional. | Lower missing `channels.yaml` log level or suppress it in demo runs. | 大力 | fixed |
| T1-F012 | T1-C2 | P2 | 3 | `T1-20260524T160511Z/T1-C2` | `providers`/`doctor` can show provider CLIs as installed/pass without proving credentials are usable for a non-interactive task. | Add an explicit auth/session health field or label installed-vs-authenticated separately. | 大力 | fixed |

## Backlog Outside Cycle 1

| id | tier | note | reason deferred |
|---|---|---|---|
| T1.5-001 | upgrade | Validate data compatibility from older `.maestro/` layouts. | Needs a fixture for old workspace state. |
| T1.5-002 | security | Exercise prompt/shell injection surfaces in user-provided goals. | Requires a dedicated threat model pass. |
| T1.5-003 | performance | Measure 20-task DAG and channel poll burst behavior. | Not cold-start critical. |
