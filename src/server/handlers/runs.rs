//! Handlers for run state, history, per-run details, log streaming, and
//! cancellation markers. None of these touch SSE/broadcast plumbing —
//! that stays in `server::events`.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Json, Response, Sse},
};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use crate::paths;
use crate::scheduler::{read_events, RunEvent, RunEventStream};
use crate::server::ServerState;

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

/// F-112: read-only run monitor projection (`maestro.run_monitor.v1`). Thin —
/// resolve run dir, load state, read findings (empty on miss), project, return.
pub async fn run_monitor_handler(Path(id): Path<String>) -> Response {
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let state = match crate::scheduler::RunState::load(&run_dir) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    // Missing ledger is `[]` (read_findings handles it); a CORRUPT ledger must
    // surface as 500, not silently project as "0 findings" (audit integrity).
    let findings = match crate::scheduler::findings::read_findings(&run_dir) {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let monitor = crate::schema::monitor::RunMonitor::from_state_and_findings(
        &state,
        &findings,
        Some(chrono::Utc::now().to_rfc3339()),
    );
    Json(monitor).into_response()
}

/// F-112: read-only task detail projection (`maestro.task_detail.v1`). The URL
/// `task` is validated as a single safe path component BEFORE the projector sees
/// it — a traversal-like id is a `400`, never passed downstream.
pub async fn task_detail_handler(Path((run, task)): Path<(String, String)>) -> Response {
    if let Err(e) = paths::validate_path_component("task id", &task) {
        return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    let run_dir = match run_dir_from_id(&run) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let state = match crate::scheduler::RunState::load(&run_dir) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    // Missing ledger is `[]` (read_findings handles it); a CORRUPT ledger must
    // surface as 500, not silently project as "0 findings" (audit integrity).
    let findings = match crate::scheduler::findings::read_findings(&run_dir) {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match crate::schema::monitor::TaskDetail::from_state_and_findings(&state, &task, &findings) {
        Some(detail) => Json(detail).into_response(),
        None => (StatusCode::NOT_FOUND, "task not found").into_response(),
    }
}

#[derive(Deserialize)]
pub struct EventsStreamQuery {
    since_seq: Option<String>,
}

/// F-115 Step 3: per-run typed event stream. SSE of `RunEvent` v2 JSON, one
/// `run_event` per event with `id = seq`. Resumable via `?since_seq=N` or a
/// `Last-Event-ID` header (query wins); only events with `seq > cursor` are sent,
/// in seq order. On entry: bad run id → 400, unknown run → 404, corrupt ledger →
/// 500 (never silently empty). After the catch-up, it subscribes to the existing
/// broadcast tick and re-reads on each change. The legacy `/api/events` state
/// tick is untouched.
pub async fn run_events_stream_handler(
    Path(id): Path<String>,
    Query(q): Query<EventsStreamQuery>,
    headers: HeaderMap,
    State(st): State<ServerState>,
) -> Response {
    let cursor = match resolve_stream_cursor(q.since_seq.as_deref(), header_last_event_id(&headers))
    {
        Ok(c) => c,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let run_dir = match run_dir_from_id(&id) {
        Ok(Some(dir)) => dir,
        Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    // Subscribe BEFORE the initial read so no broadcast tick can slip through the
    // gap between read and subscribe (N2): any event after this point is either
    // already in the initial read or its tick is queued on `rx`, and the
    // cursor-based dedup makes the resulting re-read harmless.
    let rx = st.tx.subscribe();
    // Initial read: a corrupt ledger is a 500, never a silently-empty stream.
    let initial = match read_events(&run_dir) {
        Ok(events) => events,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let stream = run_event_sse_stream(run_dir, initial, cursor, rx);
    Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

/// Resolve the stream cursor: query `since_seq` wins over a `Last-Event-ID`
/// header; absent → 0 (replay from the start). A present-but-non-numeric value
/// is a `400`.
fn resolve_stream_cursor(query: Option<&str>, header: Option<&str>) -> Result<u64, String> {
    match query.or(header) {
        None => Ok(0),
        Some(raw) => raw
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("cursor must be a non-negative integer (got {raw:?})")),
    }
}

fn header_last_event_id(headers: &HeaderMap) -> Option<&str> {
    headers.get("last-event-id").and_then(|v| v.to_str().ok())
}

/// Catch-up frames for the events with `seq > cursor`, in seq order. Each is a
/// `(seq, RunEvent-json)` pair the handler turns into an SSE `run_event`. Reuses
/// the F-115 Step-1 projector for the filter/sort so the stream and a fresh
/// projection never disagree.
fn event_stream_frames(events: &[RunEvent], cursor: u64) -> Vec<(u64, String)> {
    RunEventStream::from_events(events, Some(cursor))
        .events
        .into_iter()
        .filter_map(|ev| serde_json::to_string(&ev).ok().map(|json| (ev.seq, json)))
        .collect()
}

fn run_event_frame(seq: u64, json: String) -> Event {
    Event::default()
        .event("run_event")
        .id(seq.to_string())
        .data(json)
}

/// The SSE body: emit the catch-up frames, then on each broadcast tick re-read
/// the ledger and emit anything new (`seq > cursor`). A ledger that becomes
/// unreadable mid-stream emits a neutral `error` event and ends — it never
/// silently continues as an empty stream.
fn run_event_sse_stream(
    run_dir: PathBuf,
    initial: Vec<RunEvent>,
    start_cursor: u64,
    mut rx: tokio::sync::broadcast::Receiver<()>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    use tokio::sync::broadcast::error::RecvError;
    async_stream::stream! {
        let mut cursor = start_cursor;
        for (seq, json) in event_stream_frames(&initial, cursor) {
            cursor = cursor.max(seq);
            yield Ok(run_event_frame(seq, json));
        }
        // A missed tick (Lagged) is handled like a normal tick: re-read from the
        // durable, cursor-based ledger, so nothing is lost. The loop ends when the
        // broadcast sender is dropped (Err(Closed) fails the while-let pattern).
        while let Ok(()) | Err(RecvError::Lagged(_)) = rx.recv().await {
            match read_events(&run_dir) {
                Ok(events) => {
                    for (seq, json) in event_stream_frames(&events, cursor) {
                        cursor = cursor.max(seq);
                        yield Ok(run_event_frame(seq, json));
                    }
                }
                Err(_) => {
                    yield Ok(Event::default()
                        .event("error")
                        .data("event ledger became unreadable"));
                    break;
                }
            }
        }
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

#[cfg(test)]
mod monitor_tests {
    //! F-112 Step 2 — the two read-only handlers, exercised against a crafted
    //! run dir in a throwaway workspace. Neutral fixture names only.
    use super::{run_monitor_handler, task_detail_handler};
    use axum::body::to_bytes;
    use axum::extract::Path;
    use axum::http::StatusCode;
    use serial_test::serial;
    use tempfile::TempDir;

    /// One done run, two tasks (T1 depends on T0), one finding scoped to T0.
    const RUN_STATE: &str = r#"{"run_id":"r-mon","spec":"neutral demo","started_at":"2026-06-04T00:00:00Z","ended_at":"2026-06-04T00:01:00Z","status":"done","max_parallel":1,"pid":0,"tasks":{"T0":{"id":"T0","project":"billing-service","agent":"mock","status":"done","started_at":null,"ended_at":null,"chat_id":null,"error":null,"attempts":0,"risk_level":null,"artifacts":{},"permission":null,"workflow_outputs":{},"log_path":"/abs/run/logs/T0.log","trajectory_path":null,"depends_on":[],"parallel_group":null,"requires_approval_after":false,"kind":"agent","memory_used":[],"context_bytes":null,"skills_triggered":[],"usage":null,"steps":null,"role":"backend_rust","resolved_agent_profile":"backend-specialist","resolved_review_profile":null,"workspace_path":"/abs/secret/ws","worktree_path":null},"T1":{"id":"T1","project":"web-frontend","agent":"mock","status":"done","started_at":null,"ended_at":null,"chat_id":null,"error":null,"attempts":0,"risk_level":null,"artifacts":{},"permission":null,"workflow_outputs":{},"log_path":"/abs/run/logs/T1.log","trajectory_path":null,"depends_on":["T0"],"parallel_group":null,"requires_approval_after":false,"kind":"verify","memory_used":[],"context_bytes":null,"skills_triggered":[],"usage":null,"steps":null,"role":null,"resolved_agent_profile":null,"resolved_review_profile":"contract-reviewer","workspace_path":null,"worktree_path":null}},"approvals_pending":[],"task_order":["T0","T1"],"session_id":null,"usage":{},"budget_tokens":null,"pending_gate":null,"goal":null,"acceptance_results":[],"verified":false,"auto_actions":[],"run_dir":"."}"#;

    const FINDINGS: &str = "{\"schema_version\":\"maestro.finding.v1\",\"finding_id\":\"refute-1\",\"run_id\":\"r-mon\",\"seq\":1,\"task_id\":\"T0\",\"kind\":\"refute\",\"severity\":\"high\",\"summary\":\"x\",\"evidence_refs\":[],\"source\":\"refuter\",\"status\":\"open\",\"created_at\":\"2026-06-04T00:00:00Z\",\"provenance\":{\"producer\":\"refuter\"}}\n";

    fn workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let run = tmp.path().join(".maestro/runs/r-mon");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("RUN_STATE.json"), RUN_STATE).unwrap();
        std::fs::write(run.join("findings.ndjson"), FINDINGS).unwrap();
        unsafe { std::env::set_var("MAESTRO_WORKSPACE_ROOT", tmp.path()) };
        tmp
    }

    async fn body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    #[tokio::test]
    #[serial]
    async fn monitor_returns_schema_and_counts() {
        let _w = workspace();
        let resp = run_monitor_handler(Path("r-mon".into())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body(resp).await;
        assert_eq!(v["schema_version"], "maestro.run_monitor.v1");
        assert_eq!(v["status"], "done");
        assert_eq!(v["progress"]["total"], 2);
        assert_eq!(v["progress"]["done"], 2);
        assert_eq!(v["findings_summary"][0]["kind"], "refute");
        assert_eq!(v["findings_summary"][0]["count"], 1);
        // never leak the absolute workspace path
        assert!(!v.to_string().contains("/abs/secret"));
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    #[tokio::test]
    #[serial]
    async fn monitor_unknown_run_is_404() {
        let _w = workspace();
        let resp = run_monitor_handler(Path("no-such-run".into())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    #[tokio::test]
    #[serial]
    async fn task_detail_returns_task_scoped_findings_and_provenance() {
        let _w = workspace();
        let resp = task_detail_handler(Path(("r-mon".into(), "T0".into()))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body(resp).await;
        assert_eq!(v["schema_version"], "maestro.task_detail.v1");
        assert_eq!(v["resolved_agent_profile"], "backend-specialist");
        assert_eq!(v["downstream"][0], "T1");
        // only T0's finding
        assert_eq!(v["findings"].as_array().unwrap().len(), 1);
        assert_eq!(v["findings"][0]["task_id"], "T0");
        // T1 (no findings) returns an empty findings array
        let resp = task_detail_handler(Path(("r-mon".into(), "T1".into()))).await;
        let v = body(resp).await;
        assert_eq!(v["findings"].as_array().unwrap().len(), 0);
        assert!(!v.to_string().contains("/abs/"));
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    #[tokio::test]
    #[serial]
    async fn task_detail_unknown_task_is_404_and_bad_id_is_400() {
        let _w = workspace();
        let unknown = task_detail_handler(Path(("r-mon".into(), "ghost".into()))).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        // a traversal-like task id is rejected at the boundary (400), never
        // reaching the projector.
        for bad in ["../escape", "bad/name", ".."] {
            let resp = task_detail_handler(Path(("r-mon".into(), bad.into()))).await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "bad id {bad}");
        }
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    #[tokio::test]
    #[serial]
    async fn corrupt_findings_ledger_is_500_not_silent_empty() {
        // N1: a corrupt F-110 ledger is an audit failure — never project it as
        // "0 findings". (A MISSING ledger is still `[]`, handled by read_findings.)
        let w = workspace();
        std::fs::write(
            w.path().join(".maestro/runs/r-mon/findings.ndjson"),
            "{ not valid json\n",
        )
        .unwrap();
        let monitor = run_monitor_handler(Path("r-mon".into())).await;
        assert_eq!(monitor.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let detail = task_detail_handler(Path(("r-mon".into(), "T0".into()))).await;
        assert_eq!(detail.status(), StatusCode::INTERNAL_SERVER_ERROR);
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    /// F-115 Step 4 dogfood: the v2 event stream and the F-112 monitor must agree
    /// on the same run's lifecycle/status vocabulary, and `finding.recorded` must
    /// expose only a small payload + run-relative ref (no finding body).
    #[tokio::test]
    #[serial]
    async fn f115_events_agree_with_f112_monitor_and_finding_projection_is_minimal() {
        use crate::scheduler::findings::{append_finding, Finding, FindingKind, Severity};
        use crate::scheduler::{
            append_event, project_finding_event, read_events, RunEventKind, RunEventStatus,
        };
        let tmp = workspace();
        let run = tmp.path().join(".maestro/runs/r-mon");

        // F-115 lifecycle events for the same (done) run; status auto-fills.
        for (kind, task) in [
            (RunEventKind::RunCreated, None),
            (RunEventKind::TaskSucceeded, Some("T0")),
            (RunEventKind::TaskSucceeded, Some("T1")),
            (RunEventKind::RunCompleted, None),
        ] {
            append_event(&run, "r-mon", kind, task, None, serde_json::json!({})).unwrap();
        }

        // F-112 monitor for the same run.
        let mon = body(run_monitor_handler(Path("r-mon".into())).await).await;
        assert_eq!(mon["status"], "done");
        assert_eq!(mon["progress"]["done"], 2);

        // The two surfaces agree: run.completed carries status=done == monitor.status,
        // and the count of done task events matches monitor.progress.done.
        let events = read_events(&run).unwrap();
        assert!(events.iter().any(
            |e| e.kind == RunEventKind::RunCompleted && e.status == Some(RunEventStatus::Done)
        ));
        let task_done = events
            .iter()
            .filter(|e| {
                e.kind == RunEventKind::TaskSucceeded && e.status == Some(RunEventStatus::Done)
            })
            .count();
        assert_eq!(task_done as u64, mon["progress"]["done"].as_u64().unwrap());

        // finding.recorded carries only severity + small {kind,id,seq} + a
        // run-relative findings.ndjson ref — never the summary/body.
        let finding = Finding::new(
            "r-mon",
            FindingKind::Risk,
            Severity::High,
            "risk-gate",
            "DOGFOODSECRET summary that must not leak",
            "2026-06-04T00:00:00Z",
        )
        .task("T0");
        let written = append_finding(&run, finding).unwrap();
        project_finding_event(&run, &written, None, false);
        let proj = read_events(&run)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == RunEventKind::FindingRecorded)
            .expect("finding.recorded projected");
        assert_eq!(proj.severity, Some(Severity::High));
        assert_eq!(
            proj.refs.get("findings").and_then(|r| r.path.as_deref()),
            Some("findings.ndjson")
        );
        assert!(!proj.payload.to_string().contains("DOGFOODSECRET"));
        assert!(proj
            .message
            .as_deref()
            .is_none_or(|m| !m.contains("DOGFOODSECRET")));

        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }
}

#[cfg(test)]
mod events_stream_tests {
    //! F-115 Step 3 — the per-run typed event stream endpoint. Direct handler
    //! calls against a throwaway workspace; the SSE body is read via
    //! `into_data_stream()` with a short timeout window.
    use super::{
        event_stream_frames, read_current_state, resolve_stream_cursor, run_events_stream_handler,
        EventsStreamQuery,
    };
    use crate::scheduler::{append_event, RunEventKind};
    use crate::server::ServerState;
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use futures::StreamExt;
    use serial_test::serial;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::TempDir;

    fn state() -> ServerState {
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        ServerState { tx }
    }

    /// Workspace with a `run-evt` run holding `n` events (seq 1..=n).
    fn workspace_with_events(n: u64) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let run = tmp.path().join(".maestro/runs/run-evt");
        std::fs::create_dir_all(&run).unwrap();
        for i in 1..=n {
            append_event(
                &run,
                "run-evt",
                RunEventKind::TaskStarted,
                Some("t1"),
                None,
                serde_json::json!({ "i": i }),
            )
            .unwrap();
        }
        unsafe { std::env::set_var("MAESTRO_WORKSPACE_ROOT", tmp.path()) };
        (tmp, run)
    }

    fn cleanup() {
        unsafe { std::env::remove_var("MAESTRO_WORKSPACE_ROOT") };
    }

    /// Read SSE bytes until a `window_ms` gap with no new data.
    async fn drain(stream: &mut axum::body::BodyDataStream, window_ms: u64) -> String {
        let mut buf = Vec::new();
        while let Ok(Some(Ok(bytes))) =
            tokio::time::timeout(Duration::from_millis(window_ms), stream.next()).await
        {
            buf.extend_from_slice(&bytes);
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    fn q(since: Option<&str>) -> Query<EventsStreamQuery> {
        Query(EventsStreamQuery {
            since_seq: since.map(|s| s.to_string()),
        })
    }

    #[test]
    fn resolve_cursor_query_wins_over_header_and_rejects_non_numeric() {
        assert_eq!(resolve_stream_cursor(None, None).unwrap(), 0);
        assert_eq!(resolve_stream_cursor(Some("5"), None).unwrap(), 5);
        assert_eq!(resolve_stream_cursor(None, Some("3")).unwrap(), 3);
        // query wins even when a (here, bad) header is present
        assert_eq!(resolve_stream_cursor(Some("7"), Some("oops")).unwrap(), 7);
        assert!(resolve_stream_cursor(Some("abc"), None).is_err());
        assert!(resolve_stream_cursor(None, Some("nope")).is_err());
    }

    #[test]
    fn event_stream_frames_filters_after_cursor_in_order() {
        // Pure projector test — must NOT touch MAESTRO_WORKSPACE_ROOT, or it races
        // the serial async handler tests (N1). Use a bare temp run dir, no env.
        let tmp = TempDir::new().unwrap();
        let run = tmp.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        for i in 1..=3u64 {
            append_event(
                &run,
                "run-evt",
                RunEventKind::TaskStarted,
                Some("t1"),
                None,
                serde_json::json!({ "i": i }),
            )
            .unwrap();
        }
        let events = crate::scheduler::read_events(&run).unwrap();
        let frames = event_stream_frames(&events, 1);
        assert_eq!(
            frames.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(frames[0].1.contains("\"seq\":2"));
        assert!(!frames.iter().any(|(_, j)| j.contains("\"seq\":1")));
    }

    #[tokio::test]
    #[serial]
    async fn stream_catch_up_emits_run_event_frames_after_cursor() {
        let (_tmp, _run) = workspace_with_events(3);
        let resp = run_events_stream_handler(
            Path("run-evt".into()),
            q(Some("1")),
            HeaderMap::new(),
            State(state()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let mut stream = resp.into_body().into_data_stream();
        let body = drain(&mut stream, 250).await;
        cleanup();
        // two run_event frames for seq 2 and 3, id == seq, none for seq 1
        assert_eq!(body.matches("event: run_event").count(), 2);
        assert!(body.contains("id: 2"));
        assert!(body.contains("id: 3"));
        assert!(body.contains("\"seq\":2"));
        assert!(!body.contains("\"seq\":1"));
    }

    #[tokio::test]
    #[serial]
    async fn stream_live_tick_delivers_new_event() {
        let (_tmp, run) = workspace_with_events(2);
        let st = state();
        let resp = run_events_stream_handler(
            Path("run-evt".into()),
            q(None),
            HeaderMap::new(),
            State(st.clone()),
        )
        .await;
        let mut stream = resp.into_body().into_data_stream();
        let catch_up = drain(&mut stream, 250).await;
        assert!(catch_up.contains("id: 2"));
        // append a new event + fire the broadcast tick the watcher would emit
        append_event(
            &run,
            "run-evt",
            RunEventKind::TaskSucceeded,
            Some("t1"),
            None,
            serde_json::json!({}),
        )
        .unwrap();
        st.tx.send(()).unwrap();
        let live = drain(&mut stream, 1000).await;
        cleanup();
        assert!(live.contains("id: 3"), "live frame: {live:?}");
        assert!(live.contains("event: run_event"));
        assert!(live.contains("\"seq\":3"));
        // cursor advanced past the catch-up: the tick re-read must NOT re-emit the
        // already-delivered seq 1/2 (N2 cursor-based dedup).
        assert!(!live.contains("id: 1"), "no duplicate of seq 1: {live:?}");
        assert!(!live.contains("id: 2"), "no duplicate of seq 2: {live:?}");
    }

    #[tokio::test]
    #[serial]
    async fn stream_mid_stream_corruption_emits_error_and_ends() {
        let (_tmp, run) = workspace_with_events(1);
        let st = state();
        let resp = run_events_stream_handler(
            Path("run-evt".into()),
            q(None),
            HeaderMap::new(),
            State(st.clone()),
        )
        .await;
        let mut stream = resp.into_body().into_data_stream();
        let _catch_up = drain(&mut stream, 250).await;
        // ledger becomes unreadable mid-stream
        std::fs::write(run.join("events.ndjson"), "{not json\n").unwrap();
        st.tx.send(()).unwrap();
        let after = drain(&mut stream, 1000).await;
        cleanup();
        assert!(
            after.contains("event: error"),
            "must surface an error, not a silent empty stream: {after:?}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn stream_bad_cursor_is_400() {
        let (_tmp, _run) = workspace_with_events(1);
        let resp = run_events_stream_handler(
            Path("run-evt".into()),
            q(Some("not-a-number")),
            HeaderMap::new(),
            State(state()),
        )
        .await;
        cleanup();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    #[serial]
    async fn stream_unknown_run_is_404_and_bad_id_is_400() {
        let (_tmp, _run) = workspace_with_events(1);
        let unknown = run_events_stream_handler(
            Path("no-such-run".into()),
            q(None),
            HeaderMap::new(),
            State(state()),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        for bad in ["../escape", "bad/name", ".."] {
            let resp = run_events_stream_handler(
                Path(bad.into()),
                q(None),
                HeaderMap::new(),
                State(state()),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "bad id {bad}");
        }
        cleanup();
    }

    #[tokio::test]
    #[serial]
    async fn stream_corrupt_ledger_initial_read_is_500() {
        let (_tmp, run) = workspace_with_events(1);
        std::fs::write(run.join("events.ndjson"), "{not json\n").unwrap();
        let resp = run_events_stream_handler(
            Path("run-evt".into()),
            q(None),
            HeaderMap::new(),
            State(state()),
        )
        .await;
        cleanup();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    #[serial]
    async fn events_state_tick_source_unchanged() {
        // minimal regression on /api/events: it emits read_current_state() on each
        // tick; with no current run that is Ok(None) (the handler emits "null").
        let _tmp = workspace_with_events(1);
        assert!(matches!(read_current_state(), Ok(None)));
        cleanup();
    }
}
