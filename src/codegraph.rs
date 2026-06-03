//! Read a codegraph index (`.codegraph/codegraph.db`) for the WebUI code graph.
//!
//! Behind the optional `codegraph` feature (bundled SQLite). When the feature
//! is off, every reader returns an empty graph so the WebUI tab degrades to an
//! empty state instead of breaking the default build/CI.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Default)]
pub struct CodeGraph {
    pub nodes: Vec<FileNode>,
    pub edges: Vec<FileEdge>,
    /// Absolute repo root, so the UI can build `vscode://file/<root>/<rel>` jumps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// False when there's no index / the feature is off — UI shows empty state.
    pub available: bool,
    /// Source of the graph: "native" | "codegraph" | "understand-anything".
    pub source: &'static str,
    /// Architectural layers (only from the Understand-Anything graph).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<GraphLayer>,
    /// Guided tour steps (only from the Understand-Anything graph).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tour: Vec<TourStep>,
}

#[derive(Serialize, Default)]
pub struct FileNode {
    pub path: String,
    pub language: Option<String>,
    pub symbols: u32,
    /// LLM summary (Understand-Anything only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    /// Layer id this file belongs to (Understand-Anything only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
}

#[derive(Serialize)]
pub struct GraphLayer {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub files: Vec<String>,
}

#[derive(Serialize)]
pub struct TourStep {
    pub order: u32,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_lesson: Option<String>,
}

#[derive(Serialize)]
pub struct FileEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

/// A directed dependency between two project modules. `from` is the imported
/// (producer) module, `to` is the importing (consumer) module — matching the
/// producer→consumer arrow convention. `weight` is the count of distinct
/// consumer files behind the edge; `kind` is the channel — `"import"` for a
/// real source-code import, `"contract"` for cross-stack RPC contract coupling
/// inferred from generated-client naming, or `"import+contract"` when both
/// signals find the same pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleEdge {
    pub from: String,
    pub to: String,
    pub weight: u32,
    #[serde(default)]
    pub kind: String,
}

/// Roll the file-level graph up to module-level dependency edges.
///
/// `modules` is `(name, relative_path)` for every project; a file belongs to
/// the module with the longest matching path prefix (so `app/auth_rpc` wins
/// over `app/auth` for `app/auth_rpc/handler.go`). Edges within one module and
/// edges touching files outside every module are dropped. The result is sorted
/// for determinism.
pub fn module_edges(graph: &CodeGraph, modules: &[(String, String)]) -> Vec<ModuleEdge> {
    use std::collections::BTreeMap;

    // Longest path first so the most specific module claims a file.
    let mut by_len: Vec<(&str, &str)> = modules
        .iter()
        .filter(|(_, p)| !p.is_empty() && p != ".")
        .map(|(n, p)| (n.as_str(), p.trim_end_matches('/')))
        .collect();
    by_len.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    let module_of = |file: &str| -> Option<&str> {
        by_len
            .iter()
            .find(|(_, path)| {
                file == *path
                    || file
                        .strip_prefix(path)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(name, _)| *name)
    };

    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for e in &graph.edges {
        // Only real import/dependency edges denote a module dependency. A
        // tree-sitter code graph also emits `calls`/`references`/`instantiates`
        // edges resolved by symbol NAME, which across a large monorepo falsely
        // link same-named functions/types in unrelated modules (e.g. two
        // `test_utils.go`). Those dominate by count and would fabricate module
        // dependencies — and a genuine cross-module call always has a matching
        // import anyway, so `imports` is both precise and sufficient.
        if !is_dependency_edge(&e.kind) {
            continue;
        }
        let (Some(src_mod), Some(tgt_mod)) = (module_of(&e.source), module_of(&e.target)) else {
            continue;
        };
        if src_mod == tgt_mod {
            continue;
        }
        // `source` imports `target` ⇒ target is the producer, source the consumer.
        *counts
            .entry((tgt_mod.to_string(), src_mod.to_string()))
            .or_default() += 1;
    }

    counts
        .into_iter()
        .map(|((from, to), weight)| ModuleEdge {
            from,
            to,
            weight,
            kind: "import".into(),
        })
        .collect()
}

/// Whether an edge kind denotes a real import/dependency (vs a symbol-level
/// `calls`/`references`/`instantiates`/`extends`/`implements` edge, which a
/// code-graph resolves by name and over-links across modules). Covers the
/// codegraph/native (`imports`), Understand-Anything (`import`), and any
/// explicit `depends_on` edges.
fn is_dependency_edge(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    k.contains("import") || k == "depends_on" || k == "requires"
}

/// Find a Go module: read the `module` line of a `go.mod` at the root or, for a
/// combined multi-repo workspace, in a shallow subdirectory (e.g. `backend/`).
/// Returns `(dir_relative_to_root, module_prefix)`.
fn find_go_module(root: &Path) -> Option<(String, String)> {
    fn prefix_of(go_mod: &Path) -> Option<String> {
        std::fs::read_to_string(go_mod).ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("module ").map(|m| m.trim().to_string()))
        })
    }
    if let Some(p) = prefix_of(&root.join("go.mod")) {
        return Some((String::new(), p));
    }
    // One level down (combined workspaces mount each repo as a subdir).
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with("bazel-") {
            continue;
        }
        if let Some(p) = prefix_of(&entry.path().join("go.mod")) {
            return Some((name.into_owned(), p));
        }
    }
    None
}

/// Longest-prefix owner module for a repo-relative path.
fn module_of_path<'a>(path: &str, by_len: &[(&'a str, &'a str)]) -> Option<&'a str> {
    by_len
        .iter()
        .find(|(_, p)| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
        .map(|(name, _)| *name)
}

/// Module → module dependency edges parsed directly from **source import
/// statements**, which is the reliable signal a tree-sitter code graph misses
/// for a Go monorepo (it doesn't resolve full-path cross-module imports). For
/// Go, every cross-module import embeds the repo's go.mod path, so we map the
/// imported path back to its owning module. `weight` = number of files in the
/// consumer that import the producer. Returns empty when there's no go.mod
/// (callers then fall back to the code-graph index for other ecosystems).
pub fn scan_import_edges(root: &Path, modules: &[(String, String)]) -> Vec<ModuleEdge> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_len: Vec<(&str, &str)> = modules
        .iter()
        .filter(|(_, p)| !p.is_empty() && p != ".")
        .map(|(n, p)| (n.as_str(), p.trim_end_matches('/')))
        .collect();
    by_len.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    // The go.mod may sit at the root (a backend-only workspace) or under a
    // subdir (a combined multi-repo workspace, e.g. `backend/go.mod`). Find it
    // and remember its dir so an import `"<prefix>/app/x"` resolves to the
    // workspace-relative path `<dir>/app/x`.
    let Some((go_dir, go_prefix)) = find_go_module(root) else {
        return Vec::new();
    };
    let needle = format!("\"{go_prefix}/");

    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            if path.is_dir() {
                if fname.starts_with('.')
                    || fname.starts_with("bazel-")
                    || matches!(
                        fname.as_ref(),
                        "vendor" | "node_modules" | "target" | "dist"
                    )
                {
                    continue;
                }
                stack.push(path);
            } else if fname.ends_with(".go") {
                let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) else {
                    continue;
                };
                let Some(owner) = module_of_path(rel, &by_len) else {
                    continue;
                };
                let owner = owner.to_string();
                let Ok(txt) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let mut imported: BTreeSet<&str> = BTreeSet::new();
                let mut idx = 0;
                while let Some(pos) = txt[idx..].find(&needle) {
                    let start = idx + pos + needle.len();
                    let Some(end) = txt[start..].find('"') else {
                        break;
                    };
                    // import path after the go prefix, mapped into the workspace
                    let sub = &txt[start..start + end];
                    let target = if go_dir.is_empty() {
                        sub.to_string()
                    } else {
                        format!("{go_dir}/{sub}")
                    };
                    if let Some(m) = module_of_path(&target, &by_len) {
                        if m != owner {
                            imported.insert(m);
                        }
                    }
                    idx = start + end + 1;
                }
                for tgt in imported {
                    *counts.entry((tgt.to_string(), owner.clone())).or_default() += 1;
                }
            }
        }
    }
    counts
        .into_iter()
        .map(|((from, to), weight)| ModuleEdge {
            from,
            to,
            weight,
            kind: "import".into(),
        })
        .collect()
}

/// Cross-stack contract coupling, detected by file-path naming convention:
/// when one project's tree carries a file whose path contains another project's
/// distinctive multi-word name token (e.g. a frontend `webapp_alpha` has
/// `src/bam-idl/.../service_alpha.ts` → consumes backend `service-alpha`).
/// This is what shows front↔back coupling in a combined multi-repo workspace,
/// where TS and Go don't import each other — coupling is via generated RPC
/// clients. `weight` is the number of consumer files referencing the producer.
pub fn scan_contract_edges(root: &Path, modules: &[(String, String)]) -> Vec<ModuleEdge> {
    use std::collections::{BTreeMap, BTreeSet};

    // Distinctive tokens: snake_case basename, ≥2 words (one '_'), ≥7 chars —
    // long multi-word tokens almost never appear incidentally in another
    // project's tree unless via a deliberate generated-client naming.
    let tokens: Vec<(String, String)> = modules
        .iter()
        .filter_map(|(name, path)| {
            let base = path.rsplit('/').next()?.to_ascii_lowercase();
            if base.len() >= 7 && base.contains('_') {
                Some((name.clone(), base))
            } else {
                None
            }
        })
        .collect();
    if tokens.len() < 2 {
        return Vec::new();
    }

    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for (proj_name, proj_path) in modules {
        let proj_root = root.join(proj_path);
        if !proj_root.is_dir() {
            continue;
        }
        let mut hits: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        let mut stack = vec![proj_root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                let fname = entry.file_name();
                let fname = fname.to_string_lossy();
                if p.is_dir() {
                    if fname.starts_with('.')
                        || fname.starts_with("bazel-")
                        || matches!(
                            fname.as_ref(),
                            "vendor" | "node_modules" | "target" | "dist"
                        )
                    {
                        continue;
                    }
                    stack.push(p);
                } else {
                    let Some(rel) = p.strip_prefix(root).ok().and_then(|q| q.to_str()) else {
                        continue;
                    };
                    let rel_lc = rel.to_ascii_lowercase();
                    // Require the path to sit under a specific generated-client
                    // dir convention — otherwise a casual mention of another
                    // module's name in a generic `client/` or test fixture
                    // would produce a misleading edge.
                    if !is_generated_client_path(&rel_lc) {
                        continue;
                    }
                    for (other_name, token) in &tokens {
                        if other_name == proj_name {
                            continue;
                        }
                        if path_contains_token(&rel_lc, token) {
                            hits.entry(other_name).or_default().insert(rel.to_string());
                        }
                    }
                }
            }
        }
        for (producer, files) in hits {
            *counts
                .entry((producer.to_string(), proj_name.clone()))
                .or_default() += files.len() as u32;
        }
    }
    counts
        .into_iter()
        .map(|((from, to), weight)| ModuleEdge {
            from,
            to,
            weight,
            kind: "contract".into(),
        })
        .collect()
}

/// Does the path sit under (or pass through) a directory that's a well-known
/// generated-client convention? Restricts contract-edge detection to STRONG
/// signals where a service name in the path almost certainly means a
/// contract-derived client.
///
/// Implementation note: a previous version did a plain substring search for
/// `/marker/`. That missed a marker dir sitting at the path root (e.g. the
/// literal string `bam-idl/<service>/foo.ts` — no leading `/`). The component-
/// aware split below catches that case while still rejecting half-name
/// substrings like `not-bam-idl/...`. `bam/typings` is treated as the joined
/// two-segment marker.
pub(crate) fn is_generated_client_path(path_lc: &str) -> bool {
    // Two-segment markers must match two consecutive path components.
    const TWO_SEG_MARKERS: &[(&str, &str)] = &[("bam", "typings")];
    // Single-segment markers — match any component equal to one of these.
    const ONE_SEG_MARKERS: &[&str] = &[
        "bam-idl",
        "openapi-client",
        "swagger-client",
        "auto-gen",
        "autogen",
        "api-gen",
        "api-types",
    ];
    let comps: Vec<&str> = path_lc
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    for (i, c) in comps.iter().enumerate() {
        if ONE_SEG_MARKERS.contains(c) {
            return true;
        }
        if let Some(next) = comps.get(i + 1) {
            if TWO_SEG_MARKERS.iter().any(|(a, b)| a == c && b == next) {
                return true;
            }
        }
    }
    false
}

/// Substring match with word boundaries: `token` must be surrounded by non-
/// alphanumeric chars (or path start/end). Rules out incidental hits where the
/// token is part of a longer identifier.
pub(crate) fn path_contains_token(path: &str, token: &str) -> bool {
    let bytes = path.as_bytes();
    let tlen = token.len();
    let mut i = 0;
    while let Some(pos) = path[i..].find(token) {
        let s = i + pos;
        let e = s + tlen;
        let lb = s == 0 || !bytes[s - 1].is_ascii_alphanumeric();
        let rb = e == path.len() || !bytes[e].is_ascii_alphanumeric();
        if lb && rb {
            return true;
        }
        i = s + 1;
    }
    false
}

/// The best available module dependency graph for `root`: combine real source
/// import parsing (`scan_import_edges` — Go full-path imports) with cross-stack
/// contract coupling (`scan_contract_edges` — generated-client naming),
/// summing weights when both signals find the same edge. Falls back to the
/// code-graph index when neither yields anything.
pub fn module_dependency_edges(root: &Path, modules: &[(String, String)]) -> Vec<ModuleEdge> {
    use std::collections::BTreeMap;
    // Track which signal(s) contributed to each pair so the merged edge carries
    // the right channel label: `import` (code) / `contract` (RPC) / both.
    let mut combined: BTreeMap<(String, String), (u32, bool, bool)> = BTreeMap::new();
    for e in scan_import_edges(root, modules) {
        let entry = combined.entry((e.from, e.to)).or_default();
        entry.0 += e.weight;
        entry.1 = true;
    }
    for e in scan_contract_edges(root, modules) {
        let entry = combined.entry((e.from, e.to)).or_default();
        entry.0 += e.weight;
        entry.2 = true;
    }
    if !combined.is_empty() {
        return combined
            .into_iter()
            .map(|((from, to), (weight, has_import, has_contract))| {
                let kind = match (has_import, has_contract) {
                    (true, true) => "import+contract",
                    (false, true) => "contract",
                    _ => "import",
                };
                ModuleEdge {
                    from,
                    to,
                    weight,
                    kind: kind.into(),
                }
            })
            .collect();
    }
    if let Some(db) = find_db(root) {
        if let Ok(g) = file_graph(&db) {
            if g.available {
                return module_edges(&g, modules);
            }
        }
    }
    Vec::new()
}

/// Process-lifetime memo over [`module_dependency_edges`]. That scan walks and
/// parses every source file under `root` (seconds on a large monorepo), and the
/// read-only architecture view re-hits it on every navigation and live refresh —
/// so without a cache the dashboard looks frozen on each visit. The short TTL
/// keeps repeat calls instant while still surfacing edits within a few seconds.
/// The pure function is left untouched so tests and planner callers are
/// unaffected; only the WebUI handler uses this wrapper.
pub fn module_dependency_edges_cached(
    root: &Path,
    modules: &[(String, String)],
) -> Vec<ModuleEdge> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    type Key = (PathBuf, Vec<(String, String)>);
    type Cache = Mutex<HashMap<Key, (Instant, Vec<ModuleEdge>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    const TTL: Duration = Duration::from_secs(8);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key: Key = (root.to_path_buf(), modules.to_vec());
    if let Ok(map) = cache.lock() {
        if let Some((at, edges)) = map.get(&key) {
            if at.elapsed() < TTL {
                return edges.clone();
            }
        }
    }
    let edges = module_dependency_edges(root, modules);
    if let Ok(mut map) = cache.lock() {
        map.insert(key, (Instant::now(), edges.clone()));
    }
    edges
}

/// Module dependencies derived from the code graph: `consumer → set of producer
/// modules it imports`, **acyclic by construction**. Edges are taken
/// strongest-first and any that would close a cycle is skipped, so the result
/// is always a DAG (callers feed it into plan `depends_on` / brief, where a
/// cycle would be invalid). `min_weight` drops one-off imports that are more
/// likely resolver noise than a real dependency. Shared by the planner's task
/// ordering and the architecture brief so they agree.
pub fn derived_module_deps(
    graph: &CodeGraph,
    modules: &[(String, String)],
    min_weight: u32,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    acyclic_consumer_deps(module_edges(graph, modules), min_weight)
}

/// Derive `consumer → producers` module deps for `root` from the best available
/// signal (real source imports, else the code-graph index), acyclic +
/// weight-thresholded. Empty when nothing resolves, so callers degrade to
/// declared/contract relationships.
pub fn load_derived_module_deps(
    root: &Path,
    modules: &[(String, String)],
    min_weight: u32,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    acyclic_consumer_deps(module_dependency_edges(root, modules), min_weight)
}

fn acyclic_consumer_deps(
    mut edges: Vec<ModuleEdge>,
    min_weight: u32,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    use std::collections::{BTreeMap, BTreeSet};
    // Strongest first; ties broken deterministically.
    edges.sort_by(|a, b| {
        b.weight
            .cmp(&a.weight)
            .then(a.from.cmp(&b.from))
            .then(a.to.cmp(&b.to))
    });
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in edges {
        if e.weight < min_weight || e.from == e.to {
            continue;
        }
        // Add "consumer (e.to) depends on producer (e.from)" unless the producer
        // already (transitively) depends on the consumer — which would cycle.
        if reaches(&deps, &e.from, &e.to) {
            continue;
        }
        deps.entry(e.to.clone()).or_default().insert(e.from.clone());
    }
    deps
}

/// Does `start` (transitively) depend on `target` in a `consumer → producers`
/// map? Plain DFS with a visited set.
fn reaches(
    deps: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    start: &str,
    target: &str,
) -> bool {
    let mut stack = vec![start.to_string()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(producers) = deps.get(&node) {
            stack.extend(producers.iter().cloned());
        }
    }
    false
}

#[derive(Serialize)]
pub struct FileSymbol {
    pub name: String,
    pub kind: String,
    pub line: Option<i64>,
    pub signature: Option<String>,
}

/// Walk up from `start` (max 6 levels) for `.codegraph/codegraph.db`.
pub fn find_db(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    for _ in 0..6 {
        let c = cur?;
        let p = c.join(".codegraph").join("codegraph.db");
        if p.is_file() {
            return Some(p);
        }
        cur = c.parent();
    }
    None
}

#[cfg(feature = "codegraph")]
pub fn file_graph(db: &Path) -> anyhow::Result<CodeGraph> {
    use std::collections::{HashMap, HashSet};
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // node id (TEXT) -> file path; per-file symbol count + language.
    let mut id_file: HashMap<String, String> = HashMap::new();
    let mut symbols: HashMap<String, u32> = HashMap::new();
    let mut language: HashMap<String, Option<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, file_path, language, kind FROM nodes")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, file, lang, kind) = row?;
            if let Some(file) = file {
                id_file.insert(id, file.clone());
                if kind != "file" {
                    *symbols.entry(file.clone()).or_insert(0) += 1;
                }
                language.entry(file).or_insert(lang);
            }
        }
    }

    // Aggregate to cross-file edges (drop `contains` = file→symbol, and self).
    let mut edge_set: HashSet<(String, String, String)> = HashSet::new();
    {
        let mut stmt =
            conn.prepare("SELECT source, target, kind FROM edges WHERE kind != 'contains'")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (s, t, kind) = row?;
            if let (Some(fs), Some(ft)) = (id_file.get(&s), id_file.get(&t)) {
                if fs != ft {
                    edge_set.insert((fs.clone(), ft.clone(), kind));
                }
            }
        }
    }

    let nodes = language
        .into_iter()
        .map(|(path, language)| FileNode {
            symbols: symbols.get(&path).copied().unwrap_or(0),
            path,
            language,
            ..Default::default()
        })
        .collect();
    let edges = edge_set
        .into_iter()
        .map(|(source, target, kind)| FileEdge {
            source,
            target,
            kind,
        })
        .collect();
    let root = db
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string());
    Ok(CodeGraph {
        nodes,
        edges,
        root,
        available: true,
        source: "codegraph",
        ..Default::default()
    })
}

#[cfg(not(feature = "codegraph"))]
pub fn file_graph(_db: &Path) -> anyhow::Result<CodeGraph> {
    Ok(CodeGraph::default())
}

#[cfg(feature = "codegraph")]
pub fn file_symbols(db: &Path, file_path: &str) -> anyhow::Result<Vec<FileSymbol>> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT name, kind, start_line, signature FROM nodes \
         WHERE file_path = ?1 AND kind != 'file' ORDER BY start_line",
    )?;
    let rows = stmt.query_map([file_path], |r| {
        Ok(FileSymbol {
            name: r.get(0)?,
            kind: r.get(1)?,
            line: r.get(2)?,
            signature: r.get(3)?,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[cfg(not(feature = "codegraph"))]
pub fn file_symbols(_db: &Path, _file_path: &str) -> anyhow::Result<Vec<FileSymbol>> {
    Ok(vec![])
}

// ─── native indexer ────────────────────────────────────────────────────
//
// A dependency-light, built-in code graph so the Code Graph tab works out of
// the box — no external `codegraph` tool, no `.codegraph` DB, no feature flag.
// We walk the repo (gitignore-aware), parse each file's imports with simple
// per-language scanning, and resolve them to other repo files to form edges.
// It's heuristic, not a full AST: enough to show "what depends on what" for
// the languages maestro itself is written in (Rust, TS/JS, Python).

use std::collections::HashSet;

const NATIVE_FILE_CAP: usize = 4000;

/// Walk up from `start` (max 8 levels) for a `.git` dir; fall back to `start`.
pub fn find_repo_root(start: &Path) -> PathBuf {
    let mut cur = Some(start);
    for _ in 0..8 {
        let Some(c) = cur else { break };
        if c.join(".git").exists() {
            return c.to_path_buf();
        }
        cur = c.parent();
    }
    start.to_path_buf()
}

fn language_of(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "go" => "go",
        _ => return None,
    })
}

/// Collect repo-relative (forward-slash) source paths, gitignore-aware.
/// Vendored / generated / dependency dirs that aren't the project's own source.
/// gitignore usually covers node_modules/target/dist, but vendor/.venv/etc. are
/// often committed — excluding them keeps the file budget (and the graph) on
/// real source for large or polyglot repos.
const VENDOR_DIRS: &[&str] = &[
    "vendor",
    "third_party",
    "third-party",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    "Pods",
    ".dart_tool",
    "generated",
    "node_modules",
    "dist",
    "build",
    "target",
    ".next",
    ".turbo",
];

fn collect_source_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .parents(false)
        // Follow symlinks so a combined multi-repo workspace (each repo
        // mounted as a symlinked subdir) still produces a non-empty native
        // graph. The `ignore` crate detects symlink cycles internally.
        .follow_links(true)
        .build();
    for entry in walker.flatten() {
        if out.len() >= NATIVE_FILE_CAP {
            break;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        // Skip vendored/generated dirs so they don't crowd real source out of
        // the file budget on big repos (gitignore doesn't always cover them).
        if rel.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|d| VENDOR_DIRS.contains(&d))
        }) {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if language_of(ext).is_none() {
            continue;
        }
        out.push(rel.to_string_lossy().replace('\\', "/"));
    }
    out
}

fn dir_of(rel: &str) -> &str {
    rel.rfind('/').map(|i| &rel[..i]).unwrap_or("")
}

/// Join `dir` + a relative spec, resolving `.`/`..`, forward-slashed.
fn norm_join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        vec![]
    } else {
        dir.split('/').collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Symbols defined in one file (name + kind + 1-based line). Heuristic.
pub fn native_symbols(lang: &str, content: &str) -> Vec<FileSymbol> {
    let mut out = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim_start();
        let kw_name: Option<(&str, &str)> = match lang {
            "rust" => rust_decl(line),
            "typescript" | "tsx" | "javascript" | "jsx" => ts_decl(line),
            "python" => py_decl(line),
            "go" => go_decl(line),
            _ => None,
        };
        if let Some((kind, name)) = kw_name {
            if !name.is_empty() {
                out.push(FileSymbol {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    line: Some(i as i64 + 1),
                    signature: Some(raw.trim().chars().take(120).collect()),
                });
            }
        }
    }
    out
}

fn first_ident(s: &str) -> &str {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

fn rust_decl(line: &str) -> Option<(&'static str, &str)> {
    let l = line.strip_prefix("pub ").unwrap_or(line);
    let l = l.strip_prefix("async ").unwrap_or(l);
    for (kw, kind) in [
        ("fn ", "fn"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("type ", "type"),
        ("const ", "const"),
        ("static ", "static"),
        ("macro_rules! ", "macro"),
    ] {
        if let Some(rest) = l.strip_prefix(kw) {
            return Some((kind, first_ident(rest)));
        }
    }
    None
}

fn ts_decl(line: &str) -> Option<(&'static str, &str)> {
    let l = line.strip_prefix("export ").unwrap_or(line);
    let l = l.strip_prefix("default ").unwrap_or(l);
    for (kw, kind) in [
        ("function ", "function"),
        ("class ", "class"),
        ("interface ", "interface"),
        ("type ", "type"),
        ("enum ", "enum"),
        ("const ", "const"),
    ] {
        if let Some(rest) = l.strip_prefix(kw) {
            let name = first_ident(rest.trim_start_matches("function ").trim_start());
            return Some((kind, name));
        }
    }
    None
}

fn py_decl(line: &str) -> Option<(&'static str, &str)> {
    let l = line.strip_prefix("async ").unwrap_or(line);
    for (kw, kind) in [("def ", "def"), ("class ", "class")] {
        if let Some(rest) = l.strip_prefix(kw) {
            return Some((kind, first_ident(rest)));
        }
    }
    None
}

fn go_decl(line: &str) -> Option<(&'static str, &str)> {
    for (kw, kind) in [("func ", "func"), ("type ", "type")] {
        if let Some(rest) = line.strip_prefix(kw) {
            return Some((kind, first_ident(rest)));
        }
    }
    None
}

/// Resolve a file's imports to other repo files → `(target, kind)` edges.
fn resolve_edges(
    lang: &str,
    from_rel: &str,
    content: &str,
    files: &HashSet<String>,
    crate_root: &str,
) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    let mut push = |target: Option<String>, kind: &str| {
        if let Some(t) = target {
            if t != from_rel {
                edges.push((t, kind.to_string()));
            }
        }
    };
    match lang {
        "typescript" | "tsx" | "javascript" | "jsx" => {
            for spec in ts_import_specs(content) {
                push(resolve_ts(from_rel, &spec, files), "imports");
            }
        }
        "rust" => {
            for line in content.lines() {
                let l = line.trim_start();
                if let Some(name) = rust_mod_decl(l) {
                    push(resolve_rust_mod(from_rel, name, files), "module");
                } else if let Some(segs) = rust_use_crate(l) {
                    push(resolve_rust_crate(crate_root, &segs, files), "imports");
                }
            }
        }
        "python" => {
            for spec in py_rel_imports(content) {
                push(resolve_py(from_rel, &spec, files), "imports");
            }
        }
        _ => {}
    }
    edges
}

fn ts_import_specs(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if !(l.starts_with("import")
            || l.starts_with("export")
            || l.contains("require(")
            || l.contains("from "))
        {
            continue;
        }
        // grab the first quoted string on the line
        if let Some(spec) = first_quoted(l) {
            out.push(spec);
        }
    }
    out
}

fn first_quoted(s: &str) -> Option<String> {
    for q in ['"', '\''] {
        if let Some(a) = s.find(q) {
            if let Some(b) = s[a + 1..].find(q) {
                return Some(s[a + 1..a + 1 + b].to_string());
            }
        }
    }
    None
}

fn resolve_ts(from_rel: &str, spec: &str, files: &HashSet<String>) -> Option<String> {
    if !spec.starts_with('.') {
        return None; // bare specifier → external package
    }
    let base = norm_join(dir_of(from_rel), spec);
    let cands = [
        base.clone(),
        format!("{base}.ts"),
        format!("{base}.tsx"),
        format!("{base}.d.ts"),
        format!("{base}.js"),
        format!("{base}.jsx"),
        format!("{base}.mjs"),
        format!("{base}/index.ts"),
        format!("{base}/index.tsx"),
        format!("{base}/index.js"),
        format!("{base}/index.jsx"),
    ];
    cands.into_iter().find(|c| files.contains(c))
}

fn rust_mod_decl(line: &str) -> Option<&str> {
    let l = line.strip_prefix("pub ").unwrap_or(line);
    let rest = l.strip_prefix("mod ")?;
    let name = first_ident(rest);
    // only `mod name;` declarations (file modules), not inline `mod name {`
    if !name.is_empty() && rest[name.len()..].trim_start().starts_with(';') {
        Some(name)
    } else {
        None
    }
}

fn rust_use_crate(line: &str) -> Option<Vec<String>> {
    let l = line.strip_prefix("pub ").unwrap_or(line);
    let rest = l.strip_prefix("use crate::")?;
    // stop at group `{`, alias ` as `, glob, or terminator
    let end = rest
        .find(['{', ';'])
        .or_else(|| rest.find(" as "))
        .unwrap_or(rest.len());
    let segs: Vec<String> = rest[..end]
        .split("::")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "*")
        .map(str::to_string)
        .collect();
    (!segs.is_empty()).then_some(segs)
}

fn resolve_rust_mod(from_rel: &str, name: &str, files: &HashSet<String>) -> Option<String> {
    let dir = dir_of(from_rel);
    let base_name = from_rel.rsplit('/').next().unwrap_or("");
    // For mod.rs/lib.rs/main.rs, submodules live alongside; for foo.rs they
    // live under foo/.
    let module_dir = if matches!(base_name, "mod.rs" | "lib.rs" | "main.rs") {
        dir.to_string()
    } else {
        let stem = base_name.strip_suffix(".rs").unwrap_or(base_name);
        if dir.is_empty() {
            stem.to_string()
        } else {
            format!("{dir}/{stem}")
        }
    };
    let prefix = if module_dir.is_empty() {
        String::new()
    } else {
        format!("{module_dir}/")
    };
    [
        format!("{prefix}{name}.rs"),
        format!("{prefix}{name}/mod.rs"),
    ]
    .into_iter()
    .find(|c| files.contains(c))
}

fn resolve_rust_crate(
    crate_root: &str,
    segs: &[String],
    files: &HashSet<String>,
) -> Option<String> {
    // Try the longest path prefix first: crate::a::b::c → a/b/c.rs, then a/b.rs…
    for take in (1..=segs.len()).rev() {
        let joined = segs[..take].join("/");
        let base = if crate_root.is_empty() {
            joined
        } else {
            format!("{crate_root}/{joined}")
        };
        for cand in [format!("{base}.rs"), format!("{base}/mod.rs")] {
            if files.contains(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn py_rel_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("from .") {
            // ".foo.bar import x" → spec "./foo/bar" (one leading dot already consumed)
            let module = rest.split_whitespace().next().unwrap_or("");
            let dots = module.chars().take_while(|c| *c == '.').count();
            let tail = module.trim_start_matches('.').replace('.', "/");
            let ups = "../".repeat(dots);
            out.push(format!("./{ups}{tail}"));
        }
    }
    out
}

fn resolve_py(from_rel: &str, spec: &str, files: &HashSet<String>) -> Option<String> {
    let base = norm_join(dir_of(from_rel), spec);
    [format!("{base}.py"), format!("{base}/__init__.py")]
        .into_iter()
        .find(|c| files.contains(c))
}

/// The directory holding the crate root (`lib.rs`/`main.rs`), so `use crate::`
/// paths resolve against it. Picks the shallowest such file.
fn rust_crate_root(files: &[String]) -> String {
    files
        .iter()
        .filter(|f| matches!(f.rsplit('/').next().unwrap_or(""), "lib.rs" | "main.rs"))
        .min_by_key(|f| f.matches('/').count())
        .map(|f| dir_of(f).to_string())
        .unwrap_or_default()
}

/// Build a file-level code graph natively (no external tooling).
pub fn native_graph(root: &Path) -> CodeGraph {
    let rels = collect_source_files(root);
    if rels.is_empty() {
        return CodeGraph::default();
    }
    let set: HashSet<String> = rels.iter().cloned().collect();
    let crate_root = rust_crate_root(&rels);

    let mut nodes = Vec::with_capacity(rels.len());
    let mut edge_set: HashSet<(String, String, String)> = HashSet::new();

    for rel in &rels {
        let ext = rel.rsplit('.').next().unwrap_or("");
        let Some(lang) = language_of(ext) else {
            continue;
        };
        let content = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
        let symbols = native_symbols(lang, &content).len() as u32;
        nodes.push(FileNode {
            path: rel.clone(),
            language: Some(lang.to_string()),
            symbols,
            ..Default::default()
        });
        for (target, kind) in resolve_edges(lang, rel, &content, &set, &crate_root) {
            edge_set.insert((rel.clone(), target, kind));
        }
    }

    let edges = edge_set
        .into_iter()
        .map(|(source, target, kind)| FileEdge {
            source,
            target,
            kind,
        })
        .collect();
    CodeGraph {
        nodes,
        edges,
        root: Some(root.display().to_string()),
        available: true,
        source: "native",
        ..Default::default()
    }
}

/// Native per-file symbols for the side panel.
pub fn native_file_symbols(root: &Path, file_path: &str) -> Vec<FileSymbol> {
    let ext = file_path.rsplit('.').next().unwrap_or("");
    let Some(lang) = language_of(ext) else {
        return vec![];
    };
    let content = std::fs::read_to_string(root.join(file_path)).unwrap_or_default();
    native_symbols(lang, &content)
}

/// Heuristic "relevant code" for `query`, scoped to `root`'s subtree, using
/// only the built-in indexer (no external `codegraph` tool). Ranks files by how
/// many query keywords appear in the path or the file's symbol names; returns
/// the top `max_files` as `(relpath, symbols)`. Bounded: parses at most a small
/// prefilter of candidate files. Empty when nothing matches — so the caller
/// injects a grounding section only when it's actually relevant.
pub fn native_context(
    root: &Path,
    query: &str,
    max_files: usize,
) -> Vec<(String, Vec<FileSymbol>)> {
    let kws: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if kws.is_empty() {
        return vec![];
    }
    // Path-keyword score first (cheap), so the candidates we actually parse are
    // the most likely matches; cap parsing at 48 files regardless of repo size.
    let mut by_path: Vec<(usize, String)> = collect_source_files(root)
        .into_iter()
        .map(|f| {
            let lf = f.to_lowercase();
            (kws.iter().filter(|k| lf.contains(k.as_str())).count(), f)
        })
        .collect();
    by_path.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut scored: Vec<(usize, String, Vec<FileSymbol>)> = Vec::new();
    for (path_score, f) in by_path.into_iter().take(48) {
        let syms = native_file_symbols(root, &f);
        let sym_score = syms
            .iter()
            .filter(|s| {
                let n = s.name.to_lowercase();
                kws.iter().any(|k| n.contains(k.as_str()))
            })
            .count();
        let total = path_score * 2 + sym_score;
        if total > 0 {
            scored.push((total, f, syms));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(max_files)
        .map(|(_, f, s)| (f, s))
        .collect()
}

// ─── Understand-Anything graph (rich, LLM-enriched) ────────────────────
//
// If the Understand-Anything plugin has been run in the repo it leaves a
// `.understand-anything/knowledge-graph.json` with per-file LLM summaries,
// tags, architectural layers and a guided tour. We read it directly (no JS
// runtime needed) and present a file-level view enriched with that semantics.

#[derive(Deserialize)]
struct UaGraph {
    #[serde(default)]
    project: Option<UaProject>,
    #[serde(default)]
    nodes: Vec<UaNode>,
    #[serde(default)]
    edges: Vec<UaEdge>,
    #[serde(default)]
    layers: Vec<UaLayer>,
    #[serde(default)]
    tour: Vec<UaTour>,
}
#[derive(Deserialize)]
struct UaProject {
    #[serde(default)]
    languages: Vec<String>,
}
#[derive(Deserialize)]
struct UaNode {
    id: String,
    #[serde(rename = "type", default)]
    node_type: String,
    #[serde(rename = "filePath", default)]
    file_path: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    complexity: Option<String>,
    #[serde(rename = "lineRange", default)]
    line_range: Option<Vec<i64>>,
}
#[derive(Deserialize)]
struct UaEdge {
    source: String,
    target: String,
    #[serde(rename = "type", default)]
    edge_type: String,
}
#[derive(Deserialize)]
struct UaLayer {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "nodeIds", default)]
    node_ids: Vec<String>,
}
#[derive(Deserialize)]
struct UaTour {
    #[serde(default)]
    order: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "nodeIds", default)]
    node_ids: Vec<String>,
    #[serde(rename = "languageLesson", default)]
    language_lesson: Option<String>,
}

/// Walk up from `start` (max 8 levels) for the Understand-Anything graph.
pub fn find_understand_graph(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    for _ in 0..8 {
        let c = cur?;
        let p = c.join(".understand-anything").join("knowledge-graph.json");
        if p.is_file() {
            return Some(p);
        }
        cur = c.parent();
    }
    None
}

/// Is the external `codegraph` CLI installed (PATH or `~/.local/bin`)?
pub fn codegraph_installed() -> bool {
    if let Some(home) = dirs::home_dir() {
        if home.join(".local/bin/codegraph").exists() {
            return true;
        }
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join("codegraph").exists()))
        .unwrap_or(false)
}

/// Resolve the codegraph CLI: `~/.local/bin/codegraph` if present, else
/// `codegraph` on PATH. Single source of truth for every caller (CLI, executor
/// code-context, server build) — previously duplicated verbatim in three places.
pub fn codegraph_bin() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/bin/codegraph");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("codegraph")
}

/// Status of one code-graph engine for the chooser (CLI / doctor / WebUI).
#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub engine: String,
    pub label: String,
    /// Tool/plugin present so the graph can be (re)built.
    pub installed: bool,
    /// Built artifact present for this repo root.
    pub built: bool,
    /// Currently the source `/api/codegraph/graph` would serve.
    pub active: bool,
    /// "free" or "LLM tokens".
    pub cost: String,
    /// What to do next (install / build / run command).
    pub hint: String,
}

/// Report the three engines for `root`, in the same preference order the graph
/// endpoint uses (richest first). `active` marks the one currently served.
pub fn engine_status(root: &Path) -> Vec<EngineStatus> {
    let understand_built = find_understand_graph(root).is_some();
    let codegraph_built = find_db(root).is_some();
    let cg_installed = codegraph_installed();
    vec![
        EngineStatus {
            engine: "understand-anything".into(),
            label: "Understand-Anything (LLM summaries, layers, tour)".into(),
            installed: understand_built,
            built: understand_built,
            active: understand_built,
            cost: "LLM tokens".into(),
            hint: if understand_built {
                "built — richest graph".into()
            } else {
                "run `/understand` in Claude Code to build (LLM, uses tokens)".into()
            },
        },
        EngineStatus {
            engine: "codegraph".into(),
            label: "CodeGraph (tree-sitter symbols + edges)".into(),
            installed: cg_installed,
            built: codegraph_built,
            active: codegraph_built && !understand_built,
            cost: "free".into(),
            hint: if !cg_installed {
                "install the codegraph CLI, then `maestro codegraph build --engine codegraph`"
                    .into()
            } else if !codegraph_built {
                "`maestro codegraph build --engine codegraph`".into()
            } else {
                "built".into()
            },
        },
        EngineStatus {
            engine: "native".into(),
            label: "Native (built-in file/import graph)".into(),
            installed: true,
            built: true,
            active: !understand_built && !codegraph_built,
            cost: "free".into(),
            hint: "always available, zero deps, zero tokens".into(),
        },
    ]
}

fn parse_ua(path: &Path) -> Option<UaGraph> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// File-level CodeGraph derived from an Understand-Anything knowledge graph,
/// enriched with summaries, tags, complexity and architectural layers.
pub fn understand_graph(path: &Path) -> Option<CodeGraph> {
    let ua = parse_ua(path)?;
    if ua.nodes.is_empty() {
        return None;
    }
    let default_lang = ua
        .project
        .as_ref()
        .and_then(|p| p.languages.first().cloned());

    // node id → its file path (file nodes map to themselves; symbol nodes to
    // their owning file) so edges can be aggregated to the file level.
    let mut id_to_file: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for n in &ua.nodes {
        if let Some(fp) = n.file_path.as_deref() {
            id_to_file.insert(n.id.as_str(), fp);
        }
    }

    // layer id per file (first wins) + the layer list itself.
    let mut layer_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let layers: Vec<GraphLayer> = ua
        .layers
        .iter()
        .map(|l| {
            let files: Vec<String> = l
                .node_ids
                .iter()
                .filter_map(|id| id_to_file.get(id.as_str()).map(|f| f.to_string()))
                .collect();
            for f in &files {
                layer_of.entry(f.clone()).or_insert_with(|| l.id.clone());
            }
            GraphLayer {
                id: l.id.clone(),
                name: l.name.clone(),
                description: l.description.clone(),
                files,
            }
        })
        .collect();

    // per-file symbol counts (function/class nodes).
    let mut symbol_count: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for n in &ua.nodes {
        if n.node_type != "file" {
            if let Some(fp) = n.file_path.as_deref() {
                *symbol_count.entry(fp).or_insert(0) += 1;
            }
        }
    }

    let nodes: Vec<FileNode> = ua
        .nodes
        .iter()
        .filter(|n| n.node_type == "file")
        .filter_map(|n| {
            let path = n.file_path.clone()?;
            let language = path
                .rsplit('.')
                .next()
                .and_then(language_of)
                .map(str::to_string)
                .or_else(|| default_lang.clone());
            Some(FileNode {
                symbols: symbol_count.get(path.as_str()).copied().unwrap_or(0),
                layer: layer_of.get(&path).cloned(),
                summary: n.summary.clone(),
                tags: n.tags.clone(),
                complexity: n.complexity.clone(),
                path,
                language,
            })
        })
        .collect();

    // Aggregate edges to file level, dropping intra-file `contains` and self.
    let mut edge_set: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
    for e in &ua.edges {
        if e.edge_type == "contains" {
            continue;
        }
        if let (Some(s), Some(t)) = (
            id_to_file.get(e.source.as_str()),
            id_to_file.get(e.target.as_str()),
        ) {
            if s != t {
                edge_set.insert((s.to_string(), t.to_string(), e.edge_type.clone()));
            }
        }
    }
    let edges = edge_set
        .into_iter()
        .map(|(source, target, kind)| FileEdge {
            source,
            target,
            kind,
        })
        .collect();

    let tour: Vec<TourStep> = ua
        .tour
        .iter()
        .map(|s| TourStep {
            order: s.order,
            title: s.title.clone(),
            description: s.description.clone(),
            files: s
                .node_ids
                .iter()
                .filter_map(|id| id_to_file.get(id.as_str()).map(|f| f.to_string()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            language_lesson: s.language_lesson.clone(),
        })
        .collect();

    // repo root = parent of the `.understand-anything` dir.
    let root = path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string());

    Some(CodeGraph {
        nodes,
        edges,
        root,
        available: true,
        source: "understand-anything",
        layers,
        tour,
    })
}

/// Symbols for one file pulled from the Understand-Anything graph (function /
/// class nodes), with their LLM summary as the signature line.
pub fn understand_file_symbols(path: &Path, file_path: &str) -> Vec<FileSymbol> {
    let Some(ua) = parse_ua(path) else {
        return vec![];
    };
    let mut out: Vec<FileSymbol> = ua
        .nodes
        .iter()
        .filter(|n| n.node_type != "file" && n.file_path.as_deref() == Some(file_path))
        .map(|n| FileSymbol {
            name: n.name.clone(),
            kind: n.node_type.clone(),
            line: n.line_range.as_ref().and_then(|r| r.first().copied()),
            signature: n.summary.clone(),
        })
        .collect();
    out.sort_by_key(|s| s.line.unwrap_or(i64::MAX));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(source: &str, target: &str) -> FileEdge {
        FileEdge {
            source: source.into(),
            target: target.into(),
            kind: "import".into(),
        }
    }

    #[test]
    fn module_edges_roll_up_cross_module_imports_with_longest_prefix() {
        // `app/auth_rpc` must win over `app/auth` for `app/auth_rpc/...`.
        let modules = vec![
            ("gateway".to_string(), "app/gateway".to_string()),
            ("auth".to_string(), "app/auth".to_string()),
            ("auth-rpc".to_string(), "app/auth_rpc".to_string()),
            ("shared-log".to_string(), "shared/log".to_string()),
        ];
        let graph = CodeGraph {
            edges: vec![
                // gateway imports auth_rpc (twice) -> auth-rpc is producer, gateway consumer
                edge("app/gateway/main.go", "app/auth_rpc/client.go"),
                edge("app/gateway/handler.go", "app/auth_rpc/client.go"),
                // gateway imports shared/log
                edge("app/gateway/main.go", "shared/log/log.go"),
                // intra-module edge is ignored
                edge("app/gateway/main.go", "app/gateway/util.go"),
                // edge to a file outside every module is ignored
                edge("app/gateway/main.go", "vendor/x/y.go"),
                // auth_rpc imports shared/log
                edge("app/auth_rpc/client.go", "shared/log/log.go"),
            ],
            available: true,
            ..Default::default()
        };

        let mut edges = module_edges(&graph, &modules);
        edges.sort_by(|a, b| (a.from.clone(), a.to.clone()).cmp(&(b.from.clone(), b.to.clone())));

        assert_eq!(
            edges,
            vec![
                ModuleEdge { from: "auth-rpc".into(), to: "gateway".into(), weight: 2, kind: "import".into() },
                ModuleEdge { from: "shared-log".into(), to: "auth-rpc".into(), weight: 1, kind: "import".into() },
                ModuleEdge { from: "shared-log".into(), to: "gateway".into(), weight: 1, kind: "import".into() },
            ],
            "should roll up to longest-prefix module, drop intra-module + out-of-module edges, and count weights",
        );
    }

    fn medge(from: &str, to: &str, weight: u32) -> ModuleEdge {
        ModuleEdge {
            from: from.into(),
            to: to.into(),
            weight,
            kind: "import".into(),
        }
    }

    #[test]
    fn derived_deps_orders_consumer_after_producer_and_drops_weak_edges() {
        // producer→consumer edges: gateway imports auth (strong) and log (weak).
        let deps = acyclic_consumer_deps(
            vec![medge("auth", "gateway", 10), medge("log", "gateway", 2)],
            3,
        );
        // gateway depends on auth (w=10 ≥ 3); the w=2 log edge is dropped.
        assert_eq!(
            deps.get("gateway"),
            Some(&std::collections::BTreeSet::from(["auth".to_string()])),
        );
    }

    #[test]
    fn derived_deps_breaks_cycles_keeping_the_strongest_edge() {
        // A mutual pair (a↔b) and a 3-cycle must not produce a cyclic map.
        let deps = acyclic_consumer_deps(
            vec![
                medge("a", "b", 9), // b depends on a
                medge("b", "a", 4), // a depends on b — would cycle, dropped
                medge("b", "c", 8), // c depends on b
                medge("c", "a", 7), // a depends on c
                medge("a", "c", 3), // c depends on a — would cycle, dropped
            ],
            3,
        );
        // Acyclic: from any module's producers, you can never return to it.
        for m in ["a", "b", "c"] {
            if let Some(producers) = deps.get(m) {
                for p in producers {
                    assert!(!reaches(&deps, p, m), "cycle: {m} -> {p} -> ... -> {m}");
                }
            }
        }
        // The strongest edge (b depends on a, w=9) is always kept.
        assert!(deps.get("b").unwrap().contains("a"));
    }

    #[test]
    fn scan_import_edges_parses_real_cross_module_go_imports() {
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let prefix = "code.example/monorepo";
        fs::write(root.join("go.mod"), format!("module {prefix}\n")).unwrap();
        // gateway imports shared/log; api imports gateway + shared/log; log imports nothing.
        fs::create_dir_all(root.join("app/gateway")).unwrap();
        fs::create_dir_all(root.join("app/api")).unwrap();
        fs::create_dir_all(root.join("shared/log")).unwrap();
        fs::write(
            root.join("app/gateway/main.go"),
            format!("package gateway\nimport (\n  \"{prefix}/shared/log\"\n  \"fmt\"\n)\n"),
        )
        .unwrap();
        fs::write(
            root.join("app/api/h.go"),
            format!("package api\nimport (\n  \"{prefix}/app/gateway/client\"\n  \"{prefix}/shared/log\"\n)\n"),
        )
        .unwrap();
        fs::write(root.join("shared/log/log.go"), "package log\n").unwrap();

        let modules = vec![
            ("gateway".to_string(), "app/gateway".to_string()),
            ("api".to_string(), "app/api".to_string()),
            ("log".to_string(), "shared/log".to_string()),
        ];
        let mut edges = scan_import_edges(root, &modules);
        edges.sort_by(|a, b| (a.from.clone(), a.to.clone()).cmp(&(b.from.clone(), b.to.clone())));
        assert_eq!(
            edges,
            vec![
                ModuleEdge { from: "gateway".into(), to: "api".into(), weight: 1, kind: "import".into() },
                ModuleEdge { from: "log".into(), to: "api".into(), weight: 1, kind: "import".into() },
                ModuleEdge { from: "log".into(), to: "gateway".into(), weight: 1, kind: "import".into() },
            ],
            "should map full-path Go imports back to producer modules (intra-module + stdlib ignored)",
        );
    }

    #[test]
    fn scan_import_edges_resolves_go_mod_under_a_subdir() {
        // Combined multi-repo workspace: the backend (with its go.mod) is mounted
        // under `backend/`, so an import `<prefix>/shared/log` must map to the
        // workspace path `backend/shared/log`.
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let prefix = "code.example/monorepo";
        fs::create_dir_all(root.join("backend/app/gateway")).unwrap();
        fs::create_dir_all(root.join("backend/shared/log")).unwrap();
        fs::write(root.join("backend/go.mod"), format!("module {prefix}\n")).unwrap();
        fs::write(
            root.join("backend/app/gateway/main.go"),
            format!("package gateway\nimport \"{prefix}/shared/log\"\n"),
        )
        .unwrap();
        fs::write(root.join("backend/shared/log/log.go"), "package log\n").unwrap();
        let modules = vec![
            ("gateway".to_string(), "backend/app/gateway".to_string()),
            ("log".to_string(), "backend/shared/log".to_string()),
            ("web".to_string(), "frontend/web".to_string()), // different repo, no Go
        ];
        assert_eq!(
            scan_import_edges(root, &modules),
            vec![ModuleEdge {
                from: "log".into(),
                to: "gateway".into(),
                weight: 1,
                kind: "import".into()
            }],
        );
    }

    #[test]
    fn scan_contract_edges_detects_cross_stack_generated_client_naming() {
        // A combined workspace where a frontend project carries a generated
        // client whose path contains the backend service's snake-case name —
        // the real-world bam-idl/typings convention. Word boundaries prevent
        // matching a token inside an unrelated longer identifier.
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // backend services + a frontend project + an unrelated frontend
        for sub in [
            "backend/app/app_alpha/svc",
            "backend/app/app_beta/svc",
            "backend/shared/shared_lib/lib",
            "frontend/webapp_alpha/src/bam-idl/AppAlpha",
            "frontend/webapp_alpha/src/api/bam/typings/ops-service",
            "frontend/unrelated/src",
        ] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        // The frontend's generated dirs contain files named after backend services.
        fs::write(
            root.join("frontend/webapp_alpha/src/bam-idl/AppAlpha/app_alpha.ts"),
            "// generated",
        )
        .unwrap();
        fs::write(
            root.join("frontend/webapp_alpha/src/api/bam/typings/ops-service/app_beta.ts"),
            "// generated",
        )
        .unwrap();
        // The unrelated frontend has nothing referencing backend services.
        fs::write(root.join("frontend/unrelated/src/app.ts"), "//").unwrap();
        // False-positive trap: a library carries a generic `client/` dir whose
        // name CONTAINS a backend service name token. Old logic would have
        // misread this as "shared-lib consumes the service"; the
        // generated-client marker requirement must reject it.
        fs::create_dir_all(root.join("backend/shared/shared_lib/client/generic_shared_lib_client"))
            .unwrap();
        fs::write(
            root.join("backend/shared/shared_lib/client/generic_shared_lib_client/app_alpha.go"),
            "// generic client",
        )
        .unwrap();

        let modules = vec![
            ("app-alpha".into(), "backend/app/app_alpha".into()),
            ("app-beta".into(), "backend/app/app_beta".into()),
            ("shared-lib".into(), "backend/shared/shared_lib".into()),
            ("webapp-alpha".into(), "frontend/webapp_alpha".into()),
            ("unrelated".into(), "frontend/unrelated".into()),
        ];
        let mut edges = scan_contract_edges(root, &modules);
        edges.sort_by(|a, b| (a.from.clone(), a.to.clone()).cmp(&(b.from.clone(), b.to.clone())));
        assert_eq!(
            edges,
            vec![
                ModuleEdge {
                    from: "app-alpha".into(),
                    to: "webapp-alpha".into(),
                    weight: 1,
                    kind: "contract".into()
                },
                ModuleEdge {
                    from: "app-beta".into(),
                    to: "webapp-alpha".into(),
                    weight: 1,
                    kind: "contract".into()
                },
            ],
            "webapp-alpha's generated client dirs (bam-idl/bam/typings) yield the cross-stack contract edges; shared-lib's generic client/ does NOT produce a misleading app-alpha → shared-lib edge, and the unrelated frontend stays unlinked",
        );
    }

    #[test]
    fn path_contains_token_respects_word_boundaries() {
        assert!(path_contains_token("a/prefix_app_alpha.ts", "app_alpha"));
        assert!(path_contains_token("foo/bam-idl/app_alpha/x", "app_alpha"));
        // not a word boundary: "app_alphas" or "myapp_alpha" would be a longer ident.
        assert!(!path_contains_token("a/app_alphax.ts", "app_alpha"));
        assert!(!path_contains_token("a/xapp_alpha.ts", "app_alpha"));
    }

    // F-103: is_generated_client_path must be component-aware so a marker
    // sitting at the project root (no leading "/marker/" substring) still
    // gets caught. Pre-F-103 substring grep missed this case.
    #[test]
    fn generated_client_path_matches_root_level_marker() {
        assert!(is_generated_client_path("bam-idl/app_alpha/foo.ts"));
        assert!(is_generated_client_path("openapi-client/app_alpha.ts"));
        assert!(is_generated_client_path("bam/typings/app_alpha.d.ts"));
    }

    #[test]
    fn generated_client_path_matches_nested_marker() {
        assert!(is_generated_client_path("src/bam-idl/app_alpha/service.ts"));
        assert!(is_generated_client_path("vendor/api-types/foo"));
    }

    #[test]
    fn generated_client_path_rejects_half_name_substrings() {
        // `not-bam-idl` or `xbam-idl-y` must not register as a marker.
        assert!(!is_generated_client_path("src/not-bam-idl/foo.ts"));
        assert!(!is_generated_client_path("src/xbam-idl/foo.ts"));
        // Single-word `bam` without `/typings` next door is not a marker.
        assert!(!is_generated_client_path("src/bam/foo.ts"));
        // Plain idl/ is for the PROVIDER side, not generated-client.
        assert!(!is_generated_client_path("idl/rpc/foo.thrift"));
    }

    #[test]
    fn scan_import_edges_empty_without_go_mod() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(scan_import_edges(dir.path(), &[("a".into(), "app/a".into())]).is_empty());
    }

    #[test]
    fn module_edges_count_only_import_edges_not_name_resolved_calls() {
        let modules = vec![
            ("a".to_string(), "app/a".to_string()),
            ("b".to_string(), "app/b".to_string()),
        ];
        let kedge = |s: &str, t: &str, kind: &str| FileEdge {
            source: s.into(),
            target: t.into(),
            kind: kind.into(),
        };
        let graph = CodeGraph {
            edges: vec![
                kedge("app/a/x.go", "app/b/y.go", "imports"), // real dep
                kedge("app/a/x.go", "app/b/util.go", "calls"), // name-resolved noise
                kedge("app/a/z.go", "app/b/util.go", "references"), // noise
                kedge("app/a/z.go", "app/b/t.go", "instantiates"), // noise
            ],
            available: true,
            ..Default::default()
        };
        // Only the single `imports` edge counts → weight 1, not 4.
        assert_eq!(
            module_edges(&graph, &modules),
            vec![ModuleEdge {
                from: "b".into(),
                to: "a".into(),
                weight: 1,
                kind: "import".into()
            }],
        );
    }

    #[test]
    fn module_edges_ignores_root_and_empty_module_paths() {
        let modules = vec![
            ("root".to_string(), ".".to_string()),
            ("blank".to_string(), String::new()),
            ("svc".to_string(), "svc".to_string()),
        ];
        let graph = CodeGraph {
            edges: vec![edge("svc/a.go", "other/b.go"), edge("svc/a.go", "svc/c.go")],
            available: true,
            ..Default::default()
        };
        // "other" isn't a module; "." / "" must not greedily claim everything.
        assert!(module_edges(&graph, &modules).is_empty());
    }

    #[test]
    fn norm_join_resolves_dots() {
        assert_eq!(norm_join("web/src/components", "../api"), "web/src/api");
        assert_eq!(norm_join("web/src", "./types"), "web/src/types");
        assert_eq!(norm_join("a/b", "../../c"), "c");
    }

    #[test]
    fn rust_mod_and_use_parsing() {
        assert_eq!(rust_mod_decl("pub mod native;"), Some("native"));
        assert_eq!(rust_mod_decl("mod tests {"), None); // inline module
        assert_eq!(
            rust_use_crate("use crate::server::handlers::misc;"),
            Some(vec!["server".into(), "handlers".into(), "misc".into()])
        );
        assert_eq!(
            rust_use_crate("use crate::paths::{ensure_dir, runs_dir};"),
            Some(vec!["paths".into()])
        );
        assert_eq!(rust_use_crate("use std::collections::HashMap;"), None);
    }

    #[test]
    fn resolves_rust_mod_and_crate() {
        let files: HashSet<String> = ["src/main.rs", "src/codegraph.rs", "src/server/mod.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // `mod codegraph;` from src/main.rs → src/codegraph.rs
        assert_eq!(
            resolve_rust_mod("src/main.rs", "codegraph", &files),
            Some("src/codegraph.rs".to_string())
        );
        // `use crate::server::...` → src/server/mod.rs
        assert_eq!(
            resolve_rust_crate("src", &["server".into(), "x".into()], &files),
            Some("src/server/mod.rs".to_string())
        );
    }

    #[test]
    fn resolves_ts_relative_import() {
        let files: HashSet<String> = ["web/src/api.ts", "web/src/components/Header.tsx"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            resolve_ts("web/src/components/Header.tsx", "../api", &files),
            Some("web/src/api.ts".to_string())
        );
        assert_eq!(resolve_ts("web/src/api.ts", "react", &files), None);
    }

    #[test]
    fn understand_graph_maps_nodes_layers_and_edges() {
        let json = r#"{
          "project": {"languages": ["typescript"]},
          "nodes": [
            {"id":"file:src/a.ts","type":"file","filePath":"src/a.ts","name":"a.ts","summary":"module a","tags":["core"],"complexity":"simple"},
            {"id":"file:src/b.ts","type":"file","filePath":"src/b.ts","name":"b.ts"},
            {"id":"function:src/a.ts:foo","type":"function","filePath":"src/a.ts","name":"foo","lineRange":[3,9],"summary":"does foo"}
          ],
          "edges": [
            {"source":"file:src/a.ts","target":"file:src/b.ts","type":"imports"},
            {"source":"file:src/a.ts","target":"function:src/a.ts:foo","type":"contains"}
          ],
          "layers": [
            {"id":"layer:core","name":"Core","description":"the core","nodeIds":["file:src/a.ts"]}
          ],
          "tour": [
            {"order":1,"title":"Start","description":"begin here","nodeIds":["function:src/a.ts:foo"],"languageLesson":"ts tip"}
          ]
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let ua_dir = dir.path().join(".understand-anything");
        std::fs::create_dir_all(&ua_dir).unwrap();
        let p = ua_dir.join("knowledge-graph.json");
        std::fs::write(&p, json).unwrap();

        let g = understand_graph(&p).unwrap();
        assert_eq!(g.source, "understand-anything");
        assert_eq!(g.nodes.len(), 2); // two file nodes (function excluded)
        let a = g.nodes.iter().find(|n| n.path == "src/a.ts").unwrap();
        assert_eq!(a.summary.as_deref(), Some("module a"));
        assert_eq!(a.tags, vec!["core"]);
        assert_eq!(a.symbols, 1); // one function in a.ts
        assert_eq!(a.layer.as_deref(), Some("layer:core"));
        // contains edge dropped; imports kept
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].kind, "imports");
        assert_eq!(g.layers.len(), 1);
        assert_eq!(g.layers[0].files, vec!["src/a.ts"]);
        assert_eq!(g.tour.len(), 1);
        assert_eq!(g.tour[0].files, vec!["src/a.ts"]);

        // file symbols come from function/class nodes with summaries
        let syms = understand_file_symbols(&p, "src/a.ts");
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "foo");
        assert_eq!(syms[0].line, Some(3));
        assert_eq!(syms[0].signature.as_deref(), Some("does foo"));
    }

    #[test]
    fn native_graph_indexes_this_repo() {
        // Smoke test against maestro's own tree.
        let root = find_repo_root(&std::env::current_dir().unwrap());
        let g = native_graph(&root);
        assert!(g.available);
        assert!(g.nodes.len() > 50, "expected many source files");
        assert!(!g.edges.is_empty(), "expected resolved import edges");
    }

    #[test]
    fn collect_source_files_excludes_vendored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/app.ts"), "export const x = 1\n").unwrap();
        std::fs::create_dir_all(root.join("vendor/pkg")).unwrap();
        std::fs::write(root.join("vendor/pkg/lib.ts"), "export const y = 2\n").unwrap();
        std::fs::create_dir_all(root.join(".venv/lib")).unwrap();
        std::fs::write(root.join(".venv/lib/dep.py"), "x = 1\n").unwrap();

        let files = collect_source_files(root);
        assert!(files.contains(&"src/app.ts".to_string()), "got {files:?}");
        assert!(
            !files.iter().any(|f| f.contains("vendor/")),
            "got {files:?}"
        );
        assert!(!files.iter().any(|f| f.contains(".venv/")), "got {files:?}");
    }

    #[test]
    fn native_context_ranks_query_relevant_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("user.ts"),
            "export function formatUser(u){return u.id}\nexport interface User{id:string}\n",
        )
        .unwrap();
        std::fs::write(root.join("billing.ts"), "export function charge(){}\n").unwrap();
        std::fs::write(root.join("unrelated.ts"), "export function noop(){}\n").unwrap();

        // Query mentions "user" — the user.ts file (path + symbol match) should rank.
        let hits = native_context(root, "add email to the user profile", 5);
        let paths: Vec<&str> = hits.iter().map(|(p, _)| p.as_str()).collect();
        assert!(
            paths.contains(&"user.ts"),
            "user.ts should be a hit, got {paths:?}"
        );
        // Symbols are surfaced for grounding.
        let user = hits.iter().find(|(p, _)| p == "user.ts").unwrap();
        assert!(user
            .1
            .iter()
            .any(|s| s.name == "formatUser" || s.name == "User"));
        // An empty/keywordless query yields nothing (no noise injected).
        assert!(native_context(root, "a an of", 5).is_empty());
    }
}
