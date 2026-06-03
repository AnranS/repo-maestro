//! Bundled starter skills + memory snippets that `maestro init` drops into a
//! fresh workspace so newcomers have something concrete to read.

use anyhow::{bail, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub struct BundledSkill {
    pub scope: &'static str,
    pub name: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUpdateStatus {
    Added,
    Updated,
    Unchanged,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct SkillUpdateResult {
    pub scope: &'static str,
    pub name: &'static str,
    pub status: SkillUpdateStatus,
    pub path: PathBuf,
}

pub const SKILL_WRITE_PR_DESC: &str = r#"---
description: Write a concise, value-first PR description for a small change
trigger: pull request | PR description | write PR
---

# Write PR Description

When to use it:

- After a single-purpose change is ready to push.
- You have access to the diff and the original spec line.

Steps:

1. Open with **one sentence** answering "why this PR exists" — for the reviewer, not for yourself.
2. Bullet the **observable** change points (what behavior is now different) — not the file list.
3. If the change touches a contract, link to the schema diff and call out backwards-compat.
4. Add a "Test plan" with the exact commands a reviewer would run.
5. Keep the whole thing under 200 words. No "as discussed" filler.

Outputs:

- A single markdown block ready to paste into the PR body.
"#;

pub const SKILL_SCOPE_TASK: &str = r#"---
description: Decide if a chunk of work belongs in this PR or a follow-up
trigger: scope creep | should I include
---

# Scope the Task

Ask:

1. Does this change make the failing thing pass?
2. Does it unblock another agent task in the current run?
3. Could it be reverted independently without breaking the main goal?

If 1 or 2 → include.
If only 3 → file as follow-up, don't expand the diff.
"#;

pub const SKILL_DEBUG_FLOW: &str = r#"---
description: Systematic debugging starter; reproduce → bisect → fix → regression test
trigger: debug | bug | why is it failing
---

# Debug Flow

Steps:

1. **Reproduce** locally with the smallest possible input. If you can't, stop here and write the repro first.
2. **Bisect**: form a hypothesis ("X breaks Y"), run a single command that confirms or refutes. Don't speculate longer than one cycle.
3. **Fix** the root cause, not the symptom.
4. **Lock it in** with a regression test before claiming done.
"#;

pub const SKILL_WORKFLOW_TASK_GUARDRAILS: &str = r#"---
description: Keep one workflow task scoped, evidence-driven, and handoff-ready
trigger: workflow guardrails | stay on task | avoid scope creep | don't drift
---

# Workflow Task Guardrails

Use this for any agent task launched from a Maestro workflow.

Rules:

1. Re-read the task prompt, declared inputs, and acceptance criteria before editing.
2. Stay inside this task's project/workspace unless the prompt explicitly says otherwise.
3. Do not expand scope to adjacent cleanup, rewrites, or "while I'm here" fixes.
4. Prefer the repo's existing patterns over new abstractions.
5. Treat explicit workflow inputs as authoritative task context; use retrieval/memory only as background.
6. If an optional helper, index, MCP tool, or plugin fails once, fall back to direct local files instead of retrying it repeatedly.
7. Do not rely only on `git diff` or `git status` to discover changes; generated, ignored, or nested workspaces can hide real file edits.
8. Before finishing, run the smallest meaningful verification command available for this task.
9. Final response must include: changed behavior, files touched at a high level, commands run, and any blocker/risk for downstream tasks.

If the task cannot be completed safely, stop with a clear blocker instead of improvising.
"#;

pub const SKILL_CONTRACT_FIRST: &str = r#"---
description: Handle API/schema/protocol changes without breaking downstream projects
trigger: contract | schema | openapi | proto | api change
---

# Contract First

Use this when a task changes a shared API, schema, protocol, generated client, or data contract.

Steps:

1. Update the canonical contract file first.
2. Preserve backward compatibility unless the task explicitly asks for a breaking change.
3. Regenerate clients/mocks/types that derive from the contract.
4. Record the contract artifact as a Maestro task output when downstream tasks need it.
5. Add or update tests that prove the new contract shape is consumed correctly.
6. Call out any migration or versioning risk in the final summary.
"#;

pub const SKILL_VERIFY_BEFORE_DONE: &str = r#"---
description: Finish a task only after a concrete local verification pass or a clear blocker
trigger: verify before done | test plan | acceptance | regression
---

# Verify Before Done

Use this before claiming a workflow task is complete.

Checklist:

1. Identify the narrowest command that proves this task's change works.
2. Run it from the correct project/workspace.
3. If it fails, fix the root cause or report the blocker with the exact failing output.
4. If no command is available, explain why and name the manual check a human should perform.
5. Do not mark success based only on code inspection.
"#;

pub const SKILL_QA_WEB_FLOW: &str = r#"---
description: Verify browser-visible web flows with a real headless browser and concrete assertions
trigger: qa | e2e | end-to-end | headless | browser flow | login flow | web flow | playwright
---

# QA Web Flow

Use this when a task claims a browser-visible workflow is complete.

Preconditions:

- Identify the frontend URL, required backend services, and the user path under test.
- Prefer the project's declared test command. If there is none, use Playwright or a real headless Chrome/Edge run.
- If browser automation tooling is missing, report that as a blocker or run the smallest real-browser fallback available; do not replace it with only `curl`.

Steps:

1. Start or verify every service the flow depends on.
2. Open the page in a real headless browser.
3. Drive the visible user path: navigate, fill inputs or click controls, and wait for async UI updates.
4. Assert the contract-facing result in the DOM and, when possible, the network response status/body shape.
5. Check for visible error states and console/network failures.
6. Capture evidence: command/tool used, URL, assertions, and any screenshot/DOM excerpt/log that proves the flow.

Outputs:

- Pass/fail status for each assertion.
- Exact commands/tools used for the headless run.
- Blockers or gaps if a true headless browser run was not possible.
"#;

pub const MEMORY_GIT_CONVENTIONS: &str = r#"# Repo conventions

- Commit messages: conventional commits (feat:, fix:, refactor:, ...)
- Branch prefix for features: feat/
- One concern per task, one PR per concern
- Never push to main directly; everything goes through a PR
"#;

pub const SKILL_MONOREPO_BAZEL_GO: &str = r#"---
description: Work reliably inside a Bazel + Go monorepo (gazelle, kitex, idl contracts)
trigger: bazel | gazelle | kitex | go monorepo | BUILD.bazel | bazel test | idl
---

# Bazel/Go monorepo conventions

Use this in a Go service monorepo driven by Bazel (modules listed in
`.monorepo_config.yaml`, one root `go.mod`, `BUILD.bazel` per package).

Rules:

1. Scope changes to your module's directory (e.g. `app/<service>/`). Don't edit
   other modules unless the task says so.
2. After changing Go imports, regenerate build files with gazelle (e.g.
   `bazel run //:gazelle`) instead of hand-editing `BUILD.bazel`.
3. RPC/data contracts live in `idl/` (Thrift/Kitex). Change the `.thrift` first,
   regenerate `kitex_gen`, then update callers. Treat a contract edit as a
   cross-service change — note downstream impact.
4. Verify with Bazel for your module: `bazel build //app/<service>/...` and, when
   tests exist, `bazel test //app/<service>/...`. Don't `go build` the whole repo.
5. Keep `.monorepo_config.yaml` accurate if you add/rename a module.
"#;

pub const SKILL_MONOREPO_RUSH: &str = r#"---
description: Work reliably inside a Rush + pnpm frontend monorepo (subspaces, workspace deps)
trigger: rush | rushx | subspace | pnpm monorepo | rush.json | monorepo frontend
---

# Rush/pnpm monorepo conventions

Use this in a Rush-managed frontend monorepo (projects registered in
`rush.json`, grouped into subspaces, deps via the workspace protocol).

Rules:

1. Scope changes to your project's `projectFolder`. Cross-project changes go
   through the workspace dependency, not relative imports into another project.
2. Add dependencies with `rush add -p <pkg>` from the project folder (updates the
   lockfile correctly); don't hand-edit pnpm-lock.
3. Build/verify via Rush so the dependency graph is respected:
   `rush build --to <packageName>` (and the project's own test script via
   `rushx test`). Avoid bare `npm install` at a project.
4. Respect subspace boundaries — a project's subspace (in `rush.json`) controls
   which dependency install group it belongs to.
5. After changing a shared library, rebuild its consumers with
   `rush build --from <library>`.
"#;

pub fn bundled_skill_samples() -> Vec<BundledSkill> {
    vec![
        BundledSkill {
            scope: "_global",
            name: "write-pr-description",
            content: SKILL_WRITE_PR_DESC,
        },
        BundledSkill {
            scope: "_global",
            name: "scope-the-task",
            content: SKILL_SCOPE_TASK,
        },
        BundledSkill {
            scope: "_global",
            name: "debug-flow",
            content: SKILL_DEBUG_FLOW,
        },
        BundledSkill {
            scope: "_global",
            name: "workflow-task-guardrails",
            content: SKILL_WORKFLOW_TASK_GUARDRAILS,
        },
        BundledSkill {
            scope: "_global",
            name: "contract-first",
            content: SKILL_CONTRACT_FIRST,
        },
        BundledSkill {
            scope: "_global",
            name: "verify-before-done",
            content: SKILL_VERIFY_BEFORE_DONE,
        },
        BundledSkill {
            scope: "_global",
            name: "qa-web-flow",
            content: SKILL_QA_WEB_FLOW,
        },
        BundledSkill {
            scope: "_global",
            name: "monorepo-bazel-go",
            content: SKILL_MONOREPO_BAZEL_GO,
        },
        BundledSkill {
            scope: "_global",
            name: "monorepo-rush",
            content: SKILL_MONOREPO_RUSH,
        },
    ]
}

pub fn init_skill_samples() -> Vec<(&'static str, &'static str, &'static str)> {
    bundled_skill_samples()
        .into_iter()
        .map(|s| (s.scope, s.name, s.content))
        .collect()
}

pub fn update_bundled_skills(
    name_filter: Option<&str>,
    force: bool,
) -> Result<Vec<SkillUpdateResult>> {
    let requested = name_filter.map(str::trim).filter(|s| !s.is_empty());
    let mut out = Vec::new();

    for sample in bundled_skill_samples() {
        if let Some(name) = requested {
            let scoped = format!("{}/{}", sample.scope, sample.name);
            if sample.name != name && scoped != name {
                continue;
            }
        }

        let scope = crate::skills::SkillScope::from_dir(sample.scope);
        let path = crate::skills::skill_path(&scope, sample.name)?;
        let status = if !path.exists() {
            crate::skills::save(&scope, sample.name, sample.content)?;
            SkillUpdateStatus::Added
        } else {
            let current = std::fs::read_to_string(&path).unwrap_or_default();
            if current == sample.content {
                SkillUpdateStatus::Unchanged
            } else if force {
                crate::skills::save(&scope, sample.name, sample.content)?;
                SkillUpdateStatus::Updated
            } else {
                SkillUpdateStatus::Skipped
            }
        };

        out.push(SkillUpdateResult {
            scope: sample.scope,
            name: sample.name,
            status,
            path,
        });
    }

    if out.is_empty() {
        if let Some(name) = requested {
            bail!("unknown bundled skill {name:?}");
        }
    }

    Ok(out)
}

pub fn init_memory_samples() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![("conventions", "git.md", MEMORY_GIT_CONVENTIONS)]
}
