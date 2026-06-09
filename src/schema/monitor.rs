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
use crate::schema::artifacts::{ArtifactRef, ArtifactSource};
use crate::schema::permissions::Enforcement;
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
    /// F-123: read-only projection of the run's currently-pending review gates.
    #[serde(default)]
    pub gates: Vec<ReviewGate>,
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
            gates: review_gates(state, findings),
            usage: state.usage.clone(),
            budget_tokens: state.budget_tokens,
            updated_at,
        }
    }
}

// ── review gates (F-123) ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateScope {
    Run,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    Plan,
    Outcome,
    TaskApproval,
}

/// Gate decision status. v1 only ever projects `Pending` (currently-waiting
/// gates). The variants `Approved` / `Rejected` / `RequestChanges` are reserved
/// so decided-history can be added later WITHOUT restructuring the contract —
/// that needs write-side gate-decision events the current model doesn't record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pending,
    Approved,
    Rejected,
    RequestChanges,
}

/// Small typed evidence for a gate — counts/refs only, never large objects. Each
/// `kind` populates its own subset; the rest stay `None` (omitted).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEvidence {
    // plan
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_count: Option<u32>,
    // outcome
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_passed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_total: Option<u32>,
    // task approval
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_count: Option<u32>,
}

/// A single currently-pending review gate (read-only projection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewGate {
    /// Stable id: `run:plan` / `run:outcome` / `task:<task_id>`.
    pub gate_id: String,
    pub scope: GateScope,
    pub kind: GateKind,
    pub status: GateStatus,
    /// Set only for task-scoped gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// One-line human-readable summary of what's being decided.
    pub summary: String,
    pub evidence: GateEvidence,
}

/// F-123: project the run's CURRENTLY-PENDING review gates (read-only). v1 emits
/// only `Pending` gates. Decided history (who/when/decision) is deliberately out
/// of scope — it needs write-side gate-decision events the current model does not
/// record, and would violate "first cut read-only, no gate-write changes".
fn review_gates(state: &RunState, findings: &[Finding]) -> Vec<ReviewGate> {
    use std::collections::BTreeSet;
    let mut gates = Vec::new();

    // Run-level boundary gate (plan XOR outcome), only while the run is paused on
    // one. `pending_gate` is `None` once the human decides — v1 shows the live
    // gate, not its later decision.
    match state.pending_gate.as_deref() {
        Some("plan") => {
            let project_count = state
                .tasks
                .values()
                .map(|t| t.project.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u32;
            let task_count = state.tasks.len() as u32;
            let dependency_count: u32 = state
                .tasks
                .values()
                .map(|t| t.depends_on.len() as u32)
                .sum();
            gates.push(ReviewGate {
                gate_id: "run:plan".to_string(),
                scope: GateScope::Run,
                kind: GateKind::Plan,
                status: GateStatus::Pending,
                task_id: None,
                summary: format!(
                    "Plan gate: approve to run {task_count} task(s) across {project_count} project(s)"
                ),
                evidence: GateEvidence {
                    project_count: Some(project_count),
                    task_count: Some(task_count),
                    dependency_count: Some(dependency_count),
                    ..Default::default()
                },
            });
        }
        Some("outcome") => {
            let (passed, total) = state.acceptance_summary().unwrap_or((0, 0));
            gates.push(ReviewGate {
                gate_id: "run:outcome".to_string(),
                scope: GateScope::Run,
                kind: GateKind::Outcome,
                status: GateStatus::Pending,
                task_id: None,
                summary: format!(
                    "Outcome gate: {} · {passed}/{total} acceptance check(s) passed",
                    if state.verified {
                        "verified"
                    } else {
                        "not verified"
                    }
                ),
                evidence: GateEvidence {
                    verified: Some(state.verified),
                    acceptance_passed: Some(passed as u32),
                    acceptance_total: Some(total as u32),
                    ..Default::default()
                },
            });
        }
        _ => {}
    }

    // Task-level approval gates: each task awaiting human approval after running.
    for id in &state.approvals_pending {
        let Some(t) = state.tasks.get(id) else {
            continue;
        };
        let findings_count = findings
            .iter()
            .filter(|f| f.task_id.as_deref() == Some(id.as_str()))
            .count() as u32;
        gates.push(ReviewGate {
            gate_id: format!("task:{id}"),
            scope: GateScope::Task,
            kind: GateKind::TaskApproval,
            status: GateStatus::Pending,
            task_id: Some(id.clone()),
            summary: format!("Task `{id}` is awaiting approval before integration"),
            evidence: GateEvidence {
                risk_level: t.risk_level.clone(),
                findings_count: Some(findings_count),
                ..Default::default()
            },
        });
    }

    gates
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

// ── tool policy (F-125) ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyStatus {
    /// A permission-evidence record was resolved for this task.
    Present,
    /// No permission evidence — an explicit absence, NEVER "allow-all".
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    pub name: String,
    pub requested: bool,
    pub enforcement: Enforcement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// v1 has no per-tool idempotency model — always `"unknown"` (never assumed).
    pub idempotency: String,
}

/// F-125: read-only per-node tool-policy projection. Resolves the EXISTING
/// `PermissionEvidence` (the only source — no recompute from role defaults, which
/// would drift from what actually ran) plus observed artifacts into one auditable
/// view. v1 is projection/audit ONLY — no enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskToolPolicy {
    pub status: ToolPolicyStatus,
    pub capabilities: Vec<CapabilityPolicy>,
    /// Side-effecting capabilities the task REQUESTED (from permission evidence).
    /// `shell` is a capability, not auto a side effect, so it is not listed here.
    pub declared_effects: Vec<String>,
    /// Side effects OBSERVED in the run (changed files, PR). `branch` is excluded
    /// — it's the executor's auto worktree branch, not a real write (F-126-fu).
    pub observed_effects: Vec<String>,
    pub retry: RetryPolicy,
    /// Run-local evidence refs the node produced (derived from `TaskState`, not
    /// the evidence summary file; F-124 run-local rules apply).
    pub required_evidence: Vec<ArtifactRef>,
    /// Audit gaps — never silent: observed effects with no permission evidence,
    /// or a terminal side-effecting task with no run-local evidence refs.
    pub audit_gaps: Vec<String>,
    /// The task's CURRENTLY-pending gate id (F-123), if any. v1 projects no gate
    /// HISTORY — a completed/approved task is never flagged for an empty gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_gate_id: Option<String>,
}

fn cap(
    name: &str,
    requested: bool,
    enforcement: Enforcement,
    allowed_commands: Vec<String>,
) -> CapabilityPolicy {
    CapabilityPolicy {
        name: name.to_string(),
        requested,
        enforcement,
        allowed_commands,
    }
}

fn task_tool_policy(state: &RunState, t: &TaskState) -> TaskToolPolicy {
    let terminal = matches!(
        t.status,
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Skipped
    );

    let (status, capabilities, declared_effects) = match &t.permission {
        Some(pe) => {
            let caps = vec![
                cap(
                    "shell",
                    pe.requested.shell,
                    pe.resolved.shell,
                    pe.resolved.allowed_commands.clone(),
                ),
                cap(
                    "git_write",
                    pe.requested.git_write,
                    pe.resolved.git_write,
                    vec![],
                ),
                cap("network", pe.requested.network, pe.resolved.network, vec![]),
                cap(
                    "fs_write",
                    pe.requested.fs_write,
                    pe.resolved.fs_write,
                    vec![],
                ),
                cap(
                    "external_dir",
                    pe.requested.external_dir,
                    pe.resolved.external_dir,
                    vec![],
                ),
                cap("mcp", pe.requested.mcp, pe.resolved.mcp, vec![]),
            ];
            // `shell` is a capability, not auto a side effect (per the contract).
            let mut declared = Vec::new();
            for (req, label) in [
                (pe.requested.git_write, "git_write requested"),
                (pe.requested.fs_write, "fs_write requested"),
                (pe.requested.external_dir, "external_dir requested"),
                (pe.requested.mcp, "mcp requested"),
                (pe.requested.network, "network requested"),
            ] {
                if req {
                    declared.push(label.to_string());
                }
            }
            (ToolPolicyStatus::Present, caps, declared)
        }
        None => (ToolPolicyStatus::Absent, Vec::new(), Vec::new()),
    };

    let mut observed_effects = Vec::new();
    if !t.artifacts.files_changed.is_empty() {
        observed_effects.push(format!(
            "{} file(s) changed",
            t.artifacts.files_changed.len()
        ));
    }
    if t.artifacts
        .pr_url
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        observed_effects.push("pull request opened".to_string());
    }
    // `artifacts.branch` is NOT an observed write effect — the executor auto-fills
    // it with the per-task worktree isolation branch for every git-backed task, so
    // it can't distinguish a real push (F-126-fu). Keeps this projection's audit
    // signals consistent with the policy gate (`scheduler::policy_gate`).

    // Required evidence: the node's run-local refs straight from `TaskState`
    // (never the evidence summary file). Unsafe refs are dropped defensively.
    let mut required_evidence = t.artifacts.to_artifact_refs(&t.id);
    if t.trajectory_path.is_some() {
        required_evidence.push(ArtifactRef {
            kind: "trajectory".to_string(),
            source: ArtifactSource::AgentTask,
            task_id: Some(t.id.clone()),
            path: Some(format!("trajectories/{}.ndjson", t.id)),
            uri: None,
            name: None,
            bytes: None,
        });
    }
    required_evidence.retain(|r| r.run_local_violation().is_none());

    // F-123 has no gate HISTORY — only the CURRENTLY-pending gate is knowable.
    let pending_gate_id = if t.status == TaskStatus::AwaitingApproval
        || state.approvals_pending.iter().any(|id| id == &t.id)
    {
        Some(format!("task:{}", t.id))
    } else {
        None
    };

    let has_effects = !declared_effects.is_empty() || !observed_effects.is_empty();
    let mut audit_gaps = Vec::new();
    if matches!(status, ToolPolicyStatus::Absent) && !observed_effects.is_empty() {
        audit_gaps
            .push("observed side effects but no permission evidence was recorded".to_string());
    }
    if terminal && has_effects && required_evidence.is_empty() {
        audit_gaps
            .push("terminal task with side effects but no run-local evidence refs".to_string());
    }

    TaskToolPolicy {
        status,
        capabilities,
        declared_effects,
        observed_effects,
        retry: RetryPolicy {
            attempts: t.attempts,
            max_retries: None,
            idempotency: "unknown".to_string(),
        },
        required_evidence,
        audit_gaps,
        pending_gate_id,
    }
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
    /// F-125: read-only tool-policy projection for this node (always present;
    /// `status` is `absent` when there is no permission evidence — never
    /// "allow-all").
    pub tool_policy: TaskToolPolicy,
    /// F-136a1: read-only RuntimeProfile safety label derived from the same
    /// permission evidence (always present; absent permission projects explicitly
    /// as `unknown`/`requires_review`, never a missing field). NO enforcement.
    pub runtime_profile: crate::schema::runtime_profile::RuntimeProfileView,
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
            tool_policy: task_tool_policy(state, t),
            runtime_profile: crate::schema::runtime_profile::derive(
                t.permission.as_ref(),
                crate::scheduler::policy_gate::observed_write(t),
            ),
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
            delivery_id: None,
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

    fn acc(passed: bool) -> crate::scheduler::state::AcceptanceResult {
        crate::scheduler::state::AcceptanceResult {
            describe: "check".into(),
            check: "true".into(),
            passed,
            exit_code: Some(if passed { 0 } else { 1 }),
            output: String::new(),
            started_at: Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
            ended_at: Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 1).unwrap(),
        }
    }

    #[test]
    fn review_gates_three_states_plan_outcome_task_and_absent() {
        // absent — no pending gate, nothing awaiting approval.
        let st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        assert!(
            RunMonitor::from_state_and_findings(&st, &[], None)
                .gates
                .is_empty(),
            "no pending gate and no approvals → absent (empty gates)"
        );

        // plan gate present, evidence = project/task/dependency counts.
        let mut st = run(
            RunStatus::Running,
            vec![
                task("T0", TaskStatus::Pending, &[]),
                task("T1", TaskStatus::Pending, &["T0"]),
            ],
            &[],
        );
        st.pending_gate = Some("plan".into());
        let gates = RunMonitor::from_state_and_findings(&st, &[], None).gates;
        assert_eq!(gates.len(), 1);
        let g = &gates[0];
        assert_eq!(g.gate_id, "run:plan");
        assert_eq!(g.scope, GateScope::Run);
        assert_eq!(g.kind, GateKind::Plan);
        assert_eq!(g.status, GateStatus::Pending);
        assert_eq!(g.task_id, None);
        assert_eq!(g.evidence.task_count, Some(2));
        assert_eq!(g.evidence.project_count, Some(1));
        assert_eq!(g.evidence.dependency_count, Some(1));
        assert_eq!(
            g.evidence.verified, None,
            "plan evidence carries no outcome fields"
        );

        // outcome gate present, evidence = verified + acceptance passed/total.
        let mut st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        st.pending_gate = Some("outcome".into());
        st.verified = false;
        st.acceptance_results = vec![acc(true), acc(false)];
        let gates = RunMonitor::from_state_and_findings(&st, &[], None).gates;
        assert_eq!(gates.len(), 1);
        let g = &gates[0];
        assert_eq!(g.gate_id, "run:outcome");
        assert_eq!(g.kind, GateKind::Outcome);
        assert_eq!(g.evidence.verified, Some(false));
        assert_eq!(g.evidence.acceptance_passed, Some(1));
        assert_eq!(g.evidence.acceptance_total, Some(2));
        assert_eq!(
            g.evidence.task_count, None,
            "outcome evidence carries no plan fields"
        );

        // task-approval gate present, evidence = risk + task-scoped findings count.
        let mut st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::AwaitingApproval, &[])],
            &["T0"],
        );
        st.tasks.get_mut("T0").unwrap().risk_level = Some("high".into());
        let findings = vec![
            finding(FindingKind::Risk, Severity::High, Some("T0")),
            finding(FindingKind::Refute, Severity::Low, None), // not this task
        ];
        let m = RunMonitor::from_state_and_findings(&st, &findings, None);
        let g = m
            .gates
            .iter()
            .find(|g| g.scope == GateScope::Task)
            .expect("task gate");
        assert_eq!(g.gate_id, "task:T0");
        assert_eq!(g.kind, GateKind::TaskApproval);
        assert_eq!(g.status, GateStatus::Pending);
        assert_eq!(g.task_id, Some("T0".into()));
        assert_eq!(g.evidence.risk_level, Some("high".into()));
        assert_eq!(g.evidence.findings_count, Some(1));

        // serde round-trip of the whole monitor, gates included.
        let back: RunMonitor = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.gates, m.gates);
    }

    #[test]
    fn task_tool_policy_present_absent_observed_effects_and_audit_gaps() {
        use crate::schema::permissions::{
            Enforcement, PermissionEvidence, PermissionRequest, ResolvedPermission,
        };

        // ABSENT + no observed effects → status absent, no caps, no gaps.
        let st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        let tp = TaskDetail::from_state_and_findings(&st, "T0", &[])
            .unwrap()
            .tool_policy;
        assert_eq!(tp.status, ToolPolicyStatus::Absent);
        assert!(tp.capabilities.is_empty());
        assert!(tp.audit_gaps.is_empty());
        assert_eq!(tp.retry.idempotency, "unknown");
        assert_eq!(tp.retry.attempts, 0);

        // ABSENT + observed effects (changed files) → audit_gap, never allow-all.
        let mut st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        st.tasks.get_mut("T0").unwrap().artifacts.files_changed = vec!["src/a.rs".into()];
        let tp = TaskDetail::from_state_and_findings(&st, "T0", &[])
            .unwrap()
            .tool_policy;
        assert_eq!(tp.status, ToolPolicyStatus::Absent);
        assert!(tp.observed_effects.iter().any(|e| e.contains("file")));
        assert!(tp
            .audit_gaps
            .iter()
            .any(|g| g.contains("no permission evidence")));

        // PRESENT: 6 caps; shell is a capability (not a declared effect), git_write
        // requested IS a declared effect; allowed_commands carried on shell.
        let mut st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        st.tasks.get_mut("T0").unwrap().permission = Some(PermissionEvidence {
            schema_version: PermissionEvidence::SCHEMA_VERSION.to_string(),
            task_id: "T0".into(),
            provider_id: "mock".into(),
            mode_id: "m".into(),
            requested: PermissionRequest {
                shell: true,
                git_write: true,
                network: false,
                fs_write: false,
                external_dir: false,
                mcp: false,
                allowed_commands: vec!["echo".into()],
            },
            resolved: ResolvedPermission {
                shell: Enforcement::Hard,
                git_write: Enforcement::Soft,
                network: Enforcement::NotApplicable,
                fs_write: Enforcement::NotApplicable,
                external_dir: Enforcement::NotApplicable,
                mcp: Enforcement::NotApplicable,
                allowed_commands: vec!["echo".into()],
            },
        });
        let detail = TaskDetail::from_state_and_findings(&st, "T0", &[]).unwrap();
        let tp = &detail.tool_policy;
        assert_eq!(tp.status, ToolPolicyStatus::Present);
        assert_eq!(tp.capabilities.len(), 6);
        let shell = tp.capabilities.iter().find(|c| c.name == "shell").unwrap();
        assert!(shell.requested);
        assert_eq!(shell.enforcement, Enforcement::Hard);
        assert_eq!(shell.allowed_commands, vec!["echo".to_string()]);
        assert!(tp.declared_effects.iter().any(|e| e.contains("git_write")));
        assert!(!tp.declared_effects.iter().any(|e| e.contains("shell")));
        // serde round-trip (tool_policy included).
        let back: TaskDetail =
            serde_json::from_str(&serde_json::to_string(&detail).unwrap()).unwrap();
        assert_eq!(back.tool_policy, detail.tool_policy);

        // PENDING gate id only while awaiting approval; a done task is NOT flagged
        // for an empty gate (F-123 projects no history).
        let st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::AwaitingApproval, &[])],
            &["T0"],
        );
        let tp = TaskDetail::from_state_and_findings(&st, "T0", &[])
            .unwrap()
            .tool_policy;
        assert_eq!(tp.pending_gate_id.as_deref(), Some("task:T0"));

        let st = run(
            RunStatus::Done,
            vec![task("T1", TaskStatus::Done, &[])],
            &[],
        );
        let tp = TaskDetail::from_state_and_findings(&st, "T1", &[])
            .unwrap()
            .tool_policy;
        assert_eq!(tp.pending_gate_id, None);
        assert!(!tp.audit_gaps.iter().any(|g| g.contains("gate")));
    }

    #[test]
    fn tool_policy_worktree_branch_is_not_an_observed_effect() {
        // F-126-fu consistency: a task with only the executor's auto worktree
        // branch (no diff, no PR) must NOT list it as an observed effect, and must
        // not raise an audit gap — same calibration as the policy gate.
        let mut st = run(
            RunStatus::Running,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        st.tasks.get_mut("T0").unwrap().artifacts.branch = Some("maestro/worktree/T0".into());
        let tp = TaskDetail::from_state_and_findings(&st, "T0", &[])
            .unwrap()
            .tool_policy;
        assert!(
            tp.observed_effects.is_empty(),
            "a worktree-only branch must not be an observed effect: {:?}",
            tp.observed_effects
        );
        assert!(
            tp.audit_gaps.is_empty(),
            "a branch-only task must not raise an audit gap: {:?}",
            tp.audit_gaps
        );
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
    fn task_detail_carries_runtime_profile_present_and_absent() {
        use crate::schema::permissions::{
            provider_permission_profile, PermissionEvidence, PermissionRequest,
        };
        use crate::schema::runtime_profile::RuntimeProfile;

        // Absent permission → unknown (F-136a1: never review_only, never a missing field).
        let st = run(
            RunStatus::Done,
            vec![task("T0", TaskStatus::Done, &[])],
            &[],
        );
        let td = TaskDetail::from_state_and_findings(&st, "T0", &[]).unwrap();
        assert_eq!(td.runtime_profile.profile, RuntimeProfile::Unknown);

        // Present permission, git_write requested on the shell provider → write_local,
        // and git_write's Soft enforcement surfaces in `advisory` (never claimed hard).
        let mut t = task("T1", TaskStatus::Done, &[]);
        t.permission = Some(PermissionEvidence {
            schema_version: PermissionEvidence::SCHEMA_VERSION.to_string(),
            task_id: "T1".into(),
            provider_id: "shell".into(),
            mode_id: "m".into(),
            requested: PermissionRequest {
                shell: true,
                git_write: true,
                network: false,
                fs_write: true,
                external_dir: false,
                mcp: false,
                allowed_commands: vec![],
            },
            resolved: provider_permission_profile("shell"),
        });
        let st2 = run(RunStatus::Done, vec![t], &[]);
        let td2 = TaskDetail::from_state_and_findings(&st2, "T1", &[]).unwrap();
        assert_eq!(td2.runtime_profile.profile, RuntimeProfile::WriteLocal);
        assert!(td2
            .runtime_profile
            .advisory
            .contains(&"git_write".to_string()));
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
