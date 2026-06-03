# Experience Gauntlet Cycle 2 — Stage 1 Summary

Date: 2026-05-25
Author: 大马猴 (review) + 大力 (execution)

## TL;DR

Tier 2 flagship dogfood is complete on `main`. We ran the 4 cases from
`docs/cases/T2-flagship.md` (pnpm monorepo, self-dogfood, channel mock loop,
bench warm) and surfaced **5 frictions** (2 P1/5 and P1/4, 1 P1/4 deferred,
2 P2/3). All evidence is in `docs/cases/T2-runs/T2-20260524T163843Z/`.

The cycle is paused at the Stage 1 / Stage 2 boundary so 高鹏 can pick
direction before any product code changes are made.

## Pipeline Reference

| stage | commits |
|---|---|
| Cycle 2 case-book | `ca64cd6 docs(cases): add tier two flagship case-book` |
| Cycle 2 T1 batch wrapper | `0dffc9a chore(cases): add tier one cycle wrapper` |
| Cycle 2 Stage 1 dogfood | `6075dbb docs(cases): record tier two flagship dogfood` |

## Scope Run

| case | input | exit | output_actionability | realism_score |
|---|---|---:|---:|---:|
| T2-C1 | pnpm monorepo (`examples/monorepo-pnpm`) — analyze + dry-plan | 0 | 2 | 2 |
| T2-C2 | self-dogfood (this repo from tempdir) — analyze + dry-plan | 0 | 2 | 2 |
| T2-C3 | channel mock loop — `work --channel mock` | non-zero | 1 | 1 |
| T2-C4 | bench OSS replay `--offline` cold-cache | 1 by design | 4 | 3 |

Full per-case transcripts:
`docs/cases/T2-runs/T2-20260524T163843Z/T2-C{1,2,3,4}/`

## Findings (5)

| id | case | severity | friction_score | observation | status |
|---|---|---:|---:|---|---|
| T2-F004 | T2-C3 | P1 | 5 | `maestro work --channel mock` is an unknown argument; no user-level mock channel transport path exists. Flagship channel scenario cannot start. | confirmed |
| T2-F001 | T2-C1 | P1 | 4 | `work --root examples/monorepo-pnpm --dry` produces a plan that includes stale `T_change_codex_lab_*` tasks from outside the requested root — `--root` does not scope the synthesis to discovered projects. | confirmed |
| T2-F002 | T2-C2 | P1 | 4 | Self-dogfood dry-plan stays at project level (`T_change_maestro`); the requested file/module-level audit goal does not produce `src/cli/...` targets. | confirmed |
| T2-F003 | T2-C2 | P2 | 3 | Analyzer includes local scratchpad projects like `maestro-demo-*` when scanning the maestro repo as an external fixture; gitignore / ignore patterns are not respected at discovery. | confirmed |
| T2-F005 | T2-C4 | P2 | 3 | Bench JSON is parseable and failed rows carry `error` (T1-F003 fix still working), but pass rate is 10/15 because OSS cache is cold; no one-command warm-cache hydrate path. | confirmed |

Full schema and rating rubric: `docs/cases/T2-friction-log.md`.

## Methodology Validation

The two new Tier 2 metrics (`output_actionability`, `realism_score`) both
distinguished good from bad cases without ceiling effects:

- C4 scored `output_actionability=4` (JSON is consumable as-is) but
  `realism_score=3` (cold-cache limited the realism); they correctly
  decouple "is the output well-formed" from "did the run actually exercise
  realistic data".
- C1 and C2 both landed at `output_actionability=2`, matching the
  qualitative finding that neither dry-plan is directly executable on the
  intended target.
- C3 sat at `output_actionability=1` and `realism_score=1`, correctly
  reflecting "the case cannot run at all".

The Tier 2 case-book template (output-sample.md required, two extra metrics
columns) added <2 min of recording overhead per case and produced clearly
better evidence than Tier 1 transcripts alone.

## Stage 2 Fix Scope (proposed, awaiting 高鹏 GO)

Fix order follows the dual-axis rubric: P1 by descending `friction_score`,
then P2 backlog. Recalibrated PR threshold from cycle 2 still applies.

| id | sev/score | scope | landing path | est |
|---|---|---|---|---:|
| T2-F004 | P1/5 | Channel UX: ship `--channel mock` + user-level mock transport, **or** document channel loop as "feishu + credential only" and remove the mock-loop path from the case-book. Design decision required. | **Design dispatch first** (`docs/design/T2-F004-channel-ux.md` with 2 options + tradeoff). After my ack: PR. | design 0.5h + ~80 LoC |
| T2-F001 | P1/4 | Root scope leak. Two candidate fixes: (a) `init --analyze` prunes registry entries outside `--root`; (b) `work --root` filters projects from registry to those under root. Code investigation needed before choosing. | **Debug commit** (read `src/cli/commands/init.rs` + `plan.rs`), then PR. | debug 0.5h + ~30 LoC |
| T2-F002 | P1/4 | Planner depth: project-level plans are the current product behavior, not a regression. Improving to module-level granularity is a planner-feature track. | **Defer to Tier 2.5 / planner-depth backlog**. Out of cycle 2 scope. | n/a (deferred) |
| T2-F003 | P2/3 | Analyzer should respect gitignore (and/or built-in ignore for `maestro-demo-*` style scratchpads) during discovery. | Small PR, single discovery-walker change. | ~20 LoC |
| T2-F005 | P2/3 | Add `maestro bench hydrate` (or `--auto-fetch`) so warm-cache is a single command instead of manual `git clone`s. UX decision: separate subcommand or flag on `bench`. | **Design dispatch first** (small note in `docs/design/T2-F005-bench-hydrate.md`), then PR. | design 0.3h + ~100 LoC |

### Recommended Cycle 2 Stage 2 batch

If 高鹏 says GO:

- **Round A (parallel, can start immediately)**: T2-F001 debug + fix · T2-F003 ignore patterns. Two small PRs.
- **Round B (design-first)**: T2-F004 channel UX design dispatch · T2-F005 bench hydrate design dispatch. After my ack, two PRs.
- **Deferred**: T2-F002 → Tier 2.5 planner-depth track.

Estimate: Round A landable in ~1h after GO; Round B landable in ~2h after
GO; entire cycle 2 closed in roughly 3h of focused work.

## Process Notes

- Cycle 2 Stage 2 fixes will land via PR per the rule 大力 accepted ("all
  product/code via PR, docs/transcripts direct with pre-announcement").
- Stage 1 dogfood evidence (this commit's transcripts + friction log) was
  direct-to-main because it is pure evidence artifact, per the same rule.
- Mid-cycle review summary (this file) is also direct-to-main per the
  doc-only carve-out.

## Decision Asked Of 高鹏

Pick one of:

1. **GO Round A only** — land T2-F001 + T2-F003 fixes, hold the rest.
2. **GO Round A + Round B** — also dispatch T2-F004 + T2-F005 design notes
   and execute after my ack.
3. **HOLD entirely** — keep Cycle 2 paused at Stage 1; pick a different
   direction (Tier 2.5 cross-cut, planner-depth, etc.).
4. **REPLAN** — request a different Stage 2 scope (e.g. include T2-F002
   in-cycle, or change deferral list).

Default if no response: stay paused on standby.

---

Signed: 大马猴 (review/design), 大力 (execution/transcripts).
