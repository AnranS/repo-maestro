use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::adapter::{Artifacts, Usage};
use crate::config::{Plan, ProjectsConfig, TaskKind};
use crate::schema::permissions::PermissionEvidence;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    AwaitingApproval,
    Done,
    Failed,
    Skipped,
    Cancelled,
}

/// One decision maestro made *on the user's behalf* during a run — wiring a
/// contract dependency, retrying a task, tripping the circuit breaker, or
/// hitting an integration conflict. Surfaced in REPORT.md and summary.json so
/// the user can audit "what did maestro do for me, and why".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoAction {
    /// Machine kind: `contract_wired` | `retry` | `circuit_break` |
    /// `integration_conflict`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Human-readable one-liner explaining the action.
    pub detail: String,
}

/// Result of one acceptance check from the goal block. Persisted alongside
/// the run so the UI can render pass/fail rows and the planner has a
/// structured artefact to read when replanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceResult {
    pub describe: String,
    pub check: String,
    pub passed: bool,
    /// Exit code from the shell command; `None` means we never got far
    /// enough to run it (e.g. spawn failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Truncated stdout+stderr (last ~4 KiB) — enough for the UI to show
    /// what went wrong without blowing up the state file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub id: String,
    pub project: String,
    pub agent: String,
    pub status: TaskStatus,

    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,

    #[serde(default)]
    pub chat_id: Option<String>,

    #[serde(default)]
    pub error: Option<String>,

    /// Number of times this task was re-run after a failure (0 = first try).
    #[serde(default)]
    pub attempts: u32,

    /// Change-risk level classified from this task's diff once it completes
    /// (`"low"` | `"high"`). Drives risk-based approval gating and dashboards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,

    #[serde(default)]
    pub artifacts: Artifacts,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionEvidence>,

    /// Workflow data outputs captured from declared `tasks[*].outputs`.
    /// Values point at immutable snapshots under the run directory so
    /// downstream tasks can consume stable data even if the source workspace
    /// changes later.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workflow_outputs: BTreeMap<String, WorkflowOutputState>,

    pub log_path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_path: Option<String>,

    /// Mirror of plan fields for dashboard rendering (DAG edges, grouping).
    #[serde(default)]
    pub depends_on: Vec<String>,

    #[serde(default)]
    pub parallel_group: Option<String>,

    #[serde(default)]
    pub requires_approval_after: bool,

    #[serde(default)]
    pub kind: String,

    /// Memory slices that were injected when this task was dispatched. Each
    /// element is a "<topic>/<file>" identifier suitable for the UI's context
    /// browser to deep-link into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_used: Vec<String>,

    /// Total bytes of context (L1 facts + L2 fan-in + semantic retrieval)
    /// inlined into this task's prompt at dispatch. Answers the recurring
    /// question "did the model see what I told it to remember?" — a 0 here
    /// means literally nothing was injected, a large number means the
    /// prompt was thick with facts. Set once when the context vector is
    /// frozen; `None` for tasks where context injection didn't run
    /// (verify / `_global` shell tasks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_bytes: Option<u64>,

    /// Skill names that matched a trigger in this task's prompt and got their
    /// full body inlined for the agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills_triggered: Vec<String>,

    /// Token / cost usage reported by the adapter for this task. Aggregated
    /// into `RunState.usage` once the task finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,

    /// Number of agent "steps" (≈ tool calls) the adapter observed during
    /// this task. `None` for adapters that can't report it (shell, mock,
    /// blob-JSON cursor mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,

    /// Role that was active when this task ran (resolved from task.role
    /// or project.role). `None` for `_global`/verify tasks that don't
    /// inherit a role. Surfaced in the dashboard as a colored badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// F-114: name of the specialist `agent_profile` that supplied this task's
    /// writer role/skills/model_profile, if one matched (explicit task/project
    /// reference or a pre-dispatch trigger). `None` when no profile applied.
    /// Provenance only — the lowered role/skills/model are recorded as usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_agent_profile: Option<String>,

    /// F-114: name of the specialist `agent_profile` that supplied this task's
    /// reviewer (post-task review profile or trigger match). `None` when the
    /// review came from an explicit `review_by` role or the F-106 fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_review_profile: Option<String>,

    /// Actual directory used for execution. This may differ from the project
    /// path when worktree isolation is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,

    /// Per-task git worktree path, when isolation was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutputState {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub snapshot_path: String,
    pub bytes: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub spec: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub max_parallel: usize,
    #[serde(default)]
    pub pid: u32,
    pub tasks: BTreeMap<String, TaskState>,
    pub approvals_pending: Vec<String>,
    pub task_order: Vec<String>,

    /// If this run was launched by a chat tool-use action, the originating
    /// chat session id is captured here so the UI can show "📨 from session ..."
    /// links between the tasks view and the chat panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Sum of per-task `usage` across the run. Computed lazily on each
    /// state write so the UI header can show a live total.
    #[serde(default, skip_serializing_if = "Usage::is_zero")]
    pub usage: Usage,

    /// Token budget for this run (`--max-tokens`), if one was set. Surfaced so
    /// the UI can show consumption against the cap that the budget gate
    /// enforces; `None` means the run is uncapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,

    /// The boundary review gate the run is currently paused at, if any:
    /// `"plan"` (waiting to start) or `"outcome"` (waiting to be accepted).
    /// `None` when the run isn't waiting on a run-level gate. Drives the UI's
    /// gate card; cleared once the human approves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_gate: Option<String>,

    /// Snapshot of the plan's goal block at run start. Surfaced in the UI
    /// header so the user always sees what they asked for, not just task ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<crate::config::Goal>,

    /// Outcome of each `goal.acceptance[i].check` after the DAG finished.
    /// Empty if the plan had no goal or the run was cancelled before the
    /// verify gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_results: Vec<AcceptanceResult>,

    /// True iff every acceptance check passed AND the DAG itself succeeded.
    /// `false` for runs that finished with failing acceptance — those show
    /// up red even if every individual task was green.
    #[serde(default)]
    pub verified: bool,

    /// Decisions maestro made automatically during this run (contract edges
    /// wired, retries, circuit-breaker trips, integration conflicts), so the
    /// user can audit them after the fact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_actions: Vec<AutoAction>,

    #[serde(skip)]
    pub run_dir: PathBuf,
}

impl RunState {
    pub fn new(
        run_id: String,
        plan: &Plan,
        projects: &ProjectsConfig,
        max_parallel: usize,
        run_dir: PathBuf,
    ) -> Self {
        let mut tasks = BTreeMap::new();
        let mut order = Vec::new();
        for t in &plan.tasks {
            // Resolve the agent the same way the executor will pick later.
            let agent = t.agent.clone().unwrap_or_else(|| match t.kind {
                TaskKind::Verify => "shell".to_string(),
                TaskKind::Agent => {
                    if t.project == "_global" {
                        projects.defaults.agent.clone()
                    } else {
                        projects.resolved_agent(&t.project)
                    }
                }
            });
            let log_path = run_dir
                .join("logs")
                .join(format!("{}.log", t.id))
                .to_string_lossy()
                .to_string();
            let kind = match t.kind {
                TaskKind::Agent => "agent",
                TaskKind::Verify => "verify",
            }
            .to_string();
            tasks.insert(
                t.id.clone(),
                TaskState {
                    id: t.id.clone(),
                    project: t.project.clone(),
                    agent,
                    status: TaskStatus::Pending,
                    started_at: None,
                    ended_at: None,
                    chat_id: None,
                    error: None,
                    attempts: 0,
                    risk_level: None,
                    artifacts: Artifacts::default(),
                    permission: None,
                    workflow_outputs: BTreeMap::new(),
                    log_path,
                    trajectory_path: None,
                    depends_on: t.depends_on.clone(),
                    parallel_group: t.parallel_group.clone(),
                    requires_approval_after: t.requires_approval_after,
                    kind,
                    memory_used: vec![],
                    context_bytes: None,
                    skills_triggered: vec![],
                    usage: None,
                    steps: None,
                    role: None,
                    resolved_agent_profile: None,
                    resolved_review_profile: None,
                    workspace_path: None,
                    worktree_path: None,
                },
            );
            order.push(t.id.clone());
        }
        Self {
            run_id,
            spec: plan.spec.clone(),
            started_at: Utc::now(),
            ended_at: None,
            status: RunStatus::Running,
            max_parallel,
            pid: std::process::id(),
            tasks,
            approvals_pending: vec![],
            task_order: order,
            session_id: None,
            usage: Usage::default(),
            budget_tokens: None,
            pending_gate: None,
            goal: plan.goal.clone(),
            acceptance_results: Vec::new(),
            verified: false,
            auto_actions: Vec::new(),
            run_dir,
        }
    }

    pub fn write_atomic(&self) -> Result<()> {
        use std::io::Write;
        let target = self.run_dir.join(crate::paths::RUN_STATE_FILE);
        // Unique tmp name per writer so a concurrent external writer (e.g.
        // force_cancel_if_abandoned invoked from the server and a CLI at once)
        // can't truncate the same tmp file mid-write while another renames it.
        // fsync before the rename so a crash can't leave a half-written file
        // behind the atomic swap. The rename stays atomic (last writer wins).
        let tmp = target.with_extension(format!("json.tmp.{}", std::process::id()));
        let text = serde_json::to_string_pretty(self).context("serialize RunState")?;
        {
            let mut f = std::fs::File::create(&tmp).context("create tmp state file")?;
            f.write_all(text.as_bytes())
                .context("write tmp state file")?;
            f.sync_all().context("fsync tmp state file")?;
        }
        std::fs::rename(&tmp, &target).context("rename tmp state file")?;
        Ok(())
    }

    pub fn load(run_dir: &Path) -> Result<Self> {
        let target = run_dir.join(crate::paths::RUN_STATE_FILE);
        let text =
            std::fs::read_to_string(&target).with_context(|| format!("read {:?}", target))?;
        let mut state: RunState = serde_json::from_str(&text)?;
        state.run_dir = run_dir.to_path_buf();
        Ok(state)
    }

    /// Returns true if the run is acceptable for "post-success" side effects
    /// (archiving an L2 decision, marking the spec as delivered, …): either
    /// the run had no goal block at all, or every acceptance check passed.
    pub fn verified_or_no_goal(&self) -> bool {
        match &self.goal {
            None => true,
            Some(g) if g.acceptance.is_empty() => true,
            Some(_) => self.verified,
        }
    }

    /// Compact "n passed / m total" string for the dashboard header.
    pub fn acceptance_summary(&self) -> Option<(usize, usize)> {
        if self.acceptance_results.is_empty() {
            return None;
        }
        let passed = self.acceptance_results.iter().filter(|r| r.passed).count();
        Some((passed, self.acceptance_results.len()))
    }
}
