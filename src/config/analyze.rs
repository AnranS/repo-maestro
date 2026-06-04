//! Static analysis on a PLAN.yaml. Two layers:
//!
//! 1. **Validation** — hard errors that block `maestro run` (in addition to the
//!    cheap structural checks `Plan::validate` already does).
//! 2. **Lints** — soft findings (warnings) that the Planner agent or the user
//!    should know about before approving the plan.
//!
//! The output is structured so chat can show it as actionable items.

use serde::Serialize;

use super::{Plan, PlanTask, Project, ProjectsConfig, TaskKind};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "severity", rename_all = "lowercase")]
pub enum Finding {
    Error {
        /// Stable F-111 issue code (see `crate::schema::preview::codes`).
        code: &'static str,
        task: Option<String>,
        message: String,
    },
    Warning {
        code: &'static str,
        task: Option<String>,
        message: String,
    },
}

impl Finding {
    pub fn is_error(&self) -> bool {
        matches!(self, Finding::Error { .. })
    }

    pub fn code(&self) -> &'static str {
        match self {
            Finding::Error { code, .. } | Finding::Warning { code, .. } => code,
        }
    }

    /// Project this analyze finding onto the F-111 `Issue` envelope, so
    /// `plan validate --json` / `work --dry --json` emit one structured shape.
    pub fn to_issue(&self) -> crate::schema::preview::Issue {
        use crate::schema::preview::{Issue, IssueSeverity};
        let (severity, code, task, message) = match self {
            Finding::Error {
                code,
                task,
                message,
            } => (IssueSeverity::Error, *code, task, message),
            Finding::Warning {
                code,
                task,
                message,
            } => (IssueSeverity::Warning, *code, task, message),
        };
        let mut issue = Issue::new(code, severity, message.clone());
        if let Some(t) = task {
            issue = issue.at(t.clone());
        }
        issue
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeReport {
    pub findings: Vec<Finding>,
    pub task_count: usize,
}

impl AnalyzeReport {
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.is_error())
    }

    pub fn error_count(&self) -> usize {
        self.findings.iter().filter(|f| f.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.findings.iter().filter(|f| !f.is_error()).count()
    }
}

pub fn analyze(plan: &Plan, projects: &ProjectsConfig) -> AnalyzeReport {
    let mut findings = vec![];

    findings.extend(check_unknown_projects(plan, projects));
    findings.extend(check_shell_syntax(plan));
    findings.extend(check_contracts(plan, projects));
    findings.extend(check_agent_profiles(plan, projects));
    findings.extend(check_size(plan));

    AnalyzeReport {
        findings,
        task_count: plan.tasks.len(),
    }
}

/// Every non-`_global` task must reference a project that exists in projects.yaml.
fn check_unknown_projects(plan: &Plan, projects: &ProjectsConfig) -> Vec<Finding> {
    let mut out = vec![];
    for t in &plan.tasks {
        if t.project.is_empty() || t.project == "_global" {
            continue;
        }
        if !projects.projects.contains_key(&t.project) {
            out.push(Finding::Error {
                code: crate::schema::preview::codes::UNKNOWN_PROJECT,
                task: Some(t.id.clone()),
                message: format!(
                    "project `{}` not in projects.yaml; run `maestro ls` to see registered projects",
                    t.project
                ),
            });
        }
    }
    out
}

/// F-114: every task-level `agent_profile` / `review_profile` must name a
/// profile that exists in `defaults.agent_profiles` and is enabled. This is the
/// task-side complement to `ProjectsConfig::agent_profile_issues` (which covers
/// the project-side refs) — it catches a hand-authored `PLAN.yaml` typo before
/// dispatch instead of letting it silently fall through to default routing.
fn check_agent_profiles(plan: &Plan, projects: &ProjectsConfig) -> Vec<Finding> {
    let profiles = &projects.defaults.agent_profiles;
    let mut out = vec![];
    for t in &plan.tasks {
        for (field, reference) in [
            ("agent_profile", t.agent_profile.as_deref()),
            ("review_profile", t.review_profile.as_deref()),
        ] {
            let Some(name) = reference.map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let problem = match profiles.get(name) {
                None => Some("is not defined in defaults.agent_profiles"),
                Some(p) if !p.enabled => Some("refers to a disabled profile"),
                Some(_) => None,
            };
            if let Some(problem) = problem {
                out.push(Finding::Error {
                    code: crate::schema::preview::codes::UNKNOWN_AGENT_PROFILE,
                    task: Some(t.id.clone()),
                    message: format!("{field} `{name}` {problem}"),
                });
            }
        }
    }
    out
}

/// For tasks the shell adapter will run (kind=Verify, or kind=Agent with
/// adapter=shell), run a non-execution syntax check via `bash -n`.
fn check_shell_syntax(plan: &Plan) -> Vec<Finding> {
    use std::process::Command;

    let mut out = vec![];
    for t in &plan.tasks {
        let cmd_text = match t.kind {
            TaskKind::Verify => t.command.clone(),
            TaskKind::Agent => {
                // Only validate when the task is explicitly pinned to the
                // shell adapter; agent prompts otherwise are natural language.
                if t.agent.as_deref() == Some("shell") {
                    Some(t.prompt.clone())
                } else {
                    None
                }
            }
        };
        let Some(cmd) = cmd_text else { continue };
        if cmd.trim().is_empty() {
            continue;
        }
        // `bash -n -c <script>` parses the script with -n (no-exec), avoiding
        // the stdin pipe lifecycle hazard. -c takes the script as a single
        // argument so multiline / here-doc scripts work as-is.
        let output = match Command::new("bash")
            .args(["-n", "-c", &cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(o) => o,
            Err(_) => continue, // bash not available — silently skip
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            // Trim "bash: -c: line N:" prefix noise.
            let cleaned = stderr
                .lines()
                .map(|l| l.trim_start_matches("bash: -c: ").trim())
                .collect::<Vec<_>>()
                .join("; ");
            out.push(Finding::Error {
                code: crate::schema::preview::codes::SHELL_SYNTAX,
                task: Some(t.id.clone()),
                message: format!("shell syntax error: {}", cleaned),
            });
        }
    }
    out
}

/// Contracts-driven dependency lint.
///
/// If a task touches a project that `provides` a contract, every task in a
/// project that `consumes` the same contract should have a transitive
/// `depends_on` path back to that producer task. Otherwise we emit a warning —
/// the producer might be racing the consumer.
/// Do two declared contract paths refer to the same file? Provider-relative
/// (`types/index.d.ts`) and consumer-relative (`../shared/types/index.d.ts`)
/// spellings of the same contract should match, which exact string comparison
/// misses — so auto-discovered contracts (and hand-written ones expressed from
/// each project's own directory) never wired. Match on the last two path
/// components, with an exact-equality fast path.
fn contracts_match(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
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
    tail(a) == tail(b)
}

/// Project names whose `provides` logically matches `consumer`'s `consumes`.
pub(crate) fn producer_projects_for(projects: &ProjectsConfig, consumer: &str) -> Vec<String> {
    let Some(cp) = projects.projects.get(consumer) else {
        return vec![];
    };
    let consumes = match cp.contracts.consumes.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => return vec![],
    };
    projects
        .projects
        .iter()
        .filter(|(name, p)| {
            name.as_str() != consumer
                && p.contracts
                    .provides
                    .as_deref()
                    .is_some_and(|prov| contracts_match(prov, consumes))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn check_contracts(plan: &Plan, projects: &ProjectsConfig) -> Vec<Finding> {
    let mut out = vec![];
    let ancestors = compute_ancestors(plan);

    for (cons_name, cons_proj) in &projects.projects {
        let Some(consumes) = cons_proj.contracts.consumes.as_deref() else {
            continue;
        };
        let producer_projects = producer_projects_for(projects, cons_name);
        if producer_projects.is_empty() {
            continue;
        }
        let producer_tasks: Vec<&PlanTask> = plan
            .tasks
            .iter()
            .filter(|t| producer_projects.contains(&t.project))
            .collect();
        if producer_tasks.is_empty() {
            continue;
        }
        for consumer_task in plan.tasks.iter().filter(|t| &t.project == cons_name) {
            let links_back = ancestors
                .get(&consumer_task.id)
                .is_some_and(|anc| producer_tasks.iter().any(|prod| anc.contains(&prod.id)));
            if !links_back {
                let prod_list: Vec<&str> = producer_tasks.iter().map(|t| t.id.as_str()).collect();
                out.push(Finding::Warning {
                    code: crate::schema::preview::codes::DANGLING_CONTRACT,
                    task: Some(consumer_task.id.clone()),
                    message: format!(
                        "consumer of contract `{}` does not transitively depend on any of its producers ({}); they may race",
                        consumes,
                        prod_list.join(", ")
                    ),
                });
            }
        }
    }
    out
}

/// A consumer that may have drifted from a contract its producer changed in the
/// same run. maestro's signature failure mode: the shared contract moved but one
/// consumer wasn't updated to match — the cross-repo inconsistency that only
/// surfaces later in production.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ContractDrift {
    pub contract: String,
    pub producer: String,
    pub consumer: String,
    pub reason: String,
}

/// Detect potential contract drift after a run: for every producer→consumer
/// contract edge, flag the case where the producer changed this run but the
/// consumer did not — the consumer may now be stale against the new contract.
/// Deterministic and zero-LLM; `changed_projects` is the set of projects whose
/// tasks touched any file in this run.
pub fn detect_contract_drift(
    projects: &ProjectsConfig,
    changed_projects: &std::collections::HashSet<String>,
) -> Vec<ContractDrift> {
    let mut out = Vec::new();
    for (consumer, cp) in &projects.projects {
        let consumes = match cp.contracts.consumes.as_deref().map(str::trim) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };
        if changed_projects.contains(consumer) {
            continue; // consumer was updated this run — not stale
        }
        for producer in producer_projects_for(projects, consumer) {
            if changed_projects.contains(&producer) {
                out.push(ContractDrift {
                    contract: consumes.to_string(),
                    producer: producer.clone(),
                    consumer: consumer.clone(),
                    reason: format!(
                        "`{producer}` changed the contract `{consumes}` this run, but consumer `{consumer}` wasn't updated — verify it still matches"
                    ),
                });
            }
        }
    }
    out.sort_by(|a, b| (a.consumer.as_str(), a.producer.as_str()).cmp(&(&b.consumer, &b.producer)));
    out
}

/// Build a map from task id to the set of all ancestor task ids.
fn compute_ancestors(
    plan: &Plan,
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    let by_id: HashMap<&str, &PlanTask> = plan.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut cache: HashMap<String, HashSet<String>> = HashMap::new();

    fn rec(
        id: &str,
        by_id: &std::collections::HashMap<&str, &PlanTask>,
        cache: &mut std::collections::HashMap<String, std::collections::HashSet<String>>,
        visiting: &mut std::collections::HashSet<String>,
    ) -> std::collections::HashSet<String> {
        if let Some(c) = cache.get(id) {
            return c.clone();
        }
        // Cycle guard: a back-edge means the plan is cyclic (Plan::validate
        // rejects this separately). Returning here instead of recursing avoids
        // a stack overflow if an expanded plan ever reaches this unvalidated.
        if !visiting.insert(id.to_string()) {
            return Default::default();
        }
        let mut set: std::collections::HashSet<String> = Default::default();
        if let Some(t) = by_id.get(id) {
            for dep in &t.depends_on {
                set.insert(dep.clone());
                if by_id.contains_key(dep.as_str()) {
                    for sub in rec(dep, by_id, cache, visiting) {
                        set.insert(sub);
                    }
                }
            }
        }
        visiting.remove(id);
        cache.insert(id.to_string(), set.clone());
        set
    }

    for t in &plan.tasks {
        let mut visiting = std::collections::HashSet::new();
        rec(&t.id, &by_id, &mut cache, &mut visiting);
    }
    cache
}

/// A deterministic, zero-token "architecture brief" — a readable summary of how
/// the workspace fits together, synthesized from projects.yaml (types,
/// contracts, dependencies). The Devin-Wiki idea without the LLM cost: enough
/// for a newcomer (or an agent) to grasp the shape of the workspace at a glance.
pub fn architecture_brief(
    projects: &ProjectsConfig,
    derived: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> String {
    use std::collections::{BTreeMap, BTreeSet};
    let names: Vec<&String> = projects.projects.keys().collect();
    if names.is_empty() {
        return "No projects registered. Run `maestro init --analyze` to discover them.".into();
    }
    // The brief shows the SOURCE of each dependency, not just "depends on
    // X". After commit 4f94b94 the discovery step folds code-graph
    // derived edges into `dependencies:` for the DAG scheduler, so the
    // dependency arrays now contain BOTH manifest declarations and
    // code-graph imports. To preserve the "imports vs declares" distinction
    // in this brief, we look up each edge in `derived` and split:
    //   imported  = in derived (originated from source-imports / contracts)
    //   declared  = in p.dependencies but NOT in derived (hand-authored)
    let imported_of = |name: &str| -> BTreeSet<String> {
        derived
            .get(name)
            .into_iter()
            .flatten()
            .filter(|d| projects.projects.contains_key(d.as_str()))
            .cloned()
            .collect()
    };
    let declared_only = |p: &Project, imports: &BTreeSet<String>| -> Vec<String> {
        p.dependencies
            .iter()
            .filter(|d| !imports.contains(d.as_str()))
            .cloned()
            .collect()
    };

    // Pre-compute the imported / declared split per project so the rendering
    // loop just looks up tags. `imports_by_name` and `declared_only_by_name`
    // are stored as Vec<String> so iteration order is stable.
    let imports_set_by_name: BTreeMap<&str, BTreeSet<String>> = projects
        .projects
        .keys()
        .map(|name| (name.as_str(), imported_of(name)))
        .collect();
    let declared_only_by_name: BTreeMap<&str, Vec<String>> = projects
        .projects
        .iter()
        .map(|(name, p)| {
            (
                name.as_str(),
                declared_only(p, &imports_set_by_name[name.as_str()]),
            )
        })
        .collect();
    let imports_by_name: BTreeMap<&str, Vec<String>> = imports_set_by_name
        .iter()
        .map(|(k, v)| (*k, v.iter().cloned().collect()))
        .collect();

    // dependents[a] = projects that depend on a (declared OR imported).
    // Build from BOTH sources — derived edges might or might not also be
    // in `dependencies:` depending on whether the caller ran `mst init`
    // post-fold-in. Belt-and-suspenders.
    let mut dependents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (name, p) in &projects.projects {
        for dep in &p.dependencies {
            dependents
                .entry(dep.as_str())
                .or_default()
                .insert(name.as_str());
        }
    }
    for (name, imps) in &imports_set_by_name {
        for dep in imps {
            dependents.entry(dep.as_str()).or_default().insert(name);
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "maestro · workspace architecture brief\n\n{} project(s):\n",
        names.len()
    ));
    for (name, p) in &projects.projects {
        let ty = p.r#type.as_deref().unwrap_or("—");
        let mut tags = Vec::new();
        if let Some(prov) = &p.contracts.provides {
            tags.push(format!("provides {prov}"));
        }
        if let Some(cons) = &p.contracts.consumes {
            tags.push(format!("consumes {cons}"));
        }
        let declared_solo = &declared_only_by_name[name.as_str()];
        if !declared_solo.is_empty() {
            tags.push(format!("depends on {}", declared_solo.join(", ")));
        }
        let imps = &imports_by_name[name.as_str()];
        if !imps.is_empty() {
            tags.push(format!("imports {}", imps.join(", ")));
        }
        let no_deps = declared_solo.is_empty() && imps.is_empty();
        let no_dependents = !dependents.contains_key(name.as_str());
        let role = match (no_deps, no_dependents) {
            (true, false) => "  ← foundational (nothing it depends on)",
            (false, true) => "  ← top-level (nothing depends on it)",
            (true, true) => "  ← standalone",
            _ => "",
        };
        let detail = if tags.is_empty() {
            String::new()
        } else {
            format!("  {}", tags.join(" · "))
        };
        out.push_str(&format!("  {name} [{ty}]{detail}{role}\n"));
    }

    // Dependency flow (provider/imported → its dependents).
    if !dependents.is_empty() {
        out.push_str("\ndependency flow:\n");
        for (provider, deps) in &dependents {
            let mut d: Vec<&str> = deps.iter().copied().collect();
            d.sort();
            out.push_str(&format!("  {provider} → {}\n", d.join(", ")));
        }
    }

    // One-line summary.
    let providers: Vec<&String> = projects
        .projects
        .iter()
        .filter(|(_, p)| p.contracts.provides.is_some())
        .map(|(n, _)| n)
        .collect();
    let declared_edges: usize = declared_only_by_name.values().map(Vec::len).sum();
    let imported_edges: usize = imports_by_name.values().map(Vec::len).sum();
    out.push_str(&format!(
        "\nsummary: {} project(s), {} declared + {} code-graph dependency edge(s), {} contract provider(s).\n",
        names.len(),
        declared_edges,
        imported_edges,
        providers.len()
    ));
    out
}

/// Per-task impact preview: the contract it touches and the blast radius
/// (tasks that transitively depend on it). Computed from the plan + projects
/// before any run, so the user can judge "what will this touch and who's
/// downstream?" before committing to execute.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskImpact {
    pub task: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provides: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumes: Option<String>,
    /// Tasks that transitively depend on this one (the blast radius).
    pub downstream: Vec<String>,
}

/// Impact preview for every task in the plan, in plan order.
pub fn plan_impact(plan: &Plan, projects: &ProjectsConfig) -> Vec<TaskImpact> {
    let ancestors = compute_ancestors(plan);
    // Invert ancestors → dependents: who is downstream of each task.
    let mut dependents: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    for (task, ancs) in &ancestors {
        for a in ancs {
            dependents.entry(a.as_str()).or_default().push(task.clone());
        }
    }
    for v in dependents.values_mut() {
        v.sort();
    }
    plan.tasks
        .iter()
        .map(|t| {
            let p = projects.projects.get(&t.project);
            TaskImpact {
                task: t.id.clone(),
                project: t.project.clone(),
                provides: p.and_then(|p| p.contracts.provides.clone()),
                consumes: p.and_then(|p| p.contracts.consumes.clone()),
                downstream: dependents.get(t.id.as_str()).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// A dependency edge added to satisfy a declared contract: `consumer` should
/// run after `producer` so it doesn't build against a stale contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiredEdge {
    pub consumer: String,
    pub producer: String,
    pub contract: String,
}

/// Auto-wire the producer→consumer dependency edges implied by declared
/// contracts. For each contract, a consumer task that does not already
/// transitively depend on *any* producer task gets a `depends_on` edge to the
/// producer's terminal task(s) (the latest task in the producer's own chain,
/// so the consumer waits for the producer's work — including its verify — to
/// finish). Cycle-safe: an edge that would close a loop is skipped (left for
/// `analyze` to warn about). Idempotent: returns `[]` when nothing was missing.
///
/// This is the automatic counterpart to the "consumer … may race" lint —
/// `maestro work` already orders synthesized plans by contract, this brings
/// the same protection to hand-written / loaded plans before they execute.
pub fn wire_contract_dependencies(plan: &mut Plan, projects: &ProjectsConfig) -> Vec<WiredEdge> {
    let ancestors = compute_ancestors(plan);
    let is_ancestor = |of: &str, candidate: &str| {
        ancestors
            .get(of)
            .map(|a| a.contains(candidate))
            .unwrap_or(false)
    };

    let mut to_add: Vec<WiredEdge> = vec![];
    for (cons_name, cons_proj) in &projects.projects {
        let Some(consumes) = cons_proj.contracts.consumes.as_deref() else {
            continue;
        };
        let producer_projects = producer_projects_for(projects, cons_name);
        if producer_projects.is_empty() {
            continue;
        }
        let producer_ids: Vec<String> = plan
            .tasks
            .iter()
            .filter(|t| producer_projects.contains(&t.project))
            .map(|t| t.id.clone())
            .collect();
        if producer_ids.is_empty() {
            continue;
        }
        // Terminal producers: producer tasks no *other* producer task depends
        // on. Depending on these makes the consumer wait for the whole producer
        // side (e.g. change→verify) rather than just its first step.
        let terminals: Vec<&String> = producer_ids
            .iter()
            .filter(|p| !producer_ids.iter().any(|q| *q != **p && is_ancestor(q, p)))
            .collect();

        for consumer in plan.tasks.iter().filter(|t| &t.project == cons_name) {
            // Already linked to some producer? then there's no race to fix.
            if producer_ids
                .iter()
                .any(|pid| is_ancestor(&consumer.id, pid))
            {
                continue;
            }
            for &producer in &terminals {
                if producer == &consumer.id {
                    continue;
                }
                // Skip an edge that would create a cycle (producer already
                // depends on the consumer — a mis-declared contract direction).
                if is_ancestor(producer, &consumer.id) {
                    continue;
                }
                let edge = WiredEdge {
                    consumer: consumer.id.clone(),
                    producer: producer.clone(),
                    contract: consumes.to_string(),
                };
                if !to_add.contains(&edge) {
                    to_add.push(edge);
                }
            }
        }
    }

    // Apply: append each missing edge to the consumer's depends_on.
    for edge in &to_add {
        if let Some(task) = plan.tasks.iter_mut().find(|t| t.id == edge.consumer) {
            if !task.depends_on.contains(&edge.producer) {
                task.depends_on.push(edge.producer.clone());
            }
        }
    }
    to_add
}

/// Warn if the plan has > 20 tasks (per the design doc rule of thumb).
///
/// An audit plan (`init --analyze` / discovery) has ~2 tasks per project, so a
/// large count is inherent to the workspace size, not a runaway synthesis —
/// give actionable scoping advice for that case instead of the generic
/// "split it up" tone, which isn't useful when you genuinely want to audit
/// every project.
fn check_size(plan: &Plan) -> Vec<Finding> {
    const SOFT_CAP: usize = 20;
    let n = plan.tasks.len();
    if n <= SOFT_CAP {
        return vec![];
    }
    let is_audit = matches!(
        plan.created_by.as_deref(),
        Some("maestro-init-analyze") | Some("maestro-discover")
    );
    let message = if is_audit {
        format!(
            "{n} tasks — auditing a large workspace is naturally big. To narrow the run: \
             scope with `--project <name>`, point `--root` at a sub-directory, or raise \
             `defaults.max_total_tasks`. (≤ {SOFT_CAP} per plan is the rule of thumb for \
             interactive runs.)"
        )
    } else {
        format!("{n} tasks — consider splitting; recommended ≤ {SOFT_CAP} per plan")
    };
    vec![Finding::Warning {
        code: crate::schema::preview::codes::SIZE_WARNING,
        task: None,
        message,
    }]
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn projects(yaml: &str) -> ProjectsConfig {
        serde_yaml::from_str(yaml).expect("parse projects")
    }
    fn plan(yaml: &str) -> Plan {
        serde_yaml::from_str(yaml).expect("parse plan")
    }
    fn deps(plan: &Plan, id: &str) -> Vec<String> {
        plan.tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.depends_on.clone())
            .unwrap_or_default()
    }

    const CONTRACT_PROJECTS: &str = r#"
version: 1
projects:
  shared:
    path: shared
    contracts:
      provides: types/index.d.ts
  api:
    path: api
    contracts:
      consumes: types/index.d.ts
"#;

    #[test]
    fn wires_missing_consumer_to_producer_edge() {
        let projects = projects(CONTRACT_PROJECTS);
        let mut p = plan(
            r#"
spec: contract change
tasks:
  - { id: T_change_shared, project: shared, kind: agent }
  - { id: T_change_api, project: api, kind: agent }
"#,
        );
        // Before wiring, the analyzer flags the race.
        assert_eq!(check_contracts(&p, &projects).len(), 1);

        let wired = wire_contract_dependencies(&mut p, &projects);
        assert_eq!(wired.len(), 1);
        assert_eq!(wired[0].consumer, "T_change_api");
        assert_eq!(wired[0].producer, "T_change_shared");
        assert_eq!(deps(&p, "T_change_api"), vec!["T_change_shared"]);
        // And the race warning is gone.
        assert!(check_contracts(&p, &projects).is_empty());
    }

    #[test]
    fn drift_flags_changed_producer_with_stale_consumer() {
        use std::collections::HashSet;
        let projects = projects(CONTRACT_PROJECTS);
        // shared (producer) changed, api (consumer) did not → drift.
        let changed: HashSet<String> = ["shared".to_string()].into_iter().collect();
        let drift = detect_contract_drift(&projects, &changed);
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].producer, "shared");
        assert_eq!(drift[0].consumer, "api");

        // Both changed → consumer was updated, no drift.
        let both: HashSet<String> = ["shared", "api"].iter().map(|s| s.to_string()).collect();
        assert!(detect_contract_drift(&projects, &both).is_empty());

        // Only consumer changed → no drift (producer contract is stable).
        let only_consumer: HashSet<String> = ["api".to_string()].into_iter().collect();
        assert!(detect_contract_drift(&projects, &only_consumer).is_empty());
    }

    #[test]
    fn architecture_brief_summarizes_topology() {
        let projects = projects(
            r#"
version: 1
projects:
  shared:
    path: libs/shared
    type: library
    contracts: { provides: types/index.d.ts }
  api:
    path: services/api
    type: backend
    contracts: { consumes: types/index.d.ts }
    dependencies: [shared]
"#,
        );
        let brief = architecture_brief(&projects, &std::collections::BTreeMap::new());
        assert!(brief.contains("2 project(s)"));
        assert!(brief.contains("shared [library]"));
        assert!(brief.contains("foundational")); // shared has no deps but a dependent
        assert!(brief.contains("top-level")); // api has a dep but no dependents
        assert!(brief.contains("shared → api")); // dependency flow
        assert!(brief.contains("1 contract provider"));
    }

    #[test]
    fn architecture_brief_folds_in_code_graph_imports() {
        // Two modules that declare NO dependencies (a monorepo); the code graph
        // says `web` imports `core`. The brief must reflect that flow.
        let projects = projects(
            r#"
version: 1
projects:
  core:
    path: app/core
    type: backend
  web:
    path: app/web
    type: frontend
"#,
        );
        let mut derived = std::collections::BTreeMap::new();
        derived.insert(
            "web".to_string(),
            std::collections::BTreeSet::from(["core".to_string()]),
        );
        let brief = architecture_brief(&projects, &derived);
        assert!(
            brief.contains("imports core"),
            "web should show it imports core:\n{brief}"
        );
        assert!(
            brief.contains("core → web"),
            "flow should show core → web:\n{brief}"
        );
        assert!(
            brief.contains("0 declared + 1 code-graph"),
            "summary counts imports:\n{brief}"
        );
        assert!(!brief.contains("web [frontend]  ← standalone"));
    }

    #[test]
    fn plan_impact_reports_contract_and_downstream() {
        let projects = projects(CONTRACT_PROJECTS);
        let p = plan(
            r#"
spec: impact
tasks:
  - { id: T_shared, project: shared, kind: agent }
  - { id: T_api, project: api, kind: agent, depends_on: [T_shared] }
"#,
        );
        let impact = plan_impact(&p, &projects);
        let shared = impact.iter().find(|i| i.task == "T_shared").unwrap();
        assert_eq!(shared.provides.as_deref(), Some("types/index.d.ts"));
        assert_eq!(shared.downstream, vec!["T_api"]); // blast radius
        let api = impact.iter().find(|i| i.task == "T_api").unwrap();
        assert_eq!(api.consumes.as_deref(), Some("types/index.d.ts"));
        assert!(api.downstream.is_empty());
    }

    #[test]
    fn wires_when_provider_and_consumer_express_contract_relatively() {
        // The same contract file, written provider-relative on one side and
        // consumer-relative on the other (as auto-discovery emits). Exact-string
        // matching missed this; logical matching wires it.
        let projects = projects(
            r#"
version: 1
projects:
  shared:
    path: libs/shared
    contracts:
      provides: types/index.d.ts
  api:
    path: services/api
    contracts:
      consumes: ../../libs/shared/types/index.d.ts
"#,
        );
        let mut p = plan(
            r#"
spec: relative contract paths
tasks:
  - { id: T_shared, project: shared, kind: agent }
  - { id: T_api, project: api, kind: agent }
"#,
        );
        assert_eq!(check_contracts(&p, &projects).len(), 1);
        let wired = wire_contract_dependencies(&mut p, &projects);
        assert_eq!(wired.len(), 1);
        assert_eq!(wired[0].producer, "T_shared");
        assert_eq!(deps(&p, "T_api"), vec!["T_shared"]);
        assert!(check_contracts(&p, &projects).is_empty());
    }

    #[test]
    fn wiring_is_idempotent_when_already_linked() {
        let projects = projects(CONTRACT_PROJECTS);
        let mut p = plan(
            r#"
spec: already linked
tasks:
  - { id: T_change_shared, project: shared, kind: agent }
  - { id: T_change_api, project: api, kind: agent, depends_on: [T_change_shared] }
"#,
        );
        let wired = wire_contract_dependencies(&mut p, &projects);
        assert!(wired.is_empty(), "nothing to wire when already linked");
        assert_eq!(deps(&p, "T_change_api"), vec!["T_change_shared"]);
    }

    #[test]
    fn wires_to_terminal_producer_not_intermediate() {
        // Producer has change→verify; the consumer should wait for the verify
        // (the terminal producer task), not just the change.
        let projects = projects(CONTRACT_PROJECTS);
        let mut p = plan(
            r#"
spec: terminal producer
tasks:
  - { id: T_change_shared, project: shared, kind: agent }
  - { id: T_verify_shared, project: shared, kind: verify, depends_on: [T_change_shared] }
  - { id: T_change_api, project: api, kind: agent }
"#,
        );
        let wired = wire_contract_dependencies(&mut p, &projects);
        assert_eq!(wired.len(), 1);
        assert_eq!(wired[0].producer, "T_verify_shared");
        assert_eq!(deps(&p, "T_change_api"), vec!["T_verify_shared"]);
    }

    #[test]
    fn skips_edge_that_would_create_a_cycle() {
        // Mis-declared: the producer already depends on the consumer. Wiring the
        // contract edge would close a loop, so it must be skipped (left to warn).
        let projects = projects(CONTRACT_PROJECTS);
        let mut p = plan(
            r#"
spec: would cycle
tasks:
  - { id: T_change_shared, project: shared, kind: agent, depends_on: [T_change_api] }
  - { id: T_change_api, project: api, kind: agent }
"#,
        );
        let wired = wire_contract_dependencies(&mut p, &projects);
        assert!(wired.is_empty(), "cycle-creating edge must be skipped");
        assert!(deps(&p, "T_change_api").is_empty());
    }

    fn warning_message(findings: &[Finding]) -> String {
        findings
            .iter()
            .find_map(|f| match f {
                Finding::Warning { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("a size warning")
    }

    #[test]
    fn audit_plan_size_warning_gives_scoping_advice() {
        // An `init --analyze` audit plan (>20 tasks) should warn with
        // actionable scoping levers, not the generic "split it up" tone.
        let mut yaml = String::from("spec: audit\ncreated_by: maestro-init-analyze\ntasks:\n");
        for i in 0..21 {
            yaml.push_str(&format!(
                "  - {{ id: T{i}, project: p{i}, kind: verify, command: \"true\" }}\n"
            ));
        }
        let audit = plan(&yaml);
        let msg = warning_message(&check_size(&audit));
        assert!(
            msg.contains("--project"),
            "audit warning must name --project: {msg}"
        );
        assert!(
            msg.contains("max_total_tasks"),
            "and max_total_tasks: {msg}"
        );

        // A non-audit (e.g. work-synthesized) runaway plan keeps the generic
        // "consider splitting" tone — that advice IS useful there.
        let generic = plan(&yaml.replace("maestro-init-analyze", "maestro-work"));
        let gmsg = warning_message(&check_size(&generic));
        assert!(
            gmsg.contains("consider splitting"),
            "non-audit keeps the generic tone: {gmsg}"
        );

        // ≤ 20 tasks: no warning either way.
        let small = plan("spec: s\ncreated_by: maestro-init-analyze\ntasks:\n  - { id: T0, project: p, kind: verify, command: \"true\" }\n");
        assert!(check_size(&small).is_empty());
    }

    #[test]
    fn flags_unknown_or_disabled_task_agent_profile() {
        let projects = projects(
            "version: 1\ndefaults:\n  agent_profiles:\n    reviewer:\n      role: refuter\n    draft:\n      role: backend\n      enabled: false\nprojects:\n  api:\n    path: ./api\n",
        );
        let plan = plan(
            "spec: s\ntasks:\n  - { id: T0, project: api, kind: agent, prompt: p, agent_profile: nope }\n  - { id: T1, project: api, kind: agent, prompt: p, review_profile: draft }\n  - { id: T2, project: api, kind: agent, prompt: p, review_profile: reviewer }\n",
        );
        let out = check_agent_profiles(&plan, &projects);
        let codes: Vec<_> = out.iter().map(|f| f.code()).collect();
        assert!(out.iter().all(|f| f.is_error()));
        assert_eq!(
            codes,
            vec![
                crate::schema::preview::codes::UNKNOWN_AGENT_PROFILE,
                crate::schema::preview::codes::UNKNOWN_AGENT_PROFILE
            ],
            "T0 undefined + T1 disabled flagged; T2 (valid) not"
        );
        let msgs: String = out
            .iter()
            .map(|f| match f {
                Finding::Error { task, message, .. } => {
                    format!("{}:{message}\n", task.as_deref().unwrap_or(""))
                }
                _ => String::new(),
            })
            .collect();
        assert!(msgs.contains("T0:agent_profile `nope` is not defined"));
        assert!(msgs.contains("T1:review_profile `draft` refers to a disabled profile"));
    }

    #[test]
    fn padded_valid_task_agent_profile_passes() {
        // N1: a whitespace-padded but valid reference must validate clean (the
        // same normalization the resolver/dispatch use).
        let projects = projects(
            "version: 1\ndefaults:\n  agent_profiles:\n    reviewer:\n      role: refuter\nprojects:\n  api:\n    path: ./api\n",
        );
        let plan = plan(
            "spec: s\ntasks:\n  - { id: T0, project: api, kind: agent, prompt: p, agent_profile: \" reviewer \" }\n",
        );
        assert!(
            check_agent_profiles(&plan, &projects).is_empty(),
            "padded valid reference must pass validate"
        );
    }
}
