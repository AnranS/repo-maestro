use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub spec: String,

    #[serde(default)]
    pub created_by: Option<String>,

    #[serde(default)]
    pub confirmed_at: Option<DateTime<Utc>>,

    #[serde(default)]
    pub contracts_change: Vec<ContractChange>,

    pub tasks: Vec<PlanTask>,

    #[serde(default)]
    pub verification: BTreeMap<String, serde_yaml::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,

    /// Optional goal-driven layer (L1). When present, this block defines
    /// what "done" means objectively. `acceptance` is a list of shell
    /// checks the scheduler runs after the DAG completes; a failed check
    /// gives the orchestrator a deterministic signal to replan rather
    /// than handing the user a green "done" that's actually broken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Goal>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Goal {
    /// One-line human-readable goal. Mirrors `spec` but separated so the
    /// agent treats it as the success criterion, not as the task list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,

    /// Ordered list of acceptance criteria. Each entry is a shell command
    /// that MUST exit 0 to count as passing. The literal text is shown
    /// to the user; the `check` field is what we actually run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<Acceptance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acceptance {
    /// Human-readable claim, e.g. "POST /login returns a JWT".
    pub describe: String,
    /// Shell command run from the workspace root. Exit 0 = pass.
    pub check: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractChange {
    pub file: String,
    #[serde(default)]
    pub diff_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,

    /// Single-project task. Mutually exclusive with `project_each`.
    #[serde(default)]
    pub project: String,

    /// Template fan-out: when set (and `project` is empty), the task is expanded
    /// at load time into N concrete tasks named `<id>__<projectN>`. Dependencies
    /// stay attached to the parent id and are rewritten to the parent's
    /// expanded ids automatically (so consumers can keep `depends_on: [T_lint]`
    /// and have it transparently mean "all of them").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_each: Vec<String>,

    #[serde(default = "default_task_kind")]
    pub kind: TaskKind,

    #[serde(default)]
    pub prompt: String,

    /// For `verify` tasks: a shell command instead of an Agent prompt.
    #[serde(default)]
    pub command: Option<String>,

    #[serde(default)]
    pub depends_on: Vec<String>,

    #[serde(default)]
    pub parallel_group: Option<String>,

    #[serde(default)]
    pub requires_approval_after: bool,

    #[serde(default = "default_timeout_minutes")]
    pub timeout_minutes: u64,

    #[serde(default)]
    pub memory_inject: Vec<MemoryInject>,

    /// Explicit Maestro skills that must be injected into this task's agent
    /// prompt. Names resolve against the task project first, then `_global`.
    /// Use `<scope>/<name>` (for example `_global/workflow-task-guardrails`)
    /// to disambiguate. Missing explicit skills fail the task at dispatch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,

    /// Explicit data inputs from upstream task outputs.
    ///
    /// Shape:
    ///
    /// ```yaml
    /// inputs:
    ///   api_contract:
    ///     from: T_schema.openapi
    /// ```
    ///
    /// `Plan::expand_for_each` automatically adds the producer task to
    /// `depends_on`, so a data reference is also an ordering dependency.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, TaskInput>,

    /// Named outputs materialized after the task succeeds. A downstream
    /// `inputs.*.from` reference can point at `<task_id>.<output_name>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, TaskOutput>,

    #[serde(default)]
    pub agent: Option<String>,

    /// Per-task Cursor model override. Highest priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Per-task model profile. Resolved through
    /// `projects.yaml.defaults.model_profiles` after explicit `model` and
    /// run-level `--model`, before project/default model fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,

    /// Per-task role override. Looked up against `crate::roles`. When
    /// `None`, the executor falls back to the project's declared role
    /// (or no role at all for `_global` tasks). The role's prelude is
    /// injected into the prompt between memory facts and the user-
    /// authored `prompt` body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Role that reviews this task's output after it succeeds. The reviewer
    /// agent runs read-only and ends with `VERDICT: pass|fail`; a fail feeds
    /// the same attribution → retry recovery path, with the review feedback as
    /// the diagnosis. `None` = no review step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_by: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Agent,
    Verify,
}

fn default_task_kind() -> TaskKind {
    TaskKind::Agent
}

fn default_timeout_minutes() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInject {
    pub topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    /// Reference in the form `<task_id>.<output_name>`.
    pub from: String,

    /// Missing required inputs fail the task at dispatch time. Defaults true.
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutput {
    /// File path relative to the task workspace. `_global` tasks resolve from
    /// the workspace root. If omitted, maestro records the adapter transcript
    /// summary as the output payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,

    /// Missing required file outputs make the task fail after the adapter
    /// exits successfully. Defaults true.
    #[serde(default = "default_true")]
    pub required: bool,

    /// Max bytes copied into the run output snapshot. Defaults 64 KiB.
    #[serde(default = "default_output_max_bytes")]
    pub max_bytes: usize,
}

fn default_true() -> bool {
    true
}

fn default_output_max_bytes() -> usize {
    64 * 1024
}

impl Plan {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read plan file {:?}", path))?;
        let mut plan: Self = serde_yaml::from_str(&text).context("parse PLAN.yaml")?;
        plan = plan.expand_for_each();
        plan.validate()?;
        Ok(plan)
    }

    /// Read + parse + expand a plan WITHOUT structural validation. `--json`
    /// callers use this so they can project each *validation* failure onto its
    /// own F-111 Issue code (`plan.self_dependency` / `plan.cycle` /
    /// `plan.task_id_traversal` / …) instead of collapsing everything into a
    /// single parse error. A failure here is a genuine read/parse failure.
    pub fn read_only(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read plan file {:?}", path))?;
        let plan: Self = serde_yaml::from_str(&text).context("parse PLAN.yaml")?;
        Ok(plan.expand_for_each())
    }

    /// Refuse a plan whose expanded task count exceeds `cap`. A runaway
    /// synthesized/dynamic plan is a bug signal, not a workload — and silent
    /// truncation would drop tasks while reporting success, so this bails
    /// with an actionable error instead. `cap == 0` disables the check.
    ///
    /// Call this on an already-expanded plan (`Plan::load` expands first) so
    /// the count reflects what will actually run, including `project_each`
    /// fan-out.
    pub fn enforce_task_cap(&self, cap: usize) -> Result<()> {
        if cap == 0 {
            return Ok(());
        }
        let n = self.tasks.len();
        if n > cap {
            anyhow::bail!(
                "plan has {n} tasks, over the cap of {cap} (defaults.max_total_tasks). \
                 This usually means a synthesized plan expanded further than intended. \
                 Split the goal into smaller runs, or raise defaults.max_total_tasks \
                 (set 0 to disable the cap)."
            );
        }
        Ok(())
    }

    /// Expand `project_each` tasks into N concrete tasks, rewriting any
    /// downstream references to the template id into a list of expanded ids.
    pub fn expand_for_each(self) -> Self {
        // First pass: build map from parent id -> Vec<expanded id>
        let mut expansion_map: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for t in &self.tasks {
            if !t.project_each.is_empty() && t.project.is_empty() {
                let expanded: Vec<String> = t
                    .project_each
                    .iter()
                    .map(|p| format!("{}__{}", t.id, p))
                    .collect();
                expansion_map.insert(t.id.clone(), expanded);
            }
        }
        if expansion_map.is_empty() {
            return Self {
                tasks: add_data_dependency_edges(self.tasks),
                ..self
            };
        }

        // Second pass: rebuild task list
        let mut out_tasks: Vec<PlanTask> = Vec::with_capacity(self.tasks.len());
        for t in &self.tasks {
            if let Some(expanded_ids) = expansion_map.get(&t.id) {
                for (proj, new_id) in t.project_each.iter().zip(expanded_ids.iter()) {
                    let mut clone = t.clone();
                    clone.id = new_id.clone();
                    clone.project = proj.clone();
                    clone.project_each = vec![];
                    clone.depends_on = rewrite_deps(&clone.depends_on, &expansion_map);
                    clone.prompt = clone.prompt.replace("{{project}}", proj);
                    if let Some(cmd) = clone.command.as_mut() {
                        *cmd = cmd.replace("{{project}}", proj);
                    }
                    out_tasks.push(clone);
                }
            } else {
                let mut clone = t.clone();
                clone.depends_on = rewrite_deps(&clone.depends_on, &expansion_map);
                out_tasks.push(clone);
            }
        }

        Self {
            tasks: add_data_dependency_edges(out_tasks),
            ..self
        }
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for t in &self.tasks {
            crate::paths::validate_path_component("task id", &t.id)
                .with_context(|| format!("invalid task id {}", t.id))?;
            if !seen.insert(t.id.clone()) {
                anyhow::bail!("duplicate task id: {}", t.id);
            }
        }
        for t in &self.tasks {
            for dep in &t.depends_on {
                if !seen.contains(dep) {
                    anyhow::bail!("task {} depends on unknown task {}", t.id, dep);
                }
            }
            if t.kind == TaskKind::Agent && t.prompt.trim().is_empty() {
                anyhow::bail!("agent task {} has empty prompt", t.id);
            }
            if t.kind == TaskKind::Verify && t.command.is_none() {
                anyhow::bail!("verify task {} missing `command`", t.id);
            }
            if t.project.is_empty() && t.project_each.is_empty() {
                anyhow::bail!("task {} has neither `project` nor `project_each`", t.id);
            }
            for (name, input) in &t.inputs {
                let (producer, output) = parse_output_ref(&input.from).ok_or_else(|| {
                    anyhow::anyhow!(
                        "task {} input {} has invalid `from` {:?}; expected <task>.<output>",
                        t.id,
                        name,
                        input.from
                    )
                })?;
                let Some(producer_task) = self.task(producer) else {
                    anyhow::bail!(
                        "task {} input {} references unknown producer task {}",
                        t.id,
                        name,
                        producer
                    );
                };
                if producer == t.id {
                    anyhow::bail!("task {} input {} cannot reference itself", t.id, name);
                }
                if !producer_task.outputs.contains_key(output) {
                    anyhow::bail!(
                        "task {} input {} references unknown output {} on task {}",
                        t.id,
                        name,
                        output,
                        producer
                    );
                }
                if producer != t.id && !t.depends_on.iter().any(|d| d == producer) {
                    anyhow::bail!(
                        "task {} input {} references {} but depends_on does not include it",
                        t.id,
                        name,
                        producer
                    );
                }
            }
        }
        // Reject self-deps and dependency cycles up front so a cyclic
        // hand-authored or fanned-out plan fails with a clear error here rather
        // than stack-overflowing later in compute_ancestors.
        for t in &self.tasks {
            if t.depends_on.iter().any(|d| d == &t.id) {
                anyhow::bail!("task {} depends on itself", t.id);
            }
        }
        detect_dependency_cycle(&self.tasks)?;
        if let Some(goal) = &self.goal {
            for (i, ac) in goal.acceptance.iter().enumerate() {
                if ac.describe.trim().is_empty() {
                    anyhow::bail!("goal.acceptance[{}] has empty `describe`", i);
                }
                if ac.check.trim().is_empty() {
                    anyhow::bail!(
                        "goal.acceptance[{}] ({:?}) has empty `check`",
                        i,
                        ac.describe
                    );
                }
            }
        }
        Ok(())
    }

    /// Like `validate()`, but returns the first failure as a coded F-111 `Issue`
    /// for `--json` callers — stable machine codes for the documented structural
    /// failures (`plan.self_dependency` / `plan.cycle` / `plan.task_id_traversal`),
    /// a generic `plan.invalid` otherwise. `None` ⇒ the plan validates.
    pub fn first_validation_issue(&self) -> Option<crate::schema::preview::Issue> {
        use crate::schema::preview::{codes, Issue};
        // Valid plan ⇒ no issue.
        self.validate().err()?;
        // There IS a failure — classify it by re-checking the actual condition
        // (never by string-matching the bail message).
        for t in &self.tasks {
            if crate::paths::validate_path_component("task id", &t.id).is_err() {
                return Some(
                    Issue::error(
                        codes::TASK_ID_TRAVERSAL,
                        "task id is not a safe path component",
                    )
                    .at(t.id.clone()),
                );
            }
        }
        for t in &self.tasks {
            if t.depends_on.iter().any(|d| d == &t.id) {
                return Some(
                    Issue::error(codes::SELF_DEPENDENCY, "task depends on itself").at(t.id.clone()),
                );
            }
        }
        if detect_dependency_cycle(&self.tasks).is_err() {
            return Some(Issue::error(
                codes::CYCLE,
                "the plan has a dependency cycle",
            ));
        }
        // Any other structural failure (duplicate id, unknown dep, empty prompt,
        // …) → a generic code with a single-line, neutral message.
        let message = self
            .validate()
            .err()
            .map(|e| {
                e.to_string()
                    .lines()
                    .next()
                    .unwrap_or("plan is invalid")
                    .to_string()
            })
            .unwrap_or_else(|| "plan is invalid".to_string());
        Some(Issue::error(codes::PLAN_INVALID, message))
    }

    pub fn task(&self, id: &str) -> Option<&PlanTask> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

fn rewrite_deps(
    deps: &[String],
    expansion_map: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out = Vec::with_capacity(deps.len());
    for d in deps {
        match expansion_map.get(d) {
            Some(expanded) => out.extend(expanded.iter().cloned()),
            None => out.push(d.clone()),
        }
    }
    out
}

fn add_data_dependency_edges(mut tasks: Vec<PlanTask>) -> Vec<PlanTask> {
    for t in &mut tasks {
        for input in t.inputs.values() {
            let Some((producer, _)) = parse_output_ref(&input.from) else {
                continue;
            };
            if producer != t.id && !t.depends_on.iter().any(|d| d == producer) {
                t.depends_on.push(producer.to_string());
            }
        }
    }
    tasks
}

pub fn parse_output_ref(reference: &str) -> Option<(&str, &str)> {
    let (task, output) = reference.rsplit_once('.')?;
    if task.trim().is_empty() || output.trim().is_empty() {
        return None;
    }
    Some((task, output))
}

/// Reject a dependency cycle in `depends_on` with a clear error, before the
/// recursive `compute_ancestors` (config::analyze) would stack-overflow on one.
fn detect_dependency_cycle(tasks: &[PlanTask]) -> Result<()> {
    use std::collections::{HashMap, HashSet};
    let deps: HashMap<&str, &Vec<String>> = tasks
        .iter()
        .map(|t| (t.id.as_str(), &t.depends_on))
        .collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut stack: HashSet<String> = HashSet::new();

    fn dfs(
        id: &str,
        deps: &HashMap<&str, &Vec<String>>,
        done: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> Result<()> {
        if done.contains(id) {
            return Ok(());
        }
        if !stack.insert(id.to_string()) {
            anyhow::bail!("PLAN.yaml has a dependency cycle through task {id}");
        }
        if let Some(ds) = deps.get(id) {
            for d in ds.iter() {
                if deps.contains_key(d.as_str()) {
                    dfs(d, deps, done, stack)?;
                }
            }
        }
        stack.remove(id);
        done.insert(id.to_string());
        Ok(())
    }

    for t in tasks {
        dfs(&t.id, &deps, &mut done, &mut stack)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_task_id_path_traversal() {
        let plan: Plan = serde_yaml::from_str(
            r#"
spec: bad id
tasks:
  - id: ../escape
    project: api
    prompt: do it
"#,
        )
        .unwrap();

        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("invalid task id"));
    }

    #[test]
    fn first_validation_issue_classifies_task_id_traversal() {
        let plan: Plan = serde_yaml::from_str(
            "spec: bad id\ntasks:\n  - { id: ../escape, project: api, prompt: do it }\n",
        )
        .unwrap();
        let issue = plan.first_validation_issue().expect("a validation issue");
        assert_eq!(issue.code, crate::schema::preview::codes::TASK_ID_TRAVERSAL);
        assert_eq!(issue.path.as_deref(), Some("../escape"));
    }

    #[test]
    fn first_validation_issue_classifies_self_dependency() {
        let plan: Plan = serde_yaml::from_str(
            "spec: self\ntasks:\n  - { id: T_self, project: p, prompt: go, depends_on: [T_self] }\n",
        )
        .unwrap();
        let issue = plan.first_validation_issue().expect("a validation issue");
        assert_eq!(issue.code, crate::schema::preview::codes::SELF_DEPENDENCY);
        assert_eq!(issue.path.as_deref(), Some("T_self"));
    }

    #[test]
    fn first_validation_issue_classifies_cycle() {
        let plan: Plan = serde_yaml::from_str(
            "spec: cyclic\ntasks:\n  - { id: A, project: p, prompt: go, depends_on: [B] }\n  - { id: B, project: p, prompt: go, depends_on: [A] }\n",
        )
        .unwrap();
        let issue = plan.first_validation_issue().expect("a validation issue");
        assert_eq!(issue.code, crate::schema::preview::codes::CYCLE);
    }

    #[test]
    fn first_validation_issue_none_for_valid_plan() {
        let plan: Plan = serde_yaml::from_str(
            "spec: ok\ntasks:\n  - { id: T_a, project: p, prompt: go }\n  - { id: T_b, project: p, prompt: go, depends_on: [T_a] }\n",
        )
        .unwrap();
        assert!(plan.first_validation_issue().is_none());
    }

    #[test]
    fn validate_rejects_dependency_cycle_instead_of_crashing() {
        let plan: Plan = serde_yaml::from_str(
            r#"
spec: cyclic
tasks:
  - id: A
    project: p
    prompt: go
    depends_on: [B]
  - id: B
    project: p
    prompt: go
    depends_on: [A]
"#,
        )
        .unwrap();
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("dependency cycle"), "got: {err}");
    }

    #[test]
    fn validate_rejects_self_dependency() {
        let plan: Plan = serde_yaml::from_str(
            r#"
spec: self dep
tasks:
  - id: T
    project: p
    prompt: go
    depends_on: [T]
"#,
        )
        .unwrap();
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("depends on itself"), "got: {err}");
    }

    fn plan_with_n_tasks(n: usize) -> Plan {
        let mut yaml = String::from("spec: cap test\ntasks:\n");
        for i in 0..n {
            yaml.push_str(&format!("  - id: T{i}\n    project: p\n    prompt: go\n"));
        }
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn enforce_task_cap_allows_plan_at_or_under_cap() {
        let plan = plan_with_n_tasks(3);
        assert!(
            plan.enforce_task_cap(3).is_ok(),
            "exactly at cap is allowed"
        );
        assert!(plan.enforce_task_cap(10).is_ok(), "under cap is allowed");
    }

    #[test]
    fn enforce_task_cap_rejects_plan_over_cap() {
        let plan = plan_with_n_tasks(5);
        let err = plan.enforce_task_cap(4).unwrap_err().to_string();
        assert!(err.contains("5 tasks"), "names the actual count: {err}");
        assert!(err.contains("cap of 4"), "names the cap: {err}");
        assert!(err.contains("max_total_tasks"), "points at the knob: {err}");
    }

    #[test]
    fn enforce_task_cap_zero_disables_the_check() {
        let plan = plan_with_n_tasks(5000);
        assert!(plan.enforce_task_cap(0).is_ok(), "cap of 0 means unlimited");
    }
}
