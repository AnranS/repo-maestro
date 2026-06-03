//! Deliberation: turn a requirements document into a coordinated decomposition
//! by having each project "speak for itself" before the plan is built.
//!
//! Today's `maestro work` synthesizes a contract-ordered DAG in one shot. The
//! deliberation phase makes that *explainable and auditable*: each in-scope
//! project contributes a **position** (what it will change, the contracts it
//! provides/consumes, who it depends on, and its concerns), the positions are
//! cross-checked for contract conflicts, and the resolved ordering becomes the
//! DAG. v1 derives positions deterministically from the workspace's contract
//! graph + the requirements text — the "knows the scheduling chain" part — and
//! is the seam where an LLM agent can later refine each position.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::analyze::producer_projects_for;
use crate::config::projects::ProjectsConfig;

/// One project's stance on a requirement: its slice of the work and how it
/// relates to the rest of the workspace.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Position {
    pub project: String,
    /// foundational (provides only) · top-level (consumes only) · bridge
    /// (both) · leaf (neither). Drives ordering and framing.
    pub role: String,
    pub provides: Option<String>,
    pub consumes: Option<String>,
    /// Producer projects this one must wait for (resolved from contracts).
    pub depends_on: Vec<String>,
    pub summary: String,
    pub concerns: Vec<String>,
}

/// A contract one project changes that others consume — the cross-team seam the
/// deliberation surfaces so the plan can order producer before consumers.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Conflict {
    pub contract: String,
    pub producer: String,
    pub consumers: Vec<String>,
    pub note: String,
    /// True when a consumer is in scope and so must be re-verified against the
    /// new contract — the case a human most wants to see.
    pub needs_attention: bool,
}

/// The outcome of a deliberation: every position, the contract conflicts found,
/// and the topologically-resolved execution order.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct DeliberationReport {
    pub spec: String,
    pub positions: Vec<Position>,
    pub conflicts: Vec<Conflict>,
    pub order: Vec<String>,
}

/// A project's role from its dependency edges: depended-on but depends on
/// nothing ⇒ foundational; depends on others but nothing depends on it ⇒
/// top-level; both ⇒ bridge; neither ⇒ leaf. Edges may be contracts or
/// code-graph imports — the shape is what matters.
fn role_of(has_dependents: bool, has_dependencies: bool) -> &'static str {
    match (has_dependents, has_dependencies) {
        (true, false) => "foundational",
        (false, true) => "top-level",
        (true, true) => "bridge",
        (false, false) => "leaf",
    }
}

/// A short headline for the requirement — used to phrase each project's
/// position against the actual ask. Prefers the document's first heading (its
/// title), falling back to the first real line; strips a leading
/// "Requirement:" / "需求:" label and caps the length.
fn headline(spec: &str) -> String {
    let from_heading = spec
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim());
    let from_line = spec
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.trim_start_matches(['-', '*', ' ']).trim());
    let raw = from_heading.or(from_line).unwrap_or("");
    let raw = raw
        .strip_prefix("Requirement:")
        .or_else(|| raw.strip_prefix("Requirement"))
        .or_else(|| raw.strip_prefix("需求:"))
        .or_else(|| raw.strip_prefix("需求"))
        .unwrap_or(raw)
        .trim_start_matches([':', '：'])
        .trim();
    if raw.chars().count() > 100 {
        raw.chars().take(100).collect::<String>() + "…"
    } else {
        raw.to_string()
    }
}

/// Run the deliberation over the in-scope projects. Deterministic: positions and
/// conflicts come from the contract graph + the requirements text.
pub fn deliberate(
    spec: &str,
    projects: &ProjectsConfig,
    scope: &BTreeSet<String>,
    derived: &BTreeMap<String, BTreeSet<String>>,
) -> DeliberationReport {
    let ask = headline(spec);
    let mut positions = Vec::new();

    // All producers a project waits on — its contract producers PLUS the
    // modules it imports per the code graph (`derived`). The latter is what
    // gives a monorepo (no contracts crossing) a real ordering instead of a
    // flat "everyone independent" deliberation.
    let all_producers = |name: &str| -> Vec<String> {
        let mut deps = producer_projects_for(projects, name);
        if let Some(d) = derived.get(name) {
            deps.extend(d.iter().cloned());
        }
        deps.sort();
        deps.dedup();
        deps
    };

    for name in scope {
        let Some(p) = projects.projects.get(name) else {
            continue;
        };
        let provides = p.contracts.provides.as_deref().map(str::to_string);
        let consumes = p.contracts.consumes.as_deref().map(str::to_string);
        // Producers this project waits on, restricted to the deliberation scope.
        let depends_on: Vec<String> = all_producers(name)
            .into_iter()
            .filter(|d| scope.contains(d))
            .collect();
        // Who in scope depends on this project (so it ships first).
        let downstream: Vec<String> = scope
            .iter()
            .filter(|other| other.as_str() != name)
            .filter(|other| all_producers(other).iter().any(|d| d == name))
            .cloned()
            .collect();
        // Structural role from the actual dependency edges — works whether the
        // edges come from declared contracts or code-graph imports.
        let role = role_of(!downstream.is_empty(), !depends_on.is_empty()).to_string();

        let mut concerns = Vec::new();
        if !depends_on.is_empty() {
            let via = match &consumes {
                Some(c) => format!("consumes `{c}` and imports them — "),
                None => "imports them — ".to_string(),
            };
            concerns.push(format!(
                "{via}must land after {} so it builds against their changes",
                depends_on.join(", ")
            ));
        }
        if !downstream.is_empty() {
            let what = provides
                .as_ref()
                .map(|pv| format!("provides `{pv}`; "))
                .unwrap_or_default();
            concerns.push(format!(
                "{what}changes ripple to {}; coordinate before merging",
                downstream.join(", ")
            ));
        }

        let summary = match role.as_str() {
            "foundational" => format!("nothing it depends on in scope; lands first so {} build against \"{ask}\"", downstream.join(", ")),
            "top-level" => format!("implements \"{ask}\" once its producers ({}) land", depends_on.join(", ")),
            "bridge" => format!("both depends on and is depended on; threads \"{ask}\" through the middle of the chain"),
            _ => format!("implements its slice of \"{ask}\" independently"),
        };

        positions.push(Position {
            project: name.clone(),
            role,
            provides,
            consumes,
            depends_on,
            summary,
            concerns,
        });
    }

    // Conflicts: each in-scope producer whose contract is consumed by others.
    let mut conflicts = Vec::new();
    for pos in &positions {
        let Some(contract) = &pos.provides else {
            continue;
        };
        let consumers: Vec<String> = positions
            .iter()
            .filter(|c| c.depends_on.contains(&pos.project))
            .map(|c| c.project.clone())
            .collect();
        if !consumers.is_empty() {
            conflicts.push(Conflict {
                contract: contract.clone(),
                producer: pos.project.clone(),
                note: format!(
                    "`{}` changes `{contract}`; {} consume it and are ordered after it",
                    pos.project,
                    consumers.join(", ")
                ),
                needs_attention: true,
                consumers,
            });
        }
    }

    let order = topo_order(&positions);
    DeliberationReport {
        spec: spec.to_string(),
        positions,
        conflicts,
        order,
    }
}

/// Topological order of the positions by their `depends_on` edges (producers
/// first). Ties break alphabetically for determinism; cycles fall back to
/// declaration order so we never loop.
fn topo_order(positions: &[Position]) -> Vec<String> {
    let names: Vec<String> = positions.iter().map(|p| p.project.clone()).collect();
    let deps: HashMap<&str, &Vec<String>> = positions
        .iter()
        .map(|p| (p.project.as_str(), &p.depends_on))
        .collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    // Kahn-ish: repeatedly take the alphabetically-first node whose deps are all
    // already placed; if none qualifies (cycle), take the first remaining.
    let mut remaining: BTreeSet<String> = names.iter().cloned().collect();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .find(|n| {
                deps.get(n.as_str())
                    .map(|d| d.iter().all(|x| done.contains(x) || !remaining.contains(x)))
                    .unwrap_or(true)
            })
            .cloned()
            .or_else(|| remaining.iter().next().cloned());
        if let Some(n) = next {
            remaining.remove(&n);
            done.insert(n.clone());
            order.push(n);
        }
    }
    order
}

/// Render the deliberation as a Markdown transcript — the auditable record of
/// who said what and how the ordering was resolved.
pub fn render_transcript(r: &DeliberationReport) -> String {
    let mut s = String::new();
    s.push_str("# Deliberation\n\n");
    s.push_str("**Requirement**\n\n");
    for line in r.spec.trim().lines() {
        s.push_str("> ");
        s.push_str(line);
        s.push('\n');
    }
    s.push_str("\n## Positions\n\n");
    for p in &r.positions {
        s.push_str(&format!("### {} · _{}_\n\n", p.project, p.role));
        s.push_str(&format!("{}\n\n", p.summary));
        if let Some(pv) = &p.provides {
            s.push_str(&format!("- provides: `{pv}`\n"));
        }
        if let Some(c) = &p.consumes {
            s.push_str(&format!("- consumes: `{c}`\n"));
        }
        if !p.depends_on.is_empty() {
            s.push_str(&format!("- waits for: {}\n", p.depends_on.join(", ")));
        }
        for c in &p.concerns {
            s.push_str(&format!("- ⚠️ {c}\n"));
        }
        s.push('\n');
    }
    if !r.conflicts.is_empty() {
        s.push_str("## Contract coordination\n\n");
        for c in &r.conflicts {
            s.push_str(&format!("- {}\n", c.note));
        }
        s.push('\n');
    }
    s.push_str("## Resolved order\n\n");
    s.push_str(&r.order.join(" → "));
    s.push('\n');
    s
}

/// Per-position one-line subject + body for posting to the A2A mailbox, so the
/// deliberation shows up in the coordination panel as a real discussion.
pub fn position_message(p: &Position) -> (String, String) {
    let subject = format!("[deliberation] {} position: {}", p.project, p.role);
    let mut body = format!("{}\n", p.summary);
    if let Some(pv) = &p.provides {
        body.push_str(&format!("provides: {pv}\n"));
    }
    if let Some(c) = &p.consumes {
        body.push_str(&format!("consumes: {c}\n"));
    }
    if !p.depends_on.is_empty() {
        body.push_str(&format!("waits for: {}\n", p.depends_on.join(", ")));
    }
    for c in &p.concerns {
        body.push_str(&format!("concern: {c}\n"));
    }
    (subject, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ProjectsConfig {
        serde_yaml::from_str(
            r#"
version: 1
projects:
  shared:
    path: shared
    contracts:
      provides: src/index.js
  api:
    path: api
    contracts:
      consumes: ../shared/src/index.js
  web:
    path: web
    contracts:
      consumes: ../shared/src/index.js
"#,
        )
        .unwrap()
    }

    fn scope() -> BTreeSet<String> {
        ["shared", "api", "web"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn no_derived() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::new()
    }

    #[test]
    fn positions_capture_roles_and_dependencies() {
        let r = deliberate(
            "Add an email field to the User contract",
            &cfg(),
            &scope(),
            &no_derived(),
        );
        assert_eq!(r.positions.len(), 3);
        let shared = r.positions.iter().find(|p| p.project == "shared").unwrap();
        assert_eq!(shared.role, "foundational");
        assert!(shared.depends_on.is_empty());
        let api = r.positions.iter().find(|p| p.project == "api").unwrap();
        assert_eq!(api.role, "top-level");
        assert_eq!(api.depends_on, vec!["shared".to_string()]);
    }

    #[test]
    fn producer_is_ordered_before_consumers() {
        let r = deliberate("change contract", &cfg(), &scope(), &no_derived());
        let pos = |n: &str| r.order.iter().position(|x| x == n).unwrap();
        assert!(pos("shared") < pos("api"));
        assert!(pos("shared") < pos("web"));
    }

    #[test]
    fn code_graph_imports_drive_ordering_without_contracts() {
        // Three modules, NO contracts (a monorepo). The code graph says api and
        // web import core. Deliberation must order core first and role it as
        // foundational, purely from the derived edges.
        let projects: ProjectsConfig = serde_yaml::from_str(
            "version: 1\nprojects:\n  core: { path: app/core }\n  api: { path: app/api }\n  web: { path: app/web }\n",
        )
        .unwrap();
        let scope: BTreeSet<String> = ["core", "api", "web"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut derived = BTreeMap::new();
        derived.insert("api".to_string(), BTreeSet::from(["core".to_string()]));
        derived.insert("web".to_string(), BTreeSet::from(["core".to_string()]));
        let r = deliberate("refactor logging", &projects, &scope, &derived);
        let core = r.positions.iter().find(|p| p.project == "core").unwrap();
        assert_eq!(core.role, "foundational");
        let api = r.positions.iter().find(|p| p.project == "api").unwrap();
        assert_eq!(api.role, "top-level");
        assert_eq!(api.depends_on, vec!["core".to_string()]);
        let pos = |n: &str| r.order.iter().position(|x| x == n).unwrap();
        assert!(pos("core") < pos("api") && pos("core") < pos("web"));
    }

    #[test]
    fn conflict_lists_all_consumers_of_a_changed_contract() {
        let r = deliberate("change contract", &cfg(), &scope(), &no_derived());
        assert_eq!(r.conflicts.len(), 1);
        let c = &r.conflicts[0];
        assert_eq!(c.producer, "shared");
        assert_eq!(c.consumers, vec!["api".to_string(), "web".to_string()]);
        assert!(c.needs_attention);
    }
}
