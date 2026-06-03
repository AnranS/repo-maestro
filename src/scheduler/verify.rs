//! Goal-driven verification gate (Layer 4).
//!
//! After the DAG finishes (whether green or red), if the plan has a `goal`
//! block we run each acceptance check from the workspace root, capture exit
//! code + truncated output, and mutate the RunState in place. The caller is
//! responsible for persisting and for deciding whether to chain into a
//! replan attempt.
//!
//! This is intentionally simple: every check is a shell command, runs
//! sequentially (acceptance is usually I/O-bound and ordering matters for
//! human readability), and respects a per-check timeout that's generous
//! enough for `cargo test` / `pnpm test` but won't hang the process forever.

use std::path::Path;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::process::Command;

use crate::config::Goal;

use super::state::{AcceptanceResult, RunState, RunStatus};

/// Maximum wall-clock time per acceptance check. Generous: 10 minutes is
/// enough for a full integration test suite, short enough to avoid wedging
/// the scheduler forever on a broken check.
const PER_CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// Trim captured output to the last 4 KiB so the state file stays bounded.
const MAX_OUTPUT_BYTES: usize = 4096;

/// Run every acceptance check from `goal`, write results into `state`, and
/// flip `state.verified` based on the outcome. Does NOT call `write_atomic`;
/// the caller decides when to persist.
pub async fn run_acceptance(state: &mut RunState, goal: &Goal, workspace: &Path) {
    state.acceptance_results.clear();

    let dag_failed = matches!(state.status, RunStatus::Failed | RunStatus::Cancelled);

    for ac in &goal.acceptance {
        let started_at = Utc::now();
        let t0 = Instant::now();
        let result = exec_check(&ac.check, workspace).await;
        let ended_at = Utc::now();
        let _elapsed = t0.elapsed();

        let (passed, exit_code, output) = match result {
            Ok((code, out)) => (code == 0, Some(code), out),
            Err(e) => (false, None, format!("spawn error: {e}")),
        };

        state.acceptance_results.push(AcceptanceResult {
            describe: ac.describe.clone(),
            check: ac.check.clone(),
            passed,
            exit_code,
            output: truncate_output(&output),
            started_at,
            ended_at,
        });
    }

    let all_passed = state.acceptance_results.iter().all(|r| r.passed);
    state.verified = !dag_failed && all_passed;
}

async fn exec_check(cmd: &str, cwd: &Path) -> std::io::Result<(i32, String)> {
    let mut command = Command::new("bash");
    command
        .arg("-lc")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group so a timeout tears down the whole check subtree
    // (`cargo test` / `pnpm test` / dev servers it spawned), not just `bash`.
    crate::proc::isolate_process_group(&mut command);
    let child = command.spawn()?;
    let pid = child.id();

    let output_fut = child.wait_with_output();

    match tokio::time::timeout(PER_CHECK_TIMEOUT, output_fut).await {
        Ok(Ok(out)) => {
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&out.stdout));
            if !out.stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            Ok((out.status.code().unwrap_or(-1), combined))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            if let Some(pid) = pid {
                crate::proc::kill_process_group(pid);
            }
            Ok((
                124,
                format!(
                    "acceptance check timed out after {}s: {}",
                    PER_CHECK_TIMEOUT.as_secs(),
                    cmd
                ),
            ))
        }
    }
}

fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    let start = s.len() - MAX_OUTPUT_BYTES;
    // Don't slice mid-utf8: walk forward to a char boundary.
    let mut cut = start;
    while cut < s.len() && !s.is_char_boundary(cut) {
        cut += 1;
    }
    let mut out = String::with_capacity(MAX_OUTPUT_BYTES + 64);
    out.push_str("… (output truncated, showing last 4 KiB) …\n");
    out.push_str(&s[cut..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Acceptance, Plan};
    use crate::scheduler::state::RunStatus;
    use std::collections::BTreeMap;

    fn empty_plan() -> Plan {
        Plan {
            spec: "test".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: BTreeMap::new(),
            notice: None,
            goal: None,
        }
    }

    fn make_state() -> RunState {
        let plan = empty_plan();
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        RunState::new("test-run".into(), &plan, &projects, 1, std::env::temp_dir())
    }

    #[tokio::test]
    async fn passing_check_marks_verified() {
        let mut state = make_state();
        state.status = RunStatus::Done;
        let goal = Goal {
            description: "be green".into(),
            acceptance: vec![Acceptance {
                describe: "trivial true".into(),
                check: "true".into(),
            }],
        };
        run_acceptance(&mut state, &goal, &std::env::temp_dir()).await;
        assert_eq!(state.acceptance_results.len(), 1);
        assert!(state.acceptance_results[0].passed);
        assert!(state.verified);
    }

    #[tokio::test]
    async fn failing_check_blocks_verification() {
        let mut state = make_state();
        state.status = RunStatus::Done;
        let goal = Goal {
            description: "should fail".into(),
            acceptance: vec![
                Acceptance {
                    describe: "always true".into(),
                    check: "true".into(),
                },
                Acceptance {
                    describe: "always false".into(),
                    check: "false".into(),
                },
            ],
        };
        run_acceptance(&mut state, &goal, &std::env::temp_dir()).await;
        assert_eq!(state.acceptance_results.len(), 2);
        assert!(state.acceptance_results[0].passed);
        assert!(!state.acceptance_results[1].passed);
        assert!(!state.verified);
    }

    #[tokio::test]
    async fn dag_failure_blocks_verified_flag() {
        let mut state = make_state();
        state.status = RunStatus::Failed;
        let goal = Goal {
            description: "dag failed".into(),
            acceptance: vec![Acceptance {
                describe: "trivial true".into(),
                check: "true".into(),
            }],
        };
        run_acceptance(&mut state, &goal, &std::env::temp_dir()).await;
        assert!(state.acceptance_results[0].passed);
        // verified flag still false because the DAG itself failed
        assert!(!state.verified);
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let mut state = make_state();
        state.status = RunStatus::Done;
        let goal = Goal {
            description: "with output".into(),
            acceptance: vec![Acceptance {
                describe: "echo and exit 7".into(),
                check: "echo hello-from-check; exit 7".into(),
            }],
        };
        run_acceptance(&mut state, &goal, &std::env::temp_dir()).await;
        let r = &state.acceptance_results[0];
        assert!(!r.passed);
        assert_eq!(r.exit_code, Some(7));
        assert!(r.output.contains("hello-from-check"));
    }

    #[test]
    fn truncate_keeps_tail() {
        let big = "a".repeat(10_000);
        let truncated = truncate_output(&big);
        assert!(truncated.starts_with("… (output truncated"));
        assert!(truncated.len() < big.len());
        assert!(truncated.ends_with('a'));
    }
}
