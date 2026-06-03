//! Handlers for run state, history, per-run details, log streaming, and
//! cancellation markers. None of these touch SSE/broadcast plumbing —
//! that stays in `server::events`.

use anyhow::Result;
use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::{sse::Event, IntoResponse, Json, Response, Sse},
};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use crate::paths;

pub async fn state_handler() -> Response {
    match read_current_state() {
        Ok(Some(json)) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no current run").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// Shared between `state_handler` and the `events` SSE stream — both need
/// the latest `RUN_STATE.json` contents whenever something has changed.
pub fn read_current_state() -> Result<Option<String>> {
    let Some(dir) = paths::current_run_dir()? else {
        return Ok(None);
    };
    let file = dir.join(paths::RUN_STATE_FILE);
    if !file.exists() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(file)?))
}

pub async fn runs_handler() -> Response {
    let dir = match paths::runs_dir() {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if !dir.exists() {
        return Json(Vec::<RunSummary>::new()).into_response();
    }
    let mut runs = vec![];
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == paths::CURRENT_LINK {
                continue;
            }
            let state_file = p.join(paths::RUN_STATE_FILE);
            if state_file.exists() {
                if let Ok(text) = std::fs::read_to_string(&state_file) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        let usage = v.get("usage");
                        let tok = |k: &str| {
                            usage
                                .and_then(|u| u.get(k))
                                .and_then(|n| n.as_u64())
                                .unwrap_or(0)
                        };
                        runs.push(RunSummary {
                            run_id: name,
                            spec: v
                                .get("spec")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                            status: v
                                .get("status")
                                .and_then(|s| s.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            started_at: v
                                .get("started_at")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                            total_tokens: tok("input_tokens") + tok("output_tokens"),
                            cost_usd: usage
                                .and_then(|u| u.get("cost_usd"))
                                .and_then(|n| n.as_f64()),
                            budget_tokens: v.get("budget_tokens").and_then(|n| n.as_u64()),
                        });
                    }
                }
            }
        }
    }
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Json(runs).into_response()
}

#[derive(serde::Serialize)]
struct RunSummary {
    run_id: String,
    spec: String,
    status: String,
    started_at: String,
    /// Cumulative tokens (input + output) the run reported, for the cross-run
    /// cost trend. `0` when the run recorded no usage.
    total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u64>,
}

pub async fn run_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let file = run_dir.join(paths::RUN_STATE_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => ([(header::CONTENT_TYPE, "application/json")], text).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "run not found").into_response(),
    }
}

pub async fn run_evidence_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let file = run_dir.join("evidence").join("summary.json");
    if file.exists() {
        return match std::fs::read_to_string(&file) {
            Ok(text) => ([(header::CONTENT_TYPE, "application/json")], text).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        };
    }

    match crate::scheduler::RunState::load(&run_dir) {
        Ok(state) => {
            let evidence = crate::scheduler::evidence::build_run_evidence(&state);
            Json(evidence).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "run not found").into_response(),
    }
}

/// The run's finding ledger (F-110), append-order. Powers the run-detail
/// findings count/list. Returns `[]` for a run with no findings yet.
pub async fn run_findings_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    match crate::scheduler::findings::read_findings(&run_dir) {
        Ok(findings) => Json(findings).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn run_replay_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    match crate::scheduler::evidence::build_run_replay(&run_dir) {
        Ok(replay) => Json(replay).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn run_pr_body_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    match crate::scheduler::RunState::load(&run_dir) {
        Ok(state) => (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            crate::scheduler::evidence::render_pr_body(&state),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

fn run_dir_from_id(id: &str) -> Result<Option<PathBuf>> {
    if id == "current" {
        return paths::current_run_dir();
    }
    Ok(Some(paths::run_dir_for_id(id)?).filter(|dir| dir.exists()))
}

#[derive(Deserialize)]
pub struct LogsQuery {
    #[serde(default)]
    tail: Option<usize>,
}

pub async fn logs_handler(
    Path((run, task)): Path<(String, String)>,
    Query(q): Query<LogsQuery>,
) -> Response {
    let run_dir = if run == "current" {
        match paths::current_run_dir() {
            Ok(Some(d)) => d,
            _ => return (StatusCode::NOT_FOUND, "no current run").into_response(),
        }
    } else {
        match paths::run_dir_for_id(&run) {
            Ok(d) => d,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        }
    };
    if let Err(e) = paths::validate_path_component("task id", &task) {
        return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    let log_path: PathBuf = run_dir.join("logs").join(format!("{task}.log"));
    if !log_path.exists() {
        return (StatusCode::NOT_FOUND, "log not found").into_response();
    }
    let text = match std::fs::read_to_string(&log_path) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let body = if let Some(n) = q.tail {
        text.lines()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text
    };
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

pub async fn logs_stream_handler(
    Path((run, task)): Path<(String, String)>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let run_dir = if run == "current" {
        paths::current_run_dir().ok().flatten().unwrap_or_default()
    } else {
        paths::run_dir_for_id(&run).unwrap_or_default()
    };
    let log_path = if paths::validate_path_component("task id", &task).is_ok() {
        run_dir.join("logs").join(format!("{task}.log"))
    } else {
        PathBuf::new()
    };

    let stream = futures::stream::unfold(
        (log_path, 0u64, true),
        |(path, mut last_size, first)| async move {
            if first {
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                last_size = body.len() as u64;
                let ev = Event::default().event("log").data(body);
                return Some((Ok(ev), (path, last_size, false)));
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
            let Ok(meta) = std::fs::metadata(&path) else {
                let ev = Event::default().comment("waiting for log file");
                return Some((Ok(ev), (path, last_size, false)));
            };
            let size = meta.len();
            if size > last_size {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => {
                        return Some((Ok(Event::default().comment("io")), (path, last_size, false)))
                    }
                };
                let _ = f.seek(SeekFrom::Start(last_size));
                let mut buf = String::new();
                let _ = f.read_to_string(&mut buf);
                let ev = Event::default().event("delta").data(buf);
                Some((Ok(ev), (path, size, false)))
            } else if size < last_size {
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                let new_size = body.len() as u64;
                let ev = Event::default().event("log").data(body);
                Some((Ok(ev), (path, new_size, false)))
            } else {
                let ev = Event::default().comment("idle");
                Some((Ok(ev), (path, last_size, false)))
            }
        },
    );

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

pub async fn run_cancel(Path(id): Path<String>) -> Response {
    let dir = match paths::cancels_dir() {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if let Err(e) = paths::ensure_dir(&dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    let target = if id == "current" {
        "current".to_string()
    } else {
        id
    };
    // 1) Write the cancel marker. A live scheduler polls this on every
    //    dispatch tick and shuts down gracefully.
    let f = match paths::control_marker_path(&dir, "run id", &target) {
        Ok(f) => f,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    if let Err(e) = std::fs::write(&f, b"cancel") {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    // 2) Safety net for the abandoned-run case (owner process is dead):
    //    nobody polls the marker, so the run sits "running" forever and
    //    the UI's cancel button silently fails. If we detect a dead
    //    owner, force the state into Cancelled directly so the user sees
    //    it terminate within one SSE tick.
    if let Ok(run_dir) = resolve_run_dir(&target) {
        if let Err(e) = crate::scheduler::force_cancel_if_abandoned(&run_dir) {
            tracing::warn!("force-cancel for abandoned run {target} failed: {e:#}");
        }
    }
    (StatusCode::NO_CONTENT, "").into_response()
}

/// Resolve a run id (or the literal "current") to its on-disk directory.
fn resolve_run_dir(id: &str) -> anyhow::Result<std::path::PathBuf> {
    if id == "current" {
        return paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run"));
    }
    paths::run_dir_for_id(id)
}

#[derive(Deserialize)]
pub struct ApproveBody {
    /// Task id awaiting approval.
    task: String,
    /// "approve" → let the gated task proceed; "reject" → cancel the run (the
    /// only existing alternative to approval).
    decision: String,
}

/// Resolve a `requires_approval_after` gate from the Web UI, writing the same
/// markers the scheduler's `wait_for_approval` already polls — `approve` drops
/// the task's approval marker, `reject` drops the run's cancel marker. No
/// scheduler change: the gate loop picks the marker up on its next tick.
pub async fn run_approve(Path(id): Path<String>, body: Json<ApproveBody>) -> Response {
    let decision = body.decision.trim().to_ascii_lowercase();
    match decision.as_str() {
        "approve" => {
            let dir = match paths::approvals_dir() {
                Ok(d) => d,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
                }
            };
            if let Err(e) = paths::ensure_dir(&dir) {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
            }
            let f = match paths::control_marker_path(&dir, "task id", &body.task) {
                Ok(f) => f,
                Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
            };
            if let Err(e) = std::fs::write(&f, b"approved") {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
            }
            (StatusCode::NO_CONTENT, "").into_response()
        }
        "reject" => run_cancel(Path(id)).await,
        other => (
            StatusCode::BAD_REQUEST,
            format!("unknown decision {other:?}"),
        )
            .into_response(),
    }
}

/// Relaunch a finished run to recover from failure: spawns `maestro rerun
/// <run>/PLAN.yaml` as a detached child, which seeds every task that already
/// succeeded and only re-runs the failed + blocked ones. The new run becomes
/// `current`; the UI's live state stream picks it up. Returns the spawned pid
/// so the caller can confirm it started, not the run's outcome (the rerun
/// outlives this request).
pub async fn run_rerun(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, format!("no run {id}")).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let plan = run_dir.join(paths::PLAN_SNAPSHOT);
    if !plan.exists() {
        return (
            StatusCode::BAD_REQUEST,
            format!("run {id} has no {} to rerun", paths::PLAN_SNAPSHOT),
        )
            .into_response();
    }
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("maestro"));
    let cwd = match paths::workspace_root() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match std::process::Command::new(&bin)
        .arg("rerun")
        .arg(&plan)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => Json(serde_json::json!({ "ok": true, "pid": child.id() })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to spawn rerun: {e}"),
        )
            .into_response(),
    }
}

/// The pending change of one task plus a risk verdict — so an approval card can
/// show *what* is being approved (real diff) and *why* it matters (risk), not a
/// bare yes/no. Reads the task's worktree (which holds the change at the gate).
pub async fn task_diff(Path((id, task)): Path<(String, String)>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such run").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let state = match crate::scheduler::RunState::load(&run_dir) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let Some(ts) = state.tasks.get(&task) else {
        return (StatusCode::NOT_FOUND, "no such task").into_response();
    };
    let Some(worktree) = ts.worktree_path.as_deref() else {
        return Json(serde_json::json!({
            "diff": "", "files": [],
            "risk": { "level": "low", "reasons": ["no worktree changes recorded"] }
        }))
        .into_response();
    };
    let wt = std::path::Path::new(worktree);
    // F-108: scope the files/risk to the project workspace (a subdir in the
    // monorepo layout), not the worktree root — otherwise the list comes back
    // repo-root-relative and includes sibling projects. The diff text stays on
    // the worktree (full-change view).
    let scope = ts.workspace_path.as_deref().unwrap_or(worktree);
    let files =
        crate::gitops::changed_files_with_status(std::path::Path::new(scope)).unwrap_or_default();
    let diff = crate::gitops::worktree_diff_text(wt, 200_000).unwrap_or_default();

    // The task's project contracts (provides + consumes) feed the risk verdict.
    let contracts: Vec<String> = crate::server::handlers::projects::load_projects_cfg()
        .ok()
        .and_then(|cfg| cfg.projects.get(&ts.project).cloned())
        .map(|p| {
            p.contracts
                .provides
                .into_iter()
                .chain(p.contracts.consumes)
                .collect()
        })
        .unwrap_or_default();
    let risk = crate::scheduler::risk::classify_change_risk(&files, &contracts);

    let files_json: Vec<serde_json::Value> = files
        .into_iter()
        .map(|(c, p)| serde_json::json!({ "status": c.to_string(), "path": p }))
        .collect();
    Json(serde_json::json!({ "diff": diff, "files": files_json, "risk": risk })).into_response()
}

/// Bucket a shell command into a coarse intent for the trajectory glance.
/// Heuristic and best-effort — enough to answer "did the agent read 3 files or
/// 30" at a glance without claiming false precision.
fn classify_step(cmd: &str) -> &'static str {
    let c = cmd.to_lowercase();
    if c.contains("apply_patch")
        || c.contains("applypatch")
        || c.contains(" tee ")
        || c.contains("sed -i")
    {
        "edit"
    } else if c.contains("rg ")
        || c.contains("grep")
        || c.contains("find ")
        || c.contains("rg --files")
        || c.contains(" ls ")
    {
        "search"
    } else if c.contains("sed -n")
        || c.contains("cat ")
        || c.contains("head ")
        || c.contains("tail ")
        || c.contains("less ")
    {
        "read"
    } else if c.contains("test")
        || c.contains("npm ")
        || c.contains("node ")
        || c.contains("cargo ")
        || c.contains("pytest")
        || c.contains("check")
    {
        "run"
    } else if c.contains("git ") {
        "git"
    } else {
        "other"
    }
}

/// The agent's intermediate steps for one task — what tools/commands it ran, in
/// order, with a coarse breakdown (reads vs searches vs edits vs runs). Surfaces
/// the "harness effect" the 2026 evals literature flags: two tasks with the same
/// diff can differ wildly in *how* the agent got there. Reads the per-task
/// trajectory ndjson; returns nothing-but-empty for tasks without one.
pub async fn task_trajectory(Path((id, task)): Path<(String, String)>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such run").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let path = crate::scheduler::trajectory::trajectory_path(&run_dir, &task);
    let events = crate::scheduler::trajectory::read_trajectory(&path).unwrap_or_default();

    use crate::schema::trajectory::{TrajectoryEventKind, TrajectoryStatus};
    const CAP: usize = 300;
    let mut steps: Vec<serde_json::Value> = Vec::new();
    let mut buckets: std::collections::BTreeMap<&'static str, u32> =
        std::collections::BTreeMap::new();
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    for ev in &events {
        if let Some(u) = &ev.usage {
            tokens_in += u.input_tokens;
            tokens_out += u.output_tokens;
        }
        // One row per *completed* tool call / command (codex emits started +
        // completed pairs that share a command; we keep the completed one).
        let is_step = matches!(
            ev.kind,
            TrajectoryEventKind::ToolCall | TrajectoryEventKind::Command
        );
        let completed = !matches!(ev.status, Some(TrajectoryStatus::Started));
        if !is_step || !completed {
            continue;
        }
        let cmd = ev.command.clone().unwrap_or_default();
        let bucket = if cmd.is_empty() {
            "other"
        } else {
            classify_step(&cmd)
        };
        *buckets.entry(bucket).or_insert(0) += 1;
        if steps.len() < CAP {
            let short = if cmd.chars().count() > 200 {
                cmd.chars().take(200).collect::<String>() + "…"
            } else {
                cmd
            };
            steps.push(serde_json::json!({
                "seq": ev.seq,
                "ts": ev.timestamp,
                "command": short,
                "bucket": bucket,
                "status": ev.status.map(|s| format!("{s:?}").to_lowercase()),
            }));
        }
    }
    let total: u32 = buckets.values().sum();
    Json(serde_json::json!({
        "task": task,
        "total_steps": total,
        "buckets": buckets,
        "tokens": if tokens_in + tokens_out > 0 {
            Some(serde_json::json!({ "input_tokens": tokens_in, "output_tokens": tokens_out }))
        } else { None },
        "steps": steps,
        "truncated": total as usize > steps.len(),
    }))
    .into_response()
}

/// The whole run's outcome in one payload — goal, acceptance results, and the
/// aggregate change across *every* task's worktree (grouped per task/project
/// with a per-task diff + risk, plus an overall risk verdict). Lets a human
/// validate the complete result at the outcome boundary instead of clicking
/// into a dozen separate task diffs.
pub async fn run_outcome(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such run").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let state = match crate::scheduler::RunState::load(&run_dir) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let cfg = crate::server::handlers::projects::load_projects_cfg().ok();
    let contracts_for = |project: &str| -> Vec<String> {
        cfg.as_ref()
            .and_then(|c| c.projects.get(project).cloned())
            .map(|p| {
                p.contracts
                    .provides
                    .into_iter()
                    .chain(p.contracts.consumes)
                    .collect()
            })
            .unwrap_or_default()
    };

    // Walk tasks in plan order; only those with a worktree carry changes.
    let order = if state.task_order.is_empty() {
        state.tasks.keys().cloned().collect::<Vec<_>>()
    } else {
        state.task_order.clone()
    };
    let mut task_entries = Vec::new();
    let mut total_files = 0usize;
    let mut any_high = false;
    let mut high_reasons: Vec<String> = Vec::new();
    for task_id in order {
        let Some(ts) = state.tasks.get(&task_id) else {
            continue;
        };
        let Some(worktree) = ts.worktree_path.as_deref() else {
            continue;
        };
        let wt = std::path::Path::new(worktree);
        // F-108: project-scoped files/risk (see the per-task handler above).
        let scope = ts.workspace_path.as_deref().unwrap_or(worktree);
        let files = crate::gitops::changed_files_with_status(std::path::Path::new(scope))
            .unwrap_or_default();
        if files.is_empty() {
            continue;
        }
        let diff = crate::gitops::worktree_diff_text(wt, 80_000).unwrap_or_default();
        let risk =
            crate::scheduler::risk::classify_change_risk(&files, &contracts_for(&ts.project));
        if risk.level == "high" {
            any_high = true;
            for r in &risk.reasons {
                high_reasons.push(format!("{task_id}: {r}"));
            }
        }
        total_files += files.len();
        let files_json: Vec<serde_json::Value> = files
            .into_iter()
            .map(|(c, p)| serde_json::json!({ "status": c.to_string(), "path": p }))
            .collect();
        task_entries.push(serde_json::json!({
            "task": task_id,
            "project": ts.project,
            "files": files_json,
            "diff": diff,
            "risk": risk,
        }));
    }
    let overall_risk = serde_json::json!({
        "level": if any_high { "high" } else { "low" },
        "reasons": high_reasons,
    });

    // Contract drift: which projects changed this run → producers that moved a
    // contract while a consumer stayed put. maestro's signature cross-repo check.
    let changed_projects: std::collections::HashSet<String> = state
        .tasks
        .values()
        .filter(|t| {
            // F-108: prefer the project workspace (monorepo subdir) over the
            // worktree root so a project counts as changed only on its own edits.
            t.workspace_path
                .as_deref()
                .or(t.worktree_path.as_deref())
                .map(std::path::Path::new)
                .map(|wt| {
                    !crate::gitops::changed_files_with_status(wt)
                        .unwrap_or_default()
                        .is_empty()
                })
                .unwrap_or(false)
        })
        .map(|t| t.project.clone())
        .collect();
    let drift = cfg
        .as_ref()
        .map(|c| crate::config::detect_contract_drift(c, &changed_projects))
        .unwrap_or_default();

    Json(serde_json::json!({
        "goal": state.goal,
        "acceptance_results": state.acceptance_results,
        "verified": state.verified,
        "status": format!("{:?}", state.status).to_lowercase(),
        "total_files": total_files,
        "risk": overall_risk,
        "tasks": task_entries,
        "drift": drift,
    }))
    .into_response()
}
