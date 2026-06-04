use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::config::{Contracts, Project};

/// Read + parse a JSON file with consistent `read {}` / `parse {}` error
/// context. Use for the `?`-propagating manifest parsers (not the registry
/// loaders, which deliberately fall back on a parse error).
pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Read + parse a YAML file with the same consistent error context.
pub(super) fn read_yaml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

mod contracts;
mod imports;
mod manifests;
mod monorepo;
mod walk;

use contracts::*;
use imports::*;
use manifests::*;
use monorepo::*;
use walk::*;

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryReport {
    pub root: PathBuf,
    pub projects: BTreeMap<String, DiscoveredProject>,
    pub edges: Vec<DiscoveredEdge>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredProject {
    pub name: String,
    pub path: PathBuf,
    pub relative_path: String,
    pub confidence: u8,
    pub project_type: Option<String>,
    pub stack: Vec<String>,
    pub package_name: Option<String>,
    pub commands: BTreeMap<String, String>,
    pub provides: Option<String>,
    pub consumes: Option<String>,
    pub dependencies: Vec<String>,
    pub markers: Vec<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredEdge {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub confidence: u8,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub max_depth: usize,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self { max_depth: 3 }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    markers: Vec<String>,
    package_name: Option<String>,
    project_type: Option<String>,
    stack: BTreeSet<String>,
    commands: BTreeMap<String, String>,
    local_dep_paths: Vec<PathBuf>,
    package_deps: Vec<String>,
    workspace_package_deps: Vec<String>,
    /// Absolute paths that this project's *source code* imports via relative
    /// specifiers (`import … from "../../shared"`). Lets discovery infer
    /// cross-project edges a manifest never declared.
    source_dep_targets: Vec<PathBuf>,
    provides: Option<String>,
    evidence: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct DirAnalysis {
    package_name: Option<String>,
    project_type: Option<String>,
    stack: BTreeSet<String>,
    commands: BTreeMap<String, String>,
    local_dep_paths: Vec<PathBuf>,
    package_deps: Vec<String>,
    workspace_package_deps: Vec<String>,
    source_dep_targets: Vec<PathBuf>,
    evidence: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct WorkspaceDirs {
    members: Vec<PathBuf>,
    gradle_members: Vec<PathBuf>,
    excludes: Vec<PathBuf>,
}

#[derive(Debug)]
struct DiscoveryIgnores {
    allowed_dirs: BTreeSet<PathBuf>,
}

impl DiscoveryIgnores {
    fn load(root: &Path) -> Self {
        let mut allowed_dirs = BTreeSet::new();
        allowed_dirs.insert(root.to_path_buf());
        for entry in WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .build()
        {
            match entry {
                Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_dir()) => {
                    allowed_dirs.insert(entry.path().to_path_buf());
                }
                Ok(_) => {}
                Err(error) => tracing::warn!("skip ignored discover path: {error}"),
            }
        }
        Self { allowed_dirs }
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        is_dir && !self.allowed_dirs.contains(path)
    }
}

pub fn discover(root: &Path, opts: &DiscoverOptions) -> Result<DiscoveryReport> {
    let root = crate::paths::expand(root.to_string_lossy().as_ref())?;
    let root = root
        .canonicalize()
        .with_context(|| format!("discover root {}", root.display()))?;
    if !root.is_dir() {
        anyhow::bail!("discover root is not a directory: {}", root.display());
    }

    // Monorepo registry short-circuit. A Bazel/Go monorepo declares its modules
    // in `.monorepo_config.yaml` under a *single* root `go.mod`, so the generic
    // manifest walk would collapse the whole repo into one project. When the
    // registry is present, trust it: it's the authoritative module list.
    if let Some(report) = discover_from_monorepo_config(&root)? {
        return Ok(report);
    }
    // Rush (pnpm) monorepo: projects are registered in rush.json. Trust it over
    // a recursive package.json scan that misses the subspace structure.
    if let Some(report) = discover_from_rush(&root)? {
        return Ok(report);
    }
    // Combined multi-repo workspace: the root has no manifest of its own but
    // shallow subdirs are themselves registered monorepos (e.g. `backend/`,
    // `frontend/`). Recurse into each, prefix paths by the subdir, and merge —
    // so `maestro init --analyze` "just works" on a workspace that mounts
    // multiple repos side-by-side.
    if let Some(report) = discover_subrepo_workspace(&root, opts)? {
        return Ok(report);
    }

    let mut candidates = Vec::new();
    let ignores = DiscoveryIgnores::load(&root);
    collect_candidates(&root, &root, 0, opts.max_depth, &mut candidates, &ignores)?;
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    candidates.dedup_by(|a, b| a.path == b.path);

    let mut warnings = Vec::new();
    let mut projects = BTreeMap::new();
    let mut path_to_name = Vec::<(PathBuf, String)>::new();
    let mut package_to_name = BTreeMap::<String, String>::new();

    for candidate in &candidates {
        let base_name = candidate
            .package_name
            .clone()
            .or_else(|| {
                candidate
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "project".to_string());
        let name = unique_name(&projects, &sanitize_project_name(&base_name));
        if let Some(package) = &candidate.package_name {
            if let Some(existing) = package_to_name.insert(package.clone(), name.clone()) {
                warnings.push(format!(
                    "package name {package:?} appears in both {existing} and {name}; dependency inference will use {name}"
                ));
            }
        }
        let relative_path = display_path(relative_to(&root, &candidate.path));
        let stack = candidate.stack.iter().cloned().collect::<Vec<_>>();
        let project = DiscoveredProject {
            name: name.clone(),
            path: candidate.path.clone(),
            relative_path,
            confidence: project_confidence(candidate),
            project_type: candidate.project_type.clone(),
            stack,
            package_name: candidate.package_name.clone(),
            commands: candidate.commands.clone(),
            provides: candidate.provides.clone(),
            consumes: None,
            dependencies: Vec::new(),
            markers: candidate.markers.clone(),
            evidence: candidate.evidence.iter().cloned().collect(),
        };
        path_to_name.push((candidate.path.clone(), name.clone()));
        projects.insert(name, project);
    }

    // Canonicalize each project path ONCE: project_for_path is called per
    // dependency target below, so canonicalizing inside it was O(projects ×
    // targets) filesystem stat-walks. Fall back to the literal path when one
    // can't be canonicalized (e.g. doesn't exist on disk).
    let path_to_name_canon: Vec<(PathBuf, String)> = path_to_name
        .iter()
        .map(|(p, n)| (p.canonicalize().unwrap_or_else(|_| p.clone()), n.clone()))
        .collect();

    let mut edges = Vec::<DiscoveredEdge>::new();
    let name_to_candidate = candidates
        .iter()
        .filter_map(|candidate| {
            path_to_name
                .iter()
                .find(|(path, _)| path == &candidate.path)
                .map(|(_, name)| (name.clone(), candidate))
        })
        .collect::<BTreeMap<_, _>>();

    // provider name -> { imported-file (relative to provider) -> count }, so a
    // project another project imports across a boundary can be assigned that
    // imported file as its `provides` contract (covers the .d.ts / barrel case
    // a manifest never declares).
    let mut import_targets: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

    for (consumer, candidate) in &name_to_candidate {
        let mut deps = BTreeSet::<String>::new();
        for package in &candidate.package_deps {
            if let Some(provider) = package_to_name.get(package) {
                if provider != consumer {
                    deps.insert(provider.clone());
                    edges.push(DiscoveredEdge {
                        from: provider.clone(),
                        to: consumer.clone(),
                        reason: format!("package dependency {package}"),
                        confidence: 80,
                        evidence: vec![format!("package:{package}")],
                    });
                }
            }
        }
        for package in &candidate.workspace_package_deps {
            if let Some(provider) = package_to_name.get(package) {
                if provider != consumer {
                    deps.insert(provider.clone());
                    edges.push(DiscoveredEdge {
                        from: provider.clone(),
                        to: consumer.clone(),
                        reason: format!("workspace dependency {package}"),
                        confidence: 95,
                        evidence: vec![format!("workspace:{package}")],
                    });
                }
            }
        }
        for dep_path in &candidate.local_dep_paths {
            let resolved = normalize_join(&candidate.path, dep_path);
            if let Some(provider) = project_for_path(&path_to_name_canon, &resolved) {
                if provider != *consumer {
                    deps.insert(provider.clone());
                    edges.push(DiscoveredEdge {
                        from: provider.clone(),
                        to: consumer.clone(),
                        reason: format!(
                            "local path dependency {}",
                            display_path(Some(dep_path.clone()))
                        ),
                        confidence: 95,
                        evidence: vec![format!(
                            "local-path:{}",
                            display_path(Some(dep_path.clone()))
                        )],
                    });
                }
            }
        }
        for target in &candidate.source_dep_targets {
            if let Some(provider) = project_for_path(&path_to_name_canon, target) {
                if provider != *consumer {
                    deps.insert(provider.clone());
                    // Record the imported file relative to its provider so we
                    // can later promote it to the provider's `provides`.
                    if let Some(prov_cand) = name_to_candidate.get(&provider) {
                        if let Ok(rel) = target.strip_prefix(&prov_cand.path) {
                            let relstr = rel.to_string_lossy().replace('\\', "/");
                            if !relstr.is_empty() {
                                *import_targets
                                    .entry(provider.clone())
                                    .or_default()
                                    .entry(relstr)
                                    .or_insert(0) += 1;
                            }
                        }
                    }
                    edges.push(DiscoveredEdge {
                        from: provider.clone(),
                        to: consumer.clone(),
                        reason: "source import".to_string(),
                        confidence: 85,
                        evidence: vec!["source-import".to_string()],
                    });
                }
            }
        }
        if let Some(p) = projects.get_mut(consumer) {
            p.dependencies = deps.iter().cloned().collect();
        }
    }

    edges.sort_by(|a, b| (&a.from, &a.to, &a.reason).cmp(&(&b.from, &b.to, &b.reason)));
    let mut merged = Vec::<DiscoveredEdge>::new();
    for edge in edges {
        if let Some(last) = merged
            .last_mut()
            .filter(|last| last.from == edge.from && last.to == edge.to)
        {
            if !last.reason.contains(&edge.reason) {
                last.reason.push_str("; ");
                last.reason.push_str(&edge.reason);
            }
            last.confidence = last.confidence.max(edge.confidence);
            merge_unique(&mut last.evidence, edge.evidence);
        } else {
            merged.push(edge);
        }
    }
    let edges = merged;

    // Promote the most-imported file of each provider to its `provides` contract
    // when nothing was detected from a manifest/contract file. Ties resolve to
    // the lexicographically-smallest path (BTreeMap order + strictly-greater
    // replacement) for determinism.
    for (provider, files) in &import_targets {
        let Some(p) = projects.get_mut(provider) else {
            continue;
        };
        if p.provides.is_some() {
            continue;
        }
        let mut best: Option<(&String, usize)> = None;
        for (f, c) in files {
            if best.map(|(_, bc)| *c > bc).unwrap_or(true) {
                best = Some((f, *c));
            }
        }
        if let Some((file, _)) = best {
            p.provides = Some(file.clone());
        }
    }

    for edge in &edges {
        let provider_contract = projects.get(&edge.from).and_then(|p| p.provides.clone());
        let consumer_path = projects.get(&edge.to).map(|p| p.path.clone());
        let provider_path = projects.get(&edge.from).map(|p| p.path.clone());
        if let (Some(contract), Some(consumer_dir), Some(provider_dir)) =
            (provider_contract, consumer_path, provider_path)
        {
            let contract_abs = provider_dir.join(&contract);
            let rel = display_path(relative_to(&consumer_dir, &contract_abs));
            if let Some(consumer) = projects.get_mut(&edge.to) {
                consumer.consumes.get_or_insert(rel);
            }
        }
    }

    Ok(DiscoveryReport {
        root,
        projects,
        edges,
        warnings,
    })
}

pub fn project_from_discovery(project: &DiscoveredProject, agent: Option<&str>) -> Project {
    let path = crate::paths::workspace_root()
        .ok()
        .and_then(|root| relative_to(&root, &project.path))
        .map(|path| display_path(Some(path)))
        .unwrap_or_else(|| project.path.to_string_lossy().replace('\\', "/"));
    Project {
        path,
        r#type: project.project_type.clone(),
        stack: project.stack.clone(),
        commands: project.commands.clone(),
        contracts: Contracts {
            provides: project.provides.clone(),
            consumes: project.consumes.clone(),
        },
        dependencies: project.dependencies.clone(),
        memory_scope: vec![],
        agent: agent.map(str::to_string),
        agent_model: None,
        cursor_model: None,
        model_profile: None,
        role: None,
        agent_profile: None,
        review_profile: None,
        copy_files: Vec::new(),
    }
}

/// Merge a freshly-discovered Project INTO an existing `projects.yaml`
/// entry without clobbering user-authored fields.
///
/// Discovery's job is to (re)derive the **auto fields**:
///   path · r#type · stack · commands · dependencies
///
/// Everything else stays as the user set it:
///   memory_scope · agent · agent_model · cursor_model · model_profile ·
///   role · copy_files
///
/// `contracts` get a small extra rule — when the existing entry has
/// neither `provides` nor `consumes` set, we accept `incoming.contracts`
/// (so first-pass discovery seeds them); once set, the user wins so a
/// manual contract pin survives a re-init.
///
/// Why this lives in `config::discovery` and not in the `cli::commands`
/// layer: the CLI's `discover_and_apply` calls it, the tests pin it
/// directly, and **production + tests share this function** rather than
/// each owning a copy. Drifting copies are exactly how the silent
/// loss-of-memory_scope regression on re-init gets re-introduced.
pub fn merge_project_from_discovery(existing: &mut Project, incoming: Project) {
    existing.path = incoming.path;
    existing.r#type = incoming.r#type;
    existing.stack = incoming.stack;
    existing.commands = incoming.commands;
    existing.dependencies = incoming.dependencies;
    if existing.contracts.provides.is_none() && existing.contracts.consumes.is_none() {
        existing.contracts = incoming.contracts;
    }
    // memory_scope / agent / agent_model / cursor_model / model_profile /
    // role / copy_files are intentionally left as-is — discovery never
    // derives these and they're how the user pins per-project behavior.
}

/// F-103 — Run contract promotion in two passes on a discovery report.
/// See `docs/experience/F-103-CONTRACTS-PROMOTION-DESIGN.md` for the rules.
///
/// Pass 1 (provider): for each project that doesn't already declare a
/// `provides`, scan its tree for an IDL-family marker directory holding at
/// least one contract file. The promoted string is **project-relative**
/// (matches the existing `detect_contract` convention). Generated-client
/// dirs are rejected as provider sources via the `is_generated_client_path`
/// guard in `collect_contract_files`.
///
/// Pass 2 (consumer): for each project that doesn't already declare a
/// `consumes`, scan for a generated-client directory and try to match its
/// immediate subdir name to a registered project that **already has**
/// `provides` set by Pass 1. On a match, record a `DiscoveredEdge` on the
/// report with `reason: "generated-client contract"` and set the consumer's
/// `consumes` to the consumer-relative path of the producer's contract.
///
/// Returns `(promoted_providers, promoted_consumers)` counts so the caller
/// can surface them in the discovery status line.
pub fn promote_contracts(report: &mut DiscoveryReport) -> (usize, usize) {
    promote_contracts_with_siblings(report, &[])
}

/// Like [`promote_contracts`], but also indexes contract providers from the
/// given sibling workspace roots (F-109), so a consumer here can link
/// `consumes` to a producer that lives in another repo. Each `sibling_roots`
/// entry is a path to a workspace root with its own `.maestro/projects.yaml`;
/// only its projects that already have `contracts.provides` set are indexed
/// (no discovery is run into the sibling tree). A local provider always wins
/// over a same-named sibling, and among siblings the **first declared** wins.
pub fn promote_contracts_with_siblings(
    report: &mut DiscoveryReport,
    sibling_roots: &[PathBuf],
) -> (usize, usize) {
    use std::collections::{BTreeMap, BTreeSet};

    // ─── Pass 1: providers ────────────────────────────────────────────
    // detect_contract() is narrow on purpose (other callers depend on
    // its conservatism). For F-103 we widen the matrix per design §3.1:
    // idl/proto/openapi/thrift/schemas/contracts/api-spec family, depth
    // up to project-root + 2, and any .yaml/.yml/.json/.proto/.thrift
    // inside a marker dir counts as a contract file. We still fall back
    // to detect_contract() for the file-at-root patterns it already
    // catches (e.g. top-level `openapi.yaml`).
    let mut new_providers: Vec<(String, String)> = Vec::new();
    for (name, p) in &report.projects {
        if p.provides.is_some() {
            continue;
        }
        if let Some(rel) = detect_provider_marker(&p.path).or_else(|| detect_contract(&p.path)) {
            new_providers.push((name.clone(), rel));
        }
    }
    let promoted_providers = new_providers.len();
    for (name, rel) in &new_providers {
        if let Some(p) = report.projects.get_mut(name) {
            p.provides = Some(rel.clone());
            p.evidence
                .push(format!("contract provider promoted: {rel}"));
        }
    }

    // ─── Pass 2: consumers (depends on Pass 1 results) ────────────────
    // Pre-index providers (name → (project_root, provides_rel)) so the
    // consumer walk doesn't re-borrow report inside its loop.
    let mut provider_index: BTreeMap<String, (PathBuf, String)> = report
        .projects
        .iter()
        .filter_map(|(n, p)| {
            p.provides
                .as_ref()
                .map(|prov| (n.clone(), (p.path.clone(), prov.clone())))
        })
        .collect();

    // F-109: fold in providers from declared sibling workspaces. A local
    // provider always wins over a same-named sibling (never let a sibling
    // shadow an in-tree producer); among siblings the FIRST declared wins
    // (deterministic — don't let a later insert silently overwrite). The
    // resulting names are tracked so the consumer pass can require a strict
    // relative `consumes` path for them and mark the edge cross-workspace.
    let mut sibling_providers: BTreeSet<String> = BTreeSet::new();
    for (name, root, provides) in load_sibling_providers(sibling_roots) {
        if provider_index.contains_key(&name) {
            // Local or an earlier-declared sibling already owns this name.
            if !sibling_providers.contains(&name) {
                tracing::warn!(
                    "F-109: sibling provider {name:?} ignored — a local project already provides this contract",
                );
            } else {
                tracing::warn!(
                    "F-109: duplicate sibling provider {name:?} ignored — kept the first-declared sibling workspace",
                );
            }
            continue;
        }
        provider_index.insert(name.clone(), (root, provides));
        sibling_providers.insert(name);
    }

    struct ConsumerMatch {
        consumer: String,
        producer: String,
        consumes_rel: String,
        evidence: Vec<String>,
        reason: &'static str,
    }
    let mut new_consumers: Vec<ConsumerMatch> = Vec::new();
    for (cname, cp) in &report.projects {
        if cp.consumes.is_some() {
            continue;
        }
        let Some(hit) = find_generated_client_match(cp, &provider_index) else {
            continue;
        };
        let (producer_root, producer_provides) = &provider_index[&hit.producer];
        let producer_contract_abs = producer_root.join(producer_provides);
        let is_sibling = sibling_providers.contains(&hit.producer);
        // Consumer-relative path to producer's contract file — what every
        // downstream reader expects (matches the edge loop convention).
        let consumes_rel = if is_sibling {
            // Cross-workspace (F-109): REQUIRE a true relative path. The two
            // roots must share an ancestor; if they don't, `relative_to` would
            // fall back to the absolute target, and we must never write a
            // machine-specific absolute path into projects.yaml — so skip +
            // warn instead.
            match relative_to_strict(&cp.path, &producer_contract_abs) {
                Some(rel) => display_path(Some(rel)),
                None => {
                    tracing::warn!(
                        "F-109: cross-workspace consumer {cname:?} → sibling producer {:?} skipped — \
                         no relative path between the consumer and the sibling contract; \
                         declare the sibling workspace at a relative location",
                        hit.producer,
                    );
                    continue;
                }
            }
        } else {
            relative_to(&cp.path, &producer_contract_abs)
                .map(|p| display_path(Some(p)))
                .unwrap_or_else(|| producer_contract_abs.to_string_lossy().to_string())
        };
        // Edge evidence per design §5: both the generated-client subdir
        // (already in hit.evidence) AND the producer's contract file.
        let mut evidence = hit.evidence;
        evidence.push(format!("producer contract: {producer_provides}"));
        new_consumers.push(ConsumerMatch {
            consumer: cname.clone(),
            producer: hit.producer,
            consumes_rel,
            evidence,
            reason: if is_sibling {
                "cross-workspace generated-client contract"
            } else {
                "generated-client contract"
            },
        });
    }
    let promoted_consumers = new_consumers.len();
    for c in new_consumers {
        if let Some(p) = report.projects.get_mut(&c.consumer) {
            p.consumes = Some(c.consumes_rel.clone());
            p.evidence
                .push(format!("contract consumer promoted: {}", c.consumes_rel));
        }
        report.edges.push(DiscoveredEdge {
            from: c.producer,
            to: c.consumer,
            reason: c.reason.to_string(),
            confidence: 90,
            evidence: c.evidence,
        });
    }

    (promoted_providers, promoted_consumers)
}

pub fn render_plan(report: &DiscoveryReport) -> String {
    let mut out = String::new();
    out.push_str("spec: Verify discovered project dependency DAG\n");
    out.push_str("created_by: maestro-discover\n\n");
    out.push_str("tasks:\n");
    for name in ordered_project_names(report) {
        let Some(project) = report.projects.get(name) else {
            continue;
        };
        let task_id = task_id_for(name);
        out.push_str(&format!("  - id: {task_id}\n"));
        out.push_str(&format!("    project: {name}\n"));
        out.push_str("    kind: verify\n");
        out.push_str("    agent: shell\n");
        let deps = report
            .edges
            .iter()
            .filter(|edge| edge.to == *name)
            .map(|edge| task_id_for(&edge.from))
            .collect::<Vec<_>>();
        if !deps.is_empty() {
            out.push_str(&format!("    depends_on: [{}]\n", deps.join(", ")));
        }
        let command = project
            .commands
            .get("test")
            .or_else(|| project.commands.get("check"))
            .or_else(|| project.commands.get("build"))
            .cloned()
            .unwrap_or_else(|| format!("echo \"no verification command detected for {name}\""));
        out.push_str("    command: |\n");
        for line in command.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn project_confidence(candidate: &Candidate) -> u8 {
    let has_manifest = candidate.markers.iter().any(|marker| {
        matches!(
            marker.as_str(),
            "package.json" | "Cargo.toml" | "pyproject.toml" | "requirements.txt" | "go.mod"
        )
    });
    let mut score = if has_manifest {
        90
    } else if candidate.provides.is_some() {
        65
    } else {
        50
    };
    if candidate.package_name.is_some() {
        score += 4;
    }
    if !candidate.commands.is_empty() {
        score += 3;
    }
    if candidate.project_type.is_some() {
        score += 2;
    }
    if candidate.provides.is_some() {
        score += 1;
    }
    score.min(99)
}

fn merge_unique(target: &mut Vec<String>, incoming: Vec<String>) {
    target.extend(incoming);
    target.sort();
    target.dedup();
}

fn ordered_project_names(report: &DiscoveryReport) -> Vec<&str> {
    let mut indegree = report
        .projects
        .keys()
        .map(|name| (name.as_str(), 0usize))
        .collect::<BTreeMap<_, _>>();
    // Build the deduped adjacency FIRST, then derive indegree by counting
    // deduped children. Counting indegree per raw edge while deduping `outgoing`
    // double-counts a duplicate (from, to) pair — e.g. a source-import edge and
    // a promote_contracts generated-client edge for the same pair — so the
    // consumer's indegree never reaches 0 and the whole sort silently falls back
    // to unsorted order.
    let mut outgoing = BTreeMap::<&str, Vec<&str>>::new();
    for edge in &report.edges {
        if !report.projects.contains_key(&edge.from) || !report.projects.contains_key(&edge.to) {
            continue;
        }
        outgoing
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    for values in outgoing.values_mut() {
        values.sort();
        values.dedup();
    }
    for children in outgoing.values() {
        for child in children {
            *indegree.entry(*child).or_default() += 1;
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(*name))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::new();
    while let Some(name) = ready.pop_first() {
        ordered.push(name);
        if let Some(children) = outgoing.get(name) {
            for child in children {
                if let Some(count) = indegree.get_mut(child) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(child);
                    }
                }
            }
        }
    }
    if ordered.len() != report.projects.len() {
        return report.projects.keys().map(|s| s.as_str()).collect();
    }
    ordered
}

fn should_skip_dir(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "dist" | "build" | "venv" | "__pycache__"
        )
}

fn should_skip_path_under_root(root: &Path, path: &Path, ignores: &DiscoveryIgnores) -> bool {
    if ignores.is_ignored(path, path.is_dir()) {
        return true;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | "target" | "dist" | "build" | "venv" | "__pycache__"
            )
    })
}

fn is_contract_container_dir(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        name,
        "contract"
            | "contracts"
            | "schema"
            | "schemas"
            | "proto"
            | "protos"
            | "idl"
            | "idls"
            | "thrift"
    )
}

fn infer_type_from_path(root: &Path, dir: &Path) -> String {
    let rel = display_path(relative_to(root, dir)).to_ascii_lowercase();
    if rel.contains("web") || rel.contains("front") || rel.contains("ui") {
        "frontend".to_string()
    } else if rel.contains("api") || rel.contains("server") || rel.contains("service") {
        "backend".to_string()
    } else if rel.contains("cli") || rel.contains("tool") {
        "tool".to_string()
    } else if rel.contains("mobile")
        || rel.contains("android")
        || rel.contains("ios")
        || rel.contains("native")
    {
        "mobile".to_string()
    } else {
        "library".to_string()
    }
}

fn unique_name(existing: &BTreeMap<String, DiscoveredProject>, base: &str) -> String {
    if !existing.contains_key(base) {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn sanitize_project_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '.' | '/' | '@') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

fn task_id_for(name: &str) -> String {
    format!("verify_{}", sanitize_project_name(name).replace('-', "_"))
}

/// `path_to_name_canon` must already be canonicalized (see `discover`): this is
/// called once per dependency target, so canonicalizing the project paths here
/// made it O(projects × targets) filesystem stat-walks. Only `resolved_dep`
/// (which varies per call) is canonicalized.
fn project_for_path(
    path_to_name_canon: &[(PathBuf, String)],
    resolved_dep: &Path,
) -> Option<String> {
    let resolved_dep = resolved_dep
        .canonicalize()
        .unwrap_or_else(|_| resolved_dep.to_path_buf());
    let mut best: Option<(usize, String)> = None;
    for (project_path, name) in path_to_name_canon {
        if resolved_dep == *project_path || resolved_dep.starts_with(project_path) {
            let len = project_path.components().count();
            if best
                .as_ref()
                .map(|(best_len, _)| len > *best_len)
                .unwrap_or(true)
            {
                best = Some((len, name.clone()));
            }
        }
    }
    best.map(|(_, name)| name)
}

fn normalize_join(base: &Path, dep: &Path) -> PathBuf {
    if dep.is_absolute() {
        dep.to_path_buf()
    } else {
        base.join(dep)
    }
}

fn find_upwards(start: &Path, file: &str) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// F-109: resolve declared `sibling_workspaces` entries to absolute roots.
/// Relative entries resolve against `workspace_root` — the directory that
/// holds the `.maestro/projects.yaml` the entries were declared in — **not**
/// the scan `--root` (which may be a subdirectory; the two diverge under
/// `maestro init --analyze --root <subdir>`). Absolute entries are kept as-is.
pub fn resolve_sibling_roots(workspace_root: &Path, sibling_workspaces: &[String]) -> Vec<PathBuf> {
    sibling_workspaces
        .iter()
        .map(|s| {
            let p = Path::new(s);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                workspace_root.join(p)
            }
        })
        .collect()
}

/// F-109: load contract providers declared in sibling workspaces, in
/// declaration order. Each returned tuple is `(project_name, project_abs_path,
/// provides_rel)`. Only projects that already have `contracts.provides` set are
/// returned (no discovery is run into the sibling tree). A sibling root that's
/// missing / has no `.maestro/projects.yaml` / fails to load is skipped with a
/// warning — a half-checked-out sibling must never break discovery.
///
/// The sibling project's path is resolved against the SIBLING root (absolute
/// kept as-is; relative joined onto the sibling root) — NOT via
/// `ProjectsConfig::resolved_path`, which would resolve against the current
/// workspace.
fn load_sibling_providers(sibling_roots: &[PathBuf]) -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    for root in sibling_roots {
        let pfile = root.join(".maestro").join("projects.yaml");
        let cfg = match crate::config::ProjectsConfig::load(&pfile) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "F-109: skipping sibling workspace {} — could not load {}: {e:#}",
                    root.display(),
                    pfile.display()
                );
                continue;
            }
        };
        for (name, p) in &cfg.projects {
            let Some(provides) = p.contracts.provides.as_deref() else {
                continue;
            };
            if provides.trim().is_empty() {
                continue;
            }
            let proj_path = Path::new(&p.path);
            let abs = if proj_path.is_absolute() {
                proj_path.to_path_buf()
            } else {
                root.join(proj_path)
            };
            out.push((name.clone(), abs, provides.to_string()));
        }
    }
    out
}

/// Like [`relative_to`] but returns `None` (instead of the absolute target)
/// when the two paths share no common ancestor. Used for cross-workspace
/// `consumes` (F-109) so a producer in a disjoint root never yields an
/// absolute path written into `projects.yaml`.
fn relative_to_strict(from_dir: &Path, target: &Path) -> Option<PathBuf> {
    let rel = relative_to(from_dir, target)?;
    // `relative_to` returns the (absolute) target verbatim when there's no
    // shared ancestor; reject that — a real relative path is never absolute.
    if rel.is_absolute() {
        return None;
    }
    Some(rel)
}

fn relative_to(from_dir: &Path, target: &Path) -> Option<PathBuf> {
    let from = normalize_components(from_dir);
    let to = normalize_components(target);
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 && from.first() != to.first() {
        return Some(target.to_path_buf());
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

fn normalize_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            Component::RootDir => Some("/".to_string()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_string()),
            Component::CurDir => None,
            Component::ParentDir => Some("..".to_string()),
        })
        .collect()
}

fn display_path(path: Option<PathBuf>) -> String {
    path.unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .replace('\\', "/")
}

/// Collapse `.` and `..` segments lexically (no filesystem access), so an
/// import target matches a project root by prefix even when it doesn't
/// resolve to an on-disk file (e.g. an extensionless TS import).
fn lexical_clean(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                // Only a real directory segment can be popped; keep `..` when
                // it sits above the root prefix.
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().map(|c| c.as_os_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn discovers_node_projects_and_local_dependency_edges() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("core/package.json"),
            r#"{"name":"@acme/core","scripts":{"test":"node --test"},"dependencies":{}}"#,
        );
        write(
            &dir.path().join("core/contract/openapi.yaml"),
            "openapi: 3.0.0\n",
        );
        write(
            &dir.path().join("web/package.json"),
            r#"{"name":"web","scripts":{"test":"vitest run"},"dependencies":{"@acme/core":"file:../core","react":"latest"}}"#,
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 2 }).unwrap();
        assert_eq!(report.projects.len(), 2);
        assert!(report.projects.contains_key("acme-core"));
        assert!(report.projects.contains_key("web"));
        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].from, "acme-core");
        assert_eq!(report.edges[0].to, "web");
        assert!(report.projects["web"].confidence >= 90);
        assert!(report.projects["web"]
            .evidence
            .contains(&"marker:package.json".to_string()));
        assert!(report.projects["web"]
            .evidence
            .contains(&"dependency:@acme/core=file:../core".to_string()));
        assert!(report.edges[0].confidence >= 90);
        assert!(report.edges[0]
            .evidence
            .contains(&"package:@acme/core".to_string()));
        assert!(report.edges[0]
            .evidence
            .contains(&"local-path:../core".to_string()));
        assert_eq!(
            report.projects["web"].consumes.as_deref(),
            Some("../core/contract/openapi.yaml")
        );
    }

    #[test]
    fn malformed_monorepo_config_falls_back_to_scan() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join(".monorepo_config.yaml"),
            "modules: [this is: not valid yaml\n  oops",
        );
        write(
            &dir.path().join("svc/package.json"),
            r#"{"name":"svc","scripts":{"test":"true"}}"#,
        );
        // Must not dead-end at 0 projects — fall back to the manifest scan.
        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        assert!(
            report.projects.contains_key("svc"),
            "should fall back to scanning package.json, got {:?}",
            report.projects.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn thrift_idl_is_detected_as_a_contract() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("svc/package.json"),
            r#"{"name":"svc","scripts":{"test":"true"}}"#,
        );
        write(
            &dir.path().join("svc/idl/api.thrift"),
            "service Api { void ping() }\n",
        );
        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        assert!(report.projects.contains_key("svc"));
        assert_eq!(
            report.projects["svc"].provides.as_deref(),
            Some("idl/api.thrift")
        );
    }

    #[test]
    fn monorepo_config_yaml_drives_discovery_over_root_go_mod() {
        // A Bazel/Go monorepo: one root go.mod, modules listed in
        // .monorepo_config.yaml. Without the registry short-circuit, discovery
        // would see a single go project for the whole repo.
        let dir = TempDir::new().unwrap();
        write(&dir.path().join("go.mod"), "module code.example/monorepo\n");
        write(
            &dir.path().join(".monorepo_config.yaml"),
            "repoName: monorepo\nmodules:\n  app/svc_a:\n    modulePath: app/svc_a\n    moduleName: svc_a\n    language: go\n    serviceType: rpc\n    isLibrary: false\n    psm: foo.bar.svc_a\n  shared/libfoo:\n    modulePath: shared/libfoo\n    moduleName: libfoo\n    language: go\n    serviceType: library\n    isLibrary: true\n",
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        assert_eq!(
            report.projects.len(),
            2,
            "should list the two modules, not one root go project"
        );
        // names are sanitized (`_` -> `-`); paths are preserved.
        let svc = &report.projects["svc-a"];
        assert_eq!(svc.project_type.as_deref(), Some("backend"));
        assert_eq!(svc.relative_path, "app/svc_a");
        assert_eq!(
            report.projects["libfoo"].project_type.as_deref(),
            Some("library")
        );
        assert!(svc
            .evidence
            .iter()
            .any(|e| e == "monorepo:.monorepo_config.yaml"));
        assert!(svc.evidence.iter().any(|e| e == "psm:foo.bar.svc_a"));
    }

    #[test]
    fn rush_json_jsonc_drives_frontend_discovery() {
        // rush.json is JSONC (block + line comments). Discovery must parse it
        // and emit one project per registered entry.
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("rush.json"),
            "/* main rush config */\n{\n  // engine\n  \"rushVersion\": \"5.0.0\",\n  \"projects\": [\n    { \"packageName\": \"@org/web_app\", \"projectFolder\": \"subspaces/ops/web_app\", \"subspaceName\": \"ops\" },\n    { \"packageName\": \"my-sdk\", \"projectFolder\": \"subspaces/ops/libs/sdk\", \"subspaceName\": \"ops\" }\n  ]\n}\n",
        );
        // a Rush repo runs via its bundled bootstrap (no global `rush`)
        write(
            &dir.path().join("common/scripts/install-run-rush.js"),
            "// rush bootstrap\n",
        );
        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        assert_eq!(report.projects.len(), 2);
        let web = &report.projects["org-web-app"];
        assert_eq!(web.project_type.as_deref(), Some("frontend"));
        assert_eq!(web.relative_path, "subspaces/ops/web_app");
        assert!(web.stack.iter().any(|s| s == "subspace:ops"));
        // check command uses the bootstrap, not a global `rush`
        assert!(
            web.commands["check"].contains("install-run-rush.js"),
            "check should use the rush bootstrap: {}",
            web.commands["check"]
        );
        assert_eq!(
            report.projects["my-sdk"].project_type.as_deref(),
            Some("library")
        );
    }

    #[test]
    fn grouping_dir_with_proto_subpackage_is_not_a_phantom_project() {
        // Regression (complex-scenario self-test C1): a grouping dir like
        // `packages/` that merely *contains* a sub-package literally named
        // `proto` must not become its own contract-only project — a
        // `package.json` inside a `proto`/`schema` dir is not an API contract.
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("packages/proto/package.json"),
            r#"{"name":"proto","scripts":{"check":"node check.mjs"}}"#,
        );
        write(
            &dir.path().join("packages/core/package.json"),
            r#"{"name":"core","scripts":{"check":"node check.mjs"}}"#,
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        assert!(
            !report.projects.contains_key("packages"),
            "grouping dir must not become a phantom project: {:?}",
            report.projects.keys().collect::<Vec<_>>()
        );
        assert!(report.projects.contains_key("proto"));
        assert!(report.projects.contains_key("core"));
        assert_eq!(report.projects.len(), 2);
    }

    #[test]
    fn infers_cross_project_edge_from_source_imports() {
        // web imports shared via a relative source import but does NOT declare
        // it in package.json — discovery should still infer the edge.
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("shared/package.json"),
            r#"{"name":"@acme/shared","scripts":{"test":"node --test"}}"#,
        );
        write(
            &dir.path().join("shared/types/index.d.ts"),
            "export interface User { id: string }\n",
        );
        write(
            &dir.path().join("web/package.json"),
            r#"{"name":"web","scripts":{"test":"vitest run"},"dependencies":{"react":"latest"}}"#,
        );
        write(
            &dir.path().join("web/src/profile.ts"),
            "import { User } from '../../shared/types';\nexport const f = (u: User) => u.id;\n",
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        let edge = report
            .edges
            .iter()
            .find(|e| e.from == "acme-shared" && e.to == "web")
            .expect("source-import edge shared→web");
        assert_eq!(edge.reason, "source import");
        assert!(edge.evidence.contains(&"source-import".to_string()));
        // And it shows up as a real dependency on the consumer project.
        assert!(report.projects["web"]
            .dependencies
            .contains(&"acme-shared".to_string()));
    }

    #[test]
    fn infers_cross_project_edge_from_python_relative_imports() {
        // api imports shared via a Python relative import (`from ..shared...`)
        // with no manifest dependency — discovery should still infer the edge.
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("shared/pyproject.toml"),
            "[project]\nname = \"shared\"\nversion = \"0.1.0\"\n",
        );
        write(
            &dir.path().join("shared/types.py"),
            "class User:\n    id: str\n",
        );
        write(
            &dir.path().join("api/pyproject.toml"),
            "[project]\nname = \"api\"\nversion = \"0.1.0\"\n",
        );
        write(
            &dir.path().join("api/service.py"),
            "from ..shared.types import User\n\ndef get_user():\n    return User()\n",
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 3 }).unwrap();
        let edge = report
            .edges
            .iter()
            .find(|e| e.from == "shared" && e.to == "api")
            .expect("source-import edge shared→api from python relative import");
        assert_eq!(edge.reason, "source import");
        assert!(report.projects["api"]
            .dependencies
            .contains(&"shared".to_string()));
    }

    #[test]
    fn renders_verify_plan_with_dependency_order() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("core/package.json"),
            r#"{"name":"core","scripts":{"test":"node --test"}}"#,
        );
        write(
            &dir.path().join("cli/package.json"),
            r#"{"name":"cli","scripts":{"test":"node --test"},"dependencies":{"core":"file:../core"}}"#,
        );
        let report = discover(dir.path(), &DiscoverOptions { max_depth: 2 }).unwrap();
        let plan = render_plan(&report);
        assert!(plan.contains("project: core"));
        assert!(plan.contains("project: cli"));
        assert!(plan.contains("depends_on: [verify_core]"));
    }

    #[test]
    fn discovers_pnpm_workspaces() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("pnpm-workspace.yaml"),
            "packages: ['packages/*', '!packages/ignore']\n",
        );
        write(
            &dir.path().join("packages/core/package.json"),
            r#"{"name":"@d/core","scripts":{"test":"vitest"}}"#,
        );
        write(
            &dir.path().join("packages/cli/package.json"),
            r#"{"name":"@d/cli","dependencies":{"@d/core":"workspace:*"},"scripts":{"test":"vitest"}}"#,
        );
        write(
            &dir.path().join("packages/ignore/package.json"),
            r#"{"name":"@d/ignore","scripts":{"test":"vitest"}}"#,
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 4 }).unwrap();
        assert_eq!(report.projects.len(), 2);
        assert!(report.projects.contains_key("d-core"));
        assert!(report.projects.contains_key("d-cli"));
        assert!(!report.projects.contains_key("d-ignore"));
        let edge = report
            .edges
            .iter()
            .find(|edge| edge.from == "d-core" && edge.to == "d-cli")
            .expect("workspace dependency edge");
        assert_eq!(edge.confidence, 95);
        assert!(edge.evidence.contains(&"workspace:@d/core".to_string()));
    }

    #[test]
    fn skips_hidden_directories_and_workspace_manifests_under_them() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            &dir.path().join("packages/app/package.json"),
            r#"{"name":"app","scripts":{"test":"vitest"}}"#,
        );
        write(
            &dir.path().join(".eden-mono/temp/package.json"),
            r#"{"name":"hidden-temp","workspaces":["../../infra"]}"#,
        );
        write(
            &dir.path().join(".eden-mono/temp/pnpm-workspace.yaml"),
            "packages:\n  - ../../infra\n",
        );
        write(
            &dir.path().join("infra/package.json"),
            r#"{"name":"infra","scripts":{"format":"prettier --check ."}}"#,
        );
        write(
            &dir.path().join(".hidden/package.json"),
            r#"{"name":"hidden-direct","scripts":{"test":"vitest"}}"#,
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 4 }).unwrap();

        assert!(report.projects.contains_key("root"));
        assert!(report.projects.contains_key("app"));
        assert!(report.projects.contains_key("infra"));
        assert!(!report.projects.contains_key("hidden-temp"));
        assert!(!report.projects.contains_key("hidden-direct"));
        assert_eq!(
            report
                .projects
                .values()
                .filter(
                    |project| project.path.file_name().and_then(|name| name.to_str())
                        == Some("infra")
                )
                .count(),
            1
        );
    }

    #[test]
    fn discover_respects_root_gitignore_for_project_dirs() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(".gitignore"), "maestro-demo-*/\n");
        write(
            &dir.path().join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(
            &dir.path().join("maestro-demo-scratch/Cargo.toml"),
            "[package]\nname = \"maestro-demo-scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 2 }).unwrap();

        assert!(report.projects.contains_key("app"));
        assert!(!report.projects.contains_key("maestro-demo-scratch"));
    }

    #[test]
    fn pnpm_workspace_directory_without_manifest_is_not_a_project() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*","packages/libs/*"]}"#,
        );
        std::fs::create_dir_all(dir.path().join("packages/libs")).unwrap();
        write(
            &dir.path().join("packages/libs/unity-plugin/package.json"),
            r#"{"name":"unity-plugin","scripts":{"build":"tsc"}}"#,
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 4 }).unwrap();

        assert!(report.projects.contains_key("root"));
        assert!(report.projects.contains_key("unity-plugin"));
        assert!(!report.projects.contains_key("libs"));
    }

    #[test]
    fn discovers_cargo_workspaces() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/ignore\"]\n",
        );
        write(
            &dir.path().join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(
            &dir.path().join("crates/helper/Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(
            &dir.path().join("crates/cli/Cargo.toml"),
            "[package]\nname = \"cli\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncore = { path = \"../core\" }\n\n[target.'cfg(unix)'.dev-dependencies]\nhelper = { path = \"../helper\" }\n",
        );
        write(
            &dir.path().join("crates/ignore/Cargo.toml"),
            "[package]\nname = \"ignore\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 1 }).unwrap();
        assert_eq!(report.projects.len(), 3);
        assert!(report.projects.contains_key("core"));
        assert!(report.projects.contains_key("helper"));
        assert!(report.projects.contains_key("cli"));
        assert!(!report.projects.contains_key("ignore"));
        assert!(report
            .edges
            .iter()
            .any(|edge| edge.from == "core" && edge.to == "cli" && edge.confidence == 95));
        assert!(report
            .edges
            .iter()
            .any(|edge| edge.from == "helper" && edge.to == "cli" && edge.confidence == 95));
    }

    #[test]
    fn discovers_gradle_multi_module_includes() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("settings.gradle.kts"),
            "pluginManagement {}\ninclude(\n  \":app\",\n  \":lib:core\"\n)\nproject(\":app\").projectDir = file(\"apps/android\")\n",
        );
        std::fs::create_dir_all(dir.path().join("apps/android")).unwrap();
        std::fs::create_dir_all(dir.path().join("lib/core")).unwrap();

        let report = discover(dir.path(), &DiscoverOptions { max_depth: 1 }).unwrap();
        assert_eq!(report.projects.len(), 2);
        assert!(report
            .projects
            .values()
            .any(|project| project.relative_path == "apps/android"));
        assert!(report
            .projects
            .values()
            .all(|project| project.stack.contains(&"gradle-multi".to_string())));
    }

    // ─── F-103 contract promotion tests ──────────────────────────────
    // Helper-level + integration coverage per the design doc §6.
    mod contracts_promotion {
        use super::*;

        fn discovered(name: &str, path: PathBuf, relative_path: &str) -> DiscoveredProject {
            DiscoveredProject {
                name: name.to_string(),
                path,
                relative_path: relative_path.to_string(),
                confidence: 100,
                project_type: None,
                stack: vec![],
                package_name: None,
                commands: BTreeMap::new(),
                provides: None,
                consumes: None,
                dependencies: vec![],
                markers: vec![],
                evidence: vec![],
            }
        }

        fn empty_report(root: &std::path::Path) -> DiscoveryReport {
            DiscoveryReport {
                root: root.to_path_buf(),
                projects: BTreeMap::new(),
                edges: vec![],
                warnings: vec![],
            }
        }

        // Provider: marker dir + at least one contract file is required.
        #[test]
        fn provider_promotion_requires_marker_dir_with_at_least_one_file() {
            let tmp = tempfile::tempdir().unwrap();
            // Project A: has idl/ but with NO contract file.
            let a = tmp.path().join("a");
            std::fs::create_dir_all(a.join("idl")).unwrap();
            // Project B: has idl/ + a .thrift file.
            let b = tmp.path().join("b");
            std::fs::create_dir_all(b.join("idl")).unwrap();
            std::fs::write(b.join("idl/service.thrift"), "").unwrap();

            let mut r = empty_report(tmp.path());
            r.projects.insert("a".into(), discovered("a", a, "a"));
            r.projects.insert("b".into(), discovered("b", b, "b"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 1, "only B has a contract file");
            assert!(r.projects["a"].provides.is_none(), "A has empty idl/");
            assert!(r.projects["b"].provides.is_some(), "B should promote");
        }

        // Provider: generated-client paths are NOT eligible as providers.
        // We put the bam-idl tree *under* a real marker dir (`contracts/`)
        // so the marker scan reaches it, and the guard is what stops the
        // promotion. Without the guard, `contracts/bam-idl/svc/service.thrift`
        // would otherwise be considered a contract file.
        #[test]
        fn provider_promotion_rejects_generated_client_dirs() {
            let tmp = tempfile::tempdir().unwrap();
            let c = tmp.path().join("c");
            std::fs::create_dir_all(c.join("contracts/bam-idl/svc")).unwrap();
            std::fs::write(c.join("contracts/bam-idl/svc/service.thrift"), "").unwrap();

            let mut r = empty_report(tmp.path());
            r.projects.insert("c".into(), discovered("c", c, "c"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 0);
            assert!(r.projects["c"].provides.is_none());
        }

        // Provider N1-1: `openapi/` with a plain `service.yaml` inside
        // should promote, and provides should point at the FILE.
        #[test]
        fn provider_promotion_detects_openapi_yaml_under_openapi_dir() {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("svc");
            std::fs::create_dir_all(p.join("openapi")).unwrap();
            std::fs::write(p.join("openapi/service.yaml"), "openapi: 3.0.0\n").unwrap();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("svc".into(), discovered("svc", p.clone(), "svc"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 1);
            let provides = r.projects["svc"].provides.as_deref().unwrap();
            assert!(
                provides == "openapi/service.yaml" || provides == "openapi\\service.yaml",
                "provides must be the FILE path, got `{provides}`",
            );
            assert!(std::fs::read_to_string(p.join(provides)).is_ok());
        }

        // Provider: canonical filename (openapi.yaml) is preferred over a
        // non-canonical sibling, even when both are under the same marker.
        #[test]
        fn provider_promotion_prefers_canonical_filename() {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("svc");
            std::fs::create_dir_all(p.join("openapi")).unwrap();
            std::fs::write(p.join("openapi/zzz-extra.yaml"), "").unwrap();
            std::fs::write(p.join("openapi/openapi.yaml"), "openapi: 3.0.0\n").unwrap();

            let mut r = empty_report(tmp.path());
            r.projects.insert("svc".into(), discovered("svc", p, "svc"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 1);
            let provides = r.projects["svc"].provides.as_deref().unwrap();
            assert!(
                provides == "openapi/openapi.yaml" || provides == "openapi\\openapi.yaml",
                "canonical openapi.yaml must win over non-canonical sibling, got `{provides}`",
            );
        }

        // Provider N1-1: depth-2 marker (`src/idl/...`) must still promote
        // and provides must be the FILE under it.
        #[test]
        fn provider_promotion_detects_marker_dir_at_depth_two() {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("svc");
            std::fs::create_dir_all(p.join("src/idl")).unwrap();
            std::fs::write(p.join("src/idl/api.thrift"), "service Api {}\n").unwrap();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("svc".into(), discovered("svc", p.clone(), "svc"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 1);
            let provides = r.projects["svc"].provides.as_deref().unwrap();
            assert!(
                provides == "src/idl/api.thrift" || provides == "src\\idl\\api.thrift",
                "expected the src/idl FILE, got `{provides}`",
            );
            assert!(std::fs::read_to_string(p.join(provides)).is_ok());
        }

        // Consumer N1-2: short producer names cannot match by `contains`
        // alone. `data` inside `Metadata` is the canonical false positive
        // we must NOT promote.
        #[test]
        fn consumer_promotion_does_not_match_short_provider_inside_longer_word() {
            let tmp = tempfile::tempdir().unwrap();
            // Producer `data` — short, single-word, has a real contract.
            let prod = tmp.path().join("data");
            std::fs::create_dir_all(prod.join("idl")).unwrap();
            std::fs::write(prod.join("idl/api.proto"), "").unwrap();
            // Consumer with a generated-client dir whose subdir name HAPPENS
            // to contain "data" as a substring of "Metadata".
            let cons = tmp.path().join("client");
            std::fs::create_dir_all(cons.join("src/bam-idl/Metadata")).unwrap();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("data".into(), discovered("data", prod, "data"));
            r.projects
                .insert("client".into(), discovered("client", cons, "client"));
            let (_, consumers) = promote_contracts(&mut r);
            assert_eq!(
                consumers, 0,
                "short single-word producer must not substring-match"
            );
            assert!(r.projects["client"].consumes.is_none());
        }

        // Consumer: needs a known producer that ALREADY has `provides` set.
        #[test]
        fn consumer_promotion_requires_known_producer_with_provides() {
            let tmp = tempfile::tempdir().unwrap();
            // Producer "alphasvc" — no idl, so it won't auto-promote.
            let prod = tmp.path().join("alphasvc");
            std::fs::create_dir_all(&prod).unwrap();
            // Consumer with a bam-idl/AlphaSvc/ subdir naming the producer.
            let cons = tmp.path().join("client");
            std::fs::create_dir_all(cons.join("src/bam-idl/AlphaSvc")).unwrap();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("alphasvc".into(), discovered("alphasvc", prod, "alphasvc"));
            r.projects
                .insert("client".into(), discovered("client", cons, "client"));
            let (_, consumers) = promote_contracts(&mut r);
            assert_eq!(consumers, 0, "producer has no provides — don't invent");
            assert!(r.projects["client"].consumes.is_none());
            // And no DiscoveredEdge should have been added either.
            assert!(r.edges.is_empty());
        }

        // Consumer N2: ambiguity is resolved by IN-TREE FILE COUNT, not
        // subdir count. Beta has 1 subdir vs Alpha's 1 subdir — but Beta's
        // tree contains many more files, so Beta should win.
        #[test]
        fn consumer_promotion_picks_heaviest_producer_on_ambiguity() {
            let tmp = tempfile::tempdir().unwrap();
            for p in ["alphasvc", "betasvc"] {
                let dir = tmp.path().join(p);
                std::fs::create_dir_all(dir.join("idl")).unwrap();
                std::fs::write(dir.join("idl/service.thrift"), "").unwrap();
            }
            let cons = tmp.path().join("client");
            // Alpha subtree: 1 file.
            std::fs::create_dir_all(cons.join("src/bam-idl/AlphaSvc")).unwrap();
            std::fs::write(cons.join("src/bam-idl/AlphaSvc/api.thrift"), "").unwrap();
            // Beta subtree: 5 files spread over nested dirs.
            std::fs::create_dir_all(cons.join("src/bam-idl/BetaSvc/sub")).unwrap();
            for f in [
                "a.thrift",
                "b.thrift",
                "c.thrift",
                "d.thrift",
                "sub/e.thrift",
            ] {
                std::fs::write(cons.join("src/bam-idl/BetaSvc").join(f), "").unwrap();
            }

            let mut r = empty_report(tmp.path());
            r.projects.insert(
                "alphasvc".into(),
                discovered("alphasvc", tmp.path().join("alphasvc"), "alphasvc"),
            );
            r.projects.insert(
                "betasvc".into(),
                discovered("betasvc", tmp.path().join("betasvc"), "betasvc"),
            );
            r.projects
                .insert("client".into(), discovered("client", cons, "client"));
            let (_, consumers) = promote_contracts(&mut r);
            assert_eq!(consumers, 1);
            let edge = r
                .edges
                .iter()
                .find(|e| e.to == "client")
                .expect("client got a contract edge");
            assert_eq!(edge.from, "betasvc", "heavier in-tree file count wins");
            assert_eq!(edge.reason, "generated-client contract");
            assert_eq!(edge.confidence, 90);
        }

        // Integration: provider with an idl/ dir gets contracts.provides set
        // to the contract FILE — not the marker dir — per dali's N1 fix.
        // Downstream code (scheduler::consumed_contract_section) reads this
        // path with read_to_string, so a directory would break.
        #[test]
        fn discover_promotes_provider_when_idl_dir_present() {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("svc");
            std::fs::create_dir_all(p.join("idl")).unwrap();
            std::fs::write(p.join("idl/api.proto"), "syntax = \"proto3\";\n").unwrap();
            let p_str = p.to_string_lossy().to_string();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("svc".into(), discovered("svc", p.clone(), "svc"));
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(providers, 1);
            let provides = r.projects["svc"].provides.as_deref().unwrap();
            // PROJECT-RELATIVE, not workspace-relative. Hard rule per design §3.1.
            assert!(
                !provides.contains(p_str.as_str()),
                "provides must be project-relative, got `{provides}`",
            );
            assert!(
                provides == "idl/api.proto" || provides == "idl\\api.proto",
                "provides must be the FILE path, got `{provides}`",
            );
            // Round-trip: provider_root.join(provides) is a readable file.
            assert!(
                std::fs::read_to_string(p.join(provides)).is_ok(),
                "provides must resolve to a real readable file",
            );
        }

        // Integration: don't synthesize a consumer when the producer
        // wasn't promoted.
        #[test]
        fn discover_does_not_promote_consumer_when_producer_has_no_provides() {
            let tmp = tempfile::tempdir().unwrap();
            // Producer exists but has no contract file — provider pass skips it.
            let prod = tmp.path().join("alphasvc");
            std::fs::create_dir_all(&prod).unwrap();
            let cons = tmp.path().join("client");
            std::fs::create_dir_all(cons.join("src/bam-idl/AlphaSvc")).unwrap();

            let mut r = empty_report(tmp.path());
            r.projects
                .insert("alphasvc".into(), discovered("alphasvc", prod, "alphasvc"));
            r.projects
                .insert("client".into(), discovered("client", cons, "client"));
            let (providers, consumers) = promote_contracts(&mut r);
            assert_eq!(providers, 0);
            assert_eq!(consumers, 0);
            assert!(r.projects["client"].consumes.is_none());
        }

        // Consumer integration N1: the promoted `consumes` path, resolved
        // against the consumer project root, must point at the producer's
        // real contract file — i.e. read_to_string must succeed. This is
        // what scheduler::consumed_contract_section does at runtime.
        #[test]
        fn discover_promotes_consumer_to_readable_producer_contract_file() {
            let tmp = tempfile::tempdir().unwrap();
            // Producer billing-platform-foundation with a real openapi.yaml.
            // Long name (>=7 chars normalized) so consumer matching allows
            // the contains rule even without exact match.
            let prod = tmp.path().join("billing-platform-foundation");
            std::fs::create_dir_all(prod.join("openapi")).unwrap();
            std::fs::write(
                prod.join("openapi/openapi.yaml"),
                "openapi: 3.0.0\ninfo: { title: foundation, version: 1 }\n",
            )
            .unwrap();
            // Consumer with a generated-client subdir naming the producer.
            let cons = tmp.path().join("client");
            std::fs::create_dir_all(cons.join("src/bam-idl/BillingPlatformFoundation")).unwrap();

            let mut r = empty_report(tmp.path());
            r.projects.insert(
                "billing-platform-foundation".into(),
                discovered(
                    "billing-platform-foundation",
                    prod.clone(),
                    "billing-platform-foundation",
                ),
            );
            r.projects.insert(
                "client".into(),
                discovered("client", cons.clone(), "client"),
            );
            let (providers, consumers) = promote_contracts(&mut r);
            assert_eq!(providers, 1);
            assert_eq!(consumers, 1);

            let consumes = r.projects["client"].consumes.as_deref().unwrap();
            let resolved = cons.join(consumes);
            assert!(
                std::fs::read_to_string(&resolved).is_ok(),
                "consumes must resolve to a real readable file (got `{consumes}` → `{}`)",
                resolved.display(),
            );

            // Edge evidence carries the producer contract file too (§5).
            let edge = r
                .edges
                .iter()
                .find(|e| e.to == "client")
                .expect("client edge");
            assert!(
                edge.evidence
                    .iter()
                    .any(|s| s.contains("openapi/openapi.yaml")
                        || s.contains("openapi\\openapi.yaml")),
                "edge evidence must mention the producer contract file, got {:?}",
                edge.evidence,
            );
        }

        // Integration: user-set contracts MUST survive promotion.
        #[test]
        fn discover_preserves_user_set_contracts_through_promotion() {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("svc");
            std::fs::create_dir_all(p.join("idl")).unwrap();
            std::fs::write(p.join("idl/api.proto"), "").unwrap();

            let mut r = empty_report(tmp.path());
            let mut proj = discovered("svc", p, "svc");
            // User already declared a contract — promotion must NOT clobber it.
            proj.provides = Some("manual/path.thrift".to_string());
            r.projects.insert("svc".into(), proj);
            let (providers, _) = promote_contracts(&mut r);
            assert_eq!(
                providers, 0,
                "user-set provides must short-circuit promotion"
            );
            assert_eq!(
                r.projects["svc"].provides.as_deref(),
                Some("manual/path.thrift"),
            );
        }

        // ── F-109: cross-workspace contract providers ──────────────────
        // A sibling workspace root with one provider registered (its own
        // .maestro/projects.yaml has contracts.provides set) + the contract
        // file on disk under <root>/<proj>/<contract_rel>.
        fn make_sibling(parent: &Path, repo: &str, proj: &str, contract_rel: &str) -> PathBuf {
            let root = parent.join(repo);
            let proj_dir = root.join(proj);
            if let Some(p) = Path::new(contract_rel).parent() {
                std::fs::create_dir_all(proj_dir.join(p)).unwrap();
            }
            std::fs::write(proj_dir.join(contract_rel), "syntax=\"proto3\";\n").unwrap();
            std::fs::create_dir_all(root.join(".maestro")).unwrap();
            std::fs::write(
                root.join(".maestro/projects.yaml"),
                format!(
                    "version: 1\nprojects:\n  {proj}:\n    path: {proj}\n    contracts:\n      provides: {contract_rel}\n"
                ),
            )
            .unwrap();
            root
        }

        // A consumer project at <parent>/<repo>/<name> with a generated-client
        // subdir naming `producer` (PascalCase form, so the F-103 normalized
        // match links it). Returns the consumer project dir.
        fn make_consumer(parent: &Path, repo: &str, name: &str, producer_subdir: &str) -> PathBuf {
            let dir = parent.join(repo).join(name);
            std::fs::create_dir_all(dir.join("src/bam-idl").join(producer_subdir)).unwrap();
            dir
        }

        #[test]
        fn sibling_provider_links_cross_workspace_consumer() {
            let tmp = tempfile::tempdir().unwrap();
            let sib = make_sibling(tmp.path(), "backend", "shared-lib", "idl/user.proto");
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");

            let mut r = empty_report(&tmp.path().join("frontend"));
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            let (_, consumers) = promote_contracts_with_siblings(&mut r, &[sib]);
            assert_eq!(consumers, 1, "sibling producer should link the consumer");
            let consumes = r.projects["client"].consumes.as_deref().unwrap();
            assert!(
                !Path::new(consumes).is_absolute(),
                "consumes must be relative, got `{consumes}`"
            );
            assert!(consumes.contains("user.proto"), "got `{consumes}`");
            let edge = r.edges.iter().find(|e| e.to == "client").unwrap();
            assert_eq!(edge.reason, "cross-workspace generated-client contract");
        }

        #[test]
        fn cross_workspace_consumes_resolves_to_sibling_contract_file() {
            let tmp = tempfile::tempdir().unwrap();
            let sib = make_sibling(tmp.path(), "backend", "shared-lib", "idl/user.proto");
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");

            let mut r = empty_report(&tmp.path().join("frontend"));
            r.projects.insert(
                "client".into(),
                discovered("client", cdir.clone(), "client"),
            );
            promote_contracts_with_siblings(&mut r, &[sib]);
            let consumes = r.projects["client"].consumes.as_deref().unwrap();
            // Resolve the relative consumes against the consumer dir → must hit
            // the sibling's real contract file (the runtime read path).
            let resolved = cdir.join(consumes);
            assert!(
                std::fs::read_to_string(&resolved).is_ok(),
                "consumes must resolve to a readable sibling contract: {consumes} → {}",
                resolved.display()
            );
        }

        #[test]
        fn local_provider_wins_over_sibling_of_same_name() {
            let tmp = tempfile::tempdir().unwrap();
            // Sibling ALSO has a `shared-lib` provider, but a local one exists.
            let sib = make_sibling(tmp.path(), "backend", "shared-lib", "idl/user.proto");
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");
            // Local provider `shared-lib` in the consumer's own workspace.
            let local = tmp.path().join("frontend").join("shared-lib");
            std::fs::create_dir_all(local.join("idl")).unwrap();
            std::fs::write(local.join("idl/local.proto"), "x\n").unwrap();

            let mut r = empty_report(&tmp.path().join("frontend"));
            let mut lp = discovered("shared-lib", local, "shared-lib");
            lp.provides = Some("idl/local.proto".into());
            r.projects.insert("shared-lib".into(), lp);
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            promote_contracts_with_siblings(&mut r, &[sib]);
            let edge = r.edges.iter().find(|e| e.to == "client").unwrap();
            assert_eq!(
                edge.reason, "generated-client contract",
                "local producer must win — not the cross-workspace sibling"
            );
        }

        #[test]
        fn duplicate_sibling_provider_first_declared_wins() {
            let tmp = tempfile::tempdir().unwrap();
            // Two siblings each register a `shared-lib`; first-declared wins.
            let sib_a = make_sibling(tmp.path(), "a", "shared-lib", "idl/a.proto");
            let sib_b = make_sibling(tmp.path(), "b", "shared-lib", "idl/b.proto");
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");

            let mut r = empty_report(&tmp.path().join("frontend"));
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            promote_contracts_with_siblings(&mut r, &[sib_a, sib_b]);
            let consumes = r.projects["client"].consumes.as_deref().unwrap();
            assert!(
                consumes.contains("a.proto") && !consumes.contains("b.proto"),
                "first-declared sibling (a) must win deterministically, got `{consumes}`"
            );
        }

        #[test]
        fn empty_sibling_workspaces_is_unchanged() {
            // No siblings → identical to single-root promotion (the consumer
            // has no in-tree producer, so nothing links).
            let tmp = tempfile::tempdir().unwrap();
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");
            let mut r = empty_report(&tmp.path().join("frontend"));
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            let (_, consumers) = promote_contracts_with_siblings(&mut r, &[]);
            assert_eq!(consumers, 0);
            assert!(r.projects["client"].consumes.is_none());
        }

        #[test]
        fn sibling_relative_paths_resolve_against_workspace_root_not_scan_root() {
            // dali N1: a relative `sibling_workspaces` entry must resolve
            // against the WORKSPACE root (where projects.yaml lives), NOT a
            // scan subdir. Here the workspace root holds the config and the
            // sibling lives at <workspace>/siblings/backend; resolving against
            // a scan subdir like <workspace>/frontend would miss it.
            let ws = Path::new("/ws");
            let resolved = resolve_sibling_roots(ws, &["siblings/backend".to_string()]);
            assert_eq!(resolved, vec![PathBuf::from("/ws/siblings/backend")]);
            // absolute entries are kept verbatim regardless of base.
            let abs = resolve_sibling_roots(ws, &["/elsewhere/repo".to_string()]);
            assert_eq!(abs, vec![PathBuf::from("/elsewhere/repo")]);
        }

        #[test]
        fn sibling_relative_link_works_when_workspace_root_differs_from_scan_root() {
            // End-to-end at the promotion level: the workspace root holds the
            // consumer under a subdir (frontend/), the sibling provider is at
            // <workspace>/siblings/backend, declared relative. Resolving via
            // resolve_sibling_roots(workspace_root, ...) must still link it —
            // proving the fix isn't just the unit string-join.
            let tmp = tempfile::tempdir().unwrap();
            let ws = tmp.path();
            let sib = make_sibling(
                &ws.join("siblings"),
                "backend",
                "shared-lib",
                "idl/user.proto",
            );
            // consumer lives under frontend/ (the eventual --root <subdir>)
            let cdir = make_consumer(&ws.join("frontend"), "apps", "client", "SharedLib");

            let resolved = resolve_sibling_roots(ws, &["siblings/backend".to_string()]);
            assert_eq!(resolved, vec![sib]);

            let mut r = empty_report(&ws.join("frontend"));
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            let (_, consumers) = promote_contracts_with_siblings(&mut r, &resolved);
            assert_eq!(
                consumers, 1,
                "sibling resolved via workspace root must link"
            );
        }

        #[test]
        fn missing_sibling_root_is_skipped_not_fatal() {
            let tmp = tempfile::tempdir().unwrap();
            let cdir = make_consumer(tmp.path(), "frontend", "client", "SharedLib");
            let mut r = empty_report(&tmp.path().join("frontend"));
            r.projects
                .insert("client".into(), discovered("client", cdir, "client"));
            // Sibling root that doesn't exist — must not panic / error.
            let (_, consumers) =
                promote_contracts_with_siblings(&mut r, &[tmp.path().join("does-not-exist")]);
            assert_eq!(consumers, 0);
        }
    }
}
