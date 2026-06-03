# F-110 — Finding ledger for HOTL evidence (design)

Status: design (awaiting review before implementation) · Reviewer: cross-review
Parent: [Team agent-workbench absorption plan](TEAM-AGENT-WORKBENCH-ABSORPTION.md) — primitive #2

## Problem

Maestro already emits several streams a human reviewer cares about, but they are
separate and shaped differently: scheduler `events`, `decisions`, the run
`REPORT`, risk scores, and refuter verdicts. There is no single place that
answers "what did this run find, how sure is it, where is the evidence, and was
it acted on?" The HOTL dashboard (absorption primitive #3) needs exactly that:
one append-only **finding ledger** it can read to render approvals, blocked
tasks, and findings-with-evidence.

This is a **data-contract** change first. v1 introduces the ledger + a writer
and wires 1–2 existing producers into it. It does NOT rewrite REPORT, decisions,
or events — those keep working; the ledger is an additive, unifying view.

## File location

```
.maestro/runs/<run-id>/findings.ndjson
```

Per-run, alongside the existing run artifacts (events, report). One file per
run keeps findings scoped to the run that produced them and makes cleanup
trivial (it goes when the run dir goes). Newline-delimited JSON: one finding per
line, append-only.

## Schema (v1)

One JSON object per line:

| field | type | req | notes |
|---|---|---|---|
| `schema_version` | string | yes | ledger schema tag, `finding.v1`; gates future schema growth so readers don't guess |
| `finding_id` | string | yes | `<kind>-<seq>`; per-run monotonic `seq` minted **inside the append lock** (no content hash) |
| `run_id` | string | yes | owning run |
| `task_id` | string | no | task that produced it, when applicable |
| `kind` | enum | yes | see below |
| `severity` | enum | yes | `info` / `low` / `medium` / `high` / `critical` |
| `confidence` | number | no | 0.0–1.0 when the producer has one |
| `summary` | string | yes | one-line human-readable, length-capped; NO raw secret/internal content |
| `evidence_refs` | string[] | yes | run-relative artifact paths only, count-capped (see rules) |
| `source` | string | yes | producer id, e.g. `risk` / `refuter` / `doctor` |
| `status` | enum | yes | v1: always `open`. Adoption/dismissal is a future *separate* transition record — never an in-place edit of this append-only ledger |
| `created_at` | string | yes | RFC3339 (passed in; never minted with a forbidden clock call) |
| `provenance` | object | no | `{ producer, producer_version?, inputs_digest? }` |

### `kind` enum (v1)

`risk` · `refute` · `approval` · `learn` · `doctor` · `channel`

Closed set in v1 so the dashboard can render per-kind affordances; extend
deliberately, not ad hoc.

## Write rules (hard)

- One JSON object per line; append-only. Use the same discipline as
  `append_event` (`src/scheduler/events.rs`): a per-file lock, a single
  `write_all(line + "\n")` so concurrent appends never interleave partial lines
  and stay parseable. The `<seq>` in `finding_id` is read-last-seq + 1, computed
  **inside that same lock**, so sequence numbers are unique under concurrency.
- **No raw secrets, no internal/sensitive content** in any field — `summary`
  and `evidence_refs` are sanitized. No internal/sensitive business, platform,
  or project names, paths, or tokens (see the absorption plan's Privacy
  boundary).
- `evidence_refs` are **run-relative artifact paths only** (e.g.
  `tasks/<task-id>/diff.patch`). **Reject** (never normalize) any entry that is
  absolute, contains `..`, or is empty — a bad path is a writer error, surfaced,
  not silently rewritten. The dashboard resolves the rest under the run dir.
- **Size caps**: `summary` is length-capped (≈500 chars) and `evidence_refs` is
  count-capped (a small bound). Large evidence is NEVER inlined — it lives as a
  run artifact and is referenced by path only, so the ledger stays small and
  cheap to scan.
- `created_at` is supplied by the caller (RFC3339). Scripts/tests must not rely
  on a wall-clock call that the runtime forbids.

## v1 scope

1. Data contract: a `Finding` struct (serde) + the `kind`/`severity`/`status`
   enums.
2. A writer helper — `append_finding(run_dir, &Finding)` — takes a per-file
   lock, mints the `seq` (read-last-seq + 1) and writes a single `line + "\n"`
   inside the lock; creates `findings.ndjson` on first write. Validates
   evidence-path + size caps before writing (rejects on violation).
3. Wire in the **two** confirmed producers (smallest code surface):
   - high-risk / refuter finding (from the risk-gate + refuter path), and
   - doctor / abandoned-run finding (from the run-health/reconcile path).
4. WebUI: NOT a full redesign in this round — only surface a findings
   count / list in the run-detail / evidence area. If that UI exceeds ~100 LoC,
   it splits to a later round.

Explicitly out of v1: migrating REPORT/decisions/events onto the ledger,
cross-run aggregation, retention/revocation policy, channel-write path.

## Tests

- serde round-trip for `Finding` incl. `schema_version` (all enums, optional
  fields present/absent).
- append → file is valid NDJSON and re-parses to the same records; **concurrent
  appends parse and `seq` stays unique** (mirror the `events.rs` concurrency
  test).
- evidence-path guard: **reject** an absolute path, a `..`-traversal path, and
  an empty entry — assert the write fails (not that it is normalized), so no bad
  path is ever persisted.
- size-cap guard: an over-long `summary` or too-many `evidence_refs` is rejected.
- at least one real integration path: a high-risk/refuter run writes a
  `kind: risk` (or `refute`) finding that parses back with the expected fields.

## Resolved decisions (review)

1. **`finding_id`** — per-run monotonic `<kind>-<seq>`, with `seq` minted inside
   the append lock (read-last-seq + 1, the `events.rs` pattern). No content hash.
2. **Dedup** — the writer stays dumb (writes every finding). The dashboard / API
   display layer collapses duplicates; the ledger is the raw record.
3. **`status`** — v1 producers write `open` only. The ledger is append-only, so
   v1 makes no in-place status flips. Adoption/dismissal, if added later, is a
   *separate* transition record or state file — never a rewrite of this ledger.
