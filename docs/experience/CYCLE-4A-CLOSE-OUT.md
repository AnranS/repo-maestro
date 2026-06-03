# Experience Gauntlet Cycle 4a Close-Out (Polish Batch)

Date: 2026-05-25

## TL;DR

Cycle 4a was a paper-cut polish batch: **7 leftover items from Cycles 2/3
+ 1 F003 follow-up rename** (8 items total), all squashed into one
direct-push commit on `main`. F002 N1 (malformed inbound parallel audit
log) explicitly stayed deferred — it requires a new persistent artifact
+ a cross-transport abstraction, which is the Cycle 4b boundary, not
the 4a boundary. Cycle 4a unblocks Cycle 4b (Tier 3 edge cases) by
clearing review surface and known nits.

## Scope

| follow-up id | source | area |
|---|---|---|
| PR-40 O1 | Cycle 2 follow-up | atomic mock `FileMockTransport::send` (single `write_all` of a one-line JSONL string) |
| PR-40 O2 | Cycle 2 follow-up | docs: channel-selection precedence in `channels.yaml` (T2-flagship §C2) |
| PR-40 O3 | Cycle 2 follow-up | `select_transport_dispatches_mock_to_file_transport` unit test + `file_mock_transport_serializes_outbound_as_one_jsonl_line` unit test |
| PR-40 O4 | Cycle 2 follow-up | `<latest-run-dir>` placeholder substitution in `scripts/run-experience-case.sh` + integration test |
| PR-41 O1 | Cycle 2 follow-up | doc-comment locking the `cache_miss` bail phrase contract in `src/bench/runner.rs` against drift |
| PR-41 O2 | Cycle 2 follow-up | T2-C4 expected / pass-signal updated to post-fix behavior (separate `cache_miss` from real failures + name `maestro bench hydrate`) |
| PR-41 O3 | Cycle 2 follow-up | align-pad `HYDRATED` / `ALREADY-CACHED` / `SKIPPED` / `FAILED` status column (`format_hydrate_status_line` helper + unit test) |
| F003 N2 | Cycle 3 follow-up | rename `synthesize_file_path_match_is_case_insensitive_and_matches_basename` → `..._is_case_insensitive_path_prefix` (basename heuristic was already removed; test name had drifted) |

### Deferred (explicit, not lost)

- **F002 N1** (malformed inbound parallel audit log): requires
  a new file `channel_parse_failures.ndjson` plus a cross-transport
  abstraction. Treated as structural, not polish. Stays open as
  `known-limitation` in `docs/cases/T2_5-friction-log.md`. Reopen only
  if Cycle 4b T3-C1 dogfood (or any later) shows operators struggle to
  discover parse failures from the decisions log alone.

## Changes Shipped

| commit | landing | area | summary |
|---|---|---|---|
| `7f3751d` | direct | code+docs | All 8 polish items in one commit (186 insertions / 24 deletions across 8 files). |

The implementer chose to squash the 8 items into a single commit
rather than the 3-way split (PR-40 polish / PR-41 polish / F003 N2)
the dispatcher suggested. The trade-off is fewer commits in `git log
--oneline` against losing per-item bisectability; for a paper-cut
batch with no behavioral risk, the squash is the better call. The
dispatch already enumerates which file change maps to which
follow-up id, so the audit trail is preserved off-commit.

## Test Discipline

All polish ran the full local matrix pre-push (per Cycle 3 directive):

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- `bash tests/scripts/run_experience_case_test.sh`

Three new unit tests were added in this commit, all green pre-push:

- `file_mock_transport_serializes_outbound_as_one_jsonl_line`
- `select_transport_dispatches_mock_to_file_transport`
- `format_hydrate_status_line_aligns_status_labels`

CI post-push verification:

- `7f3751d` — run [`26384872757`](https://github.com/AnranS/repo-maestro/actions/runs/26384872757) ✅ (rust stable 2m23s + web 51s)

## Methodology Notes

- Polish stayed strictly within paper-cut boundary. Every change was
  one of: a docs revision, a one-line message-string contract pinned
  by a comment, a renamed test, or an extracted formatting helper
  with a regression test. No item was load-bearing for any user
  workflow not already covered.
- F002 N1 deferral was honored. The dispatcher was tempted to fold it
  in ("it's almost polish"), but it required a new persistent file
  and a cross-transport abstraction, which is exactly the Cycle 4b
  boundary the polish batch is supposed to clear ground for.
- Cycle 4a fit the "small batch direct-push" path documented at the
  end of `docs/experience/CYCLE-3-REPORT.md §Open Follow-Ups` — no
  design doc needed because each item was ≤ ~70 LoC and behaviorally
  additive.
- Squash-vs-split decision sat with the implementer (their lane). The
  dispatch did not enforce 3-way split; squash was acceptable as long
  as the close-out preserved per-item attribution, which this
  document does.

## Next

Cycle 4b — Tier 3 edge cases. See `docs/cases/T3-edge-cases.md`.

Dispatcher: 大马猴. Implementer: 大力. Reviewer of dispatch + final
report: 高鹏. Same direct-push protocol as Cycle 3.

---

End of Cycle 4a (polish batch).
