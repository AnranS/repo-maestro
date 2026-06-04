//! F-112 — run monitor projection.
//!
//! A read-only, stable projection of `RUN_STATE.json` + the F-110 finding
//! ledger into two contracts that monitor surfaces (WebUI / TUI / future MCP)
//! can consume without scraping `RunState` internals or coupling to scheduler
//! types: [`RunMonitor`] (run-level triage) and [`TaskDetail`] (task-level).
//!
//! Strictly pure + read-only: the builders take borrowed state + findings and
//! return owned projections — no filesystem, no clock (the projection time is
//! passed in), no state mutation, no event/finding writes. They NEVER emit an
//! absolute path (`workspace_path` / `worktree_path` and raw `log_path` are
//! omitted; artifact refs are derived run-relative from the task id).
//!
//! Design: `docs/experience/F-112-RUN-MONITOR-PROJECTION-DESIGN.md`.

use crate::adapter::Usage;
use crate::scheduler::findings::Finding;
use crate::scheduler::state::{RunState, RunStatus, TaskState, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RUN_MONITOR_V1: &str = "maestro.run_monitor.v1";
pub const TASK_DETAIL_V1: &str = "maestro.task_detail.v1";

/// The canonical serde string for a status/kind/severity enum, so a projection
/// consumer sees the same token as everywhere else (no hand-maintained mapping
/// that could drift from the enum's `serde` attribute).
fn serde_str<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn is_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
    )
}

// ── run-level ───────────────────────────────────────────────────────────────

/// Run-level triage projection (`maestro.run_monitor.v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMonitor {
    pub schema_version: String,
    pub run_id: String,
    pub status: String,
    pub spec: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub progress: RunProgress,
    pub active_tasks: Vec<MonitorTaskRef>,
    pub blocked_tasks: Vec<MonitorTaskRef>,
    pub approvals_pending: Vec<MonitorTaskRef>,
    pub findings_summary: Vec<FindingSummary>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    /// Projection time (RFC3339). Optional so deterministic tests can omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Per-status task counts. Every `TaskStatus` has its own bucket so the buckets
/// always sum to `total` (a consumer can render a self-consistent progress bar).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
    pub running: u32,
    pub pending: u32,
    pub awaiting_approval: u32,
    pub skipped: u32,
    pub cancelled: u32,
    /// Terminal outcomes: done + failed + cancelled + skipped. `awaiting_approval`
    /// is NOT settled — it's waiting on a human.
    pub settled: u32,
}

impl RunProgress {
    /// Per-status task counts for a run. Public so non-HTTP surfaces (e.g. the
    /// TUI) can reuse the same projection instead of re-implementing the tally.
    pub fn from_state(state: &RunState) -> Self {
        let mut p = RunProgress {
            total: state.tasks.len() as u32,
            ..Default::default()
        };
        for t in state.tasks.values() {
            match t.status {
                TaskStatus::Done => p.done += 1,
                TaskStatus::Failed => p.failed += 1,
                TaskStatus::Running => p.running += 1,
                TaskStatus::Pending => p.pending += 1,
                TaskStatus::AwaitingApproval => p.awaiting_approval += 1,
                TaskStatus::Skipped => p.skipped += 1,
                TaskStatus::Cancelled => p.cancelled += 1,
            }
        }
        p.settled = p.done + p.failed + p.cancelled + p.skipped;
        p
    }
}

/// A compact task reference for run-level lists (active / blocked / approvals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorTaskRef {
    pub task_id: String,
    pub project: String,
    pub status: String,
    pub kind: String,
    /// A short title if state carries one; `RunState` does not store the prompt,
    /// so this is `None` in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_agent_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_review_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
}

impl MonitorTaskRef {
    fn from_task(t: &TaskState) -> Self {
        MonitorTaskRef {
            task_id: t.id.clone(),
            project: t.project.clone(),
            status: serde_str(&t.status),
            kind: t.kind.clone(),
            title: None,
            resolved_agent_profile: t.resolved_agent_profile.clone(),
            resolved_review_profile: t.resolved_review_profile.clone(),
            risk_level: t.risk_level.clone(),
        }
    }
}

/// Count of findings by `(kind, severity)` — triage badges only, no rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingSummary {
    pub kind: String,
    pub severity: String,
    pub count: u32,
}

/// Deterministic `(kind, severity)` grouping, sorted by kind then severity.
fn findings_summary(findings: &[Finding]) -> Vec<FindingSummary> {
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for f in findings {
        let key = (serde_str(&f.kind), serde_str(&f.severity));
        *counts.entry(key).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((kind, severity), count)| FindingSummary {
            kind,
            severity,
            count,
        })
        .collect()
}

/// Is a `pending` task blocked? A dependency failed/cancelled, or — once the run
/// itself is terminal — a dependency never finished. Wall-clock staleness is NOT
/// a blocked signal in v1 (that's doctor/liveness territory).
fn is_blocked(t: &TaskState, state: &RunState, run_terminal: bool) -> bool {
    if t.status != TaskStatus::Pending {
        return false;
    }
    let dep_status = |id: &str| state.tasks.get(id).map(|d| d.status);
    let mut has_dep = false;
    for dep in &t.depends_on {
        let Some(s) = dep_status(dep) else { continue };
        has_dep = true;
        if matches!(s, TaskStatus::Failed | TaskStatus::Cancelled) {
            return true;
        }
    }
    run_terminal
        && has_dep
        && t.depends_on
            .iter()
            .filter_map(|id| dep_status(id))
            .any(|s| s != TaskStatus::Done)
}

impl RunMonitor {
    /// Project a run + its findings. Pure: `updated_at` is supplied (the caller
    /// stamps the clock; tests pass `None`).
    pub fn from_state_and_findings(
        state: &RunState,
        findings: &[Finding],
        updated_at: Option<String>,
    ) -> Self {
        let run_terminal = is_terminal(state.status);

        // Iterate in `task_order` when available (stable, plan order), else the
        // BTreeMap's key order — both deterministic.
        let ordered: Vec<&TaskState> = if state.task_order.is_empty() {
            state.tasks.values().collect()
        } else {
            state
                .task_order
                .iter()
                .filter_map(|id| state.tasks.get(id))
                .collect()
        };

        let active_tasks = ordered
            .iter()
            .filter(|t| t.status == TaskStatus::Running)
            .map(|t| MonitorTaskRef::from_task(t))
            .collect();
        let blocked_tasks = ordered
            .iter()
            .filter(|t| is_blocked(t, state, run_terminal))
            .map(|t| MonitorTaskRef::from_task(t))
            .collect();
        // Source of truth for approvals is `approvals_pending`; project each id
        // that still maps to a task (missing ids are ignored).
        let approvals_pending = state
            .approvals_pending
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .map(MonitorTaskRef::from_task)
            .collect();

        RunMonitor {
            schema_version: RUN_MONITOR_V1.to_string(),
            run_id: state.run_id.clone(),
            status: serde_str(&state.status),
            spec: state.spec.clone(),
            started_at: state.started_at.to_rfc3339(),
            ended_at: state.ended_at.map(|t| t.to_rfc3339()),
            progress: RunProgress::from_state(state),
            active_tasks,
            blocked_tasks,
            approvals_pending,
            findings_summary: findings_summary(findings),
            usage: state.usage.clone(),
            budget_tokens: state.budget_tokens,
            updated_at,
        }
    }
}

// ── task-level ──────────────────────────────────────────────────────────────

/// Coarse approval state derivable from `RunState` alone. v1 only ever projects
/// `Pending` (approved/rejected live in markers/events, which a pure
/// state-only projection deliberately does not read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskApprovalState {
    Pending,
    Approved,
    Rejected,
}

/// A link description for a task artifact — never file contents, never an
/// absolute path. `path` is a run-relative ref (or `None` for an
/// endpoint-served artifact like a computed diff).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskArtifactSummary {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub available: bool,
}

/// Task-level detail projection (`maestro.task_detail.v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub schema_version: String,
    pub run_id: String,
    pub task_id: String,
    pub project: String,
    pub status: String,
    pub kind: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_agent_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_review_profile: Option<String>,
    pub depends_on: Vec<String>,
    /// Tasks that depend on this one (computed from the run's tasks).
    pub downstream: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<TaskApprovalState>,
    pub artifacts: Vec<TaskArtifactSummary>,
    /// F-110 findings scoped to this task id.
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Run-relative artifact refs derived from the task id (never the stored
/// absolute `log_path` / `trajectory_path`). The monitor reads historical
/// `RUN_STATE.json`, which a legacy/corrupt run could have written with a
/// path-unsafe task id — so any ref we'd build from `t.id` is omitted entirely
/// (never sanitized into a different id pointing at a non-existent file) when
/// the id isn't a single safe path component.
fn task_artifacts(t: &TaskState) -> Vec<TaskArtifactSummary> {
    if crate::paths::validate_path_component("task id", &t.id).is_err() {
        return vec![];
    }
    let mut out = vec![];
    if !t.log_path.trim().is_empty() {
        out.push(TaskArtifactSummary {
            kind: "log".to_string(),
            path: Some(format!("logs/{}.log", t.id)),
            available: true,
        });
    }
    if t.trajectory_path.is_some() {
        out.push(TaskArtifactSummary {
            kind: "trajectory".to_string(),
            path: Some(format!("trajectories/{}.ndjson", t.id)),
            available: true,
        });
    }
    if !t.artifacts.files_changed.is_empty() {
        // served by `/api/runs/:id/tasks/:task/diff`, not a stored file.
        out.push(TaskArtifactSummary {
            kind: "diff".to_string(),
            path: None,
            available: true,
        });
    }
    out
}

impl TaskDetail {
    /// Project one task. Returns `None` when `task_id` is unknown in the run.
    pub fn from_state_and_findings(
        state: &RunState,
        task_id: &str,
        findings: &[Finding],
    ) -> Option<Self> {
        let t = state.tasks.get(task_id)?;

        let mut downstream: Vec<String> = state
            .tasks
            .values()
            .filter(|other| other.depends_on.iter().any(|d| d == task_id))
            .map(|other| other.id.clone())
            .collect();
        downstream.sort();

        let approval = if t.status == TaskStatus::AwaitingApproval
            || state.approvals_pending.iter().any(|id| id == task_id)
        {
            Some(TaskApprovalState::Pending)
        } else {
            None
        };

        let findings = findings
            .iter()
            .filter(|f| f.task_id.as_deref() == Some(task_id))
            .cloned()
            .collect();

        Some(TaskDetail {
            schema_version: TASK_DETAIL_V1.to_string(),
            run_id: state.run_id.clone(),
            task_id: t.id.clone(),
            project: t.project.clone(),
            status: serde_str(&t.status),
            kind: t.kind.clone(),
            agent: t.agent.clone(),
            role: t.role.clone(),
            resolved_agent_profile: t.resolved_agent_profile.clone(),
            resolved_review_profile: t.resolved_review_profile.clone(),
            depends_on: t.depends_on.clone(),
            downstream,
            risk_level: t.risk_level.clone(),
            attempts: t.attempts,
            started_at: t.started_at.map(|x| x.to_rfc3339()),
            ended_at: t.ended_at.map(|x| x.to_rfc3339()),
            approval,
            artifacts: task_artifacts(t),
            findings,
            last_error: t.error.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Artifacts;
    use crate::scheduler::findings::{Finding, FindingKind, Severity};
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn task(id: &str, status: TaskStatus, deps: &[&str]) -> TaskState {
        TaskState {
            id: id.to_string(),
            project: "billing-service".to_string(),
            agent: "mock".to_string(),
            status,
            started_at: None,
            ended_at: None,
            chat_id: None,
            error: None,
            attempts: 0,
            risk_level: None,
            artifacts: Artifacts::default(),
            permission: None,
            workflow_outputs: BTreeMap::new(),
            log_path: format!("{id}.log"),
            trajectory_path: None,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            parallel_group: None,
            requires_approval_after: false,
            kind: "agent".to_string(),
            memory_used: vec![],
            context_bytes: None,
            skills_triggered: vec![],
            usage: None,
            steps: None,
            role: None,
            resolved_agent_profile: None,
            resolved_review_profile: None,
            workspace_path: Some("/abs/secret/workspace".to_string()),
            worktree_path: Some("/abs/secret/worktree".to_string()),
        }
    }

    fn run(status: RunStatus, tasks: Vec<TaskState>, approvals: &[&str]) -> RunState {
        let mut map = BTreeMap::new();
        let order: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        for t in tasks {
            map.insert(t.id.clone(), t);
        }
        RunState {
            run_id: "r-test".into(),
            spec: "neutral demo".into(),
            started_at: Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
            ended_at: None,
            status,
            max_parallel: 4,
            pid: 0,
            tasks: map,
            approvals_pending: approvals.iter().map(|s| s.to_string()).collect(),
            task_order: order,
            session_id: None,
            usage: Default::default(),
            budget_tokens: Some(100_000),
            pending_gate: None,
            goal: None,
            acceptance_results: vec![],
            verified: false,
            auto_actions: vec![],
            run_dir: std::path::PathBuf::from("/abs/secret/run"),
        }
    }

    fn finding(kind: FindingKind, sev: Severity, task: Option<&str>) -> Finding {
        let f = Finding::new(
            "r-test",
            kind,
            sev,
            "test",
            "summary",
            "2026-06-04T00:00:00Z".to_string(),
        );
        match task {
            Some(t) => f.task(t),
            None => f,
        }
    }

    #[test]
    fn run_monitor_round_trips_and_counts_progress() {
        let st = run(
            RunStatus::Running,
            vec![
                task("T0", TaskStatus::Done, &[]),
                task("T1", TaskStatus::Running, &["T0"]),
                task("T2", TaskStatus::Pending, &["T1"]),
            ],
            &[],
        );
        let m = RunMonitor::from_state_and_findings(&st, &[], None);
        assert_eq!(m.schema_version, RUN_MONITOR_V1);
        assert_eq!(m.status, "running");
        assert_eq!(
            (
                m.progress.total,
                m.progress.done,
                m.progress.running,
                m.progress.pending
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(m.progress.settled, 1);
        // one active task (T1)
        assert_eq!(m.active_tasks.len(), 1);
        assert_eq!(m.active_tasks[0].task_id, "T1");
        // serde round-trip
        let back: RunMonitor = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn approval_pending_projects_existing_tasks_and_ignores_missing() {
        let st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::AwaitingApproval, &[])],
            &["T0", "ghost"], // ghost is not a task → ignored
        );
        let m = RunMonitor::from_state_and_findings(&st, &[], None);
        assert_eq!(m.approvals_pending.len(), 1);
        assert_eq!(m.approvals_pending[0].task_id, "T0");
    }

    #[test]
    fn blocked_tasks_need_a_failed_dep_or_terminal_unfinished_dep() {
        // a pending task with a failed dependency is blocked.
        let running = run(
            RunStatus::Running,
            vec![
                task("T0", TaskStatus::Failed, &[]),
                task("T1", TaskStatus::Pending, &["T0"]),
            ],
            &[],
        );
        let m = RunMonitor::from_state_and_findings(&running, &[], None);
        assert_eq!(
            m.blocked_tasks
                .iter()
                .map(|r| r.task_id.as_str())
                .collect::<Vec<_>>(),
            ["T1"]
        );

        // a pending task whose dep is merely still pending is NOT blocked while
        // the run is live...
        let live = run(
            RunStatus::Running,
            vec![
                task("T0", TaskStatus::Pending, &[]),
                task("T1", TaskStatus::Pending, &["T0"]),
            ],
            &[],
        );
        assert!(RunMonitor::from_state_and_findings(&live, &[], None)
            .blocked_tasks
            .is_empty());
        // ...but IS blocked once the run is terminal with an unfinished dep.
        let terminal = run(
            RunStatus::Failed,
            vec![
                task("T0", TaskStatus::Pending, &[]),
                task("T1", TaskStatus::Pending, &["T0"]),
            ],
            &[],
        );
        assert_eq!(
            RunMonitor::from_state_and_findings(&terminal, &[], None)
                .blocked_tasks
                .len(),
            // both T0 (no deps → not blocked) ... only T1 has an unfinished dep
            1
        );
    }

    #[test]
    fn findings_summary_groups_sorts_and_counts() {
        let st = run(
            RunStatus::Done,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        let fs = vec![
            finding(FindingKind::Risk, Severity::High, Some("T0")),
            finding(FindingKind::Refute, Severity::High, Some("T0")),
            finding(FindingKind::Refute, Severity::High, None),
        ];
        let m = RunMonitor::from_state_and_findings(&st, &fs, None);
        // sorted by kind then severity: refute(2) before risk(1)
        assert_eq!(
            m.findings_summary,
            vec![
                FindingSummary {
                    kind: "refute".into(),
                    severity: "high".into(),
                    count: 2
                },
                FindingSummary {
                    kind: "risk".into(),
                    severity: "high".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn task_detail_scopes_findings_computes_downstream_and_hides_abs_paths() {
        let mut t1 = task("T1", TaskStatus::Done, &["T0"]);
        t1.trajectory_path = Some("/abs/secret/run/trajectories/T1.ndjson".into());
        t1.artifacts.files_changed = vec!["src/lib.rs".into()];
        t1.role = Some("backend_rust".into());
        t1.resolved_agent_profile = Some("backend-specialist".into());
        let st = run(
            RunStatus::Done,
            vec![
                task("T0", TaskStatus::Done, &[]),
                t1,
                task("T2", TaskStatus::Done, &["T1"]),
            ],
            &[],
        );
        let fs = vec![
            finding(FindingKind::Refute, Severity::High, Some("T1")),
            finding(FindingKind::Risk, Severity::Low, Some("T0")), // other task
        ];
        let d = TaskDetail::from_state_and_findings(&st, "T1", &fs).unwrap();
        assert_eq!(d.schema_version, TASK_DETAIL_V1);
        assert_eq!(d.downstream, vec!["T2".to_string()]);
        assert_eq!(
            d.resolved_agent_profile.as_deref(),
            Some("backend-specialist")
        );
        // only T1's finding
        assert_eq!(d.findings.len(), 1);
        assert_eq!(d.findings[0].task_id.as_deref(), Some("T1"));
        // artifacts are run-relative, never absolute
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("/abs/secret"), "no absolute paths: {json}");
        assert!(d
            .artifacts
            .iter()
            .any(|a| a.kind == "log" && a.path.as_deref() == Some("logs/T1.log")));
        assert!(
            d.artifacts
                .iter()
                .any(|a| a.kind == "trajectory"
                    && a.path.as_deref() == Some("trajectories/T1.ndjson"))
        );
        assert!(d
            .artifacts
            .iter()
            .any(|a| a.kind == "diff" && a.path.is_none()));
        // unknown task → None
        assert!(TaskDetail::from_state_and_findings(&st, "nope", &fs).is_none());
        // serde round-trip
        let back: TaskDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn task_detail_approval_pending_from_status_or_list() {
        let st = run(
            RunStatus::Running,
            vec![
                task("T0", TaskStatus::AwaitingApproval, &[]),
                task("T1", TaskStatus::Pending, &[]),
            ],
            &["T1"],
        );
        assert_eq!(
            TaskDetail::from_state_and_findings(&st, "T0", &[])
                .unwrap()
                .approval,
            Some(TaskApprovalState::Pending)
        );
        assert_eq!(
            TaskDetail::from_state_and_findings(&st, "T1", &[])
                .unwrap()
                .approval,
            Some(TaskApprovalState::Pending)
        );
    }

    #[test]
    fn run_progress_buckets_cover_every_status_and_sum_to_total() {
        // N1: every TaskStatus has a bucket, so the buckets sum to `total`.
        let st = run(
            RunStatus::Running,
            vec![
                task("a", TaskStatus::Done, &[]),
                task("b", TaskStatus::Failed, &[]),
                task("c", TaskStatus::Running, &[]),
                task("d", TaskStatus::Pending, &[]),
                task("e", TaskStatus::AwaitingApproval, &[]),
                task("f", TaskStatus::Skipped, &[]),
                task("g", TaskStatus::Cancelled, &[]),
            ],
            &[],
        );
        let p = RunMonitor::from_state_and_findings(&st, &[], None).progress;
        assert_eq!(p.total, 7);
        assert_eq!(
            p.done
                + p.failed
                + p.running
                + p.pending
                + p.awaiting_approval
                + p.skipped
                + p.cancelled,
            p.total,
            "all buckets must sum to total"
        );
        assert_eq!((p.awaiting_approval, p.skipped), (1, 1));
        // settled = done + failed + cancelled + skipped (NOT awaiting_approval)
        assert_eq!(p.settled, 4);
    }

    #[test]
    fn task_artifacts_omitted_for_path_unsafe_task_id() {
        // N2: legacy/corrupt RUN_STATE could carry a path-unsafe task id; the
        // monitor must not build a run-relative ref from it (no `logs/../…`).
        for bad in ["../escape", "bad/name"] {
            let mut t = task(bad, TaskStatus::Done, &[]);
            t.log_path = format!("{bad}.log"); // non-empty
            t.trajectory_path = Some("/abs/secret/run/trajectories/x.ndjson".into());
            t.artifacts.files_changed = vec!["src/lib.rs".into()];
            let st = run(RunStatus::Done, vec![t], &[]);
            let d = TaskDetail::from_state_and_findings(&st, bad, &[]).unwrap();
            assert!(d.artifacts.is_empty(), "{bad}: artifacts must be omitted");
            let json = serde_json::to_string(&d).unwrap();
            // no path-derived artifact ref (traversal or otherwise) leaks
            assert!(!json.contains("logs/"), "no log ref for {bad}: {json}");
            assert!(
                !json.contains("trajectories/"),
                "no trajectory ref for {bad}: {json}"
            );
        }
    }
}
