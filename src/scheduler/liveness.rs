use super::state::{RunState, RunStatus, TaskStatus};
use anyhow::Result;
use chrono::Utc;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLiveness {
    Live,
    Abandoned,
    UnknownLegacy,
}

pub fn classify_run(state: &RunState) -> RunLiveness {
    classify_pid(state.pid)
}

/// If the run at `run_dir` is "running" on paper but its owner process is
/// dead, force the state into Cancelled and mark every still-running /
/// awaiting-approval task the same way. Idempotent: repeated calls on an
/// already-terminal run are a no-op.
///
/// Why: writing a cancel marker only does anything when a live scheduler
/// is polling for it. A stale run (owner crashed / kill -9'd / machine
/// rebooted) sits in "running" forever and the UI's cancel button just
/// silently fails. This is the safety net for that case.
///
/// Returns `Ok(true)` iff state was rewritten, `Ok(false)` if the run was
/// either already terminal or still owned by a live process.
pub fn force_cancel_if_abandoned(run_dir: &Path) -> Result<bool> {
    let mut state = RunState::load(run_dir)?;
    if !matches!(state.status, RunStatus::Running) {
        return Ok(false);
    }
    // UnknownLegacy (pid == 0) means the state file predates pid tracking,
    // so there's no liveness signal at all. The non-cancel callers (e.g.
    // `maestro run` previous-run guard) play it safe and leave those
    // alone, but for an explicit user-driven cancel we treat UnknownLegacy
    // as abandoned too — the user wouldn't be hitting "cancel" if the run
    // was supposed to be alive, and the alternative is a UI that's wedged
    // on a state file from before pid tracking was added.
    match classify_run(&state) {
        RunLiveness::Live => return Ok(false),
        RunLiveness::UnknownLegacy | RunLiveness::Abandoned => {}
    }
    let now = Utc::now();
    state.status = RunStatus::Cancelled;
    if state.ended_at.is_none() {
        state.ended_at = Some(now);
    }
    for task in state.tasks.values_mut() {
        if matches!(
            task.status,
            TaskStatus::Running | TaskStatus::AwaitingApproval | TaskStatus::Pending
        ) {
            task.status = TaskStatus::Cancelled;
            if task.ended_at.is_none() {
                task.ended_at = Some(now);
            }
        }
    }
    state.write_atomic()?;
    // Record the abandoned run to the per-run finding ledger (F-110). Runs only
    // on the reconcile that actually force-cancels (idempotent above), so a
    // dead run yields one `doctor` finding, not one per poll. Best-effort: a
    // ledger write must never undo the cancel we just persisted.
    let finding = super::findings::Finding::new(
        &state.run_id,
        super::findings::FindingKind::Doctor,
        super::findings::Severity::Medium,
        "liveness",
        "run abandoned (owner process not alive); force-cancelled",
        now.to_rfc3339(),
    );
    if let Err(e) = super::findings::append_finding(run_dir, finding) {
        tracing::warn!("could not append abandoned-run finding: {e:#}");
    }
    Ok(true)
}

pub fn classify_pid(pid: u32) -> RunLiveness {
    if pid == 0 {
        return RunLiveness::UnknownLegacy;
    }

    // SAFETY: kill(pid, 0) does not deliver a signal. It only asks the kernel
    // whether a process with this pid exists and is visible to us.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return RunLiveness::Live;
    }

    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EPERM) => RunLiveness::Live,
        Some(libc::ESRCH) => RunLiveness::Abandoned,
        _ => RunLiveness::Abandoned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Artifacts;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn pid_zero_is_unknown_legacy() {
        assert_eq!(classify_pid(0), RunLiveness::UnknownLegacy);
    }

    #[test]
    fn current_process_pid_is_live() {
        assert_eq!(classify_pid(std::process::id()), RunLiveness::Live);
    }

    #[test]
    fn unlikely_high_pid_is_abandoned() {
        assert_eq!(classify_pid(999_999), RunLiveness::Abandoned);
    }

    /// Helper: build a RunState with one running task, save it to a temp
    /// run-dir, return the dir + the state so tests can mutate-and-verify.
    fn make_running_run(dir: &std::path::Path, pid: u32) -> RunState {
        let mut state = RunState {
            run_id: "r-test".into(),
            spec: "demo".into(),
            started_at: Utc::now(),
            ended_at: None,
            status: RunStatus::Running,
            max_parallel: 1,
            pid,
            tasks: BTreeMap::new(),
            approvals_pending: vec![],
            task_order: vec!["T_x".into()],
            session_id: None,
            usage: Default::default(),
            budget_tokens: None,
            pending_gate: None,
            goal: None,
            acceptance_results: vec![],
            verified: false,
            auto_actions: vec![],
            run_dir: dir.to_path_buf(),
        };
        state.tasks.insert(
            "T_x".into(),
            crate::scheduler::state::TaskState {
                id: "T_x".into(),
                project: "demo".into(),
                agent: "shell".into(),
                status: TaskStatus::Running,
                started_at: Some(Utc::now()),
                ended_at: None,
                chat_id: None,
                error: None,
                attempts: 0,
                risk_level: None,
                artifacts: Artifacts::default(),
                permission: None,
                workflow_outputs: BTreeMap::new(),
                log_path: "T_x.log".into(),
                trajectory_path: None,
                depends_on: vec![],
                parallel_group: None,
                requires_approval_after: false,
                kind: "agent".into(),
                memory_used: vec![],
                context_bytes: None,
                skills_triggered: vec![],
                usage: None,
                steps: None,
                role: None,
                workspace_path: None,
                worktree_path: None,
            },
        );
        state.write_atomic().unwrap();
        state
    }

    #[test]
    fn force_cancel_marks_abandoned_run_and_running_tasks() {
        let tmp = tempdir().unwrap();
        make_running_run(tmp.path(), 999_999); // dead pid

        let updated = force_cancel_if_abandoned(tmp.path()).unwrap();
        assert!(updated, "should have force-cancelled an abandoned run");

        let reloaded = RunState::load(tmp.path()).unwrap();
        assert_eq!(reloaded.status, RunStatus::Cancelled);
        assert!(reloaded.ended_at.is_some(), "ended_at must be stamped");
        let t = reloaded.tasks.get("T_x").unwrap();
        assert_eq!(t.status, TaskStatus::Cancelled);
        assert!(t.ended_at.is_some(), "task ended_at must be stamped");
    }

    #[test]
    fn force_cancel_writes_a_doctor_finding_to_the_ledger() {
        use crate::scheduler::findings::{read_findings, FindingKind, FindingStatus};
        let tmp = tempdir().unwrap();
        make_running_run(tmp.path(), 999_999); // dead pid

        assert!(force_cancel_if_abandoned(tmp.path()).unwrap());

        // The producer wrote a finding the ledger can parse back (F-110 Step 2
        // integration path).
        let findings = read_findings(tmp.path()).unwrap();
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.kind, FindingKind::Doctor);
        assert_eq!(f.finding_id, "doctor-1");
        assert_eq!(f.run_id, "r-test");
        assert_eq!(f.status, FindingStatus::Open);
        assert!(f.summary.contains("abandoned"));

        // Idempotent: a second reconcile is a no-op and writes no new finding.
        assert!(!force_cancel_if_abandoned(tmp.path()).unwrap());
        assert_eq!(read_findings(tmp.path()).unwrap().len(), 1);
    }

    #[test]
    fn force_cancel_leaves_live_run_alone() {
        let tmp = tempdir().unwrap();
        make_running_run(tmp.path(), std::process::id()); // our own pid is alive

        let updated = force_cancel_if_abandoned(tmp.path()).unwrap();
        assert!(
            !updated,
            "must NOT touch a run still owned by a live process"
        );

        let reloaded = RunState::load(tmp.path()).unwrap();
        assert_eq!(
            reloaded.status,
            RunStatus::Running,
            "live run stays Running"
        );
    }

    #[test]
    fn force_cancel_clears_legacy_unknown_pid_runs() {
        // When the user explicitly hits cancel on a state file too old to
        // carry a pid, we'd rather force the state to Cancelled than leave
        // the UI wedged with no signal. (Non-cancel callers should use
        // `classify_run` directly to play it safe.)
        let tmp = tempdir().unwrap();
        make_running_run(tmp.path(), 0); // legacy state file with no pid

        let updated = force_cancel_if_abandoned(tmp.path()).unwrap();
        assert!(
            updated,
            "user-driven cancel must also clear unknown-pid runs"
        );
        let reloaded = RunState::load(tmp.path()).unwrap();
        assert_eq!(reloaded.status, RunStatus::Cancelled);
    }

    #[test]
    fn force_cancel_is_idempotent_on_terminal_run() {
        let tmp = tempdir().unwrap();
        let mut s = make_running_run(tmp.path(), 999_999);
        s.status = RunStatus::Done;
        s.write_atomic().unwrap();

        let updated = force_cancel_if_abandoned(tmp.path()).unwrap();
        assert!(!updated, "already-terminal run is a no-op");

        let reloaded = RunState::load(tmp.path()).unwrap();
        assert_eq!(reloaded.status, RunStatus::Done);
    }
}
