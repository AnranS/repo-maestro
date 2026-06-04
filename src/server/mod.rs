//! Axum router for the local dashboard. Most of the surface area lives in
//! `server::handlers` — this file owns the Router assembly, the broadcast
//! channel used by SSE, and the filesystem watcher that flips that channel
//! whenever `RUN_STATE.json` changes on disk.

pub mod handlers;
pub mod ui;

use anyhow::{Context, Result};
use axum::{
    response::{sse::Event, Sse},
    routing::get,
    Router,
};
use futures::stream::Stream;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use crate::paths;

use handlers::chat;
use handlers::fs;
use handlers::misc;
use handlers::projects;
use handlers::runs;
use handlers::skills_memory;

/// Shared application state. We only need a broadcast channel that fires
/// whenever something on disk changes; SSE subscribers re-read the run
/// state file when they receive a tick.
#[derive(Clone)]
pub struct ServerState {
    pub tx: broadcast::Sender<()>,
}

/// Build the axum Router used by `serve` and integration tests. Tests
/// mount this via `axum-test` instead of binding a real TCP port, so
/// they can hit every endpoint without spawning the FS watcher or
/// binding to TCP.
pub fn build_app(st: ServerState) -> Router {
    Router::new()
        // run state + history
        .route("/api/state", get(runs::state_handler))
        .route("/api/runs", get(runs::runs_handler))
        .route("/api/runs/:id", get(runs::run_handler))
        .route("/api/runs/:id/evidence", get(runs::run_evidence_handler))
        .route("/api/runs/:id/findings", get(runs::run_findings_handler))
        .route("/api/runs/:id/monitor", get(runs::run_monitor_handler))
        .route(
            "/api/runs/:id/tasks/:task/detail",
            get(runs::task_detail_handler),
        )
        .route(
            "/api/runs/:id/events/stream",
            get(runs::run_events_stream_handler),
        )
        .route("/api/runs/:id/replay", get(runs::run_replay_handler))
        .route("/api/runs/:id/pr-body", get(runs::run_pr_body_handler))
        .route(
            "/api/runs/:id/cancel",
            axum::routing::post(runs::run_cancel),
        )
        .route(
            "/api/runs/:id/approve",
            axum::routing::post(runs::run_approve),
        )
        .route("/api/runs/:id/rerun", axum::routing::post(runs::run_rerun))
        .route("/api/runs/:id/outcome", get(runs::run_outcome))
        .route("/api/runs/:id/tasks/:task/diff", get(runs::task_diff))
        .route(
            "/api/runs/:id/tasks/:task/trajectory",
            get(runs::task_trajectory),
        )
        .route("/api/logs/:run/:task", get(runs::logs_handler))
        .route(
            "/api/logs/:run/:task/stream",
            get(runs::logs_stream_handler),
        )
        // SSE — only handler that needs ServerState
        .route("/api/events", get(events_handler))
        // chat
        .route(
            "/api/chat/sessions",
            get(chat::chat_sessions_list).post(chat::chat_sessions_create),
        )
        .route(
            "/api/chat/sessions/:id",
            get(chat::chat_session_get)
                .delete(chat::chat_session_delete)
                .patch(chat::chat_session_patch),
        )
        .route(
            "/api/chat/sessions/:id/tags",
            axum::routing::put(chat::chat_session_tags_put),
        )
        .route(
            "/api/chat/sessions/:id/auto-tag",
            axum::routing::post(chat::chat_session_auto_tag),
        )
        .route(
            "/api/chat/sessions/:id/compact",
            axum::routing::post(chat::chat_session_compact),
        )
        .route("/api/external/sessions", get(chat::external_sessions_list))
        .route(
            "/api/chat/current",
            get(chat::chat_current_get).put(chat::chat_current_put),
        )
        .route(
            "/api/chat/messages",
            axum::routing::post(chat::chat_messages_post),
        )
        .route(
            "/api/chat/actions/:session_id/:action_id",
            axum::routing::post(chat::chat_action_post),
        )
        // skills + memory
        .route("/api/skills", get(skills_memory::skills_list))
        .route(
            "/api/skills/:scope/:name",
            get(skills_memory::skill_get)
                .put(skills_memory::skill_put)
                .delete(skills_memory::skill_delete),
        )
        .route("/api/memory/l1", get(skills_memory::memory_list))
        .route("/api/memory/search", get(skills_memory::memory_search))
        .route("/api/memory/item", get(skills_memory::memory_item))
        .route("/api/memory/agent-status", get(misc::agentmemory_status))
        .route("/api/memory/graph", get(misc::memory_graph))
        .route("/api/memory/starmap", get(misc::memory_starmap))
        .route(
            "/api/memory/l1/:topic/:name",
            get(skills_memory::memory_get)
                .put(skills_memory::memory_put)
                .delete(skills_memory::memory_delete),
        )
        // projects + architecture
        .route(
            "/api/projects",
            get(projects::projects_get).post(projects::projects_post),
        )
        .route(
            "/api/projects/:name",
            axum::routing::delete(projects::projects_delete),
        )
        .route(
            "/api/projects/:name/memory",
            get(projects::project_memory_get),
        )
        .route("/api/architecture", get(projects::architecture_get))
        // filesystem browse (path picker)
        .route("/api/fs/list", get(fs::fs_list_handler))
        // models / docs / defaults
        .route("/api/models", get(misc::models_list))
        .route(
            "/api/models/refresh",
            axum::routing::post(misc::models_refresh),
        )
        .route(
            "/api/settings/defaults",
            get(misc::defaults_get).put(misc::defaults_put),
        )
        .route("/api/docs/index", get(misc::docs_index_handler))
        .route("/api/docs/page", get(misc::docs_page_handler))
        // multi-agent coordination mailbox
        .route("/api/mailbox", get(misc::mailbox_list))
        .route(
            "/api/mailbox/answer",
            axum::routing::post(misc::mailbox_answer),
        )
        // code knowledge graph (codegraph)
        .route("/api/codegraph/graph", get(misc::codegraph_graph))
        .route("/api/codegraph/file", get(misc::codegraph_file))
        .route("/api/codegraph/engines", get(misc::codegraph_engines))
        .route(
            "/api/codegraph/build",
            axum::routing::post(misc::codegraph_build),
        )
        // SPA fallback for embedded web assets
        .fallback(misc::static_asset_handler)
        .with_state(st)
}

pub async fn serve(host: &str, port: u16) -> Result<()> {
    let (tx, _rx) = broadcast::channel::<()>(64);

    // Filesystem watcher pushes a tick onto `tx` whenever anything under
    // `.maestro/runs/` changes, so the SSE handler can re-read and emit.
    let watch_tx = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_state(watch_tx).await {
            tracing::warn!("state watcher exited: {e:#}");
        }
    });

    let app = build_app(ServerState { tx });

    let addr: SocketAddr = format!("{host}:{port}").parse().context("parse addr")?;
    tracing::info!("maestro dashboard listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// SSE stream that emits `state` events whenever the watcher detects a
/// `RUN_STATE.json` change. Held in this file because it's the only
/// handler that needs `ServerState`, and keeping it here lets the
/// handlers/ submodules be `ServerState`-agnostic.
async fn events_handler(
    axum::extract::State(st): axum::extract::State<ServerState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = st.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|_| {
        runs::read_current_state()
            .ok()
            .flatten()
            .map(|body| Ok(Event::default().event("state").data(body)))
    });

    // First event sent immediately. We emit `null` rather than `{}` when
    // there is no current run — the frontend treats `{}` as a truthy
    // RunState and then crashes on `Object.values(state.tasks)`.
    let initial = futures::stream::once(async {
        let body = runs::read_current_state()
            .ok()
            .flatten()
            .unwrap_or_else(|| "null".into());
        Ok::<_, Infallible>(Event::default().event("state").data(body))
    });

    Sse::new(initial.chain(stream)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// Watch `.maestro/runs/` for changes and push a tick on the broadcast channel
/// for every relevant filesystem event. Bursts are coalesced with a 150 ms
/// debounce so a single executor step doesn't produce 10 SSE events.
async fn watch_state(tx: broadcast::Sender<()>) -> Result<()> {
    use notify::{Event, EventKind, RecursiveMode, Watcher};

    let runs = paths::runs_dir()?;
    paths::ensure_dir(&runs)?;

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            if matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                let _ = notify_tx.send(());
            }
        }
    })?;
    watcher.watch(&runs, RecursiveMode::Recursive)?;

    // hold watcher alive for the lifetime of the task
    tokio::spawn(async move {
        let _w = watcher;
        std::future::pending::<()>().await;
    });

    while notify_rx.recv().await.is_some() {
        let _ = tx.send(());
        tokio::time::sleep(Duration::from_millis(150)).await;
        while notify_rx.try_recv().is_ok() {}
    }
    Ok(())
}
