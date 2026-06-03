# F-108 — risk classification when a project is a subdir of a larger repo

> **Scope.** Short design note for a dogfood finding (2026-05-29).
> **Rules + decisions only.** Owner: 街溜子-大福 (drafting, impl) · 大力
> (review). Status: design — impl after dali's GO. The example uses the
> neutral fixture shape (`shared-lib` + `idl/user.proto`).

## 1. Problem (observed in dogfood)

Risk-driven features — F-106 refute pass **and** the existing
`gate_on_high_risk` — only fire when a finished task's
`artifacts.files_changed` is populated. That list is computed by
`gitops::changed_files(output_workspace)` (a `git -C <dir> status
--porcelain`) and then diffed against the pre-run snapshot.

Two real-agent (codex) dogfood runs, identical except for repo layout:

| Layout | files_changed | risk | refuter |
|---|---|---|---|
| project **is its own git repo** | `[idl/user.proto]` | high | attached + ran ✅ |
| project is a **subdir** of a repo whose `.git` is at the workspace root | `[]` | low | never attached ❌ |

So on the second layout the high-risk contract change sailed through
with no refuter and no gate. That second layout is exactly the
**monorepo** case maestro must support (see the dev-portal monorepo
work) — one `.git` at the root, many projects as subdirs.

## 2. Likely cause (to confirm in step 1)

Two factors, at least one of which trips:

1. **Path relativity mismatch.** Contract paths (`contracts.provides`)
   are **project-relative** (`idl/user.proto`). But the change list and
   its before/after diff can come back either empty or repo-root-relative
   (`shared-lib/idl/user.proto`) depending on where `.git` sits relative
   to the `git -C <dir>` target. The risk classifier compares
   change-paths against project-relative contract paths; a
   repo-root-relative path never matches → contract-touch signal lost.
2. **Snapshot/diff semantics in a subdir.** `changed_files` runs against
   the project dir, but `git status` reports repo-wide state; the
   before/after subtraction in `run_task` can net to empty in the
   shared-`.git` case (e.g. if the agent commits, or sibling churn
   shifts the set).

I have **not** fully bisected which factor dominates — **step 1 of
implementation is a deterministic repro** (a shell-adapter task that
writes a known file in a subdir-of-repo project, asserting
`files_changed`), so the fix targets the proven cause, not a guess.

## 3. Proposed fix

Make change-tracking correct for "project is a subdir of a larger repo":

- Resolve the project's enclosing git repo root once
  (`git -C <project> rev-parse --show-toplevel`).
- Compute changed files **scoped to the project subtree** and
  **normalized to project-relative paths**, so the result is the same
  shape the single-repo case already produces (`idl/user.proto`), which
  is what the risk classifier + `contract_paths_for` already expect.
- Keep the existing `.maestro/` + transient-artifact filtering.

This is a `gitops`-layer fix (the shared `changed_files` path), so it
benefits **both** F-106 refute and `gate_on_high_risk` with one change —
no executor logic change beyond passing the project root if needed.

## 4. False-positive / safety notes

- Scoping to the project subtree also stops **cross-project bleed** —
  a change in sibling `app-alpha/` must not count as `shared-lib`'s
  change (which would mis-classify risk and mis-attribute refute).
- No behavior change for the single-repo layout (already correct); the
  regression test must prove that path still returns the same list.

## 5. Tests we'll require

- `changed_files_in_repo_subdir_returns_project_relative_paths` — the
  core repro: a repo with `shared-lib/` as a subdir, a write to
  `shared-lib/idl/user.proto`, asserts the project-scoped result is
  `[idl/user.proto]` (not empty, not `shared-lib/idl/user.proto`).
- `changed_files_excludes_sibling_project_changes` — a write in a
  sibling dir does not show up in this project's change list.
- `changed_files_single_repo_layout_unchanged` — regression guard for
  the project-is-its-own-repo case.
- Integration (mock or shell adapter, no real agent needed): a
  high-risk contract change in a subdir-of-repo project, with
  `refute_on_high_risk` (or `gate_on_high_risk`) on, now classifies
  high and attaches/gates — the end-to-end the dogfood couldn't get on
  the subdir layout.

## 6. Open questions for dali

1. **Fix location**: inside `gitops::changed_files` (resolve toplevel +
   scope + relativize internally) vs a new `changed_files_for_project`
   wrapper that callers in the risk path use. Recommendation: fix
   `changed_files` itself so every caller benefits and there's one
   correct implementation. Agree?
2. **Severity / priority**: this is a correctness gap that silently
   disables risk oversight in the monorepo layout. I'd call it P1 for
   the monorepo story but it's not a regression (pre-existing). Treat as
   the next item after dogfood, or queue behind F-104/F-105?
3. Anything you want covered in the repro before I commit to a fix
   direction?
