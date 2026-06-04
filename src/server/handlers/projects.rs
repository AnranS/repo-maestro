//! `/api/projects/*`, `/api/projects/:name/memory`, and `/api/architecture`.
//! Reads and mutates `projects.yaml`; derives the architecture graph
//! from contract edges; aggregates per-project memory + recent runs
//! for the Architecture side panel and the Context tab's project filter.

use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::memory::MemoryStore;
use crate::paths;
use crate::scheduler::RunState;

#[derive(serde::Serialize)]
struct ArchModuleView {
    name: String,
    r#type: Option<String>,
    stack: Vec<String>,
    path: String,
    provides: Option<String>,
    consumes: Option<String>,
    memory_scope: Vec<String>,
}

#[derive(serde::Serialize)]
struct ArchEdgeView {
    from: String,
    to: String,
    file: String,
    kind: String,
    confidence: Option<u8>,
    /// True when discovery inferred this edge from source imports rather than a
    /// manifest/contract declaration — so the dashboard can mark it.
    inferred: bool,
    /// Number of cross-module file imports behind a code-graph-rolled edge, so
    /// the dashboard can weight (thicken) strong links. `None` for declared edges.
    #[serde(skip_serializing_if = "Option::is_none")]
    weight: Option<u32>,
    evidence: Vec<String>,
}

#[derive(serde::Deserialize)]
struct DiscoveredEdgeRow {
    from: String,
    to: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    confidence: Option<u8>,
}

/// Load the discovered-topology provenance persisted by `discover_and_apply`.
fn load_topology_edges() -> Vec<DiscoveredEdgeRow> {
    let Ok(dir) = crate::paths::maestro_dir() else {
        return vec![];
    };
    let path = dir.join("topology.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    #[derive(serde::Deserialize)]
    struct Topo {
        #[serde(default)]
        edges: Vec<DiscoveredEdgeRow>,
    }
    serde_json::from_str::<Topo>(&text)
        .map(|t| t.edges)
        .unwrap_or_default()
}

#[derive(serde::Serialize)]
struct ArchitectureView {
    modules: Vec<ArchModuleView>,
    edges: Vec<ArchEdgeView>,
}

pub async fn architecture_get() -> Response {
    let cfg = match load_projects_cfg() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    let modules: Vec<ArchModuleView> = cfg
        .projects
        .iter()
        .map(|(name, p)| ArchModuleView {
            name: name.clone(),
            r#type: p.r#type.clone(),
            stack: p.stack.clone(),
            path: p.path.clone(),
            provides: p.contracts.provides.clone(),
            consumes: p.contracts.consumes.clone(),
            memory_scope: p.memory_scope.clone(),
        })
        .collect();

    // Edge inference: for each consumer, point at all producers of the
    // same contract. We use loose path matching (basename + tail-of-2
    // overlap) so that a consumer writing
    //   contracts.consumes: ../server/schemas/openapi.yaml
    // still finds a producer declaring
    //   contracts.provides: schemas/openapi.yaml
    // — which is exactly how relative paths sit across sibling repos.
    let mut edge_map = BTreeMap::<(String, String), ArchEdgeView>::new();
    for (name, p) in &cfg.projects {
        for dependency in &p.dependencies {
            if dependency == name || !cfg.projects.contains_key(dependency) {
                continue;
            }
            let key = (dependency.clone(), name.clone());
            edge_map.insert(
                key,
                ArchEdgeView {
                    from: dependency.clone(),
                    to: name.clone(),
                    file: "depends_on".to_string(),
                    kind: "dependency".to_string(),
                    confidence: None,
                    inferred: false,
                    weight: None,
                    evidence: vec![format!("projects.{name}.dependencies:{dependency}")],
                },
            );
        }
    }

    for (name, p) in &cfg.projects {
        let Some(consumed) = &p.contracts.consumes else {
            continue;
        };
        for (other, p2) in &cfg.projects {
            if other == name {
                continue;
            }
            let Some(provided) = &p2.contracts.provides else {
                continue;
            };
            if paths_logically_match(provided, consumed) {
                let key = (other.clone(), name.clone());
                if let Some(edge) = edge_map.get_mut(&key) {
                    edge.file = consumed.clone();
                    edge.kind = "dependency+contract".to_string();
                    edge.evidence
                        .push(format!("projects.{name}.contracts.consumes:{consumed}"));
                    edge.evidence
                        .push(format!("projects.{other}.contracts.provides:{provided}"));
                } else {
                    edge_map.insert(
                        key,
                        ArchEdgeView {
                            from: other.clone(),
                            to: name.clone(),
                            file: consumed.clone(),
                            kind: "contract".to_string(),
                            confidence: None,
                            inferred: false,
                            weight: None,
                            evidence: vec![
                                format!("projects.{name}.contracts.consumes:{consumed}"),
                                format!("projects.{other}.contracts.provides:{provided}"),
                            ],
                        },
                    );
                }
            }
        }
    }
    // Enrich with discovery provenance: mark edges inferred from source imports
    // and attach their confidence, so the dashboard can distinguish them from
    // declared dependencies.
    let discovered = load_topology_edges();
    if !discovered.is_empty() {
        for edge in edge_map.values_mut() {
            if let Some(d) = discovered
                .iter()
                .find(|d| d.from == edge.from && d.to == edge.to)
            {
                if edge.confidence.is_none() {
                    edge.confidence = d.confidence;
                }
                if d.reason.contains("source import") {
                    edge.inferred = true;
                    edge.evidence.push(format!("inferred: {}", d.reason));
                }
            }
        }
    }
    // Derive module→module edges from real imports (parsed from source, else
    // the code-graph index). This surfaces relationships in a monorepo whose
    // modules carry no declared `dependencies` and whose contracts don't cross
    // — e.g. a Bazel/Go monorepo wired purely by Go imports. Weak (one-off)
    // links are dropped once the graph is large so it stays legible.
    {
        let module_paths: Vec<(String, String)> = cfg
            .projects
            .iter()
            .map(|(name, p)| (name.clone(), p.path.clone()))
            .collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let rolled = crate::codegraph::module_dependency_edges_cached(&cwd, &module_paths);
        // Show ALL derived edges (no per-consumer cap). When edges came from
        // the old phantom-calls codegraph rollup we capped at top-3 to keep a
        // 391-edge hairball legible; now the signal is real (only import +
        // strict contract scan), the full set is small + clean, and capping
        // would just hide structure the planner is using anyway. Consistency
        // win: architecture view shows what brief / plan / deliberate see.
        for m in &rolled {
            let key = (m.from.clone(), m.to.clone());
            if m.from == m.to
                || !cfg.projects.contains_key(&m.from)
                || !cfg.projects.contains_key(&m.to)
            {
                continue;
            }
            let detail = format!("codegraph: {} cross-module import(s)", m.weight);
            match edge_map.get_mut(&key) {
                Some(edge) => {
                    edge.weight = Some(m.weight);
                    if edge.confidence.is_none() {
                        edge.confidence = Some(confidence_from_weight(m.weight));
                    }
                    edge.evidence.push(detail);
                }
                None => {
                    // Carry the channel: `import` / `contract` / `import+contract` —
                    // so the UI distinguishes RPC contract coupling from code imports.
                    let file_label = match m.kind.as_str() {
                        "contract" => "rpc contract",
                        "import+contract" => "imports + contract",
                        _ => "imports",
                    };
                    edge_map.insert(
                        key,
                        ArchEdgeView {
                            from: m.from.clone(),
                            to: m.to.clone(),
                            file: file_label.to_string(),
                            kind: m.kind.clone(),
                            confidence: Some(confidence_from_weight(m.weight)),
                            inferred: true,
                            weight: Some(m.weight),
                            evidence: vec![detail],
                        },
                    );
                }
            }
        }
    }

    let edges = edge_map.into_values().collect();

    Json(ArchitectureView { modules, edges }).into_response()
}

/// Map a code-graph rollup weight (count of cross-module file imports) to a
/// percentage confidence: one import ≈ 60%, saturating toward 95% as the link
/// strengthens.
fn confidence_from_weight(weight: u32) -> u8 {
    (60 + weight.saturating_sub(1).saturating_mul(5)).min(95) as u8
}

/// Loose path equivalence — same rules as `memory::paths_logically_match`,
/// duplicated here so `server::handlers::projects` doesn't drag in a
/// memory dep. Match on basename + two-segment tail overlap; treat empty
/// strings as no-match. Keeps `schemas/api.yaml` vs
/// `../server/schemas/api.yaml` resolving the same way for both edge
/// inference and contract-aware memory injection.
fn paths_logically_match(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let basename = |s: &str| -> String {
        s.rsplit('/')
            .next()
            .unwrap_or(s)
            .trim_end_matches(|c: char| c.is_whitespace())
            .to_string()
    };
    let ba = basename(a);
    let bb = basename(b);
    if ba.is_empty() || bb.is_empty() || ba != bb {
        return false;
    }
    let tail = |s: &str| -> String {
        let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
        let n = parts.len();
        if n >= 2 {
            format!("{}/{}", parts[n - 2], parts[n - 1])
        } else {
            parts.join("/")
        }
    };
    tail(a) == tail(b) || ba == bb
}

pub async fn projects_get() -> Response {
    let pfile = match paths::projects_file() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if !pfile.exists() {
        return Json(serde_json::json!({ "projects": [] })).into_response();
    }
    match crate::config::ProjectsConfig::load(&pfile) {
        Ok(cfg) => {
            let names: Vec<String> = cfg.projects.keys().cloned().collect();
            Json(serde_json::json!({ "projects": names })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct ProjectsPost {
    name: String,
    path: String,
    #[serde(default, rename = "type")]
    type_: Option<String>,
    #[serde(default)]
    stack: Vec<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    memory_scope: Vec<String>,
    #[serde(default)]
    provides: Option<String>,
    #[serde(default)]
    consumes: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    agent_model: Option<String>,
    #[serde(default)]
    cursor_model: Option<String>,
    #[serde(default)]
    model_profile: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

pub async fn projects_post(body: Json<ProjectsPost>) -> Response {
    let ProjectsPost {
        name,
        path,
        type_,
        stack,
        agent,
        memory_scope,
        provides,
        consumes,
        dependencies,
        agent_model,
        cursor_model,
        model_profile,
        role,
    } = body.0;

    let name = name.trim().to_string();
    let path_in = path.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name is required").into_response();
    }
    if path_in.is_empty() {
        return (StatusCode::BAD_REQUEST, "path is required").into_response();
    }

    let expanded = match paths::expand(path_in) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("bad path: {e:#}")).into_response(),
    };
    if !expanded.exists() {
        return (
            StatusCode::BAD_REQUEST,
            format!("path does not exist: {}", expanded.display()),
        )
            .into_response();
    }

    let pfile = match paths::projects_file() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let mut cfg = if pfile.exists() {
        match crate::config::ProjectsConfig::load(&pfile) {
            Ok(c) => c,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        }
    } else {
        crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        }
    };

    if cfg.projects.contains_key(&name) {
        return (
            StatusCode::CONFLICT,
            format!("project '{name}' already exists; use DELETE first or pick a different name"),
        )
            .into_response();
    }

    cfg.projects.insert(
        name.clone(),
        crate::config::Project {
            path: expanded.to_string_lossy().to_string(),
            r#type: type_.filter(|s| !s.trim().is_empty()),
            stack: stack
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            commands: Default::default(),
            contracts: crate::config::Contracts {
                provides: provides.filter(|s| !s.trim().is_empty()),
                consumes: consumes.filter(|s| !s.trim().is_empty()),
            },
            dependencies: dependencies
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            memory_scope: memory_scope
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            agent: agent.filter(|s| !s.trim().is_empty()),
            agent_model: agent_model
                .or(cursor_model)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            cursor_model: None,
            model_profile: model_profile.filter(|s| !s.trim().is_empty()),
            role: role.filter(|s| !s.trim().is_empty()),
            agent_profile: None,
            review_profile: None,
            copy_files: Vec::new(),
        },
    );

    if let Err(e) = cfg.save(&pfile) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }

    Json(cfg.projects.get(&name)).into_response()
}

pub async fn projects_delete(Path(name): Path<String>) -> Response {
    let pfile = match paths::projects_file() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if !pfile.exists() {
        return (StatusCode::NOT_FOUND, "no projects.yaml").into_response();
    }
    let mut cfg = match crate::config::ProjectsConfig::load(&pfile) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    if cfg.projects.remove(&name).is_none() {
        return (StatusCode::NOT_FOUND, format!("project '{name}' not found")).into_response();
    }
    if let Err(e) = cfg.save(&pfile) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    (StatusCode::NO_CONTENT, "").into_response()
}

/// Tolerant loader: missing `projects.yaml` yields an empty config rather
/// than erroring, so endpoints like `/api/defaults` work on a fresh
/// workspace before any project is registered.
pub(crate) fn load_projects_cfg() -> anyhow::Result<crate::config::ProjectsConfig> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        return Ok(crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        });
    }
    crate::config::ProjectsConfig::load(&pfile)
}

// ─── Per-project memory aggregate ──────────────────────────────────────

/// Bundle of everything maestro "remembers" about one project:
///   - the L1 facts in topics the project's `memory_scope` opts into
///   - every L2 decision archived under `l2_decisions/<project>/`
///   - the last few runs whose tasks touched this project
///   - contracts in/out + role + stack snapshot
///
/// Used by both the Architecture side panel (click a node) and the
/// Context tab's project filter so the UI stays in sync.
#[derive(Serialize)]
struct ProjectMemoryView {
    name: String,
    r#type: Option<String>,
    stack: Vec<String>,
    role: Option<String>,
    memory_scope: Vec<String>,
    path: String,
    contracts: ContractView,
    l1_facts: Vec<L1FactView>,
    l2_decisions: Vec<L2DecisionView>,
    recent_runs: Vec<RecentRunView>,
}

#[derive(Serialize)]
struct ContractView {
    provides: Option<String>,
    consumes: Option<String>,
}

#[derive(Serialize)]
struct L1FactView {
    topic: String,
    file: String,
    /// Up to ~600 chars of the file body so the UI can preview without
    /// a second round-trip. Full body is at `/api/memory/l1/:topic/:name`.
    preview: String,
}

#[derive(Serialize)]
struct L2DecisionView {
    file: String,
    bytes: u64,
    /// First ~600 chars of the markdown so the panel can show a glance.
    preview: String,
}

#[derive(Serialize)]
struct RecentRunView {
    run_id: String,
    spec: String,
    status: String,
    started_at: String,
    verified: bool,
    /// Tasks within this run whose `project` matches the queried project.
    task_ids_in_project: Vec<String>,
}

pub async fn project_memory_get(Path(name): Path<String>) -> Response {
    let cfg = match load_projects_cfg() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let Some(project) = cfg.projects.get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            format!("project {name:?} not registered"),
        )
            .into_response();
    };

    let memory = match MemoryStore::open() {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    let l1_facts = collect_l1_for_scope(&memory, &project.memory_scope);
    let l2_decisions = collect_l2_for_project(&memory, &name);
    let recent_runs = collect_recent_runs_for_project(&name, 8);

    Json(ProjectMemoryView {
        name: name.clone(),
        r#type: project.r#type.clone(),
        stack: project.stack.clone(),
        role: project.role.clone(),
        memory_scope: project.memory_scope.clone(),
        path: project.path.clone(),
        contracts: ContractView {
            provides: project.contracts.provides.clone(),
            consumes: project.contracts.consumes.clone(),
        },
        l1_facts,
        l2_decisions,
        recent_runs,
    })
    .into_response()
}

fn collect_l1_for_scope(memory: &MemoryStore, scope: &[String]) -> Vec<L1FactView> {
    let mut out = Vec::new();
    for topic in scope {
        let dir = memory.l1_root().join(topic);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let file = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if file.starts_with('.') {
                continue;
            }
            let preview = preview_text(&p, 600);
            out.push(L1FactView {
                topic: topic.clone(),
                file,
                preview,
            });
        }
    }
    out.sort_by(|a, b| a.topic.cmp(&b.topic).then(a.file.cmp(&b.file)));
    out
}

fn collect_l2_for_project(memory: &MemoryStore, project: &str) -> Vec<L2DecisionView> {
    let dir = memory.l2_root().join(project);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file()
            || p.extension().is_none_or(|e| e != "md")
            || p.file_name()
                .map(|s| s.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
        {
            continue;
        }
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let file = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let preview = preview_text(&p, 600);
        out.push(L2DecisionView {
            file,
            bytes,
            preview,
        });
    }
    // Newest first — archiver names files with a `YYYY-MM-DD` prefix.
    out.sort_by(|a, b| b.file.cmp(&a.file));
    out
}

fn collect_recent_runs_for_project(project: &str, limit: usize) -> Vec<RecentRunView> {
    let Ok(runs_dir) = paths::runs_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != "current"
        })
        .map(|e| e.path())
        .collect();
    // Newest first by name (timestamp prefix).
    candidates.sort();
    candidates.reverse();

    let mut out = Vec::new();
    for dir in candidates {
        let Ok(state) = RunState::load(&dir) else {
            continue;
        };
        let task_ids: Vec<String> = state
            .tasks
            .values()
            .filter(|t| t.project == project)
            .map(|t| t.id.clone())
            .collect();
        if task_ids.is_empty() {
            continue;
        }
        out.push(RecentRunView {
            run_id: state.run_id,
            spec: state.spec,
            status: format!("{:?}", state.status).to_lowercase(),
            started_at: state.started_at.to_rfc3339(),
            verified: state.verified,
            task_ids_in_project: task_ids,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn preview_text(path: &std::path::Path, cap: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    if text.len() <= cap {
        return text;
    }
    let mut cut = cap;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    format!("{}…", &text[..cut])
}
