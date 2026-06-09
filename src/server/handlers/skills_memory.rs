//! `/api/skills/*` and `/api/memory/*`. Thin wrappers over the
//! `crate::skills` and `crate::memory` modules — file IO, no streaming.

use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};

// ─── memory recall (smart search + recent learnings) ───────────────────

#[derive(serde::Deserialize)]
pub struct MemSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    k: Option<usize>,
}

#[derive(serde::Serialize)]
struct MemoryHit {
    id: String,
    /// "fact" (L1) · "decision" (L2) · "replan".
    kind: String,
    project: Option<String>,
    title: String,
    excerpt: String,
    score: f32,
    updated_ms: Option<i64>,
}

/// Recall-driven memory: with `q`, returns TF-IDF-ranked hits across L1 facts,
/// L2 decisions and replans; without `q`, returns the most recent items (the
/// "what I've learned lately" timeline). This is what makes the memory feel
/// like a memory rather than a file tree.
pub async fn memory_search(Query(p): Query<MemSearchQuery>) -> Response {
    use crate::memory::retrieval::{build_index, retrieve};
    let chunks = match build_index() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let q = p.q.trim();
    let k = p.k.unwrap_or(30).clamp(1, 100);
    let items: Vec<MemoryHit> = if q.is_empty() {
        let mut v: Vec<MemoryHit> = chunks.into_iter().map(|c| to_hit(c, 0.0)).collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        v.truncate(k);
        v
    } else {
        // Semantic recall via local embeddings when available; TF-IDF otherwise.
        let hits =
            crate::memory::semantic_rank(&chunks, q, k).unwrap_or_else(|| retrieve(&chunks, q, k));
        hits.into_iter().map(|h| to_hit(h.chunk, h.score)).collect()
    };
    Json(&items).into_response()
}

#[derive(serde::Deserialize)]
pub struct MemItemQuery {
    kind: String,
    id: String,
}

#[derive(serde::Serialize)]
struct MemoryItem {
    id: String,
    kind: String,
    title: String,
    content: String,
    path: Option<String>,
    /// Only L1 facts are editable via the UI (PUT/DELETE /api/memory/l1).
    editable: bool,
}

/// Full content of one memory item, resolved through the retrieval index (so
/// the path is never client-supplied — no traversal risk).
pub async fn memory_item(Query(p): Query<MemItemQuery>) -> Response {
    use crate::memory::retrieval::{build_index, ChunkSource};
    let want = match p.kind.as_str() {
        "fact" => ChunkSource::L1Fact,
        "decision" => ChunkSource::L2Decision,
        "replan" => ChunkSource::Replan,
        _ => return (StatusCode::BAD_REQUEST, "unknown kind").into_response(),
    };
    let chunks = match build_index() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match chunks
        .into_iter()
        .find(|c| c.source == want && c.id == p.id)
    {
        Some(c) => {
            let hit = to_hit(c.clone(), 0.0);
            Json(&MemoryItem {
                id: hit.id,
                kind: hit.kind,
                title: hit.title,
                content: c.content,
                path: c.path.map(|p| p.display().to_string()),
                editable: matches!(want, ChunkSource::L1Fact),
            })
            .into_response()
        }
        None => (StatusCode::NOT_FOUND, "memory item not found").into_response(),
    }
}

fn to_hit(chunk: crate::memory::retrieval::Chunk, score: f32) -> MemoryHit {
    use crate::memory::retrieval::ChunkSource;
    let kind = match chunk.source {
        ChunkSource::L1Fact => "fact",
        ChunkSource::L2Decision => "decision",
        ChunkSource::Replan => "replan",
    }
    .to_string();
    // L2 ids look like "<project>/<file>"; surface the project.
    let project = matches!(chunk.source, ChunkSource::L2Decision)
        .then(|| chunk.id.split('/').next().map(str::to_string))
        .flatten();
    let title = chunk
        .content
        .lines()
        .find_map(|l| l.trim().strip_prefix("# ").map(str::trim))
        .or_else(|| chunk.content.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or(&chunk.id)
        .chars()
        .take(100)
        .collect();
    let excerpt: String = chunk
        .content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(280)
        .collect();
    let updated_ms = chunk.path.and_then(|p| {
        std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
    });
    MemoryHit {
        id: chunk.id,
        kind,
        project,
        title,
        excerpt,
        score,
        updated_ms,
    }
}

// ─── skills ───────────────────────────────────────────────────────────

/// The sidebar listing is metadata only — skill bodies stay off the wire and
/// are served exclusively by the `/api/skills/:scope/:name` editor route
/// (F-121 N1: viewing the inventory must not download playbook content).
pub async fn skills_list() -> Response {
    match crate::skills::list_all_summaries() {
        Ok(map) => Json(map).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

fn parse_scope(scope: &str) -> crate::skills::SkillScope {
    crate::skills::SkillScope::from_dir(scope)
}

/// Reject a client-supplied path segment that would traverse outside the
/// intended directory, mapping it to 400 (the store layer rejects it too — this
/// just yields a clean status instead of a 404/500). Returns the error response
/// to short-circuit on, or `None` when the value is a safe single component.
fn reject_traversal(kind: &str, value: &str) -> Option<Response> {
    crate::paths::validate_path_component(kind, value)
        .err()
        .map(|e| (StatusCode::BAD_REQUEST, format!("{e}")).into_response())
}

pub async fn skill_get(Path((scope, name)): Path<(String, String)>) -> Response {
    if let Some(r) = reject_traversal("skill scope", &scope) {
        return r;
    }
    match crate::skills::load(&parse_scope(&scope), &name) {
        Ok(s) => Json(s).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "skill not found").into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct SkillPut {
    content: String,
}

pub async fn skill_put(
    Path((scope, name)): Path<(String, String)>,
    body: Json<SkillPut>,
) -> Response {
    if let Some(r) = reject_traversal("skill scope", &scope) {
        return r;
    }
    match crate::skills::save(&parse_scope(&scope), &name, &body.content) {
        Ok(p) => Json(serde_json::json!({ "path": p.display().to_string() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn skill_delete(Path((scope, name)): Path<(String, String)>) -> Response {
    if let Some(r) = reject_traversal("skill scope", &scope) {
        return r;
    }
    match crate::skills::delete(&parse_scope(&scope), &name) {
        Ok(()) => (StatusCode::NO_CONTENT, "").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize, Default)]
pub struct InventoryQuery {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    profile: Option<String>,
}

/// F-121 — read-only skill/profile visibility inventory (`maestro.skill_inventory.v1`).
/// Metadata only: it never loads or returns skill bodies, and the projection is
/// validated before emit so a malformed result is a neutral 500, never half-JSON.
/// Registered BEFORE `/api/skills/:scope/:name` so the single-segment
/// `inventory` path is never read as a `:scope`. A path-unsafe query value is a
/// 400; an unknown (but well-formed) project/profile is a 404; a missing skills
/// root is a 200 empty inventory.
pub async fn skills_inventory(Query(q): Query<InventoryQuery>) -> Response {
    // Neutral guard: a path-unsafe query value is a 400, but — unlike the editor
    // route's `reject_traversal` — the body never echoes the raw value (this is a
    // privacy/metadata surface; Step 1 also never re-emits a rejected value).
    if let Some(p) = &q.project {
        if crate::paths::validate_path_component("project", p).is_err() {
            return (StatusCode::BAD_REQUEST, "invalid project").into_response();
        }
    }
    if let Some(p) = &q.profile {
        if crate::paths::validate_path_component("profile", p).is_err() {
            return (StatusCode::BAD_REQUEST, "invalid profile").into_response();
        }
    }

    let cfg = match crate::server::handlers::projects::load_projects_cfg() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("skill inventory: config load failed: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill inventory unavailable",
            )
                .into_response();
        }
    };

    if let Some(p) = &q.project {
        if !cfg.projects.contains_key(p) {
            return (StatusCode::NOT_FOUND, "project not found").into_response();
        }
    }
    let profile_input = match &q.profile {
        Some(name) => match cfg.defaults.agent_profiles.get(name) {
            Some(ap) => Some(crate::skills::inventory::ProfileInput::Present(name, ap)),
            None => return (StatusCode::NOT_FOUND, "profile not found").into_response(),
        },
        None => None,
    };

    let root = match crate::skills::inventory::skills_inventory_root() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("skill inventory: root resolve failed: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill inventory unavailable",
            )
                .into_response();
        }
    };

    let inv = crate::skills::inventory::build_inventory(
        &root,
        q.project.as_deref(),
        profile_input,
        &crate::schema::skill_inventory::SkillInventoryBudget::default(),
    );

    if let Err(e) = crate::schema::skill_inventory::validate_inventory(&inv) {
        tracing::warn!("skill inventory: validation failed: {e:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "skill inventory unavailable",
        )
            .into_response();
    }
    Json(inv).into_response()
}

// ─── memory ────────────────────────────────────────────────────────────

pub async fn memory_list() -> Response {
    match crate::memory::MemoryStore::open().and_then(|s| s.list_l1()) {
        Ok(by_topic) => Json(by_topic).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn memory_get(Path((topic, name)): Path<(String, String)>) -> Response {
    if let Some(r) =
        reject_traversal("memory topic", &topic).or_else(|| reject_traversal("memory name", &name))
    {
        return r;
    }
    let store = match crate::memory::MemoryStore::open() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match store.read(&topic, &name) {
        Ok(content) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            content,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "memory entry not found").into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct MemoryPut {
    content: String,
}

pub async fn memory_put(
    Path((topic, name)): Path<(String, String)>,
    body: Json<MemoryPut>,
) -> Response {
    if let Some(r) =
        reject_traversal("memory topic", &topic).or_else(|| reject_traversal("memory name", &name))
    {
        return r;
    }
    let store = match crate::memory::MemoryStore::open() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match store.add(&topic, &name, &body.content) {
        Ok(p) => Json(serde_json::json!({ "path": p.display().to_string() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn memory_delete(Path((topic, name)): Path<(String, String)>) -> Response {
    if let Some(r) =
        reject_traversal("memory topic", &topic).or_else(|| reject_traversal("memory name", &name))
    {
        return r;
    }
    let store = match crate::memory::MemoryStore::open() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    match store.delete(&topic, &name) {
        Ok(()) => (StatusCode::NO_CONTENT, "").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}
