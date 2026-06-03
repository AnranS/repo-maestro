---
name: repo-maestro
description: >
  Use when a coding task must land across MULTIPLE repos/projects in
  dependency order — e.g. a shared library or contract change plus the
  services and frontends that consume it, or a monorepo plus sibling
  repos that must change together. Delegates planning and
  dependency-ordered execution to the local `maestro` CLI. Do NOT use
  for single-repo, single-surface changes — handle those directly.
---

# Repo Maestro

Drive the local `maestro` CLI to plan and run a change that spans
several projects in dependency order. This skill **drives maestro; it
does not edit repos directly** — maestro runs each step in its own git
worktree and reports back.

## When to use this skill

- The change touches **two or more** projects/repos that must change in
  the right order (a producer's contract change plus its consumers).
- A monorepo plus sibling repos that share a contract.

## When NOT to use it

- A single repo, single surface. Handle that directly — do not invoke
  maestro for it.

## Steps

1. **Confirm scope.** If only one repo/surface is involved, stop and
   handle it directly. Otherwise continue.
2. **Check the CLI is available:** run `maestro --version`. If it is not
   installed, tell the user to install Repo Maestro (see its README) and
   stop — do not improvise a multi-repo run by hand.
3. **Discover the workspace:** `maestro init --analyze --root <dir>`.
   This registers the projects and surfaces the dependency graph and
   contracts. Use the names it discovers — do not invent project names.
4. **Plan dry-first:** `maestro work "<goal>" --root <dir> --dry`. This
   renders every task prompt and the per-task blast radius (which
   contracts it touches, how many downstream tasks it affects) **without
   spending any agent calls.**
5. **Show the plan to the user.** Present the task DAG, the contract
   edges, and any high-risk / contract-touching tasks. **Wait for the
   user's explicit approval / go before running anything.**
6. **Execute only after approval:** `maestro work "<goal>" --root <dir>
   --run`.
7. **Report the result.** Summarize `REPORT.md` and `RUN_STATE.json`
   (what ran, what blocked); offer `maestro open` to inspect the run in
   the dashboard.

## Guardrails

- **Always `--dry` before `--run`.** Never run without the user seeing
  the blast radius first.
- **Explicit approval gates the run.** A dry plan is not a go.
- **Surface high-risk and contract-touching tasks** before running.
- **This skill drives maestro; it does not edit repos directly.** All
  file changes happen inside maestro's per-task worktrees.
