use crate::bench::fixture::{Fixture, FixtureKind};
use crate::bench::runner::BenchRun;
use crate::bench::score::BenchResult;
use crate::paths;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn write_lesson_draft(
    run: &BenchRun,
    fixture: &Fixture,
    result: &BenchResult,
) -> Result<Option<PathBuf>> {
    write_lesson_draft_in_workspace(&paths::workspace_root()?, run, fixture, result)
}

fn write_lesson_draft_in_workspace(
    workspace_root: &Path,
    run: &BenchRun,
    fixture: &Fixture,
    result: &BenchResult,
) -> Result<Option<PathBuf>> {
    if result.passed {
        return Ok(None);
    }

    let dir = workspace_root.join("docs").join("lessons");
    std::fs::create_dir_all(&dir).with_context(|| format!("create lessons dir {dir:?}"))?;
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let path = dir.join(format!("{date}-bench-{}.md", fixture.id));
    std::fs::write(&path, render_lesson(run, fixture, result))
        .with_context(|| format!("write bench lesson draft {path:?}"))?;
    Ok(Some(path))
}

fn render_lesson(run: &BenchRun, fixture: &Fixture, result: &BenchResult) -> String {
    let actual_projects = run
        .plan
        .as_ref()
        .map(|plan| {
            plan.tasks
                .iter()
                .map(|task| task.project.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(no plan)".to_string());
    let actual_files = run
        .plan
        .as_ref()
        .map(|plan| {
            plan.tasks
                .iter()
                .flat_map(|task| task.outputs.values())
                .filter_map(|output| output.path.as_deref())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(no files)".to_string());
    let next_step = match fixture.kind {
        FixtureKind::OssReplay if result.plan_score.project_coverage < 1.0 => {
            "Planner prompt: improve project selection for dependency-aware changes."
        }
        FixtureKind::OssReplay if result.plan_score.file_jaccard < 0.5 => {
            "Planner prompt or replay adapter: improve file/output prediction for this change shape."
        }
        FixtureKind::OssReplay if result.plan_score.forbidden_hit => {
            "Planner guardrails: tighten noise filtering for unrelated files."
        }
        FixtureKind::ContractBreak => {
            "Role/skill coverage: ensure contract consumers are planned or verify catches the break."
        }
        _ => "Inspect fixture output and update the benchmark or planner heuristic.",
    };

    format!(
        r#"# Bench lesson: {fixture_id}

## Fixture

- kind: `{kind:?}`
- goal: {goal}
- run: `{run_id}`

## Expected

- projects: {expected_projects}
- touched files: {expected_files}
- contract consumers: {contract_consumers}
- forbidden files: {forbidden_files}

## Actual

- planned projects: {actual_projects}
- planned files: {actual_files}
- run error: {error}

## Score

- passed: `{passed}`
- project coverage: `{project_coverage:.2}`
- file jaccard: `{file_jaccard:.2}`
- forbidden hit: `{forbidden_hit}`
- task budget ok: `{task_budget_ok}`

## Failure point

{failure_point}

## Suggested next step

{next_step}
"#,
        fixture_id = fixture.id,
        kind = fixture.kind,
        goal = fixture.goal,
        run_id = run.id,
        expected_projects = list(&fixture.expected.required_projects),
        expected_files = list(&fixture.expected.touched_files),
        contract_consumers = list(&fixture.expected.required_contract_consumers),
        forbidden_files = list(&fixture.expected.forbidden_files),
        actual_projects = actual_projects,
        actual_files = actual_files,
        error = run.error.as_deref().unwrap_or("(none)"),
        passed = result.passed,
        project_coverage = result.plan_score.project_coverage,
        file_jaccard = result.plan_score.file_jaccard,
        forbidden_hit = result.plan_score.forbidden_hit,
        task_budget_ok = result.plan_score.task_budget_ok,
        failure_point = failure_point(result),
        next_step = next_step,
    )
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

fn failure_point(result: &BenchResult) -> &'static str {
    if result.plan_score.project_coverage < 1.0 {
        "Plan missed at least one required project."
    } else if result.plan_score.file_jaccard < 0.5 {
        "Plan file overlap fell below the replay threshold."
    } else if result.plan_score.forbidden_hit {
        "Plan touched a forbidden file."
    } else if !result.plan_score.task_budget_ok {
        "Plan exceeded the fixture task budget."
    } else if result
        .contract_score
        .as_ref()
        .map(|score| !score.catches_at_plan && !score.catches_at_verify)
        .unwrap_or(false)
    {
        "Contract break was not exposed at plan or verify time."
    } else {
        "Benchmark failed without a more specific score signal."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::fixture::{Budget, Expected, FixtureKind};
    use crate::bench::score::PlanScore;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    #[test]
    fn writes_lesson_for_failed_fixture() {
        let workspace = workspace();
        let fixture = Fixture {
            id: "oss-miss".to_string(),
            kind: FixtureKind::OssReplay,
            goal: "Replay a missed plan".to_string(),
            upstream: None,
            expected: Expected {
                required_projects: vec!["core".to_string()],
                touched_files: vec!["core/src/lib.rs".to_string()],
                ..Expected::default()
            },
            budget: Budget::default(),
        };
        let run = BenchRun {
            id: "run-1".to_string(),
            fixture_id: fixture.id.clone(),
            started_at: chrono::Utc::now(),
            plan: None,
            run_state: None,
            error: Some("planner returned no plan".to_string()),
        };
        let result = BenchResult {
            fixture_id: fixture.id.clone(),
            kind: FixtureKind::OssReplay,
            plan_score: PlanScore {
                project_coverage: 0.0,
                file_jaccard: 0.0,
                forbidden_hit: false,
                task_budget_ok: false,
            },
            contract_score: None,
            error: Some("planner returned no plan".to_string()),
            cache_miss: false,
            passed: false,
            runtime_ms: 1,
        };

        let path = write_lesson_draft_in_workspace(workspace.path(), &run, &fixture, &result)
            .expect("lesson")
            .expect("path");
        let text = std::fs::read_to_string(&path).expect("lesson text");

        assert!(path.starts_with(workspace.path().join("docs/lessons")));
        assert!(text.contains("Bench lesson: oss-miss"));
        assert!(text.contains("Plan missed at least one required project."));
    }

    #[test]
    fn skips_lesson_for_passed_fixture() {
        let workspace = workspace();
        let fixture = Fixture {
            id: "ok".to_string(),
            kind: FixtureKind::OssReplay,
            goal: "ok".to_string(),
            upstream: None,
            expected: Expected::default(),
            budget: Budget::default(),
        };
        let run = BenchRun {
            id: "run".to_string(),
            fixture_id: fixture.id.clone(),
            started_at: chrono::Utc::now(),
            plan: None,
            run_state: None,
            error: None,
        };
        let result = BenchResult {
            fixture_id: fixture.id.clone(),
            kind: FixtureKind::OssReplay,
            plan_score: PlanScore {
                project_coverage: 1.0,
                file_jaccard: 1.0,
                forbidden_hit: false,
                task_budget_ok: true,
            },
            contract_score: None,
            error: None,
            cache_miss: false,
            passed: true,
            runtime_ms: 1,
        };

        let path = write_lesson_draft_in_workspace(workspace.path(), &run, &fixture, &result)
            .expect("lesson");

        assert!(path.is_none());
    }
}
