use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio::task::JoinSet;

use crate::adapter::{self, AgentTask, ExecutionMode, MemorySlice};
use crate::config::{
    channels::ChannelConfig, parse_output_ref, Plan, PlanTask, ProjectsConfig, TaskKind, TaskOutput,
};
use crate::gitops;
use crate::memory::MemoryStore;
use crate::paths;

use super::dag::TaskGraph;
use super::events::{self, RunEventKind};
use super::executor_util::*;
use super::replan;
use super::state::{AutoAction, RunState, RunStatus, TaskStatus, WorkflowOutputState};
use super::trajectory;
use super::verify;
use super::worktree_policy::WorktreePolicy;

#[derive(Debug, Clone)]
pub struct ExecConfig {
    pub max_parallel: usize,
    pub continue_on_error: bool,
    pub only: Option<Vec<String>>,
    pub skip: Vec<String>,
    pub session_id: Option<String>,

    /// Run-level model override; wins over project default but loses to per-
    /// task `model:` in the PLAN.
    pub model_override: Option<String>,

    /// When true, every agent/verify task that hits a git-backed project
    /// runs in its own `git worktree`. This is the safety belt that lets
    /// `max_parallel > 1` work on real repos without `git status` races.
    /// Auto-enabled when `max_parallel > 1`; set to false to opt out.
    pub isolate_with_worktrees: bool,

    /// Previous run state used by `maestro rerun --from` to preserve outputs
    /// and integrated patches for tasks that are intentionally skipped.
    pub seed_skipped_from: Option<RunState>,

    /// Optional caller-provided run id. Used by `maestro work --run` so its
    /// SIGINT bridge can target the concrete cancel marker for the run it is
    /// driving.
    pub run_id: Option<String>,

    /// Contract producer→consumer dependency edges the CLI auto-wired before
    /// this run. Recorded into the run's auto-actions ledger for auditing.
    pub wired_contract_edges: Vec<crate::config::WiredEdge>,

    /// Optional token budget for the whole run. When the cumulative usage
    /// crosses it, maestro escalates and stops (unless `continue_on_error`),
    /// so an agent run can't silently burn unbounded tokens.
    pub max_tokens: Option<u64>,

    /// Boundary-based review gates (anti decision-fatigue). When `plan_gate`
    /// is set, the run pauses before dispatching any task until a human
    /// approves the plan (intent gate). When `outcome_gate` is set, it pauses
    /// after the work + acceptance checks until a human approves the result
    /// (outcome gate). Used together, they concentrate human judgment at the
    /// two boundaries instead of on every task.
    pub plan_gate: bool,
    pub outcome_gate: bool,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            continue_on_error: false,
            only: None,
            skip: vec![],
            session_id: None,
            model_override: None,
            isolate_with_worktrees: true,
            seed_skipped_from: None,
            run_id: None,
            wired_contract_edges: vec![],
            max_tokens: None,
            plan_gate: false,
            outcome_gate: false,
        }
    }
}

/// Run-level approval marker ids for the two boundary gates. They reuse the
/// per-task approval marker machinery (`wait_for_approval` / the `approve`
/// endpoint) with reserved ids that can't collide with a real task id.
const GATE_PLAN: &str = "__gate_plan__";
const GATE_OUTCOME: &str = "__gate_outcome__";

/// Bounded retries per failed task before maestro escalates (see the dispatch
/// loop's failure recovery).
const DEFAULT_MAX_RETRIES: u32 = 2;

/// Max wall-clock a run will sit with ALL remaining work parked on unresolved
/// blocking mailbox messages and nothing running, before escalating instead of
/// polling forever. Bounds the one wait path that previously had no timeout, so
/// an unanswered blocking message (or two mutually-blocking tasks) can't wedge
/// the run with only `maestro cancel-run` as an escape.
const BLOCKED_MAIL_TIMEOUT: Duration = Duration::from_secs(1800);

/// What each dispatched task resolves to in the `running` JoinSet: the task id
/// plus the adapter's run result.
type TaskJoin = (String, Result<adapter::AgentResult>);

/// Control signal returned by an extracted dispatch-loop phase, telling the
/// loop whether to re-iterate, stop, or fall through to the next phase.
enum LoopFlow {
    /// `continue` the dispatch loop.
    Continue,
    /// `break` out of the dispatch loop.
    Break,
    /// Fall through to the next phase in the loop body.
    Proceed,
}

#[derive(Debug, Clone)]
struct IntegrationWorktree {
    repo_root: PathBuf,
    path: PathBuf,
    branch: String,
}

#[derive(Debug, Default)]
struct IntegrationWorktrees {
    by_repo: BTreeMap<String, IntegrationWorktree>,
    /// Per repo, the files each already-integrated task touched. Used to
    /// attribute a later task's integration conflict to the task it clashes
    /// with (the classic monorepo "two tasks edited the same shared file"
    /// case).
    integrated_files: BTreeMap<String, Vec<(String, Vec<String>)>>,
}

impl IntegrationWorktrees {
    fn ensure(
        &mut self,
        repo_root: &Path,
        run_dir: &Path,
        run_id: &str,
    ) -> Result<IntegrationWorktree> {
        let repo_root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        let key = repo_key(&repo_root);
        if let Some(existing) = self.by_repo.get(&key) {
            return Ok(existing.clone());
        }

        let target = run_dir.join("integration").join(&key);
        let branch = format!("maestro/{run_id}/integration-{key}");
        let guard = gitops::WorktreeGuard::create(repo_root.clone(), target, branch.clone())?;
        let path = guard.keep();
        let integration = IntegrationWorktree {
            repo_root,
            path,
            branch,
        };
        self.by_repo.insert(key, integration.clone());
        Ok(integration)
    }

    fn prepare_global_workspace(
        &self,
        workspace: &Path,
        run_dir: &Path,
    ) -> Result<Option<PathBuf>> {
        if self.by_repo.is_empty() {
            return Ok(None);
        }

        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        if self.by_repo.len() == 1 {
            let integration = self.by_repo.values().next().expect("one integration");
            if same_path(&integration.repo_root, &workspace) {
                return Ok(Some(integration.path.clone()));
            }
            if integration.repo_root.strip_prefix(&workspace).is_err() {
                return Ok(Some(integration.path.clone()));
            }
        }

        let bridge = run_dir.join("integration").join("workspace");
        for integration in self.by_repo.values() {
            let Ok(relative_repo) = integration.repo_root.strip_prefix(&workspace) else {
                return Ok(None);
            };
            if relative_repo.as_os_str().is_empty() {
                if self.by_repo.len() == 1 {
                    return Ok(Some(integration.path.clone()));
                }
                return Ok(None);
            }
            link_integration_repo(&bridge.join(relative_repo), &integration.path)?;
        }
        Ok(Some(bridge))
    }

    fn integrate_task(
        &mut self,
        run_dir: &Path,
        task_id: &str,
        task: &super::state::TaskState,
    ) -> Result<bool> {
        let Some(worktree_path) = task.worktree_path.as_deref() else {
            return Ok(false);
        };
        let worktree = PathBuf::from(worktree_path);
        let Some(repo_root) = gitops::worktree_root(&worktree) else {
            return Ok(false);
        };
        let key = repo_key(&repo_root.canonicalize().unwrap_or(repo_root));
        let Some(integration) = self.by_repo.get(&key).cloned() else {
            return Ok(false);
        };
        let patch_path = run_dir
            .join("integration")
            .join("patches")
            .join(format!("{}.patch", safe_patch_name(task_id)));
        if !gitops::write_index_patch(&worktree, &patch_path)? {
            return Ok(false);
        }
        // The files this task touched — recorded on success for later conflict
        // attribution, and intersected against prior tasks on conflict.
        let task_files = gitops::changed_files(&worktree).unwrap_or_default();
        match gitops::apply_patch_file_and_commit(
            &integration.path,
            &patch_path,
            &format!("maestro: integrate {task_id}"),
        ) {
            Ok(committed) => {
                if committed {
                    self.integrated_files
                        .entry(key)
                        .or_default()
                        .push((task_id.to_string(), task_files));
                }
                Ok(committed)
            }
            Err(e) => {
                if let Some(conflict) = e.downcast_ref::<gitops::PatchConflict>() {
                    let culprits = self.attribute_conflict(&key, &conflict.files);
                    let files = conflict.files.join(", ");
                    let with = if culprits.is_empty() {
                        "already-integrated changes".to_string()
                    } else {
                        format!("task(s) {}", culprits.join(", "))
                    };
                    anyhow::bail!(
                        "integration conflict: this task's edits to [{files}] overlap {with}. \
                         These tasks changed the same region of a shared file in parallel — \
                         serialize them with `depends_on` (so one runs after the other), or split \
                         the edits across non-overlapping regions/files."
                    );
                }
                Err(e)
            }
        }
    }

    /// Prior tasks (in this repo's integration) whose files intersect `files`.
    fn attribute_conflict(&self, key: &str, files: &[String]) -> Vec<String> {
        let touched: std::collections::HashSet<&str> = files.iter().map(|f| f.as_str()).collect();
        self.integrated_files
            .get(key)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(_, fs)| fs.iter().any(|f| touched.contains(f.as_str())))
                    .map(|(t, _)| t.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn integrate_seed_task(
        &mut self,
        run_dir: &Path,
        run_id: &str,
        task_id: &str,
        task: &super::state::TaskState,
        repo_root_hint: Option<&Path>,
    ) -> Result<bool> {
        let Some(worktree_path) = task.worktree_path.as_deref() else {
            return Ok(false);
        };
        let worktree = PathBuf::from(worktree_path);
        let Some(repo_root) = gitops::worktree_root(&worktree) else {
            return Ok(false);
        };
        let integration_repo_root = repo_root_hint.unwrap_or(&repo_root);
        self.ensure(integration_repo_root, run_dir, run_id)?;
        self.integrate_task(run_dir, task_id, task)
    }
}

fn repo_key(path: &Path) -> String {
    let identity = gitops::git_common_dir(path)
        .or_else(|| path.canonicalize().ok())
        .unwrap_or_else(|| path.to_path_buf());
    let display = identity.to_string_lossy();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    display.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn safe_patch_name(task_id: &str) -> String {
    task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    a == b
}

fn link_integration_repo(link: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() || meta.is_file() => {
            std::fs::remove_file(link).with_context(|| format!("remove {:?}", link))?;
        }
        Ok(meta) if meta.is_dir() => {
            std::fs::remove_dir(link).with_context(|| format!("remove {:?}", link))?;
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("stat {:?}", link)),
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .with_context(|| format!("symlink {:?} -> {:?}", link, target))?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
            .with_context(|| format!("symlink {:?} -> {:?}", link, target))?;
    }
    Ok(())
}

pub fn generate_run_id() -> String {
    format!(
        "{}_{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8]
    )
}

/// Immutable run context threaded through the phases of `run_plan`: the run
/// identity, the shared `RunState` handle, and the event/channel plumbing.
/// Bundles the arguments that otherwise repeat across ~80 call sites so the
/// extracted phase helpers stay readable.
struct RunCtx {
    run_id: String,
    run_dir: PathBuf,
    state: Arc<Mutex<RunState>>,
    state_tx: broadcast::Sender<()>,
    channels: Option<ChannelConfig>,
    dry_run: bool,
}

impl RunCtx {
    /// Append a run event (wraps `record_event` with this run's plumbing).
    fn record(
        &self,
        kind: RunEventKind,
        task: Option<&str>,
        message: Option<String>,
        payload: serde_json::Value,
    ) {
        record_event(
            &self.run_dir,
            &self.run_id,
            kind,
            task,
            message,
            payload,
            self.channels.as_ref(),
            self.dry_run,
        );
    }

    /// Persist the shared state to disk.
    async fn write_state(&self) -> Result<()> {
        write_state(&self.state).await
    }

    /// Nudge subscribers that state changed (file watcher / SSE).
    fn tick(&self) {
        let _ = self.state_tx.send(());
    }

    /// Lock the shared run state.
    async fn lock(&self) -> tokio::sync::MutexGuard<'_, RunState> {
        self.state.lock().await
    }

    /// Pause at a run-level review gate (plan / outcome): mark `pending_gate`,
    /// record the request, and wait for the human's approval marker. On
    /// approval, clears `pending_gate` and records the grant. Returns the
    /// outcome so the caller can branch on Approved vs Cancelled.
    async fn await_gate(
        &self,
        gate: &str,
        marker: &str,
        request_msg: &str,
        request_payload: serde_json::Value,
    ) -> Result<ApprovalWaitOutcome> {
        {
            self.lock().await.pending_gate = Some(gate.to_string());
        }
        self.write_state().await?;
        self.record(
            RunEventKind::TaskApprovalRequested,
            None,
            Some(request_msg.to_string()),
            request_payload,
        );
        self.tick();
        let gate_opened_at = std::time::SystemTime::now();
        let outcome = wait_for_approval(marker, &self.run_id, gate_opened_at).await?;
        if matches!(outcome, ApprovalWaitOutcome::Approved) {
            {
                self.lock().await.pending_gate = None;
            }
            self.write_state().await?;
            self.record(
                RunEventKind::TaskApprovalGranted,
                None,
                Some(format!("{gate} gate approved")),
                json!({ "gate": gate }),
            );
        }
        Ok(outcome)
    }
}

/// L4 verification gate: run the goal's acceptance checks from the (optionally
/// integrated) workspace and update `verified` + `acceptance_results`. Writes a
/// REPLAN.md prompt when a check fails. No-op on cancellation or when the plan
/// has no acceptance checks.
async fn run_verify_gate(
    ctx: &RunCtx,
    plan: &Plan,
    cfg: &ExecConfig,
    integrations: &mut IntegrationWorktrees,
    cancelled: bool,
) -> Result<()> {
    if cancelled {
        return Ok(());
    }
    let Some(goal) = plan.goal.clone() else {
        return Ok(());
    };
    if goal.acceptance.is_empty() {
        return Ok(());
    }
    let workspace_root = paths::workspace_root()?;
    let workspace = if cfg.isolate_with_worktrees && cfg.max_parallel > 1 {
        integrations
            .prepare_global_workspace(&workspace_root, &ctx.run_dir)?
            .unwrap_or(workspace_root)
    } else {
        workspace_root
    };
    ctx.record(
        RunEventKind::VerifyStarted,
        None,
        Some("acceptance checks started".to_string()),
        json!({ "checks": goal.acceptance.len() }),
    );
    let snap = {
        let mut s = ctx.lock().await;
        verify::run_acceptance(&mut s, &goal, &workspace).await;
        s.clone()
    };
    ctx.write_state().await?;
    let passed = snap.acceptance_results.iter().filter(|r| r.passed).count();
    ctx.record(
        RunEventKind::VerifyCompleted,
        None,
        Some(format!(
            "acceptance checks completed: {passed}/{} passed",
            snap.acceptance_results.len()
        )),
        json!({
            "passed": passed,
            "total": snap.acceptance_results.len(),
            "verified": snap.verified,
        }),
    );
    ctx.tick();
    // If any check failed, materialise a REPLAN.md the user can hand to the
    // planner agent in one click.
    if !snap.verified {
        match replan::write_replan_prompt(&snap.run_dir, plan, &snap) {
            Ok(Some(path)) => {
                tracing::info!(
                    "acceptance failed; wrote replan prompt to {}",
                    path.display()
                );
                ctx.record(
                    RunEventKind::ReplanWritten,
                    None,
                    Some(format!("wrote {}", path.display())),
                    json!({ "path": path }),
                );
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("could not write replan prompt: {e:#}"),
        }
    }
    Ok(())
}

/// Budget gate: if cumulative token usage has crossed `--max-tokens`, escalate
/// (event + notification + callout) once and, unless `continue_on_error`, abort
/// the in-flight tasks and signal the loop to break.
async fn check_budget_gate(
    ctx: &RunCtx,
    cfg: &ExecConfig,
    task_id: &str,
    running: &mut JoinSet<TaskJoin>,
    budget_tripped: &mut bool,
    had_failure: &mut bool,
) -> Result<LoopFlow> {
    let Some(max) = cfg.max_tokens else {
        return Ok(LoopFlow::Proceed);
    };
    let used = ctx.lock().await.usage.total_tokens();
    if used <= max || *budget_tripped {
        return Ok(LoopFlow::Proceed);
    }
    *budget_tripped = true;
    {
        let mut s = ctx.lock().await;
        s.auto_actions.push(AutoAction {
            kind: "budget_exceeded".into(),
            task: Some(task_id.to_string()),
            detail: format!("token budget exceeded: {used} > {max} — stopping the run"),
        });
    }
    ctx.write_state().await?;
    tracing::error!("token budget exceeded ({used} > {max}); stopping run");
    ctx.record(
        RunEventKind::TaskFailed,
        Some(task_id),
        Some(format!(
            "escalation: token budget exceeded ({used} > {max})"
        )),
        json!({ "escalation": true, "reason": "budget_exceeded", "used": used, "budget": max }),
    );
    ctx.tick();
    eprintln!(
        "🚨 escalation: token budget exceeded ({used} > {max}) — stopping the run. Raise --max-tokens or split the work."
    );
    *had_failure = true;
    if !cfg.continue_on_error {
        running.abort_all();
        while running.join_next().await.is_some() {}
        return Ok(LoopFlow::Break);
    }
    Ok(LoopFlow::Proceed)
}

/// Failure recovery for a finished task. No-op (Proceed) when the task
/// succeeded. On failure: retry with an injected diagnosis while the budget +
/// circuit breaker allow (Continue), else escalate — mark Failed, emit the
/// escalation event/callout, and Break (or Continue under `--continue-on-error`).
#[allow(clippy::too_many_arguments)]
async fn handle_task_failure(
    ctx: &RunCtx,
    projects: &ProjectsConfig,
    cfg: &ExecConfig,
    task_id: &str,
    res: &Result<adapter::AgentResult>,
    retries_used: &mut HashMap<String, u32>,
    attributions: &mut HashMap<String, String>,
    last_errors: &mut HashMap<String, String>,
    had_failure: &mut bool,
    ready: &mut VecDeque<String>,
    running: &mut JoinSet<TaskJoin>,
) -> Result<LoopFlow> {
    if res.is_ok() {
        return Ok(LoopFlow::Proceed);
    }
    let used = retries_used.get(task_id).copied().unwrap_or(0);
    let err_msg = res
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    // Circuit breaker: a retry that reproduces the same failure made no progress
    // — stop now rather than spend the rest of the budget.
    let sig = error_signature(&err_msg);
    let no_progress = last_errors.get(task_id).is_some_and(|prev| prev == &sig);
    last_errors.insert(task_id.to_string(), sig);

    if used < DEFAULT_MAX_RETRIES && !no_progress {
        // Heuristic attribution: the error + the tail of the task log.
        let log_tail = {
            let log_path = {
                let s = ctx.lock().await;
                s.tasks.get(task_id).map(|t| t.log_path.clone())
            };
            log_path
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .map(|c| {
                    let mut tail: Vec<&str> = c.lines().rev().take(25).collect();
                    tail.reverse();
                    tail.join("\n")
                })
                .unwrap_or_default()
        };
        let diagnosis = format!(
            "Attempt {} failed.\nError: {}\n\nLast log lines:\n{}",
            used + 1,
            err_msg.trim(),
            log_tail.trim()
        );
        attributions.insert(task_id.to_string(), diagnosis);
        retries_used.insert(task_id.to_string(), used + 1);
        {
            let mut s = ctx.lock().await;
            if let Some(ts) = s.tasks.get_mut(task_id) {
                ts.status = TaskStatus::Pending;
                ts.attempts = used + 1;
                ts.ended_at = None;
            }
            s.auto_actions.push(AutoAction {
                kind: "retry".into(),
                task: Some(task_id.to_string()),
                detail: format!(
                    "retried after failure (attempt {}/{})",
                    used + 2,
                    DEFAULT_MAX_RETRIES + 1
                ),
            });
        }
        ctx.write_state().await?;
        ctx.record(
            RunEventKind::TaskStarted,
            Some(task_id),
            Some(format!(
                "retrying after failure (attempt {}/{})",
                used + 2,
                DEFAULT_MAX_RETRIES + 1
            )),
            json!({ "retry": used + 1, "max_retries": DEFAULT_MAX_RETRIES }),
        );
        ctx.tick();
        tracing::warn!(
            "task {task_id} failed; retrying with diagnosis ({}/{})",
            used + 1,
            DEFAULT_MAX_RETRIES
        );
        cleanup_worktree_for_retry(&ctx.state, projects, &ctx.run_id, task_id).await;
        ready.push_front(task_id.to_string());
        return Ok(LoopFlow::Continue);
    }

    // maestro is giving up — circuit breaker tripped or retry budget spent. Both
    // mean "automated recovery failed; a human should look", so we escalate.
    let breaker_tripped = no_progress && used < DEFAULT_MAX_RETRIES;
    let reason = if breaker_tripped {
        "circuit breaker tripped — the same error reproduced on retry"
    } else {
        "retry budget exhausted"
    };
    let attempts = used + 1;
    {
        let mut s = ctx.lock().await;
        if let Some(ts) = s.tasks.get_mut(task_id) {
            ts.status = TaskStatus::Failed;
            ts.error.get_or_insert_with(|| err_msg.clone());
        }
        if breaker_tripped {
            s.auto_actions.push(AutoAction {
                kind: "circuit_break".into(),
                task: Some(task_id.to_string()),
                detail: format!(
                    "stopped retrying after {attempts} identical failure(s) — same error reproduced, no progress"
                ),
            });
        }
    }
    ctx.write_state().await?;
    let err_excerpt: String = err_msg.lines().take(3).collect::<Vec<_>>().join(" ");
    tracing::error!(
        "escalation: task {task_id} {reason} after {attempts} attempt(s) — needs human attention"
    );
    ctx.record(
        RunEventKind::TaskFailed,
        Some(task_id),
        Some(format!("escalation: {reason} after {attempts} attempt(s)")),
        json!({
            "escalation": true,
            "reason": if breaker_tripped { "circuit_breaker" } else { "retry_budget_exhausted" },
            "attempts": attempts,
            "error": err_excerpt,
        }),
    );
    ctx.tick();
    eprintln!(
        "🚨 escalation: task `{task_id}` {reason} after {attempts} attempt(s) — maestro gave up; a human should look.\n   error: {err_excerpt}\n   next: maestro logs {task_id}   ·   fix and `maestro rerun --from {task_id}`   ·   `--continue-on-error` to keep going past it"
    );
    *had_failure = true;
    if !cfg.continue_on_error {
        running.abort_all();
        while running.join_next().await.is_some() {}
        Ok(LoopFlow::Break)
    } else {
        Ok(LoopFlow::Continue)
    }
}

/// Integrate a successful task's patch into the per-repo integration worktree.
/// An integration conflict marks the task Failed and signals Break (unless
/// `--continue-on-error`). No-op (Proceed) for failed tasks or when there's no
/// snapshot to integrate.
async fn integrate_completed_task(
    ctx: &RunCtx,
    cfg: &ExecConfig,
    task_id: &str,
    res: &Result<adapter::AgentResult>,
    integrations: &mut IntegrationWorktrees,
    had_failure: &mut bool,
    running: &mut JoinSet<TaskJoin>,
) -> Result<LoopFlow> {
    if res.is_err() {
        return Ok(LoopFlow::Proceed);
    }
    let task_snapshot = {
        let s = ctx.lock().await;
        s.tasks.get(task_id).cloned()
    };
    let Some(task_snapshot) = task_snapshot else {
        return Ok(LoopFlow::Proceed);
    };
    match integrations.integrate_task(&ctx.run_dir, task_id, &task_snapshot) {
        Ok(true) => {
            ctx.record(
                RunEventKind::TaskSucceeded,
                Some(task_id),
                Some("task patch integrated".to_string()),
                json!({
                    "integration": true,
                    "branch": task_snapshot.artifacts.branch,
                }),
            );
        }
        Ok(false) => {}
        Err(e) => {
            let message = format!("integration apply failed: {e:#}");
            {
                let mut s = ctx.lock().await;
                if let Some(ts) = s.tasks.get_mut(task_id) {
                    ts.status = TaskStatus::Failed;
                    ts.error = Some(message.clone());
                }
                s.auto_actions.push(AutoAction {
                    kind: "integration_conflict".into(),
                    task: Some(task_id.to_string()),
                    detail: message.clone(),
                });
            }
            ctx.write_state().await?;
            ctx.record(
                RunEventKind::TaskFailed,
                Some(task_id),
                Some("task integration failed".to_string()),
                json!({ "error": message }),
            );
            ctx.tick();
            *had_failure = true;
            if !cfg.continue_on_error {
                running.abort_all();
                while running.join_next().await.is_some() {}
                return Ok(LoopFlow::Break);
            }
        }
    }
    Ok(LoopFlow::Proceed)
}

/// Reviewer gate for a successful task (`review_by`): run the reviewer persona
/// over the task's work. A FAIL verdict feeds the same retry path as a real
/// failure (Continue), or escalates when retries are spent (Break, or Continue
/// under `--continue-on-error`). PASS / no reviewer / un-runnable → Proceed.
#[allow(clippy::too_many_arguments)]
async fn handle_task_review(
    ctx: &RunCtx,
    plan: &Plan,
    projects: &ProjectsConfig,
    cfg: &ExecConfig,
    task_id: &str,
    retries_used: &mut HashMap<String, u32>,
    attributions: &mut HashMap<String, String>,
    had_failure: &mut bool,
    ready: &mut VecDeque<String>,
    running: &mut JoinSet<TaskJoin>,
) -> Result<LoopFlow> {
    // Resolve which reviewer (if any) runs over this finished task. Explicit
    // `review_by` wins; otherwise F-106 auto-attaches the builtin `refuter` to
    // high-risk tasks when `refute_on_high_risk` is on. Risk is computed here
    // (not read from the stored `risk_level`, which the later approval gate is
    // what writes — N1-A).
    let explicit = plan.task(task_id).and_then(|t| t.review_by.clone());
    let reviewer_role = if let Some(role) = explicit {
        role
    } else if projects.defaults.refute_on_high_risk {
        let project = plan
            .task(task_id)
            .map(|t| t.project.clone())
            .unwrap_or_default();
        let is_high = compute_and_store_risk(ctx, projects, task_id, &project).await;
        match resolve_reviewer_role(None, true, is_high) {
            Some(role) => {
                tracing::debug!(
                    "task {task_id} high-risk + refute_on_high_risk → attaching builtin refuter"
                );
                role
            }
            None => return Ok(LoopFlow::Proceed),
        }
    } else {
        return Ok(LoopFlow::Proceed);
    };
    let snap = {
        let s = ctx.lock().await;
        s.tasks.get(task_id).map(|ts| {
            (
                ts.worktree_path
                    .clone()
                    .or_else(|| ts.workspace_path.clone()),
                ts.log_path.clone(),
                ts.agent.clone(),
                ts.project.clone(),
            )
        })
    };
    let Some((Some(ws), impl_log, adapter_name, project)) = snap else {
        return Ok(LoopFlow::Proceed);
    };
    let impl_prompt = plan
        .task(task_id)
        .map(|t| t.prompt.clone())
        .unwrap_or_default();
    let (verdict, review_usage) = match run_review(
        task_id,
        &reviewer_role,
        PathBuf::from(ws),
        impl_log,
        impl_prompt,
        adapter_name,
        project,
        projects,
        &ctx.run_dir,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // A reviewer that can't even run shouldn't fail the task.
            tracing::warn!("review for {task_id} failed to run: {e:#}");
            (None, None)
        }
    };
    // Account for the reviewer's token cost. These are full LLM invocations
    // (one per review, plus one per review-rejection retry); previously their
    // usage was dropped, letting a configured reviewer/refuter run overshoot
    // the --max-tokens budget gate unseen.
    if let Some(u) = &review_usage {
        ctx.lock().await.usage.add(u);
    }
    let Some(feedback) = verdict else {
        return Ok(LoopFlow::Proceed);
    };
    let used = retries_used.get(task_id).copied().unwrap_or(0);
    if used < DEFAULT_MAX_RETRIES {
        attributions.insert(
            task_id.to_string(),
            format!("Reviewer ({reviewer_role}) REJECTED the work:\n{feedback}"),
        );
        retries_used.insert(task_id.to_string(), used + 1);
        {
            let mut s = ctx.lock().await;
            if let Some(ts) = s.tasks.get_mut(task_id) {
                ts.status = TaskStatus::Pending;
                ts.attempts = used + 1;
                ts.ended_at = None;
            }
        }
        ctx.write_state().await?;
        ctx.record(
            RunEventKind::TaskStarted,
            Some(task_id),
            Some(format!(
                "review rejected; retrying (attempt {}/{})",
                used + 2,
                DEFAULT_MAX_RETRIES + 1
            )),
            json!({ "review": "fail", "retry": used + 1 }),
        );
        ctx.tick();
        tracing::warn!(
            "task {task_id} review rejected by {reviewer_role}; retrying ({}/{})",
            used + 1,
            DEFAULT_MAX_RETRIES
        );
        cleanup_worktree_for_retry(&ctx.state, projects, &ctx.run_id, task_id).await;
        ready.push_front(task_id.to_string());
        return Ok(LoopFlow::Continue);
    }
    {
        let mut s = ctx.lock().await;
        if let Some(ts) = s.tasks.get_mut(task_id) {
            ts.status = TaskStatus::Failed;
            ts.error = Some(format!("review rejected after {} attempt(s)", used + 1));
        }
    }
    ctx.write_state().await?;
    ctx.record(
        RunEventKind::TaskFailed,
        Some(task_id),
        Some("review rejected; retries exhausted".to_string()),
        json!({ "review": "fail" }),
    );
    ctx.tick();
    *had_failure = true;
    if !cfg.continue_on_error {
        running.abort_all();
        while running.join_next().await.is_some() {}
        Ok(LoopFlow::Break)
    } else {
        Ok(LoopFlow::Continue)
    }
}

/// The contract files a project declares (provides + consumes) — fed to the
/// change-risk classifier so a diff touching a shared interface scores high.
fn contract_paths_for(cfg: &ProjectsConfig, project: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = cfg.projects.get(project) {
        for path in [
            p.contracts.provides.as_deref(),
            p.contracts.consumes.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !path.trim().is_empty() {
                out.push(path.to_string());
            }
        }
    }
    out
}

/// Per-task approval gate (`requires_approval_after`): pause the finished task
/// until the human approves (via `maestro approve <task>` or the dashboard). On
/// rejection, cancel the run — abort in-flight tasks and set `*cancelled` so the
/// loop breaks on its next check.
/// Pure risk decision: is a finished task's change high-risk, given the net
/// files it changed and the project's contract paths? Empty change = not
/// high-risk. Kept pure (no state, no I/O) so the review-time and
/// approval-gate callers share one classification and it stays unit-testable.
fn task_is_high_risk(files: &[String], contracts: &[String]) -> bool {
    if files.is_empty() {
        return false;
    }
    // status is unavailable from the path list; tag as modified — the
    // classifier's contract-touch / large-blast signals don't need it.
    let with_status: Vec<(char, String)> = files.iter().map(|f| ('M', f.clone())).collect();
    crate::scheduler::risk::classify_change_risk(&with_status, contracts).level == "high"
}

/// Pure auto-attach decision (F-106): which reviewer role, if any, runs over a
/// finished task. Explicit `review_by` always wins; otherwise, when
/// `refute_on_high_risk` is on and the change is high-risk, the builtin
/// `refuter` is attached. Pure so the ordering bug (reading a not-yet-written
/// `risk_level`) can't sneak back in — `is_high_risk` must be computed from
/// files, see `task_is_high_risk`.
fn resolve_reviewer_role(
    explicit_review_by: Option<&str>,
    refute_on_high_risk: bool,
    is_high_risk: bool,
) -> Option<String> {
    if let Some(r) = explicit_review_by {
        return Some(r.to_string());
    }
    if refute_on_high_risk && is_high_risk {
        return Some("refuter".to_string());
    }
    None
}

/// Classify a finished task's change risk from its net `artifacts.files_changed`
/// plus the project's contract paths, store `risk_level` on the task if not
/// already set, and return whether it's high-risk.
///
/// Shared by the review step (refute auto-attach, runs first) and the approval
/// gate (runs later). The review step MUST call this rather than reading the
/// stored `risk_level`: the gate is what writes that field, and it runs *after*
/// review, so at review time the stored value is still `None` (F-106 N1-A).
async fn compute_and_store_risk(
    ctx: &RunCtx,
    projects: &ProjectsConfig,
    task_id: &str,
    project: &str,
) -> bool {
    let files = {
        let s = ctx.lock().await;
        s.tasks
            .get(task_id)
            .map(|ts| ts.artifacts.files_changed.clone())
            .unwrap_or_default()
    };
    let contracts = contract_paths_for(projects, project);
    let is_high = task_is_high_risk(&files, &contracts);
    if !files.is_empty() {
        // Always overwrite with the freshly computed level: a refute fail →
        // retry re-runs the task with a (possibly) different file set, and the
        // dashboard/report risk_level should reflect the latest classification,
        // not a stale first value (F-106 review follow-up).
        let level = if is_high { "high" } else { "low" };
        let mut s = ctx.lock().await;
        if let Some(ts) = s.tasks.get_mut(task_id) {
            ts.risk_level = Some(level.to_string());
        }
    }
    is_high
}

async fn handle_post_task_approval(
    ctx: &RunCtx,
    plan: &Plan,
    projects: &ProjectsConfig,
    task_id: &str,
    running: &mut JoinSet<TaskJoin>,
    cancelled: &mut bool,
) -> Result<()> {
    let Some(t) = plan.task(task_id).cloned() else {
        return Ok(());
    };

    // Classify this task's change risk (reliable net `artifacts.files_changed`,
    // not a live worktree diff which may be empty before integration), record
    // it on the task, and let a high-risk change trip the approval gate when
    // the run opts into risk-driven oversight (`defaults.gate_on_high_risk`).
    // Same helper the review step used, so both see one classification.
    let high_risk = compute_and_store_risk(ctx, projects, task_id, &t.project).await;
    ctx.write_state().await?;

    let risk_gated = high_risk && projects.defaults.gate_on_high_risk;
    if !t.requires_approval_after && !risk_gated {
        return Ok(());
    }
    {
        let mut s = ctx.lock().await;
        if let Some(ts) = s.tasks.get_mut(task_id) {
            ts.status = TaskStatus::AwaitingApproval;
        }
        if !s.approvals_pending.iter().any(|x| x == task_id) {
            s.approvals_pending.push(task_id.to_string());
        }
    }
    ctx.write_state().await?;
    let reason = if risk_gated && !t.requires_approval_after {
        "high-risk change — awaiting approval before integration"
    } else {
        "task is awaiting approval"
    };
    ctx.record(
        RunEventKind::TaskApprovalRequested,
        Some(task_id),
        Some(reason.to_string()),
        json!({ "high_risk": high_risk, "risk_gated": risk_gated }),
    );
    ctx.tick();
    tracing::info!(
        "task {task_id} awaiting approval{} — run `maestro approve {task_id}` to continue",
        if risk_gated && !t.requires_approval_after {
            " (high-risk change)"
        } else {
            ""
        }
    );
    let gate_opened_at = std::time::SystemTime::now();
    match wait_for_approval(task_id, &ctx.run_id, gate_opened_at).await? {
        ApprovalWaitOutcome::Approved => {
            {
                let mut s = ctx.lock().await;
                if let Some(ts) = s.tasks.get_mut(task_id) {
                    ts.status = TaskStatus::Done;
                }
                s.approvals_pending.retain(|x| x != task_id);
            }
            ctx.write_state().await?;
            ctx.record(
                RunEventKind::TaskApprovalGranted,
                Some(task_id),
                Some("approval marker consumed".to_string()),
                json!({}),
            );
            ctx.tick();
        }
        ApprovalWaitOutcome::Cancelled => {
            *cancelled = true;
            running.abort_all();
            while running.join_next().await.is_some() {}
            {
                let mut s = ctx.lock().await;
                if let Some(ts) = s.tasks.get_mut(task_id) {
                    ts.status = TaskStatus::Cancelled;
                }
                s.approvals_pending.retain(|x| x != task_id);
            }
            ctx.write_state().await?;
            ctx.record(
                RunEventKind::CancelRequested,
                Some(task_id),
                Some("cancel marker detected while awaiting approval".to_string()),
                json!({}),
            );
            ctx.tick();
        }
    }
    Ok(())
}

/// Pure precedence for "which memory topics should this task be injected with":
///   * If the task carries an explicit `memory_inject:` block in the plan,
///     those topics win — the plan author asked for a specific selection.
///   * Otherwise fall back to the project's declared `memory_scope`.
///   * If neither exists (e.g. `_global` task with no explicit injection),
///     returns an empty list and the agent runs with no L1 prelude.
fn resolve_memory_topics(task: &PlanTask, projects: &ProjectsConfig) -> Vec<String> {
    if !task.memory_inject.is_empty() {
        return task.memory_inject.iter().map(|m| m.topic.clone()).collect();
    }
    projects
        .projects
        .get(&task.project)
        .map(|p| p.memory_scope.clone())
        .unwrap_or_default()
}

/// Pick the on-disk workspace this task will run in, and (for `_global`
/// tasks under parallel isolation) prepare the shared integration worktree
/// so multiple `_global` tasks don't tread on each other.
///
/// Returns `(workspace, global_integration)`:
///   * `workspace` — absolute path the adapter will `cd` into. Either the
///     project's `resolved_path`, or for `_global`, a dedicated integration
///     worktree (when isolation is on and parallelism > 1).
///   * `global_integration` — Some(...) iff exactly one integration repo
///     exists and this is a `_global` task using it, so the caller knows
///     to honour the global integration's branch/repo when committing.
///
/// Extracted from `dispatch_one_task` so workspace preparation can be
/// reasoned about independently of state-lock + adapter plumbing.
fn resolve_task_workspace(
    task: &PlanTask,
    projects: &ProjectsConfig,
    cfg: &ExecConfig,
    integrations: &mut IntegrationWorktrees,
    run_dir: &Path,
) -> Result<(PathBuf, Option<IntegrationWorktree>)> {
    let mut workspace = if task.project == "_global" {
        paths::workspace_root()?
    } else {
        projects.resolved_path(&task.project)?
    };
    let mut global_integration: Option<IntegrationWorktree> = None;
    if task.project == "_global" && cfg.isolate_with_worktrees && cfg.max_parallel > 1 {
        if let Some(global_workspace) =
            integrations.prepare_global_workspace(&workspace, run_dir)?
        {
            workspace = global_workspace;
            if integrations.by_repo.len() == 1 {
                global_integration = integrations.by_repo.values().next().cloned();
            }
        }
    }
    Ok((workspace, global_integration))
}

/// Pure routing decision: given the plan's task and the workspace's projects
/// config, picks the adapter name (+ adapter handle) and surfaces the
/// underlying routing outcome so downstream phases can see the model hint.
///
/// Precedence for the adapter (highest first):
///   1. `task.agent` — explicit per-task override always wins.
///   2. routing policy match — `projects.defaults.routing` consulted with the
///      task's `kind` (`agent`/`verify`) and whether the project's contract
///      paths are non-empty (`touches_contract`).
///   3. project default — verify tasks fall back to `shell`; agent tasks fall
///      back to `projects.resolved_agent(...)`.
///
/// The `RouteOutcome` is returned alongside so the caller can apply the
/// route's model hint (only when the task hasn't pinned its own model) —
/// this matches the existing dispatch_one_task behavior verbatim.
///
/// Extracted from `dispatch_one_task` so the routing decision can be reasoned
/// about (and eventually tested) in isolation from semaphore + state plumbing.
fn resolve_adapter_for_task(
    task: &PlanTask,
    projects: &ProjectsConfig,
) -> (
    String,
    std::sync::Arc<dyn adapter::AgentAdapter>,
    crate::scheduler::routing::RouteOutcome,
) {
    let touches_contract = !contract_paths_for(projects, &task.project).is_empty();
    let kind_str = match task.kind {
        TaskKind::Agent => "agent",
        TaskKind::Verify => "verify",
    };
    let route =
        crate::scheduler::routing::route(&projects.defaults.routing, kind_str, touches_contract);

    let adapter_name = task
        .agent
        .clone()
        .or_else(|| route.agent.clone())
        .unwrap_or_else(|| match task.kind {
            TaskKind::Verify => "shell".to_string(),
            TaskKind::Agent => projects.resolved_agent(&task.project),
        });
    let adapter = adapter::pick(&adapter_name);
    (adapter_name, adapter, route)
}

/// Dispatch one ready task: enforce the blocking-mail gate, build the task's
/// workspace + prompt (memory, skills, role, contracts, code context), and spawn
/// the agent into `running`. Returns Break when a setup failure aborts the run
/// (`!continue_on_error`), else Proceed (task spawned, set aside, or skipped).
/// Assemble an agent task's prompt prelude: project instructions, prior-attempt
/// diagnosis, the consumed-contract content, code-graph context, mailbox inbox,
/// mode constraints, role prelude, and skill section — joined in priority order.
/// Pure synchronous reads (no run-state lock, no await) — extracted from
/// `dispatch_one_task` to keep that function focused.
#[allow(clippy::too_many_arguments)]
fn assemble_prompt_prelude(
    task: &PlanTask,
    task_id: &str,
    projects: &ProjectsConfig,
    integrations: &IntegrationWorktrees,
    resolved_role: &Option<crate::roles::Role>,
    resolved_role_name: &Option<String>,
    mode: &crate::modes::Mode,
    instruction_section: Option<String>,
    skill_section: Option<String>,
    attributions: &HashMap<String, String>,
) -> Option<String> {
    let role_prelude = resolved_role
        .as_ref()
        .map(crate::roles::render_section)
        .filter(|s| !s.is_empty());
    let mode_section = Some(crate::modes::render_prompt_constraints(mode));

    // Deliver unresolved mailbox messages addressed to this task into its prompt
    // — this closes the multi-agent loop: peers write to the mailbox and the
    // recipient actually reads them here, by project / role / task id.
    let inbox_section = if matches!(task.kind, TaskKind::Agent) {
        let role_id = resolved_role_name.clone().unwrap_or_default();
        let identities: Vec<&str> = [task.project.as_str(), role_id.as_str(), task_id]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let messages = crate::mailbox::MailboxStore::open()
            .ok()
            .and_then(|store| store.inbox_for(&identities).ok())
            .unwrap_or_default();
        if !messages.is_empty() {
            tracing::info!(
                task = %task_id,
                count = messages.len(),
                "delivered mailbox messages to task prompt"
            );
        }
        let audience = match resolved_role_name.as_deref() {
            Some(role) => format!("project `{}`, role `{}`", task.project, role),
            None => format!("project `{}`", task.project),
        };
        crate::mailbox::render_inbox(&messages, &audience)
    } else {
        None
    };

    // Failure recovery: if a previous attempt failed, inject its diagnosis so
    // the re-run fixes the cause instead of repeating it.
    let attribution_section = if matches!(task.kind, TaskKind::Agent) {
        attributions.get(task_id).map(|d| {
            format!(
                "## Previous attempt failed\n\nYour last attempt at this task failed. \
                 Diagnosis below — fix the underlying cause, don't just retry blindly.\n\n```\n{}\n```",
                d.trim()
            )
        })
    } else {
        None
    };

    // codegraph: inject a "relevant code" section (symbols + file:line) so the
    // agent is grounded in the actual codebase structure.
    let code_context_section = if matches!(task.kind, TaskKind::Agent) {
        match projects.resolved_path(&task.project).ok() {
            Some(r) => {
                // Keep the project's declared contract files in scope so a
                // subpackage consumer still sees its interface symbols.
                let keep: Vec<String> = projects
                    .projects
                    .get(&task.project)
                    .map(|p| {
                        p.contracts
                            .provides
                            .iter()
                            .chain(p.contracts.consumes.iter())
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                build_code_context(&r, &task.prompt, &keep)
            }
            None => None,
        }
    } else {
        None
    };

    // Inject the actual consumed-contract content so a downstream agent codes
    // against the producer's real (just-integrated) interface.
    let contract_section = if matches!(task.kind, TaskKind::Agent) {
        consumed_contract_section(projects, &task.project, integrations)
    } else {
        None
    };

    adapter::join_prompt_prelude(&[
        instruction_section,
        attribution_section,
        contract_section,
        code_context_section,
        inbox_section,
        mode_section,
        role_prelude,
        skill_section,
    ])
}

/// Assemble an agent task's read-only memory context: topic-scoped L1 facts,
/// contract- and topology-aware L2 fan-in, and prompt-similarity TF-IDF
/// retrieval (index cached across the dispatch wave via `memory_index`), each
/// slice byte-capped. Pure read of the memory store — no run-state, no async —
/// extracted from `dispatch_one_task` to keep that function focused.
fn assemble_memory_context(
    task: &PlanTask,
    projects: &ProjectsConfig,
    memory: &MemoryStore,
    memory_index: &mut Option<Vec<crate::memory::retrieval::Chunk>>,
) -> Vec<MemorySlice> {
    let topics = resolve_memory_topics(task, projects);
    let mut context = if matches!(task.kind, TaskKind::Agent) {
        // Mirror the sibling load_l2_* paths below: a memory-load failure
        // shouldn't kill the task (the agent can still work without facts),
        // but it MUST be visible in the logs so the user can diagnose
        // "where did my memory go?" without grepping for silent fallbacks.
        match memory.load_for_topics(&topics) {
            Ok(slices) => slices,
            Err(e) => {
                tracing::warn!(
                    "memory load_for_topics failed for task {}: {e:#} (continuing with empty context)",
                    task.id,
                );
                vec![]
            }
        }
    } else {
        vec![]
    };

    // Contract-aware fan-in: if this project consumes a contract,
    // pull the most recent L2 decisions from whatever project
    // PRODUCES that contract. The consumer doesn't need to know
    // who the producer is — the projects.yaml graph does.
    if matches!(task.kind, TaskKind::Agent) && task.project != "_global" {
        match memory.load_l2_for_contract(&task.project, projects) {
            Ok(slices) if !slices.is_empty() => {
                tracing::debug!(
                    "task {} pulling {} L2 slice(s) via contract.consumes",
                    task.id,
                    slices.len()
                );
                context.extend(slices);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("contract-aware L2 fan-in failed: {e:#}"),
        }

        // Topology-aware fan-in: also pull recent L2 from every project
        // this one depends on (incl. edges discovery inferred from
        // source imports), deduped against the contract fan-in above.
        match memory.load_l2_for_dependencies(&task.project, projects) {
            Ok(slices) if !slices.is_empty() => {
                let already: std::collections::HashSet<String> =
                    context.iter().map(|s| s.topic.clone()).collect();
                let mut added = 0_usize;
                for slice in slices {
                    if already.contains(&slice.topic) {
                        continue;
                    }
                    context.push(slice);
                    added += 1;
                }
                if added > 0 {
                    tracing::debug!(
                        "task {} pulling {} L2 slice(s) via dependency topology",
                        task.id,
                        added
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("topology-aware L2 fan-in failed: {e:#}"),
        }
    }

    // Semantic (lexical TF-IDF) retrieval: top-K relevant stable
    // L1/L2 snippets by the task prompt itself, in addition to
    // the scope-based injection above. Replan transcripts stay
    // available to dedicated replan flows, but are filtered from
    // ordinary task dispatch so stale failures do not swamp the
    // deterministic workflow inputs. Bounded by content-byte budget
    // so we don't blow the prompt context window.
    if matches!(task.kind, TaskKind::Agent) {
        // Build the retrieval index once per dispatch wave and reuse it across
        // the tasks dispatched together (run_plan invalidates it after each
        // completion, when new L2 may have been archived) instead of re-reading
        // and re-tokenizing the whole memory corpus once per task. On a build
        // error we log and skip retrieval — matching the sibling memory loads,
        // which warn rather than failing silently.
        let index_ref: Option<&Vec<crate::memory::retrieval::Chunk>> = match memory_index {
            Some(ref idx) => Some(idx),
            None => match crate::memory::retrieval::build_index() {
                Ok(built) => {
                    *memory_index = Some(built);
                    memory_index.as_ref()
                }
                Err(e) => {
                    tracing::warn!(
                        "retrieval index build failed for task {}: {e:#} (continuing without prompt-similarity retrieval)",
                        task.id,
                    );
                    None
                }
            },
        };
        if let Some(index) = index_ref {
            if !index.is_empty() {
                // De-duplicate against what's already in `context`
                // by id, so a fact pulled by topic-scope doesn't
                // also get pulled as a retrieval hit.
                let already: std::collections::HashSet<String> =
                    context.iter().map(|s| s.topic.clone()).collect();
                // Scope L2 retrieval to this task's project + its
                // dependencies, so it doesn't surface unrelated
                // projects' decisions on a shared prompt term.
                let mut relevant_projects = vec![task.project.clone()];
                if let Some(p) = projects.projects.get(&task.project) {
                    relevant_projects.extend(p.dependencies.iter().cloned());
                }
                let raw_hits = crate::memory::retrieval::retrieve_task_context(
                    index,
                    &task.prompt,
                    5,
                    &relevant_projects,
                );
                let fitted = crate::memory::retrieval::fit_budget(raw_hits, 3_500, 1_500);
                let mut added = 0_usize;
                for hit in fitted {
                    let slice = hit.into_slice();
                    if already.contains(&slice.topic) {
                        continue;
                    }
                    context.push(slice);
                    added += 1;
                }
                if added > 0 {
                    tracing::debug!(
                        "task {} adding {} retrieval slice(s) (top-K by prompt similarity)",
                        task.id,
                        added
                    );
                }
            }
        }
    }

    // Cap each memory slice so a single oversized L1/L2 file can't blow the
    // prompt context window. The TF-IDF hits above are already byte-budgeted;
    // this bounds the topic-scoped and contract/topology fan-in slices too.
    const MAX_SLICE_CHARS: usize = 8_000;
    for slice in context.iter_mut() {
        if slice.content.chars().count() > MAX_SLICE_CHARS {
            let capped: String = slice.content.chars().take(MAX_SLICE_CHARS).collect();
            slice.content = format!("{capped}\n… (memory slice truncated)");
        }
    }
    context
}

/// Mark a task Failed during the synchronous setup phase of `dispatch_one_task`
/// (workflow-input / skill / worktree-policy resolution), write a one-line log,
/// record the failure event, and return the loop control: `Break` to abort the
/// run (default) or `Proceed` to keep other independent tasks moving under
/// `--continue-on-error`. Extracted from four byte-identical inline blocks.
#[allow(clippy::too_many_arguments)]
async fn fail_task_inline(
    state: &Arc<Mutex<RunState>>,
    run_dir: &Path,
    run_id: &str,
    channels_config: Option<&ChannelConfig>,
    envelope_dry_run: bool,
    state_tx: &broadcast::Sender<()>,
    cfg: &ExecConfig,
    log_path: &Path,
    task_id: &str,
    stage: &str,
    message: String,
    ready: &mut VecDeque<String>,
    running: &mut JoinSet<TaskJoin>,
    had_failure: &mut bool,
) -> Result<LoopFlow> {
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(task_id) {
            ts.status = TaskStatus::Failed;
            ts.ended_at = Some(Utc::now());
            ts.error = Some(message.clone());
        }
    }
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(log_path, format!("[maestro] {stage} failed: {message}\n"));
    write_state(state).await?;
    record_event(
        run_dir,
        run_id,
        RunEventKind::TaskFailed,
        Some(task_id),
        Some(format!("{stage} failed")),
        json!({ "error": message }),
        channels_config,
        envelope_dry_run,
    );
    let _ = state_tx.send(());
    *had_failure = true;
    if !cfg.continue_on_error {
        ready.clear();
        running.abort_all();
        while running.join_next().await.is_some() {}
        return Ok(LoopFlow::Break);
    }
    Ok(LoopFlow::Proceed)
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_one_task(
    state: &Arc<Mutex<RunState>>,
    plan: &Plan,
    projects: &ProjectsConfig,
    cfg: &ExecConfig,
    sem: &Arc<Semaphore>,
    memory: &MemoryStore,
    run_id: String,
    run_dir: PathBuf,
    channels_config: Option<ChannelConfig>,
    envelope_dry_run: bool,
    state_tx: &broadcast::Sender<()>,
    task_id: String,
    attributions: &HashMap<String, String>,
    integrations: &mut IntegrationWorktrees,
    running: &mut JoinSet<TaskJoin>,
    ready: &mut VecDeque<String>,
    blocked: &mut Vec<String>,
    had_failure: &mut bool,
    memory_index: &mut Option<Vec<crate::memory::retrieval::Chunk>>,
) -> Result<LoopFlow> {
    // Blocking-mail hard gate (A·①b): if an Agent task has unresolved
    // blocking messages addressed to it, set it aside (AwaitingApproval)
    // and move on — *other independent tasks keep dispatching*. The
    // loop-top recheck re-admits it once a peer/human resolves the mail.
    if let Some(ids) = task_identities(plan, projects, &task_id) {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        if open_blocking_count(&refs) > 0 {
            {
                let mut s = state.lock().await;
                if let Some(ts) = s.tasks.get_mut(&task_id) {
                    ts.status = TaskStatus::AwaitingApproval;
                }
            }
            write_state(state).await?;
            let _ = state_tx.send(());
            if !blocked.contains(&task_id) {
                blocked.push(task_id.clone());
            }
            tracing::info!(
                "task {task_id} gated on unresolved blocking mailbox message(s); \
                         continuing with other tasks"
            );
            return Ok(LoopFlow::Proceed);
        }
    }

    let permit = sem.clone().acquire_owned().await?;
    let task = plan
        .task(&task_id)
        .with_context(|| format!("task {} not in plan", task_id))?
        .clone();

    let (workspace, global_integration) =
        resolve_task_workspace(&task, projects, cfg, integrations, &run_dir)?;

    // mark running
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.status = TaskStatus::Running;
            ts.started_at = Some(Utc::now());
        }
    }
    write_state(state).await?;
    let _ = state_tx.send(());

    let log_path = {
        let s = state.lock().await;
        let ts = s.tasks.get(&task_id).with_context(|| {
            format!("task {task_id} disappeared from RunState between dispatch and log read")
        })?;
        PathBuf::from(ts.log_path.clone())
    };

    // Routing policy: pick agent/model from the task's static traits before it
    // runs. An explicit task.agent still wins; routing only overrides the
    // default per-project resolution. Empty policy = no-op.
    let (adapter_name, adapter, route) = resolve_adapter_for_task(&task, projects);

    // reflect the actually-picked adapter in state (not just the default)
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.agent = adapter_name.clone();
        }
    }
    record_event(
        &run_dir,
        &run_id,
        RunEventKind::TaskStarted,
        Some(&task_id),
        Some(format!("task started in project {}", task.project)),
        json!({
            "project": task.project,
            "agent": adapter_name,
            "kind": match task.kind {
                TaskKind::Agent => "agent",
                TaskKind::Verify => "verify",
            },
        }),
        channels_config.as_ref(),
        envelope_dry_run,
    );

    let mut context = assemble_memory_context(&task, projects, memory, memory_index);

    let workflow_inputs = match load_workflow_inputs(&task, state).await {
        Ok(inputs) => inputs,
        Err(e) => {
            let message = format!("{e:#}");
            return fail_task_inline(
                state,
                &run_dir,
                &run_id,
                channels_config.as_ref(),
                envelope_dry_run,
                state_tx,
                cfg,
                &log_path,
                &task_id,
                "workflow input resolution",
                message,
                ready,
                running,
                had_failure,
            )
            .await;
        }
    };
    context.extend(workflow_inputs);

    if !context.is_empty() {
        tracing::debug!(
            "task {} injecting {} memory slice(s)",
            task.id,
            context.len()
        );
    }

    // Resolve explicit + trigger-matched skills based on phrases in
    // the prompt. Explicit skill dependencies are fail-fast so a
    // workflow cannot accidentally run without its guardrails.
    let prompt_for_trigger = match task.kind {
        TaskKind::Agent => task.prompt.clone(),
        TaskKind::Verify => task.command.clone().unwrap_or_default(),
    };
    let resolved_role_name = crate::roles::resolve_for_task(
        task.role.as_deref(),
        projects.resolved_role(&task.project).as_deref(),
    );
    let resolved_role = resolved_role_name
        .as_deref()
        .and_then(crate::roles::try_load);
    let mode = resolved_role
        .as_ref()
        .map(crate::modes::Mode::from_role)
        .unwrap_or_else(crate::modes::Mode::permissive);
    let permission = crate::schema::permissions::resolve_permission_evidence(
        &task_id,
        &adapter_name,
        &mode.id,
        &mode.allowed_tools,
    );
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.permission = Some(permission);
        }
    }
    let injected_skills = if matches!(task.kind, TaskKind::Agent) {
        let skill_result = if mode.skills.is_empty() {
            crate::skills::resolve_for_task(&prompt_for_trigger, Some(&task.project), &task.skills)
        } else {
            crate::skills::for_mode(&mode)
        };
        match skill_result {
            Ok(skills) => skills,
            Err(e) => {
                let message = format!("{e:#}");
                return fail_task_inline(
                    state,
                    &run_dir,
                    &run_id,
                    channels_config.as_ref(),
                    envelope_dry_run,
                    state_tx,
                    cfg,
                    &log_path,
                    &task_id,
                    "skill resolution",
                    message,
                    ready,
                    running,
                    had_failure,
                )
                .await;
            }
        }
    } else {
        Vec::new()
    };
    let skill_section = crate::skills::render_task_section(&injected_skills);

    // Record memory + skills used on the task state so the UI can show
    // them in the per-task expand panel. Also stash the total context
    // byte count so users can answer "did the model see what I told it
    // to remember?" without digging through the prompt log.
    let context_bytes: u64 = context.iter().map(|m| m.content.len() as u64).sum();
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.memory_used = context.iter().map(|m| m.topic.clone()).collect();
            ts.skills_triggered = injected_skills
                .iter()
                .map(|s| format!("{}/{}", s.scope.dir_name(), s.name))
                .collect();
            // Only set for Agent tasks where context injection actually ran;
            // verify / shell tasks leave it None to keep the dashboard honest.
            if matches!(task.kind, TaskKind::Agent) {
                ts.context_bytes = Some(context_bytes);
            }
        }
    }

    // Resolve model: explicit model -> run override -> model profile
    // -> project/default model. None means the adapter uses its
    // account default.
    let model = projects.resolved_task_model(
        &task.project,
        task.model.as_deref(),
        cfg.model_override.as_deref(),
        task.model_profile.as_deref(),
        resolved_role_name.as_deref(),
    );
    // Routing model override applies only when the task didn't pin a model.
    let model = if task.model.is_none() {
        route.model.clone().or(model)
    } else {
        model
    };
    // `project_instructions` is also read downstream (the task log + AgentTask),
    // so compute it here and pass the rendered section into the prelude helper.
    let project_instructions = if matches!(task.kind, TaskKind::Agent) {
        crate::project_instructions::load_for_task(projects, &task.project)
    } else {
        Vec::new()
    };
    let instruction_section = crate::project_instructions::render_section(&project_instructions);
    let prompt_prelude = assemble_prompt_prelude(
        &task,
        &task_id,
        projects,
        integrations,
        &resolved_role,
        &resolved_role_name,
        &mode,
        instruction_section,
        skill_section,
        attributions,
    );
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.role = resolved_role
                .as_ref()
                .map(|r| r.name.clone())
                .or(resolved_role_name.clone());
        }
    }

    // Adapter-agnostic memory header in the log.
    let mut header = format!(
        "[maestro] task={} project={} adapter={} kind={:?} model={} role={}\n",
        task.id,
        task.project,
        adapter_name,
        task.kind,
        model.as_deref().unwrap_or("(adapter default)"),
        resolved_role
            .as_ref()
            .map(|r| r.name.as_str())
            .unwrap_or("(none)")
    );
    if context.is_empty() {
        header.push_str("[maestro] memory: (none injected — set memory_scope on the project or memory_inject on the task)\n");
    } else {
        header.push_str(&format!(
            "[maestro] memory: injecting {} slice(s):\n",
            context.len()
        ));
        for slice in &context {
            header.push_str(&format!("[maestro]   - {}\n", slice.topic));
        }
    }
    if !injected_skills.is_empty() {
        header.push_str(&format!(
            "[maestro] skills: injecting {} skill(s):\n",
            injected_skills.len()
        ));
        for skill in &injected_skills {
            header.push_str(&format!(
                "[maestro]   - {}/{}\n",
                skill.scope.dir_name(),
                skill.name
            ));
        }
    }
    if !project_instructions.is_empty() {
        header.push_str(&format!(
            "[maestro] AGENTS.md: injecting {} file(s):\n",
            project_instructions.len()
        ));
        for instruction in &project_instructions {
            header.push_str(&format!("[maestro]   - {}\n", instruction.source));
        }
    }
    header.push_str("[maestro] ----\n");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&log_path, header).ok();

    // Decide whether to isolate this task in a git worktree.
    // Skip when the user opted out, the workspace isn't a git repo
    // (e.g. `_global`, or freshly scaffolded with --no-git), or
    // when concurrency is 1 so there's nothing to race against.
    let git_worktree_context =
        if cfg.isolate_with_worktrees && cfg.max_parallel > 1 && task.project != "_global" {
            gitops::worktree_context(&workspace)
        } else {
            None
        };
    let worktree_policy = if git_worktree_context.is_some() {
        match projects
            .projects
            .get(&task.project)
            .map(|project| WorktreePolicy::from_config(&projects.defaults, project))
            .transpose()
            .and_then(|policy| {
                policy
                    .map(|policy| policy.validate_copy_files(&workspace).map(|()| policy))
                    .transpose()
            }) {
            Ok(policy) => policy,
            Err(e) => {
                let message = format!("worktree policy rejected copy_files: {e}");
                return fail_task_inline(
                    state,
                    &run_dir,
                    &run_id,
                    channels_config.as_ref(),
                    envelope_dry_run,
                    state_tx,
                    cfg,
                    &log_path,
                    &task_id,
                    "worktree policy",
                    message,
                    ready,
                    running,
                    had_failure,
                )
                .await;
            }
        }
    } else {
        None
    };
    let integration_start = if let Some(context) = git_worktree_context.as_ref() {
        match integrations.ensure(&context.repo_root, &run_dir, &run_id) {
            Ok(integration) => Some(integration.branch),
            Err(e) => {
                tracing::warn!(
                            "task {task_id}: integration worktree create failed ({e:#}); task will start from repo HEAD"
                        );
                None
            }
        }
    } else {
        None
    };
    let worktree_guard = if let Some(context) = git_worktree_context.as_ref() {
        let target = run_dir
            .join("worktrees")
            .join(&task_id)
            .canonicalize()
            .unwrap_or_else(|_| run_dir.join("worktrees").join(&task_id));
        let branch = format!("maestro/{}/{}", run_id, task_id);
        match gitops::WorktreeGuard::create_from(
            context.repo_root.clone(),
            target,
            branch,
            integration_start.as_deref(),
        ) {
            Ok(g) => {
                if let Some(policy) = worktree_policy.as_ref() {
                    let copy_target = if context.project_relative_path.as_os_str().is_empty() {
                        g.path().to_path_buf()
                    } else {
                        g.path().join(&context.project_relative_path)
                    };
                    if let Err(e) = policy.copy_files_into(&workspace, &copy_target) {
                        let message = format!("worktree policy copy failed: {e}");
                        return fail_task_inline(
                            state,
                            &run_dir,
                            &run_id,
                            channels_config.as_ref(),
                            envelope_dry_run,
                            state_tx,
                            cfg,
                            &log_path,
                            &task_id,
                            "worktree policy",
                            message,
                            ready,
                            running,
                            had_failure,
                        )
                        .await;
                    }
                }
                Some(g)
            }
            Err(e) => {
                // Non-fatal: log a warning and fall back to running
                // in the original workspace. We'd rather lose the
                // isolation than abort the whole DAG.
                tracing::warn!(
                            "task {task_id}: git worktree create failed ({e:#}); running in original workspace"
                        );
                None
            }
        }
    } else {
        None
    };
    let worktree_branch = worktree_guard.as_ref().map(|g| g.branch().to_string());
    let execution_branch = worktree_branch
        .clone()
        .or_else(|| global_integration.as_ref().map(|i| i.branch.clone()));
    let effective_workspace = match (worktree_guard.as_ref(), git_worktree_context.as_ref()) {
        (Some(g), Some(context)) if context.project_relative_path.as_os_str().is_empty() => {
            g.path().to_path_buf()
        }
        (Some(g), Some(context)) => g.path().join(&context.project_relative_path),
        _ => workspace.clone(),
    };
    if worktree_guard.is_some() && !effective_workspace.exists() {
        if let Err(e) = std::fs::create_dir_all(&effective_workspace) {
            tracing::warn!(
                "task {task_id}: could not create mapped worktree workspace {}: {e:#}",
                effective_workspace.display()
            );
        }
    }
    let trajectory_path = matches!(adapter_name.as_str(), "codex" | "shell")
        .then(|| trajectory::trajectory_path(&run_dir, &task_id));
    {
        let mut s = state.lock().await;
        if let Some(ts) = s.tasks.get_mut(&task_id) {
            ts.workspace_path = Some(effective_workspace.to_string_lossy().to_string());
            ts.trajectory_path = trajectory_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
            ts.worktree_path = worktree_guard
                .as_ref()
                .map(|g| g.path().to_string_lossy().to_string())
                .or_else(|| {
                    global_integration
                        .as_ref()
                        .map(|integration| integration.path.to_string_lossy().to_string())
                });
            if ts.artifacts.branch.is_none() {
                ts.artifacts.branch = execution_branch.clone();
            }
        }
    }
    write_state(state).await?;
    let _ = state_tx.send(());

    let agent_task = AgentTask {
        task_id: task.id.clone(),
        workspace: effective_workspace.clone(),
        prompt: match task.kind {
            TaskKind::Agent => task.prompt.clone(),
            TaskKind::Verify => task.command.clone().unwrap_or_else(|| task.prompt.clone()),
        },
        context,
        timeout: Duration::from_secs(task.timeout_minutes * 60),
        mode: ExecutionMode::Apply,
        resume_chat_id: None,
        log_path,
        trajectory: trajectory_path.map(|path| adapter::TrajectoryContext {
            run_id: run_id.clone(),
            task_id: task_id.clone(),
            provider_id: adapter_name.clone(),
            path,
        }),
        model,
        role_prelude: prompt_prelude,
        allowed_tools: mode.allowed_tools.clone(),
    };

    let state_for_task = state.clone();
    let state_tx_for_task = state_tx.clone();
    let task_id_for_join = task_id.clone();
    let run_dir_for_task = run_dir.clone();
    let run_id_for_task = run_id.clone();
    let output_workspace = effective_workspace.clone();
    let output_defs = task.outputs.clone();
    let worktree_branch_for_task = execution_branch.clone();
    let channels_config_for_task = channels_config.clone();
    let envelope_dry_run_for_task = envelope_dry_run;

    running.spawn(async move {
        let _permit = permit;
        // Hold the worktree alive for the entire agent run. We keep
        // the checkout after completion so uncommitted agent edits are
        // still inspectable and `maestro pr draft` has a real branch
        // + workspace to target.
        let wt = worktree_guard;
        let track_changed_files = gitops::is_git_repo(&output_workspace);
        let files_changed_before = if track_changed_files {
            gitops::changed_files(&output_workspace).ok()
        } else {
            None
        };
        let mut res = adapter.run(agent_task).await;
        let mut workflow_outputs = BTreeMap::new();
        if let Ok(r) = &res {
            match capture_task_outputs(
                &run_dir_for_task,
                &output_workspace,
                &task_id_for_join,
                &output_defs,
                &r.transcript_summary,
            )
            .await
            {
                Ok(outputs) => workflow_outputs = outputs,
                Err(e) => res = Err(e),
            }
        }
        let inferred_files_changed = if res.is_ok() && track_changed_files {
            match gitops::changed_files(&output_workspace) {
                Ok(files) => match files_changed_before.as_ref() {
                    Some(before) => files
                        .into_iter()
                        .filter(|file| !before.contains(file))
                        .collect(),
                    None => files,
                },
                Err(e) => {
                    tracing::debug!(
                        "task {task_id_for_join}: could not infer changed files: {e:#}"
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let kept_worktree_path = wt.map(|guard| guard.keep());
        let output_names = workflow_outputs.keys().cloned().collect::<Vec<_>>();
        let event = match &res {
            Ok(r) => (
                RunEventKind::TaskSucceeded,
                Some("task succeeded".to_string()),
                json!({
                    "chat_id": r.chat_id,
                    "steps": r.steps,
                    "outputs": output_names,
                    "branch": worktree_branch_for_task.clone(),
                    "worktree_kept": kept_worktree_path,
                }),
            ),
            Err(e) => (
                RunEventKind::TaskFailed,
                Some("task failed".to_string()),
                json!({
                    "error": format!("{e:#}"),
                    "branch": worktree_branch_for_task.clone(),
                    "worktree_kept": kept_worktree_path,
                }),
            ),
        };
        {
            let mut s = state_for_task.lock().await;
            if let Some(ts) = s.tasks.get_mut(&task_id_for_join) {
                ts.ended_at = Some(Utc::now());
                match &res {
                    Ok(r) => {
                        ts.status = TaskStatus::Done;
                        ts.chat_id = r.chat_id.clone();
                        let mut artifacts = r.artifacts.clone();
                        artifacts
                            .files_changed
                            .retain(|file| !gitops::is_transient_artifact(file));
                        if artifacts.branch.is_none() {
                            artifacts.branch = worktree_branch_for_task.clone();
                        }
                        if artifacts.files_changed.is_empty() {
                            artifacts.files_changed = inferred_files_changed;
                        }
                        ts.artifacts = artifacts;
                        ts.workflow_outputs = workflow_outputs;
                        ts.steps = r.steps;
                        if let Some(u) = &r.usage {
                            ts.usage = Some(u.clone());
                            s.usage.add(u);
                        }
                    }
                    Err(e) => {
                        ts.status = TaskStatus::Failed;
                        ts.error = Some(format!("{e:#}"));
                    }
                }
            }
        }
        record_event(
            &run_dir_for_task,
            &run_id_for_task,
            event.0,
            Some(&task_id_for_join),
            event.1,
            event.2,
            channels_config_for_task.as_ref(),
            envelope_dry_run_for_task,
        );
        let _ = state_tx_for_task.send(());
        (task_id_for_join, res)
    });
    Ok(LoopFlow::Proceed)
}

pub async fn run_plan(plan: Plan, projects: ProjectsConfig, cfg: ExecConfig) -> Result<RunState> {
    let memory = MemoryStore::open()?;

    // Keep codegraph indexes fresh so the per-task "Relevant code" injection
    // reflects the current tree. Best-effort: skips repos without an index or
    // when the CLI is missing.
    sync_codegraph_indexes(&plan, &projects).await;

    let run_id = cfg.run_id.clone().unwrap_or_else(generate_run_id);

    let run_dir = paths::runs_dir()?.join(&run_id);
    paths::ensure_dir(&run_dir)?;
    paths::ensure_dir(&run_dir.join("logs"))?;
    paths::ensure_dir(&paths::approvals_dir()?)?;
    let channels_config = crate::channel::load_channels_config(&paths::workspace_root()?);
    let envelope_dry_run = channels_config
        .as_ref()
        .and_then(|_| crate::channel::resolve(&run_dir).ok().flatten())
        .map(|envelope| envelope.dry_run)
        .unwrap_or(false);

    // snapshot plan
    std::fs::write(
        run_dir.join(paths::PLAN_SNAPSHOT),
        serde_yaml::to_string(&plan)?,
    )
    .context("write PLAN snapshot")?;

    // refresh `current` symlink
    let current = paths::current_run_link()?;
    let _ = std::fs::remove_file(&current);
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&run_id, &current);
    #[cfg(not(unix))]
    let _ = std::fs::write(&current, &run_id);

    let mut state = RunState::new(
        run_id.clone(),
        &plan,
        &projects,
        cfg.max_parallel,
        run_dir.clone(),
    );
    state.session_id = cfg.session_id.clone();
    state.budget_tokens = cfg.max_tokens;
    for e in &cfg.wired_contract_edges {
        state.auto_actions.push(AutoAction {
            kind: "contract_wired".into(),
            task: Some(e.consumer.clone()),
            detail: format!(
                "`{}` set to depend on `{}` (contract `{}`) so it can't run against a stale contract",
                e.consumer, e.producer, e.contract
            ),
        });
    }
    let state = Arc::new(Mutex::new(state));
    write_state(&state).await?;
    record_event(
        &run_dir,
        &run_id,
        RunEventKind::RunCreated,
        None,
        Some(format!("run started: {}", plan.spec)),
        json!({
            "spec": plan.spec,
            "task_count": plan.tasks.len(),
            "max_parallel": cfg.max_parallel,
            "session_id": cfg.session_id,
        }),
        channels_config.as_ref(),
        envelope_dry_run,
    );

    let (state_tx, _) = broadcast::channel::<()>(64);

    // Bundle the run plumbing once so the phase helpers stay readable.
    let ctx = RunCtx {
        run_id: run_id.clone(),
        run_dir: run_dir.clone(),
        state: state.clone(),
        state_tx: state_tx.clone(),
        channels: channels_config.clone(),
        dry_run: envelope_dry_run,
    };

    // Plan gate (boundary review): pause before any task runs until a human
    // approves the plan — the "intent" boundary.
    if cfg.plan_gate {
        eprintln!(
            "⏸ plan gate: review the plan, then `maestro approve {GATE_PLAN}` (or approve in the dashboard) to start"
        );
        if let ApprovalWaitOutcome::Cancelled = ctx
            .await_gate(
                "plan",
                GATE_PLAN,
                "plan gate: run awaiting approval to start",
                json!({ "gate": "plan", "task_count": plan.tasks.len() }),
            )
            .await?
        {
            let final_state = {
                let mut s = ctx.lock().await;
                s.pending_gate = None;
                s.status = RunStatus::Cancelled;
                s.ended_at = Some(Utc::now());
                s.clone()
            };
            ctx.write_state().await?;
            ctx.record(
                RunEventKind::RunCancelled,
                None,
                Some("plan gate rejected — run cancelled before any task ran".to_string()),
                json!({ "gate": "plan" }),
            );
            return Ok(final_state);
        }
    }

    let graph = TaskGraph::from_plan(&plan)?;
    let sem = Arc::new(Semaphore::new(cfg.max_parallel.max(1)));
    let mut running: JoinSet<TaskJoin> = JoinSet::new();
    let mut integrations = IntegrationWorktrees::default();

    let only: Option<HashSet<String>> = cfg.only.as_ref().map(|v| v.iter().cloned().collect());
    let skip: HashSet<String> = cfg.skip.iter().cloned().collect();

    let mut ready: VecDeque<String> = VecDeque::from_iter(graph.initial_ready());
    // Tasks that satisfy dependency gates: succeeded tasks plus explicit run
    // filters/skips. Failed tasks are intentionally excluded so
    // `--continue-on-error` keeps only independent work moving.
    let mut completed: HashSet<String> = HashSet::new();
    let mut had_failure = false;
    let mut budget_tripped = false;

    // Failure recovery: re-run a failed task up to DEFAULT_MAX_RETRIES times,
    // injecting a heuristic diagnosis (error + log tail) of the previous
    // attempt into the next run's prompt. `retries_used` bounds the loop;
    // `attributions` carries the diagnosis into the re-dispatch.
    let mut retries_used: HashMap<String, u32> = HashMap::new();
    let mut attributions: HashMap<String, String> = HashMap::new();
    // Circuit breaker: the normalized error signature of a task's previous
    // attempt. If a retry reproduces the same signature (no progress), we stop
    // retrying early instead of burning the whole budget on an identical fail.
    let mut last_errors: HashMap<String, String> = HashMap::new();

    // mark skipped tasks up-front
    let mut skipped_tasks = Vec::new();
    let mut seeded_skipped_tasks = Vec::new();
    {
        let mut s = state.lock().await;
        for t in &plan.tasks {
            let should_skip =
                skip.contains(&t.id) || only.as_ref().map(|o| !o.contains(&t.id)).unwrap_or(false);
            if should_skip {
                let previous = cfg
                    .seed_skipped_from
                    .as_ref()
                    .and_then(|seed| seed.tasks.get(&t.id))
                    .filter(|ts| ts.status == TaskStatus::Done)
                    .cloned();
                if let Some(ts) = s.tasks.get_mut(&t.id) {
                    if let Some(previous) = previous {
                        ts.artifacts = previous.artifacts.clone();
                        ts.artifacts
                            .files_changed
                            .retain(|file| !gitops::is_transient_artifact(file));
                        ts.workflow_outputs = previous.workflow_outputs.clone();
                        ts.chat_id = previous.chat_id.clone();
                        ts.steps = previous.steps;
                        ts.usage = previous.usage.clone();
                        ts.permission = previous.permission.clone();
                        ts.trajectory_path = previous.trajectory_path.clone();
                        ts.workspace_path = previous.workspace_path.clone();
                        ts.worktree_path = previous.worktree_path.clone();
                        let repo_root_hint = if t.project == "_global" {
                            None
                        } else {
                            projects
                                .resolved_path(&t.project)
                                .ok()
                                .and_then(|path| gitops::worktree_context(&path))
                                .map(|context| context.repo_root)
                        };
                        seeded_skipped_tasks.push((t.id.clone(), ts.clone(), repo_root_hint));
                    }
                    ts.status = TaskStatus::Skipped;
                    ts.ended_at = Some(Utc::now());
                }
                completed.insert(t.id.clone());
                skipped_tasks.push(t.id.clone());
            }
        }
    }
    for task_id in &skipped_tasks {
        record_event(
            &run_dir,
            &run_id,
            RunEventKind::TaskSkipped,
            Some(task_id),
            Some("task skipped by run filter".to_string()),
            json!({}),
            channels_config.as_ref(),
            envelope_dry_run,
        );
    }
    for (task_id, seeded_task, repo_root_hint) in &seeded_skipped_tasks {
        match integrations.integrate_seed_task(
            &run_dir,
            &run_id,
            task_id,
            seeded_task,
            repo_root_hint.as_deref(),
        ) {
            Ok(true) => record_event(
                &run_dir,
                &run_id,
                RunEventKind::TaskSucceeded,
                Some(task_id),
                Some("seeded skipped task patch integrated".to_string()),
                json!({
                    "integration": true,
                    "rerun_seed": true,
                    "branch": seeded_task.artifacts.branch,
                }),
                channels_config.as_ref(),
                envelope_dry_run,
            ),
            Ok(false) => {}
            Err(e) => {
                anyhow::bail!("seed skipped task {task_id} from previous run failed: {e:#}");
            }
        }
    }
    write_state(&state).await?;
    let _ = state_tx.send(());

    // expand ready set across skipped front nodes
    let mut work_queue = VecDeque::new();
    let mut seen_ready: HashSet<String> = ready.iter().cloned().collect();
    let mut seen_work = HashSet::new();
    while let Some(id) = ready.pop_front() {
        if completed.contains(&id) {
            for d in graph.downstream(&id) {
                if all_deps_done(&plan, &completed, &d) && seen_ready.insert(d.clone()) {
                    ready.push_back(d);
                }
            }
        } else if seen_work.insert(id.clone()) {
            work_queue.push_back(id);
        }
    }
    ready = work_queue;

    let mut cancelled = false;
    // Tasks set aside by the blocking-mail gate; re-admitted once their mail
    // clears (see the loop-top recheck below).
    let mut blocked: Vec<String> = Vec::new();
    // When the run is making no progress because everything left is parked on
    // the mailbox, this marks when that stall began so we can time-bound it.
    let mut all_blocked_since: Option<std::time::Instant> = None;
    // Memory retrieval index, built once per dispatch wave and shared across the
    // tasks dispatched together; invalidated at the end of each iteration so a
    // task that archived new L2 is reflected in the next wave's retrieval.
    let mut memory_index: Option<Vec<crate::memory::retrieval::Chunk>> = None;
    while !ready.is_empty() || !running.is_empty() || !blocked.is_empty() {
        if check_cancelled(&run_id)? {
            tracing::warn!("run {} cancelled via `maestro cancel-run`", run_id);
            record_event(
                &run_dir,
                &run_id,
                RunEventKind::CancelRequested,
                None,
                Some("cancel marker detected".to_string()),
                json!({}),
                channels_config.as_ref(),
                envelope_dry_run,
            );
            cancelled = true;
            running.abort_all();
            while running.join_next().await.is_some() {}
            break;
        }
        // Re-admit gated tasks whose blocking mail has cleared.
        if !blocked.is_empty() {
            let mut still = Vec::new();
            for tid in std::mem::take(&mut blocked) {
                let cleared = match task_identities(&plan, &projects, &tid) {
                    Some(ids) => {
                        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                        open_blocking_count(&refs) == 0
                    }
                    None => true,
                };
                if cleared {
                    {
                        let mut s = state.lock().await;
                        if let Some(ts) = s.tasks.get_mut(&tid) {
                            ts.status = TaskStatus::Pending;
                        }
                    }
                    let _ = state_tx.send(());
                    ready.push_back(tid);
                    // Progress was made; reset the no-progress stall timer.
                    all_blocked_since = None;
                } else {
                    still.push(tid);
                }
            }
            blocked = still;
        }

        // dispatch
        while let Some(task_id) = ready.pop_front() {
            if running.len() >= cfg.max_parallel.max(1) {
                ready.push_front(task_id);
                break;
            }

            if let LoopFlow::Break = dispatch_one_task(
                &state,
                &plan,
                &projects,
                &cfg,
                &sem,
                &memory,
                run_id.clone(),
                run_dir.clone(),
                channels_config.clone(),
                envelope_dry_run,
                &state_tx,
                task_id,
                &attributions,
                &mut integrations,
                &mut running,
                &mut ready,
                &mut blocked,
                &mut had_failure,
                &mut memory_index,
            )
            .await?
            {
                break;
            }
        }

        // If nothing is running and the only remaining work is gated on the
        // mailbox, poll for clearance instead of exiting the run.
        if running.is_empty() && !blocked.is_empty() {
            if check_cancelled(&run_id)? {
                cancelled = true;
                break;
            }
            // Bound this wait like every other one in the system. After the
            // deadline, escalate the parked tasks to a human-visible failure
            // and let the run finalize rather than polling forever.
            let since = *all_blocked_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= BLOCKED_MAIL_TIMEOUT {
                let secs = BLOCKED_MAIL_TIMEOUT.as_secs();
                let parked = std::mem::take(&mut blocked);
                {
                    let mut s = state.lock().await;
                    for tid in &parked {
                        if let Some(ts) = s.tasks.get_mut(tid) {
                            ts.status = TaskStatus::Failed;
                            ts.error = Some(format!(
                                "blocked on unresolved blocking mailbox message for >{secs}s — needs human attention"
                            ));
                        }
                    }
                }
                write_state(&state).await?;
                for tid in &parked {
                    record_event(
                        &run_dir,
                        &run_id,
                        RunEventKind::TaskFailed,
                        Some(tid),
                        Some(format!("escalated: blocking mail unresolved for >{secs}s")),
                        json!({ "blocked_mail_timeout": true }),
                        channels_config.as_ref(),
                        envelope_dry_run,
                    );
                }
                let _ = state_tx.send(());
                had_failure = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
            continue;
        }

        // wait for one to finish, but keep polling cancellation while all
        // permits are occupied by long-running tasks.
        let joined = loop {
            tokio::select! {
                joined = running.join_next() => break joined,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    if check_cancelled(&run_id)? {
                        tracing::warn!("run {} cancelled via `maestro cancel-run`", run_id);
                        record_event(
                            &run_dir,
                            &run_id,
                            RunEventKind::CancelRequested,
                            None,
                            Some("cancel marker detected".to_string()),
                            json!({}),
                            channels_config.as_ref(),
                            envelope_dry_run,
                        );
                        cancelled = true;
                        running.abort_all();
                        while running.join_next().await.is_some() {}
                        break None;
                    }
                }
            }
        };
        if cancelled {
            break;
        }
        let Some(joined) = joined else {
            break;
        };
        let (task_id, res) = joined?;
        write_state(&state).await?;
        let _ = state_tx.send(());

        match handle_task_failure(
            &ctx,
            &projects,
            &cfg,
            &task_id,
            &res,
            &mut retries_used,
            &mut attributions,
            &mut last_errors,
            &mut had_failure,
            &mut ready,
            &mut running,
        )
        .await?
        {
            LoopFlow::Continue => continue,
            LoopFlow::Break => break,
            LoopFlow::Proceed => {}
        }

        // ③ reviewer: review a successful task's work. A FAIL verdict feeds the
        // same attribution → retry recovery path as a real failure.
        match handle_task_review(
            &ctx,
            &plan,
            &projects,
            &cfg,
            &task_id,
            &mut retries_used,
            &mut attributions,
            &mut had_failure,
            &mut ready,
            &mut running,
        )
        .await?
        {
            LoopFlow::Continue => continue,
            LoopFlow::Break => break,
            LoopFlow::Proceed => {}
        }

        // Budget gate: once cumulative token usage crosses the cap, escalate and
        // stop — an agent run shouldn't silently burn unbounded tokens.
        if let LoopFlow::Break = check_budget_gate(
            &ctx,
            &cfg,
            &task_id,
            &mut running,
            &mut budget_tripped,
            &mut had_failure,
        )
        .await?
        {
            break;
        }

        // handle approval after task completion
        handle_post_task_approval(
            &ctx,
            &plan,
            &projects,
            &task_id,
            &mut running,
            &mut cancelled,
        )
        .await?;

        if cancelled {
            break;
        }

        if let LoopFlow::Break = integrate_completed_task(
            &ctx,
            &cfg,
            &task_id,
            &res,
            &mut integrations,
            &mut had_failure,
            &mut running,
        )
        .await?
        {
            break;
        }

        // Only treat the task as completed (and unblock its consumers) when it
        // actually reached Done. A producer that succeeded at the agent step but
        // FAILED to integrate (patch/merge conflict) under --continue-on-error
        // sets status=Failed yet returns Proceed above; unblocking its consumers
        // here would let them read the producer's *un*-integrated contract from
        // the integration worktree and code against a stale interface with no
        // warning. Leaving it out of `completed` lets mark_blocked_pending_tasks
        // correctly skip the consumers as blocked-by-failed-dependency.
        let task_done = ctx
            .lock()
            .await
            .tasks
            .get(&task_id)
            .map(|t| matches!(t.status, TaskStatus::Done))
            .unwrap_or(false);
        if task_done {
            completed.insert(task_id.clone());

            for d in graph.downstream(&task_id) {
                if all_deps_done(&plan, &completed, &d) && !completed.contains(&d) {
                    ready.push_back(d);
                }
            }
        }

        // A completed task may have archived new L2 decisions; drop the cached
        // retrieval index so the next dispatch wave rebuilds against fresh memory.
        memory_index = None;
    }

    // finalize
    let (final_status, blocked_tasks, cancelled_tasks) = {
        let mut s = state.lock().await;
        s.ended_at = Some(Utc::now());
        let (blocked_tasks, cancelled_tasks) = if cancelled {
            (
                Vec::new(),
                mark_unfinished_tasks_cancelled(&mut s, "run cancelled"),
            )
        } else if had_failure {
            let blocked = mark_blocked_pending_tasks(&mut s, &plan);
            let cancelled =
                mark_unfinished_tasks_cancelled(&mut s, "run aborted after task failure");
            (blocked, cancelled)
        } else {
            (Vec::new(), Vec::new())
        };
        s.status = if cancelled {
            RunStatus::Cancelled
        } else if had_failure {
            RunStatus::Failed
        } else {
            RunStatus::Done
        };
        (s.status, blocked_tasks, cancelled_tasks)
    };
    write_state(&state).await?;
    for (task_id, message) in &cancelled_tasks {
        record_event(
            &run_dir,
            &run_id,
            RunEventKind::TaskCancelled,
            Some(task_id),
            Some(message.clone()),
            json!({}),
            channels_config.as_ref(),
            envelope_dry_run,
        );
    }
    for (task_id, message) in &blocked_tasks {
        record_event(
            &run_dir,
            &run_id,
            RunEventKind::TaskSkipped,
            Some(task_id),
            Some(message.clone()),
            json!({ "blocked": true }),
            channels_config.as_ref(),
            envelope_dry_run,
        );
    }
    record_event(
        &run_dir,
        &run_id,
        if cancelled {
            RunEventKind::RunCancelled
        } else {
            RunEventKind::RunCompleted
        },
        None,
        Some(format!("run finished with status {:?}", final_status)),
        json!({
            "status": format!("{:?}", final_status),
            "had_failure": had_failure,
            "cancelled": cancelled,
        }),
        channels_config.as_ref(),
        envelope_dry_run,
    );
    let _ = state_tx.send(());

    // L4 — verification gate. Skipped on cancellation: if the user pulled the
    // brake, they're not asking us to assert correctness.
    run_verify_gate(&ctx, &plan, &cfg, &mut integrations, cancelled).await?;

    // Outcome gate (boundary review): after the work + acceptance checks, pause
    // until a human approves the result — the "outcome" boundary. Only for runs
    // that actually completed work to review (not cancelled / already failed).
    if cfg.outcome_gate && !cancelled && !had_failure {
        eprintln!(
            "⏸ outcome gate: review the result, then `maestro approve {GATE_OUTCOME}` (or approve in the dashboard) to accept"
        );
        if let ApprovalWaitOutcome::Cancelled = ctx
            .await_gate(
                "outcome",
                GATE_OUTCOME,
                "outcome gate: run awaiting acceptance",
                json!({ "gate": "outcome" }),
            )
            .await?
        {
            {
                let mut s = ctx.lock().await;
                s.pending_gate = None;
                s.status = RunStatus::Cancelled;
            }
            ctx.write_state().await?;
            ctx.record(
                RunEventKind::RunCancelled,
                None,
                Some("outcome gate rejected — result not accepted".to_string()),
                json!({ "gate": "outcome" }),
            );
        }
    }

    let final_state = state.lock().await.clone();

    if let Err(e) = crate::scheduler::evidence::write_run_evidence(&final_state) {
        tracing::warn!("could not write run evidence: {e:#}");
    }
    if let Err(e) = crate::scheduler::evidence::write_pr_body(&final_state) {
        tracing::warn!("could not write PR body draft: {e:#}");
    }

    // Persist human-readable reports for ✅ Done runs (and Failed for forensics).
    if matches!(final_state.status, RunStatus::Done | RunStatus::Failed) {
        if let Err(e) = crate::reports::write_run_report(&plan, &final_state) {
            tracing::warn!("could not write run report: {e:#}");
        }
    }
    if matches!(final_state.status, RunStatus::Done) && final_state.verified_or_no_goal() {
        if let Err(e) = crate::reports::archive_l2_decision(&plan, &final_state) {
            tracing::warn!("could not archive L2 decision: {e:#}");
        }
        // Skill synthesis (opt-in, OFF by default): draft a reusable playbook
        // PROPOSAL from a verified, complex run. Inert until a human runs
        // `maestro learn promote`. Deterministic, no LLM.
        if crate::config::Settings::load().learning.synthesize_skills {
            match crate::learn::ProposalStore::open()
                .and_then(|s| s.synthesize_skill_proposal(&final_state, &plan))
            {
                Ok(n) if n > 0 => tracing::info!(
                    "learning: drafted a skill playbook proposal — review with `maestro learn list`"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("learning: could not draft skill proposal: {e:#}"),
            }
        }
    }

    // Failure-driven learning (opt-in, OFF by default): distill this run's
    // failures into INERT guardrail proposals under .maestro/proposals/. A
    // proposal never affects a future run — it only becomes a live skill when a
    // human runs `maestro learn promote`. Deterministic, no LLM.
    if crate::config::Settings::load().learning.propose_guardrails {
        let had_failure = matches!(final_state.status, RunStatus::Failed)
            || final_state.acceptance_results.iter().any(|r| !r.passed);
        if had_failure {
            match crate::learn::ProposalStore::open()
                .and_then(|s| s.propose_from_failures(&final_state))
            {
                Ok(n) if n > 0 => tracing::info!(
                    "learning: wrote {n} guardrail proposal(s) — review with `maestro learn list`"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("learning: could not write guardrail proposals: {e:#}"),
            }
        }
    }

    // On a clean run, tear down the integration worktrees once all reporting is
    // done. They are pure scratch (never inspected) and would otherwise pile up
    // on disk and in each source repo's `.git/worktrees` registry run after run.
    // `remove_worktree` also drops the registration (≈ `git worktree prune`).
    // Task worktrees are kept for inspection / `maestro pr`; failed runs keep
    // everything so the failure can be examined.
    if matches!(final_state.status, RunStatus::Done) {
        for integ in integrations.by_repo.values() {
            gitops::remove_worktree(&integ.repo_root, &integ.path);
        }
    }

    Ok(final_state)
}

#[allow(clippy::too_many_arguments)]
fn record_event(
    run_dir: &Path,
    run_id: &str,
    kind: RunEventKind,
    task_id: Option<&str>,
    message: Option<String>,
    payload: serde_json::Value,
    channels: Option<&ChannelConfig>,
    dry_run: bool,
) {
    if let Err(e) = events::append_event_with_subscribe(
        run_dir, run_id, kind, task_id, message, payload, channels, dry_run,
    ) {
        tracing::warn!("could not append run event: {e:#}");
    }
}

async fn load_workflow_inputs(
    task: &PlanTask,
    state: &Arc<Mutex<RunState>>,
) -> Result<Vec<MemorySlice>> {
    if task.inputs.is_empty() {
        return Ok(vec![]);
    }

    let refs: Vec<(String, String, String, bool)> = task
        .inputs
        .iter()
        .filter_map(|(alias, input)| {
            parse_output_ref(&input.from).map(|(producer, output)| {
                (
                    alias.clone(),
                    producer.to_string(),
                    output.to_string(),
                    input.required,
                )
            })
        })
        .collect();

    let output_paths = {
        let s = state.lock().await;
        refs.iter()
            .map(|(alias, producer, output, required)| {
                let found = s
                    .tasks
                    .get(producer)
                    .and_then(|ts| ts.workflow_outputs.get(output))
                    .cloned();
                (
                    alias.clone(),
                    producer.clone(),
                    output.clone(),
                    *required,
                    found,
                )
            })
            .collect::<Vec<_>>()
    };

    let mut slices = Vec::new();
    for (alias, producer, output, required, found) in output_paths {
        let Some(meta) = found else {
            if required {
                anyhow::bail!(
                    "required workflow input {} from {}.{} is not available",
                    alias,
                    producer,
                    output
                );
            }
            continue;
        };
        let bytes = tokio::fs::read(&meta.snapshot_path)
            .await
            .with_context(|| format!("read workflow output snapshot {}", meta.snapshot_path))?;
        let content = String::from_utf8_lossy(&bytes).to_string();
        let mut body = format!(
            "alias: {alias}\nfrom: {producer}.{output}\nsnapshot: {}\n",
            meta.snapshot_path
        );
        if let Some(source) = &meta.source_path {
            body.push_str(&format!("source: {source}\n"));
        }
        if meta.truncated {
            body.push_str("truncated: true\n");
        }
        body.push_str("---\n");
        body.push_str(&content);
        slices.push(MemorySlice {
            topic: format!("workflow/{producer}.{output} as {alias}"),
            content: body,
        });
    }
    Ok(slices)
}

async fn capture_task_outputs(
    run_dir: &Path,
    workspace: &Path,
    task_id: &str,
    outputs: &BTreeMap<String, TaskOutput>,
    transcript_summary: &str,
) -> Result<BTreeMap<String, WorkflowOutputState>> {
    if outputs.is_empty() {
        return Ok(BTreeMap::new());
    }

    let out_dir = run_dir.join("outputs").join(sanitize_id(task_id));
    tokio::fs::create_dir_all(&out_dir)
        .await
        .with_context(|| format!("create workflow output dir {}", out_dir.display()))?;

    let mut captured = BTreeMap::new();
    for (name, spec) in outputs {
        let (source_path, mut bytes) = if let Some(path) = spec.path.as_deref() {
            let src = workspace.join(path);
            match tokio::fs::read(&src).await {
                Ok(bytes) => (Some(src), bytes),
                Err(err) if !spec.required && err.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(err) => {
                    anyhow::bail!(
                        "capture output {}.{} from {} failed: {}",
                        task_id,
                        name,
                        src.display(),
                        err
                    );
                }
            }
        } else {
            (None, transcript_summary.as_bytes().to_vec())
        };

        let truncated = bytes.len() > spec.max_bytes;
        if truncated {
            bytes.truncate(spec.max_bytes);
        }

        let snapshot = out_dir.join(sanitize_id(name));
        tokio::fs::write(&snapshot, &bytes)
            .await
            .with_context(|| format!("write workflow output snapshot {}", snapshot.display()))?;

        captured.insert(
            name.clone(),
            WorkflowOutputState {
                name: name.clone(),
                source_path: source_path.map(|p| p.to_string_lossy().to_string()),
                snapshot_path: snapshot.to_string_lossy().to_string(),
                bytes: bytes.len(),
                truncated,
            },
        );
    }
    Ok(captured)
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn check_cancelled(run_id: &str) -> Result<bool> {
    let dir = paths::cancels_dir()?;
    if !dir.exists() {
        return Ok(false);
    }
    let marker = paths::control_marker_path(&dir, "run id", run_id)?;
    let global = paths::control_marker_path(&dir, "run id", "current")?;
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
        return Ok(true);
    }
    if global.exists() {
        let _ = std::fs::remove_file(&global);
        return Ok(true);
    }
    Ok(false)
}

async fn write_state(state: &Arc<Mutex<RunState>>) -> Result<()> {
    let s = state.lock().await;
    s.write_atomic()
}

fn all_deps_done(plan: &Plan, completed: &HashSet<String>, task_id: &str) -> bool {
    let Some(t) = plan.task(task_id) else {
        return false;
    };
    t.depends_on.iter().all(|d| completed.contains(d))
}

fn mark_blocked_pending_tasks(state: &mut RunState, plan: &Plan) -> Vec<(String, String)> {
    let mut marked = Vec::new();
    loop {
        let statuses: BTreeMap<String, (TaskStatus, Option<String>)> = state
            .tasks
            .iter()
            .map(|(id, task)| (id.clone(), (task.status, task.error.clone())))
            .collect();
        let mut changed = false;

        for task in &plan.tasks {
            let Some(current) = state.tasks.get(&task.id) else {
                continue;
            };
            if current.status != TaskStatus::Pending {
                continue;
            }

            let blocked_by = task.depends_on.iter().find(|dep| {
                statuses
                    .get(*dep)
                    .map(|(status, error)| {
                        *status == TaskStatus::Failed
                            || (*status == TaskStatus::Skipped
                                && error
                                    .as_deref()
                                    .map(|e| e.starts_with("blocked by failed dependency"))
                                    .unwrap_or(false))
                    })
                    .unwrap_or(false)
            });
            let Some(blocked_by) = blocked_by else {
                continue;
            };

            let message = format!("blocked by failed dependency: {blocked_by}");
            if let Some(task_state) = state.tasks.get_mut(&task.id) {
                task_state.status = TaskStatus::Skipped;
                task_state.ended_at = Some(Utc::now());
                task_state.error = Some(message.clone());
            }
            marked.push((task.id.clone(), message));
            changed = true;
        }

        if !changed {
            break;
        }
    }
    marked
}

fn mark_unfinished_tasks_cancelled(state: &mut RunState, reason: &str) -> Vec<(String, String)> {
    let mut marked = Vec::new();
    for task in state.tasks.values_mut() {
        if matches!(
            task.status,
            TaskStatus::Pending | TaskStatus::Running | TaskStatus::AwaitingApproval
        ) {
            task.status = TaskStatus::Cancelled;
            task.ended_at = Some(Utc::now());
            task.error = Some(reason.to_string());
            marked.push((task.id.clone(), reason.to_string()));
        }
    }
    marked
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalWaitOutcome {
    Approved,
    Cancelled,
}

/// Working-tree diff of a task's worktree vs HEAD (the implementer's changes),
/// capped for prompt size. `None` when there's no diff or git fails.
fn git_diff(ws: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(ws)
        .args(["diff", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let d = String::from_utf8_lossy(&out.stdout);
    let d = d.trim();
    if d.is_empty() {
        return None;
    }
    Some(d.chars().take(6000).collect())
}

/// Best-effort: drop a task's prior worktree + branch before a retry so the
/// re-run gets a clean isolated worktree instead of falling back to the repo
/// root (the branch `maestro/<run>/<task>` already exists from attempt N).
async fn cleanup_worktree_for_retry(
    state: &std::sync::Arc<tokio::sync::Mutex<RunState>>,
    projects: &ProjectsConfig,
    run_id: &str,
    task_id: &str,
) {
    let (worktree, project) = {
        let s = state.lock().await;
        match s.tasks.get(task_id) {
            Some(ts) => (ts.worktree_path.clone(), ts.project.clone()),
            None => return,
        }
    };
    let Some(worktree) = worktree else { return };
    let Ok(repo) = projects.resolved_path(&project) else {
        return;
    };
    let _ = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "remove", "--force", &worktree])
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "-D", &format!("maestro/{run_id}/{task_id}")])
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "prune"])
        .output()
        .await;
}

/// Refresh codegraph indexes for the repos this run touches (best-effort), so
/// the injected "Relevant code" context reflects the current tree.
async fn sync_codegraph_indexes(plan: &Plan, projects: &ProjectsConfig) {
    use std::collections::HashSet;
    let mut roots: HashSet<std::path::PathBuf> = HashSet::new();
    for task in &plan.tasks {
        if !matches!(task.kind, TaskKind::Agent) {
            continue;
        }
        if let Ok(repo) = projects.resolved_path(&task.project) {
            if let Some(root) = find_codegraph_root(&repo) {
                roots.insert(root);
            }
        }
    }
    for root in roots {
        // `codegraph sync` takes the project path as a positional arg (not
        // `--path`, which only `context` accepts) — passing the flag makes the
        // sync silently fail, so the per-task code context never refreshes.
        match tokio::process::Command::new(crate::codegraph::codegraph_bin())
            .arg("sync")
            .arg(&root)
            .output()
            .await
        {
            Ok(o) if o.status.success() => {
                tracing::info!("codegraph synced index at {}", root.display());
            }
            _ => {} // no index / CLI missing / sync failed — non-fatal
        }
    }
}

/// Resolve `.`/`..` in a path lexically (no filesystem access, so it works for
/// a not-yet-created file). Used to clean a `base.join(consumes)` target before
/// `strip_prefix` against an integration repo root — replacing the old
/// `canonicalize()`, which also resolved `..` but errored on missing files.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// If `project` consumes a contract, inject the contract file's *current*
/// content so the agent codes against the real interface — including a
/// producer's just-integrated change — rather than a prose description of it.
/// Reads from the producer's integration worktree when the contract lives in an
/// integrated repo (so a downstream consumer sees the upstream edit), else the
/// resolved base path.
/// Fallback for a consumer whose `consumes` path doesn't resolve to any
/// integration repo (polyrepo, or a consumer-relative vs monorepo-root spelling
/// mismatch): locate the producer(s) via the same matcher the DAG used and read
/// the producer's `provides` file from its integration worktree (else its base).
fn producer_contract_content(
    projects: &ProjectsConfig,
    consumer: &str,
    integrations: &IntegrationWorktrees,
) -> Option<String> {
    for producer in crate::config::analyze::producer_projects_for(projects, consumer) {
        let Some(pp) = projects.projects.get(&producer) else {
            continue;
        };
        let Some(provides) = pp
            .contracts
            .provides
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Ok(pbase) = projects.resolved_path(&producer) else {
            continue;
        };
        let pbase_canon = pbase.canonicalize().unwrap_or(pbase);
        let ptarget = lexical_normalize(&pbase_canon.join(provides));
        let from_integ = integrations.by_repo.values().find_map(|integ| {
            let root = integ
                .repo_root
                .canonicalize()
                .unwrap_or_else(|_| integ.repo_root.clone());
            let rel = ptarget.strip_prefix(&root).ok()?;
            std::fs::read_to_string(integ.path.join(rel)).ok()
        });
        if let Some(text) = from_integ.or_else(|| std::fs::read_to_string(&ptarget).ok()) {
            return Some(text);
        }
    }
    None
}

fn consumed_contract_section(
    projects: &ProjectsConfig,
    project: &str,
    integrations: &IntegrationWorktrees,
) -> Option<String> {
    let p = projects.projects.get(project)?;
    let consumes = p.contracts.consumes.as_deref().map(str::trim)?.to_string();
    if consumes.is_empty() {
        return None;
    }
    let base = projects.resolved_path(project).ok()?;
    // Canonicalize the project root (it exists) but NOT the contract file:
    // canonicalize() errors on a not-yet-created path, which is exactly the
    // contract-first case where the producer creates the file only in its
    // integration worktree. Joining onto the canonical base keeps the
    // strip_prefix lookups below sound without requiring the file on disk.
    let base_canon = base.canonicalize().unwrap_or(base);
    // Normalize `.`/`..` lexically (a relative `consumes` like `../shared/x` is
    // common) so strip_prefix below matches, without canonicalize()'s
    // requirement that the file already exist on disk.
    let target = lexical_normalize(&base_canon.join(&consumes));
    // Prefer the integrated copy (reflects the producer's applied change). When
    // several integration worktrees could match (nested repos), pick the
    // LONGEST matching repo_root — the most specific — so the result is
    // deterministic rather than hash-ordered first-match.
    let content = integrations
        .by_repo
        .values()
        .filter_map(|integ| {
            let root = integ
                .repo_root
                .canonicalize()
                .unwrap_or_else(|_| integ.repo_root.clone());
            let rel = target.strip_prefix(&root).ok()?;
            let text = std::fs::read_to_string(integ.path.join(rel)).ok()?;
            Some((root.as_os_str().len(), text))
        })
        .max_by_key(|(root_len, _)| *root_len)
        .map(|(_, text)| text)
        .or_else(|| std::fs::read_to_string(&target).ok())
        // Last resort: the consumer's `consumes` path didn't resolve to any
        // integration repo (polyrepo, or a consumer-relative vs monorepo-root
        // spelling mismatch). Fall back to the producer's own `provides` file,
        // located via the same matcher the DAG used to order producer→consumer.
        .or_else(|| producer_contract_content(projects, project, integrations))?;
    // Cap by lines AND bytes: a generated/minified contract can be a few
    // enormous lines that a line-only cap would let blow the prompt budget.
    const MAX_LINES: usize = 120;
    const MAX_CHARS: usize = 12_000;
    let by_lines: String = content
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = if by_lines.chars().count() > MAX_CHARS {
        let capped: String = by_lines.chars().take(MAX_CHARS).collect();
        format!("{capped}\n… [contract truncated — read the full file at `{consumes}`]")
    } else {
        by_lines
    };
    if trimmed.trim().is_empty() {
        return None;
    }
    Some(format!(
        "## Consumed contract: `{consumes}`\n\nThis project consumes the contract below. Code against this \
         exact interface — it reflects the producer's latest integrated change, not a description.\n\n```\n{}\n```",
        trimmed.trim()
    ))
}

pub(crate) fn build_code_context(
    repo_path: &std::path::Path,
    query: &str,
    keep_paths: &[String],
) -> Option<String> {
    // Prefer the external codegraph index when present (richest); otherwise fall
    // back to the built-in native scan so grounding works with zero deps — the
    // same fallback the dashboard's code-graph tab uses.
    codegraph_cli_context(repo_path, query, keep_paths)
        .or_else(|| native_code_context(repo_path, query))
}

/// Run a read-only reviewer agent over a finished task's reported work. Returns
/// `Some(feedback)` when the reviewer fails the work (fed into the retry as the
/// diagnosis), or `None` when it passes / can't be reviewed.
#[allow(clippy::too_many_arguments)]
async fn run_review(
    task_id: &str,
    reviewer_role: &str,
    workspace: PathBuf,
    impl_log_path: String,
    impl_prompt: String,
    adapter_name: String,
    project: String,
    projects: &ProjectsConfig,
    run_dir: &std::path::Path,
) -> Result<(Option<String>, Option<adapter::Usage>)> {
    // Prefer the actual git diff of the implementer's worktree as the review
    // subject; fall back to the task log when there's no diff (e.g. mock runs).
    let subject = match git_diff(&workspace) {
        Some(diff) => format!("## Changes the implementer made (git diff)\n```diff\n{diff}\n```"),
        None => {
            let log_tail = std::fs::read_to_string(&impl_log_path)
                .ok()
                .map(|c| {
                    let mut t: Vec<&str> = c.lines().rev().take(80).collect();
                    t.reverse();
                    t.join("\n")
                })
                .unwrap_or_default();
            format!("## What the implementer reported\n{log_tail}")
        }
    };
    let role_prelude = crate::roles::try_load(reviewer_role)
        .as_ref()
        .map(crate::roles::render_section)
        .filter(|s| !s.is_empty());
    let model = projects.resolved_task_model(&project, None, None, None, Some(reviewer_role));
    let prompt = format!(
        "You are reviewing another agent's completed work on a task. Do NOT make changes — review only.\n\n\
         ## Task\n{impl_prompt}\n\n\
         {subject}\n\n\
         ## Your job\n\
         Assess whether the work correctly and completely satisfies the task. End your reply with \
         EXACTLY one line — `VERDICT: pass` if it's acceptable, or `VERDICT: fail` if it needs rework. \
         If fail, put specific, actionable reasons on the lines ABOVE the verdict.",
    );
    let reviewer_task = adapter::AgentTask {
        task_id: format!("{task_id}__review"),
        workspace,
        prompt,
        context: vec![],
        timeout: Duration::from_secs(600),
        mode: ExecutionMode::Plan,
        resume_chat_id: None,
        log_path: run_dir.join(format!("{task_id}.review.log")),
        trajectory: None,
        model,
        role_prelude,
        allowed_tools: crate::modes::AllowedTools::default(),
    };
    let result = adapter::pick(&adapter_name).run(reviewer_task).await?;
    tracing::info!(task = %task_id, reviewer = %reviewer_role, "review completed");
    let usage = result.usage;
    if parse_verdict(&result.transcript_summary) {
        Ok((None, usage))
    } else {
        Ok((Some(result.transcript_summary), usage))
    }
}

/// Mailbox identities of an Agent task (project / task id / resolved role),
/// used to match blocking messages addressed to it. `None` for non-Agent tasks.
fn task_identities(plan: &Plan, projects: &ProjectsConfig, task_id: &str) -> Option<Vec<String>> {
    let t = plan.task(task_id)?;
    if !matches!(t.kind, TaskKind::Agent) {
        return None;
    }
    let role = crate::roles::resolve_for_task(
        t.role.as_deref(),
        projects.resolved_role(&t.project).as_deref(),
    )
    .unwrap_or_default();
    let mut ids = vec![t.project.clone(), task_id.to_string()];
    if !role.is_empty() {
        ids.push(role);
    }
    Some(ids)
}

/// Count of unresolved *blocking* mailbox messages addressed to any of
/// `identities`. Used by the dispatch-time blocking gate; opens the mailbox
/// fresh each call so a peer resolving a message is picked up on the next poll.
fn open_blocking_count(identities: &[&str]) -> usize {
    crate::mailbox::MailboxStore::open()
        .ok()
        .and_then(|store| store.inbox_for(identities).ok())
        .map(|msgs| msgs.iter().filter(|m| m.blocking).count())
        .unwrap_or(0)
}

async fn wait_for_approval(
    task_id: &str,
    run_id: &str,
    gate_opened_at: std::time::SystemTime,
) -> Result<ApprovalWaitOutcome> {
    // Approval markers are keyed only by task id in a run-independent dir, so a
    // marker left over from a crashed prior attempt — or from a different run,
    // since task ids like T_1/T_lint repeat — must not silently auto-approve
    // this gate. Only honor a marker created at/after the gate opened. A 2s
    // grace absorbs coarse filesystem mtime granularity so a legitimate
    // same-second approval is never rejected; a stale marker is always older.
    let cutoff = gate_opened_at
        .checked_sub(Duration::from_secs(2))
        .unwrap_or(gate_opened_at);
    let dir = paths::approvals_dir()?;
    let path = paths::control_marker_path(&dir, "task id", task_id)?;
    loop {
        if path.exists() {
            let fresh = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(|mtime| mtime >= cutoff)
                .unwrap_or(true);
            let _ = std::fs::remove_file(&path);
            if fresh {
                return Ok(ApprovalWaitOutcome::Approved);
            }
            // Stale marker discarded; keep waiting for a fresh approval.
        }
        if check_cancelled(run_id)? {
            return Ok(ApprovalWaitOutcome::Cancelled);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    const SAMPLE_CONTEXT: &str = "\
## Code Context

**Query:** add email to profile

### Entry Points

- **renderProfile** (function) - packages/web/src/profile.ts:12
  `(u: User) -> string`
- **getUser** (function) - packages/api/src/users.ts:8
  `(id: string) -> User`
- **User** (interface) - shared/types/index.d.ts:1
- **User** (interface) - .maestro/runs/r1/worktrees/T_x/shared/types/index.d.ts:2

### Related Symbols

- packages/web/src/profile.ts: renderProfile:12, formatName:30
- packages/api/src/users.ts: getUser:8, listUsers:20";

    mod refute_pass {
        //! F-106 adversarial refute pass — the pure decision logic.
        use super::super::{resolve_reviewer_role, task_is_high_risk};

        #[test]
        fn refuter_role_loads_from_builtin_bundle() {
            // The auto-attach resolves to the literal role name "refuter";
            // that role must exist in the embedded builtin bundle or the
            // review step silently degrades to no prelude.
            let role = crate::roles::try_load("refuter");
            assert!(role.is_some(), "builtin refuter role must load");
        }

        #[test]
        fn auto_attach_picks_refuter_only_when_high_risk_and_toggle_on() {
            // toggle on + high-risk + no explicit reviewer → refuter
            assert_eq!(
                resolve_reviewer_role(None, true, true),
                Some("refuter".to_string())
            );
            // toggle on but NOT high-risk → no reviewer
            assert_eq!(resolve_reviewer_role(None, true, false), None);
        }

        #[test]
        fn explicit_review_by_wins_over_auto_refute() {
            // an explicit reviewer always wins, even on a high-risk task
            // with the toggle on — we don't override the user's choice.
            assert_eq!(
                resolve_reviewer_role(Some("qa"), true, true),
                Some("qa".to_string())
            );
        }

        #[test]
        fn auto_attach_noop_when_toggle_off() {
            // toggle off → never auto-attach, regardless of risk
            assert_eq!(resolve_reviewer_role(None, false, true), None);
            assert_eq!(resolve_reviewer_role(None, false, false), None);
        }

        #[test]
        fn auto_attach_computes_risk_before_approval_gate() {
            // N1-A regression. The approval gate (which writes
            // `risk_level`) runs AFTER the review step, so at review time
            // the stored risk is still None. The auto-attach decision must
            // therefore derive risk from the task's files_changed, which
            // `task_is_high_risk` does — proven here so a refactor that
            // reads the stored (None) field instead gets caught.
            let contracts = vec!["api/openapi.yaml".to_string()];
            // a contract-touching change is high-risk from the file list alone
            assert!(
                task_is_high_risk(&["api/openapi.yaml".to_string()], &contracts),
                "contract touch is high-risk from files, no stored risk_level needed"
            );
            // and that flows into an attached refuter
            let is_high = task_is_high_risk(&["api/openapi.yaml".to_string()], &contracts);
            assert_eq!(
                resolve_reviewer_role(None, true, is_high),
                Some("refuter".to_string())
            );
            // an empty change is never high-risk → no refuter
            assert!(!task_is_high_risk(&[], &contracts));
            assert_eq!(
                resolve_reviewer_role(None, true, task_is_high_risk(&[], &contracts)),
                None
            );
        }
    }

    #[test]
    fn consumed_contract_section_reads_producer_integrated_content() {
        use super::{consumed_contract_section, IntegrationWorktree, IntegrationWorktrees};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // base producer contract (stale) + consumer dir
        std::fs::create_dir_all(root.join("shared/src")).unwrap();
        std::fs::write(root.join("shared/src/index.js"), "// OLD: no email\n").unwrap();
        std::fs::create_dir_all(root.join("api")).unwrap();
        // producer's integration worktree carries the NEW contract
        std::fs::create_dir_all(root.join("integ/src")).unwrap();
        std::fs::write(
            root.join("integ/src/index.js"),
            "// NEW: includes email field\n",
        )
        .unwrap();

        let projects: crate::config::ProjectsConfig = serde_yaml::from_str(&format!(
            "version: 1\nprojects:\n  api:\n    path: {}/api\n    contracts:\n      consumes: ../shared/src/index.js\n",
            root.display()
        ))
        .unwrap();

        let mut integrations = IntegrationWorktrees::default();
        integrations.by_repo.insert(
            "k".to_string(),
            IntegrationWorktree {
                repo_root: root.join("shared"),
                path: root.join("integ"),
                branch: "b".into(),
            },
        );

        let section = consumed_contract_section(&projects, "api", &integrations)
            .expect("consumer should get the contract section");
        assert!(section.contains("NEW: includes email"), "got: {section}");
        assert!(
            !section.contains("OLD"),
            "should prefer integrated, got: {section}"
        );
        assert!(section.contains("../shared/src/index.js"));

        // A project with no `consumes` gets no section.
        let none: crate::config::ProjectsConfig = serde_yaml::from_str(&format!(
            "version: 1\nprojects:\n  lib:\n    path: {}/shared\n",
            root.display()
        ))
        .unwrap();
        assert!(consumed_contract_section(&none, "lib", &integrations).is_none());
    }

    #[test]
    fn scope_context_keeps_own_subtree_and_contracts_drops_siblings() {
        use super::scope_context_markdown;
        let scoped = scope_context_markdown(
            SAMPLE_CONTEXT,
            "packages/web/",
            &["shared/types/index.d.ts".to_string()],
        )
        .expect("web has in-scope symbols");

        // Own package symbols stay.
        assert!(scoped.contains("renderProfile"));
        assert!(scoped.contains("packages/web/src/profile.ts"));
        // Declared contract file stays even though it's outside the subtree.
        assert!(scoped.contains("shared/types/index.d.ts"));
        // …but the stale .maestro worktree copy of that contract is dropped.
        assert!(!scoped.contains(".maestro/"), "got: {scoped}");
        // Sibling package is filtered out entirely.
        assert!(!scoped.contains("packages/api"), "got: {scoped}");
        assert!(!scoped.contains("getUser"), "got: {scoped}");
        assert!(!scoped.contains("listUsers"), "got: {scoped}");
        // Section headers survive.
        assert!(scoped.contains("### Entry Points"));
    }

    #[test]
    fn scope_context_returns_none_when_nothing_in_scope() {
        use super::scope_context_markdown;
        // A package with no matching symbols and no kept contracts → no noise.
        assert!(scope_context_markdown(SAMPLE_CONTEXT, "packages/mobile/", &[]).is_none());
    }

    #[test]
    fn error_signature_ignores_volatile_bits() {
        use super::error_signature;
        // Same underlying error, different timestamps / counts / line numbers →
        // identical signature (circuit breaker should treat as "no progress").
        let a = "Error: cargo test failed at 2026-05-26T04:36:01\n  3 tests failed (attempt 1)";
        let b = "Error: cargo test failed at 2026-05-26T05:10:59\n  3 tests failed (attempt 2)";
        assert_eq!(error_signature(a), error_signature(b));
        // A genuinely different error → different signature (retry is worthwhile).
        let c = "Error: type mismatch in src/lib.rs";
        assert_ne!(error_signature(a), error_signature(c));
    }

    #[test]
    fn parse_verdict_reads_last_verdict_line() {
        use super::parse_verdict;
        assert!(parse_verdict("looks good\nVERDICT: pass"));
        assert!(!parse_verdict(
            "missing tests\nVERDICT: fail — add coverage"
        ));
        // last verdict wins
        assert!(!parse_verdict(
            "VERDICT: pass\n...then on reflection\nVERDICT: fail"
        ));
        // no verdict / unrecognized -> safe default pass
        assert!(parse_verdict("the work is fine"));
        assert!(parse_verdict(""));
        // case-insensitive
        assert!(!parse_verdict("verdict:  FAIL"));
    }

    #[tokio::test]
    async fn rejects_disallowed_shell_command() {
        let mode = crate::modes::Mode {
            allowed_tools: crate::modes::AllowedTools {
                shell: true,
                allowed_commands: vec!["npm test*".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let err = crate::adapter::shell::run_with_mode("rm -rf /", &mode)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not in allowlist"));
    }

    mod memory_topic_resolution {
        //! Lock the precedence: explicit `task.memory_inject` always beats
        //! the project's `memory_scope`. This is a small function but the
        //! precedence is easy to invert by accident — the test makes the
        //! invariant explicit so anyone touching it has to break the test
        //! consciously.
        use super::super::resolve_memory_topics;
        use crate::config::{PlanTask, ProjectsConfig};

        fn task(yaml: &str) -> PlanTask {
            serde_yaml::from_str(yaml).unwrap()
        }
        fn projects(yaml: &str) -> ProjectsConfig {
            serde_yaml::from_str(yaml).unwrap()
        }

        const SCOPED: &str = r#"
version: 1
defaults:
  agent: cursor
projects:
  demo:
    path: .
    memory_scope: [backend/decisions, backend/policies]
"#;

        #[test]
        fn explicit_inject_overrides_project_scope() {
            let p = projects(SCOPED);
            let t = task(
                "id: T_x\nproject: demo\nkind: agent\nmemory_inject:\n  - topic: hotfix-only\n",
            );
            assert_eq!(
                resolve_memory_topics(&t, &p),
                vec!["hotfix-only".to_string()]
            );
        }

        #[test]
        fn project_scope_used_when_no_explicit_inject() {
            let p = projects(SCOPED);
            let t = task("id: T_x\nproject: demo\nkind: agent\n");
            assert_eq!(
                resolve_memory_topics(&t, &p),
                vec![
                    "backend/decisions".to_string(),
                    "backend/policies".to_string()
                ],
            );
        }

        #[test]
        fn empty_when_neither_scope_nor_inject() {
            let p = projects(
                "version: 1\ndefaults:\n  agent: cursor\nprojects:\n  demo:\n    path: .\n",
            );
            let t = task("id: T_x\nproject: _global\nkind: agent\n");
            assert!(resolve_memory_topics(&t, &p).is_empty());
        }
    }

    mod workspace_resolution {
        //! Pin the per-task workspace contract: a non-`_global` task always
        //! runs in `projects.resolved_path(project)`, with no global
        //! integration handle. (Global isolation involves env vars + on-disk
        //! worktree prep and is exercised by the e2e suite, not here.)
        use super::super::{resolve_task_workspace, ExecConfig, IntegrationWorktrees};
        use crate::config::{PlanTask, ProjectsConfig};

        fn projects() -> ProjectsConfig {
            serde_yaml::from_str(
                "version: 1\ndefaults:\n  agent: cursor\nprojects:\n  demo:\n    path: /tmp/demo-proj\n",
            )
            .unwrap()
        }

        fn task_yaml(s: &str) -> PlanTask {
            serde_yaml::from_str(s).unwrap()
        }

        #[test]
        fn non_global_task_runs_in_project_path() {
            let projects = projects();
            let cfg = ExecConfig::default();
            let mut ints = IntegrationWorktrees::default();
            let t = task_yaml("id: T_x\nproject: demo\nkind: agent\n");
            let (ws, gi) = resolve_task_workspace(
                &t,
                &projects,
                &cfg,
                &mut ints,
                std::path::Path::new("/tmp"),
            )
            .expect("workspace resolution must succeed");
            assert_eq!(ws.to_string_lossy(), "/tmp/demo-proj");
            assert!(
                gi.is_none(),
                "non-global tasks never carry a global integration handle"
            );
        }

        #[test]
        fn missing_project_surfaces_an_error_instead_of_panicking() {
            // Earlier, a typo'd project name was masked by an unwrap deep
            // inside resolved_path. The contract is: bad project → Err, not
            // panic. (Belt-and-suspenders for the no-production-unwrap rule.)
            let projects = projects();
            let cfg = ExecConfig::default();
            let mut ints = IntegrationWorktrees::default();
            let t = task_yaml("id: T_x\nproject: nope-typo\nkind: agent\n");
            let err = resolve_task_workspace(
                &t,
                &projects,
                &cfg,
                &mut ints,
                std::path::Path::new("/tmp"),
            )
            .expect_err("unknown project must Err");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("nope-typo") || msg.to_lowercase().contains("project"),
                "error should mention the bad project: {msg}",
            );
        }
    }

    /// Lock the adapter-routing precedence so a future refactor of
    /// `resolve_adapter_for_task` can't silently change which agent runs
    /// a task. Each test asserts one tier of the precedence ladder.
    /// YAML fixtures keep the tests robust to PlanTask / ProjectsConfig
    /// schema growth — adding a new optional field doesn't break them.
    mod routing_precedence {
        use super::super::resolve_adapter_for_task;
        use crate::config::{PlanTask, ProjectsConfig};

        fn projects_from_yaml(yaml: &str) -> ProjectsConfig {
            serde_yaml::from_str(yaml).expect("projects.yaml fixture must parse")
        }

        fn task_from_yaml(yaml: &str) -> PlanTask {
            serde_yaml::from_str(yaml).expect("plan task fixture must parse")
        }

        const PROJECTS_NO_ROUTING: &str = r#"
version: 1
defaults:
  agent: cursor
projects:
  demo:
    path: .
"#;

        const PROJECTS_ROUTE_TO_CODEX: &str = r#"
version: 1
defaults:
  agent: cursor
  routing:
    - when: { kind: agent }
      agent: codex
projects:
  demo:
    path: .
"#;

        #[test]
        fn explicit_task_agent_wins_over_routing_and_default() {
            let projects = projects_from_yaml(PROJECTS_ROUTE_TO_CODEX);
            let t = task_from_yaml("id: T_x\nproject: demo\nkind: agent\nagent: claude\n");
            let (name, _, _) = resolve_adapter_for_task(&t, &projects);
            assert_eq!(
                name, "claude",
                "explicit task.agent must override everything"
            );
        }

        #[test]
        fn routing_policy_wins_when_task_agent_is_unset() {
            let projects = projects_from_yaml(PROJECTS_ROUTE_TO_CODEX);
            let t = task_from_yaml("id: T_x\nproject: demo\nkind: agent\n");
            let (name, _, _) = resolve_adapter_for_task(&t, &projects);
            assert_eq!(
                name, "codex",
                "routing rule must override the project default"
            );
        }

        #[test]
        fn verify_falls_back_to_shell_when_nothing_routes() {
            let projects = projects_from_yaml(PROJECTS_NO_ROUTING);
            let t = task_from_yaml("id: T_v\nproject: demo\nkind: verify\n");
            let (name, _, _) = resolve_adapter_for_task(&t, &projects);
            assert_eq!(
                name, "shell",
                "verify tasks default to shell, not the project agent"
            );
        }

        #[test]
        fn agent_falls_back_to_project_default_when_nothing_routes() {
            let projects = projects_from_yaml(
                "version: 1\ndefaults:\n  agent: codex\nprojects:\n  demo:\n    path: .\n",
            );
            let t = task_from_yaml("id: T_x\nproject: demo\nkind: agent\n");
            let (name, _, _) = resolve_adapter_for_task(&t, &projects);
            assert_eq!(name, "codex");
        }
    }
}
