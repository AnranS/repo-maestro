//! Replan prompt generator (L4 follow-through).
//!
//! When the acceptance gate fails, we don't auto-loop into another DAG run
//! — that's a great way to torch tokens and confuse the user. Instead we
//! materialise a structured "fix these failures" prompt the user can hand
//! to the planner agent (`maestro plan create -i REPLAN.md` style).
//!
//! The prompt is intentionally:
//!   - small (fits in a single chat turn)
//!   - explicit about what passed AND what failed (planner needs both)
//!   - free of speculation about root cause — we don't have the diff,
//!     only the shell output. Diagnosis is the planner's job.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Plan;

use super::state::RunState;

/// Generate a replan prompt and write it as `REPLAN.md` next to the run.
/// Returns the path written, or `Ok(None)` if no replan is warranted
/// (no goal, no failures, or run was cancelled).
pub fn write_replan_prompt(
    run_dir: &Path,
    plan: &Plan,
    state: &RunState,
) -> Result<Option<std::path::PathBuf>> {
    let failing: Vec<_> = state
        .acceptance_results
        .iter()
        .filter(|r| !r.passed)
        .collect();
    if failing.is_empty() {
        return Ok(None);
    }
    let Some(goal) = &state.goal else {
        return Ok(None);
    };

    let mut s = String::new();
    s.push_str("# Replan request\n\n");
    s.push_str(&format!("Original spec: **{}**\n\n", plan.spec));
    if !goal.description.is_empty() {
        s.push_str(&format!("Goal: {}\n\n", goal.description));
    }
    s.push_str(&format!(
        "The DAG ran (status: `{:?}`) but {} of {} acceptance checks failed.\n\n",
        state.status,
        failing.len(),
        state.acceptance_results.len()
    ));

    s.push_str("## Failing checks\n\n");
    for r in &failing {
        s.push_str(&format!("### ❌ {}\n\n", r.describe));
        s.push_str(&format!("```sh\n{}\n```\n\n", r.check));
        let code = r
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "(spawn error)".into());
        s.push_str(&format!("Exit code: `{code}`\n\n"));
        if !r.output.is_empty() {
            s.push_str("Output:\n\n```\n");
            // Cap to ~2 KiB in the replan prompt — full output is in REPORT.md.
            const CAP: usize = 2048;
            if r.output.len() > CAP {
                let mut cut = r.output.len() - CAP;
                while cut < r.output.len() && !r.output.is_char_boundary(cut) {
                    cut += 1;
                }
                s.push_str("… (truncated) …\n");
                s.push_str(&r.output[cut..]);
            } else {
                s.push_str(&r.output);
            }
            if !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str("```\n\n");
        }
    }

    // Passing checks: planner needs to know what's already working so it
    // doesn't accidentally regress them.
    let passing: Vec<_> = state
        .acceptance_results
        .iter()
        .filter(|r| r.passed)
        .collect();
    if !passing.is_empty() {
        s.push_str("## Already passing (do not regress)\n\n");
        for r in &passing {
            s.push_str(&format!("- ✅ {}\n", r.describe));
        }
        s.push('\n');
    }

    s.push_str("## Instructions for the planner\n\n");
    s.push_str(
        "Produce a NEW `PLAN.yaml` whose goal is the same as above. Its tasks should fix only what's broken — keep the passing checks in the acceptance block so we keep verifying them. Reuse task ids from the previous plan when the work is genuinely the same; otherwise pick fresh ids.\n\n",
    );
    s.push_str(&format!(
        "Previous run id: `{}` (see `{}` for full task logs).\n",
        state.run_id,
        run_dir.display()
    ));

    let path = run_dir.join("REPLAN.md");
    std::fs::write(&path, s).with_context(|| format!("write replan prompt {:?}", path))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Acceptance, Goal, Plan};
    use crate::scheduler::state::{AcceptanceResult, RunStatus};
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn make_plan() -> Plan {
        Plan {
            spec: "ship login".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: BTreeMap::new(),
            notice: None,
            goal: Some(Goal {
                description: "users can log in with email/password".into(),
                acceptance: vec![Acceptance {
                    describe: "POST /login returns 200".into(),
                    check: "curl -fsS localhost:8000/login".into(),
                }],
            }),
        }
    }

    fn make_state(plan: &Plan) -> RunState {
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        let mut s = RunState::new(
            "run-test-001".into(),
            plan,
            &projects,
            1,
            std::env::temp_dir(),
        );
        s.status = RunStatus::Done;
        s
    }

    #[test]
    fn no_failures_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = make_plan();
        let mut state = make_state(&plan);
        state.acceptance_results.push(AcceptanceResult {
            describe: "POST /login returns 200".into(),
            check: "curl -fsS localhost:8000/login".into(),
            passed: true,
            exit_code: Some(0),
            output: "".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
        });
        let written = write_replan_prompt(tmp.path(), &plan, &state).unwrap();
        assert!(written.is_none());
        assert!(!tmp.path().join("REPLAN.md").exists());
    }

    #[test]
    fn writes_prompt_with_failure_context() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = make_plan();
        let mut state = make_state(&plan);
        state.acceptance_results.push(AcceptanceResult {
            describe: "POST /login returns 200".into(),
            check: "curl -fsS localhost:8000/login".into(),
            passed: false,
            exit_code: Some(7),
            output: "curl: (7) Failed to connect to localhost port 8000".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
        });
        let written = write_replan_prompt(tmp.path(), &plan, &state)
            .unwrap()
            .unwrap();
        let body = std::fs::read_to_string(&written).unwrap();
        assert!(body.contains("ship login"));
        assert!(body.contains("users can log in"));
        assert!(body.contains("POST /login returns 200"));
        assert!(body.contains("curl: (7)"));
        assert!(body.contains("Exit code: `7`"));
    }
}
