//! Reports module: REPORT.md generation + L2 decision archive.
//! Tests construct a synthetic RunState directly so they don't depend on
//! the executor at all.

use std::collections::BTreeMap;

use chrono::Utc;
use maestro::config::{Plan, Project, ProjectsConfig};
use maestro::reports::{archive_l2_decision, write_run_report};
use maestro::scheduler::{RunState, RunStatus, TaskStatus};
use serial_test::serial;
use tempfile::TempDir;

fn fresh_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
    }
    dir
}
fn clear() {
    unsafe {
        std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
    }
}

fn make_plan() -> (Plan, ProjectsConfig) {
    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    for name in ["api", "web"] {
        projects.projects.insert(
            name.into(),
            Project {
                path: format!("./{name}"),
                r#type: None,
                stack: vec![],
                commands: BTreeMap::new(),
                contracts: Default::default(),
                dependencies: Vec::new(),
                memory_scope: vec![],
                agent: Some("shell".into()),
                agent_model: None,
                cursor_model: None,
                model_profile: None,
                role: None,
                copy_files: Vec::new(),
            },
        );
    }
    let plan: Plan = serde_yaml::from_str(
        r#"
spec: do a thing
tasks:
  - id: T_api
    project: api
    prompt: "rebuild auth"
  - id: T_web
    project: web
    prompt: "wire new endpoint"
    depends_on: [T_api]
  - id: T_verify
    project: _global
    kind: verify
    command: "echo ok"
    depends_on: [T_web]
"#,
    )
    .unwrap();
    (plan, projects)
}

fn finished_state(plan: &Plan, projects: &ProjectsConfig, run_dir: std::path::PathBuf) -> RunState {
    let mut state = RunState::new(
        "20260516-101010_abcd1234".into(),
        plan,
        projects,
        4,
        run_dir,
    );
    state.started_at = Utc::now();
    state.ended_at = Some(Utc::now());
    state.status = RunStatus::Done;
    for t in state.tasks.values_mut() {
        t.status = TaskStatus::Done;
        t.started_at = Some(Utc::now());
        t.ended_at = Some(Utc::now());
    }
    state
}

#[test]
#[serial]
fn write_run_report_creates_both_canonical_and_mirror() {
    let dir = fresh_workspace();
    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join("20260516-101010_abcd1234");
    std::fs::create_dir_all(&run_dir).unwrap();

    let (plan, projects) = make_plan();
    let state = finished_state(&plan, &projects, run_dir.clone());

    write_run_report(&plan, &state).unwrap();

    // Canonical
    let canonical = run_dir.join("REPORT.md");
    assert!(canonical.exists(), "missing {canonical:?}");
    let body = std::fs::read_to_string(&canonical).unwrap();
    assert!(body.contains("do a thing"), "spec missing from body");
    assert!(body.contains("`T_api`"));
    assert!(body.contains("`T_web`"));

    // Mirror lives under <workspace>/plans/<date>-<slug>.report.md
    let plans_dir = dir.path().join("plans");
    let entries: Vec<_> = std::fs::read_dir(&plans_dir).unwrap().collect();
    assert!(!entries.is_empty(), "no mirror report in plans/");

    clear();
}

#[test]
#[serial]
fn write_run_report_surfaces_task_errors() {
    let dir = fresh_workspace();
    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join("20260516-101010_abcd1234");
    std::fs::create_dir_all(&run_dir).unwrap();

    let (plan, projects) = make_plan();
    let mut state = finished_state(&plan, &projects, run_dir.clone());
    state.status = RunStatus::Failed;
    state.tasks.get_mut("T_api").unwrap().status = TaskStatus::Failed;
    state.tasks.get_mut("T_api").unwrap().error = Some("boom: exit 1".into());

    write_run_report(&plan, &state).unwrap();

    let body = std::fs::read_to_string(run_dir.join("REPORT.md")).unwrap();
    assert!(body.contains("boom: exit 1"), "error not in report");
    assert!(body.to_lowercase().contains("failed"));

    clear();
}

#[test]
#[serial]
fn archive_l2_decision_writes_per_project_files() {
    let dir = fresh_workspace();
    let run_dir = dir.path().join(".maestro").join("runs").join("rid");
    std::fs::create_dir_all(&run_dir).unwrap();

    let (plan, projects) = make_plan();
    let state = finished_state(&plan, &projects, run_dir);

    archive_l2_decision(&plan, &state).unwrap();

    let l2 = dir
        .path()
        .join(".maestro")
        .join("memory")
        .join("l2_decisions");
    let api_dir = l2.join("api");
    let web_dir = l2.join("web");
    assert!(api_dir.exists(), "api L2 dir missing");
    assert!(web_dir.exists(), "web L2 dir missing");

    // `_global` tasks (T_verify) shouldn't get their own scope
    assert!(
        !l2.join("_global").exists(),
        "_global decisions should not be archived"
    );

    // The archived file mentions the task that touched the project
    let api_files: Vec<_> = std::fs::read_dir(&api_dir).unwrap().collect();
    assert_eq!(api_files.len(), 1);
    let body = std::fs::read_to_string(api_files[0].as_ref().unwrap().path()).unwrap();
    assert!(body.contains("T_api"));
    assert!(body.contains("do a thing"));

    clear();
}

#[test]
#[serial]
fn archive_l2_decision_no_projects_is_noop() {
    let dir = fresh_workspace();
    let run_dir = dir.path().join(".maestro").join("runs").join("rid");
    std::fs::create_dir_all(&run_dir).unwrap();

    let projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    // Plan with only `_global` tasks
    let plan: Plan = serde_yaml::from_str(
        r#"
spec: cleanup
tasks:
  - id: T_only_global
    project: _global
    kind: verify
    command: "echo nothing"
"#,
    )
    .unwrap();
    let state = finished_state(&plan, &projects, run_dir);

    archive_l2_decision(&plan, &state).unwrap();

    let l2 = dir
        .path()
        .join(".maestro")
        .join("memory")
        .join("l2_decisions");
    // Either doesn't exist at all, or exists but empty
    if l2.exists() {
        let kids: Vec<_> = std::fs::read_dir(&l2).unwrap().collect();
        assert!(
            kids.is_empty(),
            "no project dirs should be archived when only _global tasks ran"
        );
    }

    clear();
}

#[test]
#[serial]
fn report_renders_automatic_actions_section() {
    let dir = fresh_workspace();
    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join("20260516-101010_abcd1234");
    std::fs::create_dir_all(&run_dir).unwrap();

    let (plan, projects) = make_plan();
    let mut state = finished_state(&plan, &projects, run_dir.clone());
    state.auto_actions = vec![
        maestro::scheduler::AutoAction {
            kind: "contract_wired".into(),
            task: Some("T_web".into()),
            detail: "`T_web` set to depend on `T_api` (contract `types/x.d.ts`)".into(),
        },
        maestro::scheduler::AutoAction {
            kind: "circuit_break".into(),
            task: Some("T_api".into()),
            detail: "stopped retrying after 2 identical failure(s)".into(),
        },
    ];

    write_run_report(&plan, &state).unwrap();

    let body = std::fs::read_to_string(run_dir.join("REPORT.md")).unwrap();
    assert!(body.contains("## Automatic actions"), "section missing");
    assert!(body.contains("Contract dependencies wired"));
    assert!(body.contains("Circuit breaker"));
    assert!(body.contains("set to depend on"));

    clear();
}
