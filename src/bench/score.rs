use crate::bench::fixture::{Expected, Fixture, FixtureKind};
use crate::bench::runner::BenchRun;
use crate::config::Plan;
use crate::scheduler::{RunState, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanScore {
    pub project_coverage: f64,
    pub file_jaccard: f64,
    pub forbidden_hit: bool,
    pub task_budget_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractScore {
    pub catches_at_plan: bool,
    pub catches_at_verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchResult {
    pub fixture_id: String,
    pub kind: FixtureKind,
    pub plan_score: PlanScore,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_score: Option<ContractScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cache_miss: bool,
    pub passed: bool,
    pub runtime_ms: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn score_plan(plan: &Plan, expected: &Expected) -> PlanScore {
    let plan_projects: BTreeSet<&str> = plan
        .tasks
        .iter()
        .map(|task| task.project.as_str())
        .filter(|project| !project.is_empty())
        .collect();
    let required_projects: BTreeSet<&str> = expected
        .required_projects
        .iter()
        .map(String::as_str)
        .collect();
    let covered = required_projects
        .iter()
        .filter(|project| plan_projects.contains(**project))
        .count();
    let project_coverage = ratio(covered, required_projects.len());

    let planned_files = planned_files(plan, None);
    let expected_files: BTreeSet<&str> =
        expected.touched_files.iter().map(String::as_str).collect();
    let file_jaccard = jaccard(&planned_files, &expected_files);

    let forbidden_hit = expected
        .forbidden_files
        .iter()
        .any(|file| planned_files.contains(file.as_str()));

    PlanScore {
        project_coverage,
        file_jaccard,
        forbidden_hit,
        task_budget_ok: true,
    }
}

pub fn score_contract(
    plan: &Plan,
    run_state: Option<&RunState>,
    expected: &Expected,
) -> ContractScore {
    let plan_projects: BTreeSet<&str> = plan
        .tasks
        .iter()
        .map(|task| task.project.as_str())
        .filter(|project| !project.is_empty())
        .collect();
    let catches_at_plan = expected
        .required_contract_consumers
        .iter()
        .any(|consumer| plan_projects.contains(consumer.as_str()));
    let catches_at_verify = run_state
        .map(|state| {
            state
                .tasks
                .values()
                .any(|task| task.kind == "verify" && task.status == TaskStatus::Failed)
        })
        .unwrap_or(false);

    ContractScore {
        catches_at_plan,
        catches_at_verify,
    }
}

pub fn evaluate(run: &BenchRun, fixture: &Fixture) -> BenchResult {
    let plan = run.plan.as_ref();
    let mut plan_score = plan
        .map(|plan| score_plan_with_state(plan, run.run_state.as_ref(), &fixture.expected))
        .unwrap_or_else(|| PlanScore {
            project_coverage: 0.0,
            file_jaccard: 0.0,
            forbidden_hit: false,
            task_budget_ok: false,
        });
    if let Some(plan) = plan {
        plan_score.task_budget_ok = plan.tasks.len() <= fixture.budget.max_tasks;
    }
    let contract_score = if matches!(fixture.kind, FixtureKind::ContractBreak) {
        plan.map(|plan| score_contract(plan, run.run_state.as_ref(), &fixture.expected))
    } else {
        None
    };
    let passed = match fixture.kind {
        FixtureKind::OssReplay => {
            plan_score.project_coverage >= 1.0
                && plan_score.file_jaccard >= 0.5
                && !plan_score.forbidden_hit
                && plan_score.task_budget_ok
        }
        FixtureKind::ContractBreak => contract_score
            .as_ref()
            .map(|score| score.catches_at_plan || score.catches_at_verify)
            .unwrap_or(false),
    };
    let runtime_ms = chrono::Utc::now()
        .signed_duration_since(run.started_at)
        .num_milliseconds()
        .max(0) as u64;

    BenchResult {
        fixture_id: fixture.id.clone(),
        kind: fixture.kind,
        plan_score,
        contract_score,
        error: run.error.clone(),
        cache_miss: false,
        passed,
        runtime_ms,
    }
}

fn score_plan_with_state(plan: &Plan, state: Option<&RunState>, expected: &Expected) -> PlanScore {
    let mut score = score_plan(plan, expected);
    let planned = planned_files(plan, state);
    let expected_files: BTreeSet<&str> =
        expected.touched_files.iter().map(String::as_str).collect();
    score.file_jaccard = jaccard(&planned, &expected_files);
    score.forbidden_hit = expected
        .forbidden_files
        .iter()
        .any(|file| planned.contains(file.as_str()));
    score
}

fn planned_files<'a>(plan: &'a Plan, state: Option<&'a RunState>) -> BTreeSet<&'a str> {
    let mut files = BTreeSet::new();
    for task in &plan.tasks {
        for output in task.outputs.values() {
            if let Some(path) = output.path.as_deref() {
                files.insert(path);
            }
        }
    }
    if let Some(state) = state {
        for task in state.tasks.values() {
            for file in &task.artifacts.files_changed {
                files.insert(file.as_str());
            }
            for output in task.workflow_outputs.values() {
                if let Some(source) = output.source_path.as_deref() {
                    files.insert(source);
                }
            }
        }
    }
    files
}

fn ratio(num: usize, denom: usize) -> f64 {
    if denom == 0 {
        1.0
    } else {
        num as f64 / denom as f64
    }
}

fn jaccard(a: &BTreeSet<&str>, b: &BTreeSet<&str>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    ratio(intersection, union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::fixture::{Budget, Fixture};
    use crate::config::{PlanTask, TaskKind, TaskOutput};
    use std::collections::BTreeMap;

    fn plan(projects_and_files: &[(&str, &[&str])]) -> Plan {
        Plan {
            spec: "bench".to_string(),
            created_by: None,
            confirmed_at: None,
            contracts_change: Vec::new(),
            tasks: projects_and_files
                .iter()
                .enumerate()
                .map(|(idx, (project, files))| {
                    let mut outputs = BTreeMap::new();
                    for (file_idx, file) in files.iter().enumerate() {
                        outputs.insert(
                            format!("file_{file_idx}"),
                            TaskOutput {
                                path: Some((*file).to_string()),
                                description: String::new(),
                                required: true,
                                max_bytes: 64 * 1024,
                            },
                        );
                    }
                    PlanTask {
                        id: format!("T{idx}"),
                        project: (*project).to_string(),
                        project_each: Vec::new(),
                        kind: TaskKind::Agent,
                        prompt: String::new(),
                        command: None,
                        depends_on: Vec::new(),
                        parallel_group: None,
                        requires_approval_after: false,
                        timeout_minutes: 30,
                        memory_inject: Vec::new(),
                        skills: Vec::new(),
                        inputs: BTreeMap::new(),
                        outputs,
                        agent: None,
                        model: None,
                        model_profile: None,
                        role: None,
                        review_by: None,
                    }
                })
                .collect(),
            verification: BTreeMap::new(),
            notice: None,
            goal: None,
        }
    }

    #[test]
    fn plan_score_jaccard_correct() {
        let plan = plan(&[("api", &["a.rs", "b.rs"]), ("web", &["c.ts"])]);
        let expected = Expected {
            touched_files: vec!["a.rs".into(), "b.rs".into(), "d.ts".into()],
            required_projects: vec!["api".into(), "web".into()],
            forbidden_files: vec!["c.ts".into()],
            required_contract_consumers: Vec::new(),
        };

        let score = score_plan(&plan, &expected);

        assert_eq!(score.project_coverage, 1.0);
        assert!((score.file_jaccard - 0.5).abs() < f64::EPSILON);
        assert!(score.forbidden_hit);
        assert!(score.task_budget_ok);
    }

    #[test]
    fn contract_break_catches_at_plan() {
        let plan = plan(&[("producer", &[]), ("consumer", &[])]);
        let expected = Expected {
            required_contract_consumers: vec!["consumer".into()],
            ..Expected::default()
        };

        let score = score_contract(&plan, None, &expected);

        assert!(score.catches_at_plan);
        assert!(!score.catches_at_verify);
    }

    #[test]
    fn evaluate_contract_break_passes_when_consumer_is_in_plan() {
        let plan = plan(&[("consumer", &[])]);
        let fixture = Fixture {
            id: "cb".to_string(),
            kind: FixtureKind::ContractBreak,
            goal: "catch break".to_string(),
            upstream: None,
            expected: Expected {
                required_contract_consumers: vec!["consumer".into()],
                ..Expected::default()
            },
            budget: Budget::default(),
        };
        let run = BenchRun {
            id: "run".to_string(),
            fixture_id: "cb".to_string(),
            started_at: chrono::Utc::now(),
            plan: Some(plan),
            run_state: None,
            error: None,
        };

        let result = evaluate(&run, &fixture);

        assert!(result.passed);
        assert!(result.contract_score.unwrap().catches_at_plan);
    }

    #[test]
    fn evaluate_preserves_run_error_for_json_reports() {
        let fixture = Fixture {
            id: "oss".to_string(),
            kind: FixtureKind::OssReplay,
            goal: "replay".to_string(),
            upstream: None,
            expected: Expected::default(),
            budget: Budget::default(),
        };
        let run = BenchRun {
            id: "run".to_string(),
            fixture_id: "oss".to_string(),
            started_at: chrono::Utc::now(),
            plan: None,
            run_state: None,
            error: Some("fixture oss not cached and --offline set".to_string()),
        };

        let result = evaluate(&run, &fixture);

        assert_eq!(
            result.error.as_deref(),
            Some("fixture oss not cached and --offline set")
        );
        assert!(!result.passed);
    }
}
