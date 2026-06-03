//! Post-run artifact writers:
//!
//! - `write_run_report`  — human-readable markdown report next to the run dir
//!   AND a copy under `<workspace>/plans/<date>-<spec>.report.md`.
//! - `archive_l2_decision` — for each project touched by a Done run, write a
//!   decision-record markdown into `.maestro/memory/l2_decisions/<project>/`.
//!   These appear automatically in chat's memory injection on later runs.

use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::Plan;
use crate::paths;
use crate::scheduler::RunState;

pub fn write_run_report(plan: &Plan, state: &RunState) -> Result<()> {
    let body = render_report(plan, state);

    // Always next to the run dir as REPORT.md.
    let primary = state.run_dir.join(paths::RUN_REPORT_FILE);
    std::fs::write(&primary, &body).with_context(|| format!("write run report {:?}", primary))?;

    // Mirror to <workspace>/plans/ for human discoverability.
    let plans_dir = paths::workspace_root()?.join("plans");
    paths::ensure_dir(&plans_dir)?;
    let date = state.started_at.format("%Y%m%d-%H%M%S");
    let slug = slugify(&state.spec);
    let mirror = plans_dir.join(format!("{date}-{slug}.report.md"));
    std::fs::write(&mirror, &body).with_context(|| format!("write mirror report {:?}", mirror))?;

    tracing::info!(
        "run report written to {} and {}",
        primary.display(),
        mirror.display()
    );
    Ok(())
}

pub fn archive_l2_decision(plan: &Plan, state: &RunState) -> Result<()> {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let slug = slugify(&state.spec);

    // Group tasks by project; verify/_global tasks don't get their own scope.
    let mut by_project: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in &plan.tasks {
        if t.project.is_empty() || t.project == "_global" {
            continue;
        }
        by_project.entry(&t.project).or_default().push(&t.id);
    }
    if by_project.is_empty() {
        return Ok(());
    }

    let l2_root = paths::maestro_dir()?.join("memory").join("l2_decisions");
    paths::ensure_dir(&l2_root)?;

    let header = format!(
        "---\nrun_id: {run_id}\nspec: {spec:?}\nstarted_at: {started}\nstatus: {status:?}\n---\n\n",
        run_id = state.run_id,
        spec = state.spec,
        started = state.started_at,
        status = state.status,
    );

    for (project, task_ids) in &by_project {
        let dir = l2_root.join(project);
        paths::ensure_dir(&dir)?;
        let path = dir.join(format!(
            "{date}-{slug}-{}.md",
            &state.run_id[..15.min(state.run_id.len())]
        ));

        let mut body = String::new();
        body.push_str(&header);
        body.push_str(&format!("# Decision · {}\n\n", state.spec));
        body.push_str(&format!(
            "Captured automatically by maestro at the end of run `{}`.\n\n",
            state.run_id
        ));

        body.push_str("## Tasks in this project\n\n");
        for tid in task_ids {
            if let Some(t) = plan.task(tid) {
                let task_state = state.tasks.get(*tid);
                let status = task_state
                    .map(|ts| format!("{:?}", ts.status))
                    .unwrap_or_else(|| "?".into());
                body.push_str(&format!("### `{}` — {}\n", tid, status));
                if !t.prompt.trim().is_empty() {
                    body.push_str("Prompt:\n\n```text\n");
                    body.push_str(t.prompt.trim());
                    body.push_str("\n```\n\n");
                }
                if let Some(cmd) = &t.command {
                    body.push_str("Command:\n\n```bash\n");
                    body.push_str(cmd.trim());
                    body.push_str("\n```\n\n");
                }
            }
        }

        body.push_str("## What to remember next time\n\n");
        body.push_str(
            "_(Optional human edit) Capture the decision rationale here so future plans can avoid re-doing the analysis._\n",
        );

        std::fs::write(&path, body).with_context(|| format!("write l2 decision {:?}", path))?;
    }

    // Cap each touched project's archive so L2 doesn't grow without bound.
    if let Ok(store) = crate::memory::MemoryStore::open() {
        for project in by_project.keys() {
            match store.prune_l2(project, crate::memory::L2_MAX_PER_PROJECT) {
                Ok(n) if n > 0 => {
                    tracing::debug!("pruned {n} old L2 decision(s) for project {project}")
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn render_report(plan: &Plan, state: &RunState) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", state.spec));
    s.push_str(&format!("- **run id**: `{}`\n", state.run_id));
    if let Some(sid) = &state.session_id {
        s.push_str(&format!("- **chat session**: `{}`\n", sid));
    }
    s.push_str(&format!("- **status**: `{:?}`\n", state.status));
    if let Some((p, n)) = state.acceptance_summary() {
        let verdict = if state.verified {
            "✅ verified"
        } else if p == n {
            "✅ checks passed (DAG had failures)"
        } else {
            "❌ acceptance failed"
        };
        s.push_str(&format!("- **verification**: {p}/{n} — {verdict}\n"));
    } else if state.goal.is_some() {
        s.push_str("- **verification**: (no acceptance checks defined)\n");
    }
    s.push_str(&format!("- **started**: {}\n", state.started_at));
    if let Some(end) = state.ended_at {
        s.push_str(&format!("- **ended**: {}\n", end));
        let secs = (end - state.started_at).num_seconds();
        s.push_str(&format!("- **duration**: {}s\n", secs));
    }
    let evidence_path = state.run_dir.join("evidence").join("summary.json");
    if evidence_path.exists() {
        s.push_str("- **evidence**: `evidence/summary.json`\n");
    }
    if state.run_dir.join("PR_BODY.md").exists() {
        s.push_str("- **PR draft**: `PR_BODY.md`\n");
    }
    s.push('\n');

    // L1 goal block + L4 acceptance results
    if let Some(goal) = &state.goal {
        s.push_str("## Goal\n\n");
        if !goal.description.is_empty() {
            s.push_str(&format!("> {}\n\n", goal.description));
        }
        if !state.acceptance_results.is_empty() {
            s.push_str("### Acceptance\n\n");
            for r in &state.acceptance_results {
                let icon = if r.passed { "✅" } else { "❌" };
                let code = r
                    .exit_code
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "spawn error".into());
                s.push_str(&format!("- {icon} **{}** — _{}_\n", r.describe, code));
                s.push_str(&format!("  - `{}`\n", r.check));
                if !r.passed && !r.output.is_empty() {
                    s.push_str("  - <details><summary>output</summary>\n\n");
                    s.push_str("    ```\n");
                    for line in r.output.lines() {
                        s.push_str("    ");
                        s.push_str(line);
                        s.push('\n');
                    }
                    s.push_str("    ```\n  </details>\n");
                }
            }
            s.push('\n');
        } else if !goal.acceptance.is_empty() {
            s.push_str("### Acceptance\n\n");
            s.push_str("_(acceptance gate not run — DAG was cancelled before verification)_\n\n");
        }
    }

    // Summary counts
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in state.tasks.values() {
        *counts
            .entry(format!("{:?}", t.status).to_lowercase())
            .or_insert(0) += 1;
    }
    s.push_str("## Task counts\n\n");
    for (k, v) in &counts {
        s.push_str(&format!("- {k}: **{v}**\n"));
    }
    s.push('\n');

    // What maestro decided on the user's behalf (contract wiring, retries,
    // circuit-breaker trips, integration conflicts) — grouped by kind.
    if !state.auto_actions.is_empty() {
        s.push_str("## Automatic actions\n\n");
        s.push_str("_Decisions maestro made for you during this run:_\n\n");
        let mut by_kind: BTreeMap<&str, Vec<&crate::scheduler::state::AutoAction>> =
            BTreeMap::new();
        for a in &state.auto_actions {
            by_kind.entry(a.kind.as_str()).or_default().push(a);
        }
        for (kind, items) in &by_kind {
            let label = match *kind {
                "contract_wired" => "🔗 Contract dependencies wired",
                "retry" => "🔁 Retries",
                "circuit_break" => "⛔ Circuit breaker",
                "integration_conflict" => "⚠️ Integration conflicts",
                other => other,
            };
            s.push_str(&format!("- **{label}** ({})\n", items.len()));
            for a in items {
                match &a.task {
                    Some(t) => s.push_str(&format!("  - `{t}`: {}\n", a.detail)),
                    None => s.push_str(&format!("  - {}\n", a.detail)),
                }
            }
        }
        s.push('\n');
    }

    // Per-task narrative
    s.push_str("## Tasks (in order)\n\n");
    for id in &state.task_order {
        let Some(ts) = state.tasks.get(id) else {
            continue;
        };
        s.push_str(&format!(
            "### `{}` · {}\n\n",
            ts.id,
            format!("{:?}", ts.status).to_lowercase()
        ));
        s.push_str(&format!(
            "- project: `{}`  · agent: `{}`  · kind: `{}`\n",
            ts.project, ts.agent, ts.kind
        ));
        if !ts.depends_on.is_empty() {
            s.push_str(&format!(
                "- depends on: {}\n",
                ts.depends_on
                    .iter()
                    .map(|d| format!("`{}`", d))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let (Some(start), Some(end)) = (&ts.started_at, &ts.ended_at) {
            let ms = (*end - *start).num_milliseconds().max(0);
            s.push_str(&format!("- duration: {ms}ms\n"));
        }
        if let Some(path) = &ts.workspace_path {
            s.push_str(&format!("- workspace: `{path}`\n"));
        }
        if let Some(path) = &ts.worktree_path {
            s.push_str(&format!("- worktree: `{path}`\n"));
        }
        if let Some(err) = &ts.error {
            s.push_str(&format!("- **error**: `{}`\n", err));
        }
        if let Some(pr) = &ts.artifacts.pr_url {
            s.push_str(&format!("- PR: {}\n", pr));
        }
        if let Some(plan_task) = plan.task(&ts.id) {
            if !plan_task.prompt.trim().is_empty() {
                s.push_str("\n```text\n");
                s.push_str(plan_task.prompt.trim());
                s.push_str("\n```\n");
            }
            if let Some(cmd) = &plan_task.command {
                s.push_str("\n```bash\n");
                s.push_str(cmd.trim());
                s.push_str("\n```\n");
            }
        }
        s.push('\n');
    }

    s.push_str("---\n_Generated by maestro._\n");
    s
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in s.chars().take(80) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "run".into()
    } else {
        trimmed
    }
}

/// One-line summary suitable for the `maestro runs` listing.
pub fn run_summary(state: &RunState) -> String {
    let dur = state
        .ended_at
        .map(|e| (e - state.started_at).num_seconds())
        .unwrap_or(0);
    let session = match &state.session_id {
        Some(s) => format!(" [session {}]", &s[..s.len().min(8)]),
        None => String::new(),
    };
    format!("{:?}  {:>5}s  {}{}", state.status, dur, state.spec, session)
}

#[allow(unused)]
pub fn dummy_path() -> PathBuf {
    PathBuf::new()
}
