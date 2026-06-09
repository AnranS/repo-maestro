//! Smaller, mostly-stateless endpoints that didn't justify their own
//! file: settings defaults, model cache, embedded docs, static assets.

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};

use crate::paths;

use super::projects::load_projects_cfg;
use crate::server::ui;

// ─── defaults (projects.yaml's defaults block) ─────────────────────────

pub async fn defaults_get() -> Response {
    match load_projects_cfg() {
        Ok(cfg) => Json(&cfg.defaults).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/providers/profiles` — read-only per-provider enforcement matrix (F-136a2).
/// Projects `provider_permission_profile` so the Settings "Providers" block shows the
/// honest hard/soft/advisory boundary without a drifting hand-copy in TS. No behavior,
/// no settings, no query params (unknown providers are covered by a Rust unit test).
pub async fn provider_profiles() -> Response {
    Json(crate::schema::permissions::provider_enforcement_profiles()).into_response()
}

#[derive(serde::Deserialize)]
pub struct DefaultsPut {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    branch_prefix: Option<String>,
    #[serde(default)]
    max_parallel: Option<usize>,
    /// `Some("")` clears the value.
    #[serde(default)]
    agent_model: Option<String>,
    /// Legacy alias for `agent_model`. `Some("")` clears the canonical value.
    #[serde(default)]
    cursor_model: Option<String>,
    #[serde(default)]
    tagger_model: Option<String>,
}

pub async fn defaults_put(body: Json<DefaultsPut>) -> Response {
    let Ok(mut cfg) = load_projects_cfg() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load projects.yaml",
        )
            .into_response();
    };
    if let Some(v) = &body.agent {
        if !v.trim().is_empty() {
            cfg.defaults.agent = v.clone();
        }
    }
    if let Some(v) = &body.branch_prefix {
        if !v.trim().is_empty() {
            cfg.defaults.branch_prefix = v.clone();
        }
    }
    if let Some(n) = body.max_parallel {
        if n > 0 {
            cfg.defaults.max_parallel = n;
        }
    }
    if let Some(v) = body.agent_model.as_ref().or(body.cursor_model.as_ref()) {
        cfg.defaults.agent_model = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_string())
        };
        cfg.defaults.cursor_model = None;
    }
    if let Some(v) = &body.tagger_model {
        cfg.defaults.tagger_model = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_string())
        };
    }
    let pfile = match paths::projects_file() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if let Err(e) = cfg.save(&pfile) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    Json(&cfg.defaults).into_response()
}

// ─── agentmemory (external local memory engine) ────────────────────────

/// Whether the agentmemory engine is reachable on its local REST port (3111).
/// A light TCP probe — surfaces the connection in the Memory tab. (Pulling its
/// memories in awaits a stable documented API; its surface is currently MCP.)
pub async fn agentmemory_status() -> Response {
    let connected = "127.0.0.1:3111"
        .parse()
        .ok()
        .map(|addr| {
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
                .is_ok()
        })
        .unwrap_or(false);
    Json(&serde_json::json!({ "connected": connected, "port": 3111 })).into_response()
}

/// Knowledge graph over L2 decision memory: project + run nodes, `produced`
/// and `consumes` edges. Powers the Memory tab's graph mode.
pub async fn memory_graph() -> Response {
    let l2_root = match paths::maestro_dir() {
        Ok(d) => d.join("memory").join(crate::memory::L2_DIR),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    // Contract overlay is best-effort — if projects.yaml is missing we still
    // return the run/project graph.
    let cfg = load_projects_cfg().ok();
    match crate::memory::graph::build_from(&l2_root, cfg.as_ref()) {
        Ok(g) => Json(&g).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// F-118: read-only local runtime readiness (`maestro.runtime_health.v1`). The
/// report is validated BEFORE it is serialized, so a self-inconsistent report
/// becomes a 500 rather than half-structured JSON. It carries no absolute path,
/// env value, or provider stdout/stderr — only verdicts + symbolic refs. Powers
/// the Dashboard's six readiness rows.
pub async fn runtime_health() -> Response {
    let generated_at = chrono::Utc::now().to_rfc3339();
    runtime_health_response(crate::runtime_health::build_validated_report(generated_at).await)
}

/// Map a built report (or its validation error) to a response. Split out so the
/// 500 path is unit-testable: the validator's error text can quote a REJECTED
/// message / ref / path, so it must never reach the HTTP body — we log it
/// server-side and return a fixed neutral string instead.
fn runtime_health_response(
    result: anyhow::Result<crate::schema::runtime_health::RuntimeHealthReport>,
) -> Response {
    match result {
        Ok(report) => Json(&report).into_response(),
        Err(e) => {
            tracing::warn!(error = ?e, "runtime health report failed validation");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime health report failed validation",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn runtime_health_500_body_never_carries_raw_error_detail() {
        // an error that quotes a rejected path/ref/message must not reach the body.
        let leaky = anyhow::anyhow!(
            "health ref \"/opt/secret/.maestro\" rejected; message at /opt/x has MAESTRO_CODEX"
        );
        let resp = runtime_health_response(Err(leaky));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8_lossy(&bytes);
        assert_eq!(body, "runtime health report failed validation");
        for leak in ["/opt/secret", "/opt/x", "MAESTRO_CODEX", "rejected"] {
            assert!(!body.contains(leak), "500 body leaked {leak:?}: {body}");
        }
    }
}

/// Decision-level star map over L2 memory: stars (decisions) clustered into
/// project systems, stitched by run constellations, with contract gravity.
/// Powers the Memory tab's star-map mode.
pub async fn memory_starmap() -> Response {
    let l2_root = match paths::maestro_dir() {
        Ok(d) => d.join("memory").join(crate::memory::L2_DIR),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let cfg = load_projects_cfg().ok();
    match crate::memory::graph::build_starmap(&l2_root, cfg.as_ref()) {
        Ok(g) => Json(&g).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ─── codegraph (code knowledge graph view) ─────────────────────────────

fn codegraph_db() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    crate::codegraph::find_db(&cwd)
}

fn codegraph_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    crate::codegraph::find_repo_root(&cwd)
}

/// Which engines power the code graph (for the dashboard chooser).
pub async fn codegraph_engines() -> Response {
    Json(&crate::codegraph::engine_status(&codegraph_root())).into_response()
}

#[derive(serde::Deserialize)]
pub struct CodegraphBuildBody {
    engine: String,
}

/// Build a richer code graph on demand. Only `codegraph` is buildable
/// server-side (free, tree-sitter); `understand` returns the command to run in
/// Claude Code (it's an LLM pass — maestro can't run it and won't spend tokens
/// on the user's behalf).
pub async fn codegraph_build(body: Json<CodegraphBuildBody>) -> Response {
    let root = codegraph_root();
    match body.engine.as_str() {
        "codegraph" => {
            if !crate::codegraph::codegraph_installed() {
                return (
                    StatusCode::BAD_REQUEST,
                    "codegraph CLI not installed (PATH or ~/.local/bin)".to_string(),
                )
                    .into_response();
            }
            let bin = crate::codegraph::codegraph_bin();
            if crate::codegraph::find_db(&root).is_none() {
                let _ = std::process::Command::new(&bin).arg("init").arg(&root).status();
            }
            let ok = std::process::Command::new(&bin)
                .arg("index")
                .arg(&root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                Json(serde_json::json!({ "ok": true, "engine": "codegraph" })).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "codegraph index failed".to_string())
                    .into_response()
            }
        }
        "understand" | "understand-anything" => Json(serde_json::json!({
            "ok": false,
            "manual": true,
            "command": format!("/understand {}", root.display()),
            "note": "Understand-Anything is a Claude Code plugin and does an LLM pass — run it there; it uses tokens.",
        }))
        .into_response(),
        other => (
            StatusCode::BAD_REQUEST,
            format!("unknown engine `{other}`"),
        )
            .into_response(),
    }
}

/// File-level code graph (nodes = files, edges = cross-file imports/modules).
///
/// Source preference, richest first:
///   1. Understand-Anything graph (`.understand-anything/knowledge-graph.json`)
///      — LLM summaries, tags, layers, tour;
///   2. external `.codegraph` tree-sitter DB (needs the `codegraph` feature);
///   3. native walk — always available, so the tab works out of the box.
pub async fn codegraph_graph() -> Response {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    // The native fallback rebuilds the whole graph (parsing every source file)
    // on each request — seconds on a large workspace, and the dashboard re-hits
    // it on every visit / live refresh. Cache the serialized response briefly so
    // repeat calls are instant. (CodeGraph isn't Clone, so we memo the JSON.)
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, (Instant, String)>>> = OnceLock::new();
    const TTL: Duration = Duration::from_secs(8);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let root = codegraph_root();
    if let Ok(map) = cache.lock() {
        if let Some((at, body)) = map.get(&root) {
            if at.elapsed() < TTL {
                return ([(header::CONTENT_TYPE, "application/json")], body.clone())
                    .into_response();
            }
        }
    }
    let body = compute_codegraph_json(&root);
    if let Ok(mut map) = cache.lock() {
        map.insert(root.clone(), (Instant::now(), body.clone()));
    }
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Serialize the best-available code graph for `root` to JSON: an
/// understand-anything graph if present, else a built codegraph DB, else the
/// native (parse-on-the-fly) graph. Split out so [`codegraph_graph`] can memo
/// the result.
fn compute_codegraph_json(root: &std::path::Path) -> String {
    if let Some(p) = crate::codegraph::find_understand_graph(root) {
        if let Some(g) = crate::codegraph::understand_graph(&p) {
            return serde_json::to_string(&g).unwrap_or_default();
        }
    }
    if let Some(db) = codegraph_db() {
        if let Ok(g) = crate::codegraph::file_graph(&db) {
            if g.available {
                return serde_json::to_string(&g).unwrap_or_default();
            }
        }
    }
    serde_json::to_string(&crate::codegraph::native_graph(root)).unwrap_or_default()
}

#[derive(serde::Deserialize)]
pub struct CodeGraphFileQuery {
    path: String,
}

/// Symbols defined in one file (for the side panel on node click).
pub async fn codegraph_file(Query(p): Query<CodeGraphFileQuery>) -> Response {
    let root = codegraph_root();
    if let Some(ua) = crate::codegraph::find_understand_graph(&root) {
        let s = crate::codegraph::understand_file_symbols(&ua, &p.path);
        if !s.is_empty() {
            return Json(&s).into_response();
        }
    }
    if let Some(db) = codegraph_db() {
        if let Ok(s) = crate::codegraph::file_symbols(&db, &p.path) {
            if !s.is_empty() {
                return Json(&s).into_response();
            }
        }
    }
    Json(&crate::codegraph::native_file_symbols(&root, &p.path)).into_response()
}

// ─── mailbox (multi-agent coordination) ────────────────────────────────

/// All mailbox messages (open + resolved), newest-first. Surfaces the
/// agent-to-agent coordination the scheduler delivers into prompts so the
/// Web UI can show who told whom what, and whether it's been handled.
pub async fn mailbox_list() -> Response {
    let store = match crate::mailbox::MailboxStore::open() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match store.list(&crate::mailbox::MailFilter::default()) {
        Ok(messages) => Json(&messages).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct MailboxAnswerBody {
    id: String,
    keys: Vec<String>,
}

/// Answer a mailbox `ask` message: records the chosen option key(s) and marks
/// it resolved (which unblocks any task waiting on it). Powers the Web UI's
/// answerable question buttons.
pub async fn mailbox_answer(body: Json<MailboxAnswerBody>) -> Response {
    let store = match crate::mailbox::MailboxStore::open() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match store.answer(&body.id, &body.keys) {
        Ok(message) => Json(&message).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

// ─── models ────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
pub struct ModelsQuery {
    #[serde(default)]
    provider: Option<String>,
}

pub async fn models_list(Query(q): Query<ModelsQuery>) -> Response {
    // First hit (no cache) transparently triggers a refresh so the dropdown
    // immediately shows the user's full account model list instead of the
    // tiny hardcoded fallback. Subsequent hits read straight from disk.
    let list = match provider_query(q.provider.as_deref()) {
        Ok(Some(provider)) => crate::models::load_or_refresh_for(provider).await,
        Ok(None) => crate::models::load_or_refresh_all().await,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    Json(list).into_response()
}

pub async fn models_refresh(Query(q): Query<ModelsQuery>) -> Response {
    let result = match provider_query(q.provider.as_deref()) {
        Ok(Some(provider)) => crate::models::refresh_provider(provider).await,
        Ok(None) => crate::models::refresh_all().await,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match result {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

fn provider_query(provider: Option<&str>) -> Result<Option<&str>, String> {
    let Some(provider) = provider.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    if crate::models::MODEL_PROVIDERS.contains(&provider) {
        Ok(Some(provider))
    } else {
        Err(format!("unknown provider `{provider}`"))
    }
}

// ─── docs ──────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
pub struct DocsLangQuery {
    #[serde(default)]
    lang: Option<String>,
}

pub async fn docs_index_handler(Query(q): Query<DocsLangQuery>) -> Response {
    match crate::docs::load_index(q.lang.as_deref()) {
        Ok(idx) => Json(idx).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct DocsPageQuery {
    file: String,
    #[serde(default)]
    lang: Option<String>,
}

pub async fn docs_page_handler(Query(q): Query<DocsPageQuery>) -> Response {
    match crate::docs::load_page(&q.file, q.lang.as_deref()) {
        Ok(body) => (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
    }
}

// ─── static asset fallback ─────────────────────────────────────────────

pub async fn static_asset_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let asset_path = if path.is_empty() { "index.html" } else { path };
    match ui::asset(asset_path) {
        Some(asset) => ([(header::CONTENT_TYPE, asset.mime)], asset.body).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
