//! A small knowledge graph derived from L2 decision memory.
//!
//! The Memory tab's list view answers "what do I remember?"; this answers
//! "how is it connected?". We read every archived L2 decision
//! (`memory/l2_decisions/<project>/*.md`), and emit two kinds of nodes:
//!
//!   - **project** — one per project that has any decision, sized by how many
//!     decisions it accumulated.
//!   - **run** — one per `run_id` found across decisions, labeled by its spec.
//!
//! A `produced` edge connects each run to every project it touched, so a run
//! that changed three projects together visibly stitches them. On top of that
//! we overlay each project's declared contract dependency (`consumes` →
//! `provides`) as a `consumes` edge, surfacing the structural coupling the
//! scheduler already uses for L2 fan-in.

use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::config::ProjectsConfig;

#[derive(Serialize, Default)]
pub struct MemoryGraph {
    pub nodes: Vec<MemNode>,
    pub edges: Vec<MemEdge>,
}

#[derive(Serialize)]
pub struct MemNode {
    /// Stable id: `project:<name>` or `run:<run_id>`.
    pub id: String,
    /// `"project"` | `"run"`.
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// project: number of decisions; run: number of projects touched.
    pub weight: u32,
}

#[derive(Serialize)]
pub struct MemEdge {
    pub source: String,
    pub target: String,
    /// `"produced"` (run→project) | `"consumes"` (project→project).
    pub kind: String,
}

/// Accumulator for one run: `(spec, status, projects it touched)`.
type RunAcc = (Option<String>, Option<String>, Vec<String>);

/// Minimal frontmatter we care about from one decision file.
struct Frontmatter {
    run_id: Option<String>,
    spec: Option<String>,
    status: Option<String>,
}

/// Parse the leading `--- ... ---` YAML block by hand. The corpus is tiny and
/// the keys are flat strings, so a full YAML parse isn't worth the dep here.
fn parse_frontmatter(content: &str) -> Frontmatter {
    let mut fm = Frontmatter {
        run_id: None,
        spec: None,
        status: None,
    };
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return fm;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim().trim_matches('"').to_string();
        match key.trim() {
            "run_id" => fm.run_id = Some(val),
            "spec" => fm.spec = Some(val),
            "status" => fm.status = Some(val),
            _ => {}
        }
    }
    fm
}

fn short(spec: &str, max: usize) -> String {
    let s = spec.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{}…", truncated.trim_end())
}

// ─── Star map (decision-level constellation view) ──────────────────────
//
// Where `MemoryGraph` is a 2-level project+run graph for a hierarchical
// layout, the star map is decision-grained for an organic force layout:
//   - star        = one L2 decision (the atom of memory)
//   - project     = a star system (cluster); stars pull toward its centroid
//   - constellation = a run; lines stitch the stars it produced ACROSS
//                     projects — the visual of "different projects' memory
//                     assembled together"
//   - contract    = gravity; provides→consumes pulls coupled systems close

#[derive(Serialize)]
pub struct StarMap {
    pub stars: Vec<Star>,
    pub projects: Vec<StarProject>,
    pub runs: Vec<Constellation>,
    pub contracts: Vec<ContractLink>,
}

#[derive(Serialize)]
pub struct Star {
    /// Stable id: `<project>/<filename>`.
    pub id: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    /// 0..1, 1 = newest. Drives star brightness.
    pub recency: f32,
    /// How many projects the producing run touched — drives star size.
    pub blast: u32,
}

#[derive(Serialize)]
pub struct StarProject {
    pub name: String,
    pub stars: u32,
}

#[derive(Serialize)]
pub struct Constellation {
    pub run_id: String,
    pub spec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Star ids this run produced, across all projects it touched.
    pub star_ids: Vec<String>,
}

#[derive(Serialize)]
pub struct ContractLink {
    pub from: String,
    pub to: String,
}

/// Extract a sortable timestamp from an L2 decision filename. Files look like
/// `2026-05-27-<slug>-20260527-080531.md`; prefer the trailing run stamp,
/// else fall back to the leading `YYYY-MM-DD` date.
fn ts_from_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".md").unwrap_or(name);
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() >= 2 {
        let (d, t) = (parts[parts.len() - 2], parts[parts.len() - 1]);
        if d.len() == 8 && t.len() == 6 && d.bytes().chain(t.bytes()).all(|b| b.is_ascii_digit()) {
            return Some(format!("{d}-{t}"));
        }
    }
    if parts.len() >= 3 && parts[0].len() == 4 {
        return Some(format!("{}-{}-{}", parts[0], parts[1], parts[2]));
    }
    None
}

/// Lexically resolve `rel` against `base` (a project dir), collapsing `.`
/// and `..`, into a normalized slash path — for comparing a consumer's
/// contract path with a provider's across different project dirs.
fn norm_join(base: &str, rel: &str) -> String {
    let mut comps: Vec<&str> = Vec::new();
    for part in base.split('/').chain(rel.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                comps.pop();
            }
            p => comps.push(p),
        }
    }
    comps.join("/")
}

/// Numeric key for recency ranking (digits only, zero-padded to 14).
fn ts_key(ts: &str) -> u64 {
    let mut digits: String = ts.chars().filter(|c| c.is_ascii_digit()).collect();
    while digits.len() < 14 {
        digits.push('0');
    }
    digits
        .get(..14)
        .unwrap_or(digits.as_str())
        .parse()
        .unwrap_or(0)
}

/// Build a decision-level star map from the L2 decisions under `l2_root`.
pub fn build_starmap(l2_root: &std::path::Path, cfg: Option<&ProjectsConfig>) -> Result<StarMap> {
    struct Raw {
        id: String,
        project: String,
        run_id: Option<String>,
        title: String,
        status: Option<String>,
        ts: Option<String>,
    }
    // run_id -> (spec, status, star_ids, distinct projects touched)
    type RunStars = (Option<String>, Option<String>, Vec<String>, Vec<String>);
    let mut raw: Vec<Raw> = Vec::new();
    let mut project_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut runs: BTreeMap<String, RunStars> = BTreeMap::new();

    if l2_root.is_dir() {
        for proj_entry in std::fs::read_dir(l2_root)?.flatten() {
            if !proj_entry.path().is_dir() {
                continue;
            }
            let project = proj_entry.file_name().to_string_lossy().to_string();
            for file in std::fs::read_dir(proj_entry.path())?.flatten() {
                let p = file.path();
                if !p.is_file() || p.extension().is_none_or(|e| e != "md") {
                    continue;
                }
                let fname = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let Ok(content) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let fm = parse_frontmatter(&content);
                let id = format!("{project}/{fname}");
                let ts = ts_from_filename(&fname);
                let title = fm
                    .spec
                    .as_deref()
                    .map(|s| short(s, 48))
                    .unwrap_or_else(|| project.clone());
                *project_counts.entry(project.clone()).or_insert(0) += 1;
                if let Some(rid) = &fm.run_id {
                    let e = runs.entry(rid.clone()).or_insert_with(|| {
                        (fm.spec.clone(), fm.status.clone(), Vec::new(), Vec::new())
                    });
                    e.2.push(id.clone());
                    if !e.3.contains(&project) {
                        e.3.push(project.clone());
                    }
                }
                raw.push(Raw {
                    id,
                    project: project.clone(),
                    run_id: fm.run_id,
                    title,
                    status: fm.status,
                    ts,
                });
            }
        }
    }

    // Recency ranking across all stars.
    let keys: Vec<u64> = raw
        .iter()
        .filter_map(|r| r.ts.as_deref().map(ts_key))
        .collect();
    let (min_k, max_k) = (
        keys.iter().copied().min().unwrap_or(0),
        keys.iter().copied().max().unwrap_or(0),
    );
    let span = max_k.saturating_sub(min_k);
    let blast_of = |rid: &Option<String>| -> u32 {
        rid.as_ref()
            .and_then(|r| runs.get(r))
            .map(|e| e.3.len() as u32)
            .unwrap_or(1)
    };

    let stars: Vec<Star> = raw
        .iter()
        .map(|r| {
            let recency = match (&r.ts, span) {
                (Some(ts), s) if s > 0 => 0.4 + 0.6 * ((ts_key(ts) - min_k) as f32 / s as f32),
                _ => 1.0,
            };
            Star {
                id: r.id.clone(),
                project: r.project.clone(),
                run_id: r.run_id.clone(),
                title: r.title.clone(),
                status: r.status.clone(),
                ts: r.ts.clone(),
                recency,
                blast: blast_of(&r.run_id),
            }
        })
        .collect();

    let projects: Vec<StarProject> = project_counts
        .into_iter()
        .map(|(name, stars)| StarProject { name, stars })
        .collect();

    let constellations: Vec<Constellation> = runs
        .into_iter()
        .map(|(run_id, (spec, status, star_ids, _))| Constellation {
            run_id: run_id.clone(),
            spec: spec.map(|s| short(&s, 60)).unwrap_or(run_id),
            status,
            star_ids,
        })
        .collect();

    // Contract gravity: consumer→provider. Resolve each side's contract path
    // against its own project dir to an absolute key, so a consumer linking
    // `../proto/src/types.js` matches the provider's `src/types.js` exactly —
    // without the false positives a basename match would give when several
    // projects share a conventional name like `src/index.js`.
    let mut contracts: Vec<ContractLink> = Vec::new();
    if let Some(cfg) = cfg {
        let mut provider_of: BTreeMap<String, &str> = BTreeMap::new();
        for (name, p) in &cfg.projects {
            if let Some(provides) = p.contracts.provides.as_deref() {
                provider_of.insert(norm_join(&p.path, provides), name.as_str());
            }
        }
        for (name, p) in &cfg.projects {
            if let Some(consumes) = p.contracts.consumes.as_deref() {
                if let Some(provider) = provider_of.get(&norm_join(&p.path, consumes)) {
                    if provider != &name.as_str() {
                        contracts.push(ContractLink {
                            from: name.clone(),
                            to: provider.to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(StarMap {
        stars,
        projects,
        runs: constellations,
        contracts,
    })
}

/// Build the graph from the L2 decisions under `l2_root`, overlaying contract
/// edges from `cfg` when provided.
pub fn build_from(l2_root: &std::path::Path, cfg: Option<&ProjectsConfig>) -> Result<MemoryGraph> {
    // project -> decision count
    let mut project_decisions: BTreeMap<String, u32> = BTreeMap::new();
    // run_id -> (spec, status, set of projects)
    let mut runs: BTreeMap<String, RunAcc> = BTreeMap::new();

    if l2_root.is_dir() {
        for proj_entry in std::fs::read_dir(l2_root)?.flatten() {
            if !proj_entry.path().is_dir() {
                continue;
            }
            let project = proj_entry.file_name().to_string_lossy().to_string();
            for file in std::fs::read_dir(proj_entry.path())?.flatten() {
                let p = file.path();
                if !p.is_file() || p.extension().is_none_or(|e| e != "md") {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&p) else {
                    continue;
                };
                *project_decisions.entry(project.clone()).or_insert(0) += 1;
                let fm = parse_frontmatter(&content);
                if let Some(rid) = fm.run_id {
                    let entry = runs.entry(rid).or_insert((fm.spec, fm.status, Vec::new()));
                    // first spec/status wins; keep the per-run project list unique
                    if !entry.2.contains(&project) {
                        entry.2.push(project.clone());
                    }
                }
            }
        }
    }

    let mut nodes: Vec<MemNode> = Vec::new();
    let mut edges: Vec<MemEdge> = Vec::new();

    for (project, count) in &project_decisions {
        nodes.push(MemNode {
            id: format!("project:{project}"),
            kind: "project".into(),
            label: project.clone(),
            status: None,
            run_id: None,
            weight: *count,
        });
    }

    for (rid, (spec, status, projects)) in &runs {
        let label = spec
            .as_deref()
            .map(|s| short(s, 40))
            .unwrap_or_else(|| rid.clone());
        nodes.push(MemNode {
            id: format!("run:{rid}"),
            kind: "run".into(),
            label,
            status: status.clone(),
            run_id: Some(rid.clone()),
            weight: projects.len() as u32,
        });
        for project in projects {
            // Only link to projects we actually emitted as nodes.
            if project_decisions.contains_key(project) {
                edges.push(MemEdge {
                    source: format!("run:{rid}"),
                    target: format!("project:{project}"),
                    kind: "produced".into(),
                });
            }
        }
    }

    // Contract overlay: consumer consumes the contract some other project
    // provides. Only draw edges between projects that have memory nodes.
    if let Some(cfg) = cfg {
        let mut provider_of: BTreeMap<&str, &str> = BTreeMap::new();
        for (name, p) in &cfg.projects {
            if let Some(provides) = p.contracts.provides.as_deref() {
                provider_of.insert(provides, name.as_str());
            }
        }
        for (name, p) in &cfg.projects {
            let Some(consumes) = p.contracts.consumes.as_deref() else {
                continue;
            };
            let Some(provider) = provider_of.get(consumes) else {
                continue;
            };
            if project_decisions.contains_key(name) && project_decisions.contains_key(*provider) {
                edges.push(MemEdge {
                    source: format!("project:{name}"),
                    target: format!("project:{provider}"),
                    kind: "consumes".into(),
                });
            }
        }
    }

    Ok(MemoryGraph { nodes, edges })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_links_runs_to_projects() {
        let dir = tempfile::tempdir().unwrap();
        let l2 = dir.path();
        for proj in ["demo-core", "demo-cli"] {
            let pd = l2.join(proj);
            std::fs::create_dir_all(&pd).unwrap();
            std::fs::write(
                pd.join("2026-05-25-x-run1.md"),
                "---\nrun_id: run1\nspec: \"shared upgrade\"\nstatus: Done\n---\nbody",
            )
            .unwrap();
        }
        let g = build_from(l2, None).unwrap();
        // two project nodes + one run node
        assert_eq!(g.nodes.iter().filter(|n| n.kind == "project").count(), 2);
        assert_eq!(g.nodes.iter().filter(|n| n.kind == "run").count(), 1);
        // run touched both projects → two produced edges
        assert_eq!(g.edges.iter().filter(|e| e.kind == "produced").count(), 2);
        let run = g.nodes.iter().find(|n| n.kind == "run").unwrap();
        assert_eq!(run.label, "shared upgrade");
        assert_eq!(run.weight, 2);
    }

    #[test]
    fn empty_when_no_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let g = build_from(dir.path(), None).unwrap();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }
}
