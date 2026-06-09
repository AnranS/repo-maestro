//! Integration tests for `maestro::scheduler::executor`. These run the real
//! executor end-to-end against shell-`verify` tasks (no `cursor-agent` needed),
//! pointing it at a `tempfile::TempDir` via `MAESTRO_WORKSPACE_ROOT`.
//!
//! Why `#[serial]` everywhere: `MAESTRO_WORKSPACE_ROOT` is process-global state,
//! and running these tests in parallel would race on it.

use std::collections::BTreeMap;
use std::time::Duration;

use maestro::config::{AgentProfile, Plan, Project, ProjectsConfig};
use maestro::modes::AllowedTools;
use maestro::scheduler::trajectory::read_trajectory;
use maestro::scheduler::{run_plan, ExecConfig, RunStatus, TaskStatus};
use maestro::schema::permissions::{resolve_permission_evidence, Enforcement};
use maestro::schema::trajectory::{Redaction, TrajectoryEventKind};
use maestro::skills::{save, SkillScope};
use serial_test::serial;
use tempfile::TempDir;

fn make_workspace() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    // Point maestro at this dir for the duration of the test. Restored to a
    // sensible empty default at the end so subsequent tests can either rely
    // on their own override or fall back to cwd.
    unsafe {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
    }
    dir
}

fn clear_workspace() {
    unsafe {
        std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
    }
}

fn projects_for(workspace: &std::path::Path, names: &[&str]) -> ProjectsConfig {
    let mut p = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    for n in names {
        p.projects.insert(
            (*n).to_string(),
            Project {
                // Absolute path so resolution doesn't fall back to process cwd
                // (which is the real project, not our tempdir).
                path: workspace.join(n).to_string_lossy().to_string(),
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
                agent_profile: None,
                review_profile: None,
                copy_files: Vec::new(),
            },
        );
    }
    p
}

/// Build a plan from raw YAML and run expand + validate so callers get the
/// same massaging the CLI applies.
fn parse_plan(yaml: &str) -> Plan {
    let mut plan: Plan = serde_yaml::from_str(yaml).expect("parse plan");
    plan = plan.expand_for_each();
    plan.validate().expect("plan validates");
    plan
}

/// Make project directories so workspace path resolution succeeds.
fn ensure_dirs(workspace: &std::path::Path, names: &[&str]) {
    for n in names {
        std::fs::create_dir_all(workspace.join(n)).unwrap();
    }
}

fn init_git_repo(path: &std::path::Path) {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["commit", "--allow-empty", "-q", "-m", "init"]);
}

#[test]
fn permission_evidence_resolver_covers_enforcement_states() {
    let shell = resolve_permission_evidence(
        "T_shell",
        "shell",
        "qa",
        &AllowedTools {
            shell: true,
            git_write: false,
            network: false,
            allowed_commands: vec!["npm test*".to_string()],
        },
    );
    assert_eq!(shell.resolved.shell, Enforcement::Hard);
    assert_eq!(shell.resolved.git_write, Enforcement::Soft);
    assert_eq!(shell.requested.allowed_commands, vec!["npm test*"]);

    let mock =
        resolve_permission_evidence("T_mock", "mock", "permissive", &AllowedTools::default());
    assert_eq!(mock.resolved.shell, Enforcement::NotApplicable);
    assert_eq!(mock.resolved.network, Enforcement::NotApplicable);

    let codex = resolve_permission_evidence(
        "T_codex",
        "codex",
        "restricted",
        &AllowedTools {
            shell: true,
            git_write: false,
            network: false,
            allowed_commands: Vec::new(),
        },
    );
    assert_eq!(codex.resolved.network, Enforcement::Hard);
    assert_eq!(codex.resolved.shell, Enforcement::Soft);
    assert_eq!(codex.resolved.git_write, Enforcement::Soft);

    let known_cli =
        resolve_permission_evidence("T_gemini", "gemini", "permissive", &AllowedTools::default());
    assert_eq!(known_cli.resolved.shell, Enforcement::Unsupported);
    assert_eq!(known_cli.resolved.git_write, Enforcement::Unsupported);
    assert_eq!(known_cli.resolved.network, Enforcement::Unsupported);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn permission_evidence_is_written_to_state_and_summary() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: permission evidence
tasks:
  - id: T_permissions
    project: api
    kind: verify
    agent: shell
    command: "echo permissions"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    let permission = final_state.tasks["T_permissions"]
        .permission
        .as_ref()
        .expect("task permission evidence");
    assert_eq!(permission.schema_version, "maestro.permission.v1");
    assert_eq!(permission.provider_id, "shell");
    assert_eq!(permission.resolved.shell, Enforcement::Hard);

    let summary_path = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("evidence")
        .join("summary.json");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(summary_path).unwrap()).unwrap();
    assert_eq!(
        evidence["tasks"][0]["permission"]["schema_version"],
        "maestro.permission.v1"
    );
    assert_eq!(
        evidence["tasks"][0]["permission"]["resolved"]["shell"],
        "hard"
    );
    let manifest_path = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("evidence")
        .join("artifacts.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], "maestro.artifact_manifest.v1");

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f114_writer_profile_is_lowered_and_recorded_in_task_state() {
    // End-to-end: a project-bound `agent_profile` is resolved at dispatch, its
    // role lowered into the task, and its name recorded as provenance in
    // RunState (the wiring the unit tests can't see).
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.defaults.agent_profiles.insert(
        "writer".into(),
        AgentProfile {
            role: "backend_rust".into(), // builtin role
            skills: vec![],
            model_profile: None,
            context_budget_bytes: None,
            priority: 0,
            triggers: vec![],
            outputs: vec![],
            enabled: true,
        },
    );
    projects.projects.get_mut("api").unwrap().agent_profile = Some("writer".into());

    let plan = parse_plan(
        r#"
spec: writer profile
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    let t0 = &final_state.tasks["T0"];
    assert_eq!(t0.status, TaskStatus::Done);
    assert_eq!(
        t0.resolved_agent_profile.as_deref(),
        Some("writer"),
        "the bound profile name is recorded as provenance"
    );
    assert_eq!(
        t0.role.as_deref(),
        Some("backend_rust"),
        "the profile's role is lowered into the task"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f114_dispatch_fails_fast_on_unknown_project_agent_profile() {
    // A project that pins a profile the workspace never defines must fail the
    // task at dispatch, not silently fall back to default routing.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.projects.get_mut("api").unwrap().agent_profile = Some("ghost".into());

    let plan = parse_plan(
        r#"
spec: bad profile
tasks:
  - id: T0
    project: api
    kind: agent
    agent: mock
    prompt: "do thing"
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    let t0 = &final_state.tasks["T0"];
    assert_eq!(t0.status, TaskStatus::Failed);
    assert!(
        t0.error.as_deref().unwrap_or_default().contains("ghost"),
        "failure must name the undefined profile: {:?}",
        t0.error
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f114_review_profile_provenance_is_recorded_in_task_state() {
    // Step 7: a project `review_profile` that runs is recorded on the task as
    // `resolved_review_profile` (the provenance the UI label shows).
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.defaults.agent_profiles.insert(
        "rev".into(),
        AgentProfile {
            role: "refuter".into(),
            skills: vec![],
            model_profile: None,
            context_budget_bytes: None,
            priority: 0,
            triggers: vec![],
            outputs: vec![],
            enabled: true,
        },
    );
    projects.projects.get_mut("api").unwrap().review_profile = Some("rev".into());

    let plan = parse_plan(
        r#"
spec: review provenance
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    assert_eq!(
        final_state.tasks["T0"].resolved_review_profile.as_deref(),
        Some("rev"),
        "the review profile that ran is recorded as provenance"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f114_review_fails_fast_on_unknown_review_profile() {
    // Step 5 guard: a finished task whose `review_profile` names a profile the
    // workspace never defines must fail (never silently skip the review). The
    // task itself succeeds first; the review step then fails it.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.projects.get_mut("api").unwrap().review_profile = Some("ghost_reviewer".into());

    let plan = parse_plan(
        r#"
spec: bad review profile
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    let t0 = &final_state.tasks["T0"];
    assert_eq!(t0.status, TaskStatus::Failed);
    assert!(
        t0.error
            .as_deref()
            .unwrap_or_default()
            .contains("ghost_reviewer"),
        "failure must name the undefined review profile: {:?}",
        t0.error
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f114_explicit_review_by_wins_over_bad_project_review_profile() {
    // Step-5 N2: a higher-priority explicit `review_by` must short-circuit
    // before the project `review_profile`, so a bad (undefined) project
    // `review_profile` can NOT fail the task as a config error.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.projects.get_mut("api").unwrap().review_profile = Some("ghost".into());

    let plan = parse_plan(
        r#"
spec: review_by wins
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
    review_by: refuter
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    // The bad project review_profile is shadowed by the explicit review_by, so
    // whatever the review outcome, the task must not fail as a `ghost` config
    // error.
    let err = final_state.tasks["T0"]
        .error
        .as_deref()
        .unwrap_or_default()
        .to_string();
    assert!(
        !err.contains("ghost"),
        "explicit review_by must shadow the bad project review_profile: {err:?}"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn run_plan_uses_configured_run_id_for_targeted_cancellation() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);
    let plan = parse_plan(
        r#"
spec: configured run id
tasks:
  - id: T_verify
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
"#,
    );

    let final_state = run_plan(
        plan,
        projects,
        ExecConfig {
            run_id: Some("t4-f002-target".to_string()),
            ..ExecConfig::default()
        },
    )
    .await
    .expect("run_plan");

    assert_eq!(final_state.run_id, "t4-f002-target");
    assert_eq!(
        final_state.pid,
        std::process::id(),
        "run state should persist the writer process id for liveness checks"
    );
    assert!(dir
        .path()
        .join(".maestro")
        .join("runs")
        .join("t4-f002-target")
        .join("RUN_STATE.json")
        .exists());

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn shell_task_writes_trajectory_and_evidence_refs() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: trajectory evidence
tasks:
  - id: T_trace
    project: api
    kind: verify
    agent: shell
    command: "AWS_SECRET_ACCESS_KEY=supersecret printf trace"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    let task = &final_state.tasks["T_trace"];
    let trajectory_path = task.trajectory_path.as_ref().expect("task trajectory path");
    let events = read_trajectory(std::path::Path::new(trajectory_path)).unwrap();
    assert_eq!(events.first().unwrap().kind, TrajectoryEventKind::Command);
    assert_eq!(events.first().unwrap().redaction, Redaction::SecretStripped);
    assert!(!events
        .first()
        .unwrap()
        .command
        .as_ref()
        .unwrap()
        .contains("supersecret"));
    assert_eq!(events.last().unwrap().kind, TrajectoryEventKind::Final);

    let summary_path = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("evidence")
        .join("summary.json");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(summary_path).unwrap()).unwrap();
    assert_eq!(evidence["tasks"][0]["trajectory_path"], *trajectory_path);
    assert_eq!(
        evidence["tasks"][0]["artifact_refs"][0]["kind"],
        "trajectory"
    );
    // F-124 B1: the artifact ref path must be run-relative (real writer output),
    // NOT the absolute on-disk trajectory_path.
    assert_eq!(
        evidence["tasks"][0]["artifact_refs"][0]["path"],
        "trajectories/T_trace.ndjson"
    );
    let summary_ref_path = evidence["tasks"][0]["artifact_refs"][0]["path"]
        .as_str()
        .unwrap();
    assert!(
        !std::path::Path::new(summary_ref_path).is_absolute(),
        "trajectory ref path must not be absolute: {summary_ref_path}"
    );

    let manifest_path = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("evidence")
        .join("artifacts.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["artifacts"][0]["kind"], "trajectory");
    assert_eq!(
        manifest["artifacts"][0]["path"],
        "trajectories/T_trace.ndjson"
    );

    clear_workspace();
}

// ─── 1. Happy-path linear chain ──────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn linear_chain_completes_all_done() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: chain test
tasks:
  - id: T1
    project: api
    kind: verify
    agent: shell
    command: "echo T1 ran"
  - id: T2
    project: web
    kind: verify
    agent: shell
    depends_on: [T1]
    command: "echo T2 ran"
  - id: T3
    project: _global
    kind: verify
    agent: shell
    depends_on: [T2]
    command: "echo T3 ran"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    for id in ["T1", "T2", "T3"] {
        assert_eq!(
            final_state.tasks[id].status,
            TaskStatus::Done,
            "task {id} should be done"
        );
    }

    clear_workspace();
}

// ─── 2. Failure halts downstream by default ──────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn failure_aborts_downstream_when_continue_on_error_is_false() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: failure test
tasks:
  - id: T_bad
    project: api
    kind: verify
    agent: shell
    command: "exit 7"
  - id: T_downstream
    project: web
    kind: verify
    agent: shell
    depends_on: [T_bad]
    command: "echo unreachable"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);
    assert_eq!(final_state.tasks["T_bad"].status, TaskStatus::Failed);
    // Downstream task was never started — it stays Pending (not Skipped),
    // because the abort happens immediately after T_bad fails.
    assert!(matches!(
        final_state.tasks["T_downstream"].status,
        TaskStatus::Pending | TaskStatus::Skipped
    ));
    assert!(final_state.tasks["T_bad"].error.is_some());

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn failure_aborts_running_siblings_as_cancelled() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: abort running sibling
tasks:
  - id: T_bad
    project: api
    kind: verify
    agent: shell
    command: "exit 1"
  - id: T_slow
    project: web
    kind: verify
    agent: shell
    command: "sleep 10; echo slow"
"#,
    );

    let final_state = run_plan(
        plan,
        projects,
        ExecConfig {
            max_parallel: 2,
            ..ExecConfig::default()
        },
    )
    .await
    .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);
    assert_eq!(final_state.tasks["T_bad"].status, TaskStatus::Failed);
    assert_eq!(final_state.tasks["T_slow"].status, TaskStatus::Cancelled);
    assert_eq!(
        final_state.tasks["T_slow"].error.as_deref(),
        Some("run aborted after task failure")
    );

    clear_workspace();
}

// ─── 2b. Circuit breaker: stop retrying when the error doesn't change ─────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn circuit_breaker_stops_retry_when_error_repeats() {
    // A task that fails deterministically with the same error every run. The
    // bounded-retry budget is 2 (3 executions), but the circuit breaker should
    // trip on the 2nd execution because the error signature repeats — so the
    // task is only retried once (attempts == 1), not twice.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: circuit breaker test
tasks:
  - id: T_flaky
    project: api
    kind: verify
    agent: shell
    command: "echo 'boom: deterministic failure' >&2; exit 3"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);
    assert_eq!(final_state.tasks["T_flaky"].status, TaskStatus::Failed);
    // Circuit breaker tripped after a single retry — without it, the task would
    // have burned the full budget and reached attempts == 2.
    assert_eq!(
        final_state.tasks["T_flaky"].attempts, 1,
        "circuit breaker should stop after 1 retry when the error repeats"
    );
    // The decisions are recorded in the audit ledger.
    let kinds: Vec<&str> = final_state
        .auto_actions
        .iter()
        .map(|a| a.kind.as_str())
        .collect();
    assert!(kinds.contains(&"retry"), "ledger should record the retry");
    assert!(
        kinds.contains(&"circuit_break"),
        "ledger should record the circuit-breaker trip"
    );
    // Escalation: a distinct event (routed to notification channels) is emitted
    // so the stuck task surfaces to a human, not just a log line.
    let events = maestro::scheduler::read_events(&final_state.run_dir).expect("read events");
    let escalation = events
        .iter()
        .find(|e| e.payload.get("escalation").and_then(|v| v.as_bool()) == Some(true))
        .expect("an escalation event should be recorded");
    assert_eq!(
        escalation.payload.get("reason").and_then(|v| v.as_str()),
        Some("circuit_breaker")
    );
    assert_eq!(escalation.task_id.as_deref(), Some("T_flaky"));

    clear_workspace();
}

// ─── 3. continue_on_error keeps independent tasks going ──────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn continue_on_error_runs_independent_tasks() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: continue-on-error
tasks:
  - id: T_bad
    project: api
    kind: verify
    agent: shell
    command: "exit 1"
  - id: T_good
    project: web
    kind: verify
    agent: shell
    command: "echo good"
"#,
    );

    let cfg = ExecConfig {
        continue_on_error: true,
        ..ExecConfig::default()
    };

    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);
    assert_eq!(final_state.tasks["T_bad"].status, TaskStatus::Failed);
    assert_eq!(final_state.tasks["T_good"].status, TaskStatus::Done);

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn continue_on_error_does_not_release_failed_fanin_downstream() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: continue-on-error fan-in
tasks:
  - id: T_bad
    project: api
    kind: verify
    agent: shell
    command: "exit 1"
  - id: T_good
    project: web
    kind: verify
    agent: shell
    command: "sleep 0.1; echo good"
  - id: T_fanin
    project: _global
    kind: verify
    agent: shell
    depends_on: [T_bad, T_good]
    command: "echo should-not-run"
"#,
    );

    let cfg = ExecConfig {
        continue_on_error: true,
        max_parallel: 2,
        ..ExecConfig::default()
    };

    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);
    assert_eq!(final_state.tasks["T_bad"].status, TaskStatus::Failed);
    assert_eq!(final_state.tasks["T_good"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["T_fanin"].status, TaskStatus::Skipped);
    assert_eq!(
        final_state.tasks["T_fanin"].error.as_deref(),
        Some("blocked by failed dependency: T_bad")
    );
    assert!(
        !std::path::Path::new(&final_state.tasks["T_fanin"].log_path).exists(),
        "fan-in task should not dispatch when one dependency failed"
    );

    clear_workspace();
}

// ─── 4. only/skip filtering ──────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn only_filter_runs_specified_subset() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web", "cli"]);
    let projects = projects_for(dir.path(), &["api", "web", "cli"]);

    let plan = parse_plan(
        r#"
spec: only filter
tasks:
  - id: A
    project: api
    kind: verify
    agent: shell
    command: "echo a"
  - id: B
    project: web
    kind: verify
    agent: shell
    command: "echo b"
  - id: C
    project: cli
    kind: verify
    agent: shell
    command: "echo c"
"#,
    );

    let cfg = ExecConfig {
        only: Some(vec!["A".into(), "C".into()]),
        ..ExecConfig::default()
    };

    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.tasks["A"].status, TaskStatus::Done);
    assert_eq!(
        final_state.tasks["B"].status,
        TaskStatus::Skipped,
        "B not in `only` → should be skipped"
    );
    assert_eq!(final_state.tasks["C"].status, TaskStatus::Done);

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn skip_filter_drops_named_tasks() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: skip filter
tasks:
  - id: keep
    project: api
    kind: verify
    agent: shell
    command: "echo keep"
  - id: drop
    project: web
    kind: verify
    agent: shell
    command: "echo drop"
"#,
    );

    let cfg = ExecConfig {
        skip: vec!["drop".into()],
        ..ExecConfig::default()
    };

    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.tasks["keep"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["drop"].status, TaskStatus::Skipped);

    clear_workspace();
}

// ─── 5. Approval gate ────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn approval_gate_releases_when_marker_written() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: approval test
tasks:
  - id: T_gate
    project: api
    kind: verify
    agent: shell
    command: "echo gate"
    requires_approval_after: true
  - id: T_after
    project: api
    kind: verify
    agent: shell
    depends_on: [T_gate]
    command: "echo after"
"#,
    );

    // Background task: poll for the gate task to enter awaiting_approval, then
    // write the approval marker. Without this the run hangs forever.
    let workspace = dir.path().to_path_buf();
    let approver = tokio::spawn(async move {
        let approvals_dir = workspace.join(".maestro").join("control").join("approvals");
        for _ in 0..200 {
            if approvals_dir.exists() {
                std::fs::write(approvals_dir.join("T_gate"), b"ok").ok();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("approvals dir never appeared");
    });

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    approver.await.ok();

    assert_eq!(final_state.status, RunStatus::Done);
    assert_eq!(final_state.tasks["T_gate"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["T_after"].status, TaskStatus::Done);

    clear_workspace();
}

// ─── 6. Cancellation ──────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn cancel_run_marker_stops_dag_between_tasks() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    // Chained tasks so this case pins the between-task path separately from
    // the mid-task cancellation path below.
    let plan = parse_plan(
        r#"
spec: cancel test
tasks:
  - id: T_first
    project: api
    kind: verify
    agent: shell
    command: "sleep 0.3"
  - id: T_second
    project: api
    kind: verify
    agent: shell
    depends_on: [T_first]
    command: "echo should not run"
"#,
    );

    let workspace = dir.path().to_path_buf();
    // The executor creates `.maestro/control/approvals` but not `cancels` — the
    // CLI `maestro cancel-run` is what mkdirs it. Our test acts in lieu of the
    // CLI so we mkdir it ourselves before dropping the marker.
    let canceller = tokio::spawn(async move {
        let cancels_dir = workspace.join(".maestro").join("control").join("cancels");
        std::fs::create_dir_all(&cancels_dir).ok();
        // Wait until the run dir exists (proves run_plan started), then
        // drop the marker. The check_cancelled call at the top of the
        // dispatch loop picks it up before T_second is dispatched.
        let runs_dir = workspace.join(".maestro").join("runs");
        for _ in 0..100 {
            if runs_dir.exists() {
                std::fs::write(cancels_dir.join("current"), b"cancel").ok();
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("runs dir never appeared");
    });

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    canceller.await.ok();

    assert_eq!(final_state.status, RunStatus::Cancelled);
    // T_first ran to completion; this test covers cancellation before the
    // dependent task is dispatched.
    assert_eq!(final_state.tasks["T_first"].status, TaskStatus::Done);
    // T_second never started and is finalized as cancelled because the run was cancelled.
    assert_eq!(final_state.tasks["T_second"].status, TaskStatus::Cancelled);

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn cancel_run_marker_releases_approval_wait() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: cancel approval wait
tasks:
  - id: T_gate
    project: api
    kind: verify
    agent: shell
    command: "echo gate"
    requires_approval_after: true
  - id: T_after
    project: api
    kind: verify
    agent: shell
    depends_on: [T_gate]
    command: "echo should not run"
"#,
    );

    let workspace = dir.path().to_path_buf();
    let canceller = tokio::spawn(async move {
        let cancels_dir = workspace.join(".maestro").join("control").join("cancels");
        std::fs::create_dir_all(&cancels_dir).ok();
        for _ in 0..200 {
            let current = workspace.join(".maestro").join("runs").join("current");
            let state_path = std::fs::read_link(&current)
                .ok()
                .map(|target| workspace.join(".maestro").join("runs").join(target))
                .map(|run_dir| run_dir.join("RUN_STATE.json"));
            if let Some(state_path) = state_path {
                if let Ok(text) = std::fs::read_to_string(state_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        let pending = json
                            .get("approvals_pending")
                            .and_then(|v| v.as_array())
                            .map(|items| items.iter().any(|v| v.as_str() == Some("T_gate")))
                            .unwrap_or(false);
                        if pending {
                            std::fs::write(cancels_dir.join("current"), b"cancel").ok();
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("approval wait never appeared");
    });

    let final_state = tokio::time::timeout(
        Duration::from_secs(3),
        run_plan(plan, projects, ExecConfig::default()),
    )
    .await
    .expect("run_plan should exit after cancel marker")
    .expect("run_plan");
    canceller.await.ok();

    assert_eq!(final_state.status, RunStatus::Cancelled);
    assert_eq!(final_state.tasks["T_gate"].status, TaskStatus::Cancelled);
    assert!(final_state.approvals_pending.is_empty());
    assert_eq!(final_state.tasks["T_after"].status, TaskStatus::Cancelled);

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn cancel_run_marker_aborts_running_task_promptly() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);
    let late_marker = dir.path().join("late-marker");

    let plan = parse_plan(
        r#"
spec: cancel running task
tasks:
  - id: T_slow
    project: api
    kind: verify
    agent: shell
    command: |
      sleep 1
      touch ../late-marker
"#,
    );

    let workspace = dir.path().to_path_buf();
    let canceller = tokio::spawn(async move {
        let cancels_dir = workspace.join(".maestro").join("control").join("cancels");
        std::fs::create_dir_all(&cancels_dir).ok();
        for _ in 0..200 {
            let current = workspace.join(".maestro").join("runs").join("current");
            let state_path = std::fs::read_link(&current)
                .ok()
                .map(|target| workspace.join(".maestro").join("runs").join(target))
                .map(|run_dir| run_dir.join("RUN_STATE.json"));
            if let Some(state_path) = state_path {
                if let Ok(text) = std::fs::read_to_string(state_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        let running = json
                            .get("tasks")
                            .and_then(|v| v.get("T_slow"))
                            .and_then(|v| v.get("status"))
                            .and_then(|v| v.as_str())
                            == Some("running");
                        if running {
                            std::fs::write(cancels_dir.join("current"), b"cancel").ok();
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("running task state never appeared");
    });

    let final_state = tokio::time::timeout(
        Duration::from_secs(2),
        run_plan(plan, projects, ExecConfig::default()),
    )
    .await
    .expect("run_plan should exit promptly after cancel marker")
    .expect("run_plan");
    canceller.await.ok();

    assert_eq!(final_state.status, RunStatus::Cancelled);
    assert_eq!(final_state.tasks["T_slow"].status, TaskStatus::Cancelled);
    assert_eq!(
        final_state.tasks["T_slow"].error.as_deref(),
        Some("run cancelled")
    );

    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !late_marker.exists(),
        "cancelled shell command should not continue after run_plan exits"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn cancel_run_marker_does_not_wait_for_pending_permit() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);
    let second_started = dir.path().join("second-started");

    let plan = parse_plan(
        r#"
spec: cancel while waiting for permit
tasks:
  - id: T_first
    project: api
    kind: verify
    agent: shell
    command: "sleep 5"
  - id: T_second
    project: api
    kind: verify
    agent: shell
    command: "touch ../second-started"
"#,
    );

    let workspace = dir.path().to_path_buf();
    let canceller = tokio::spawn(async move {
        let cancels_dir = workspace.join(".maestro").join("control").join("cancels");
        std::fs::create_dir_all(&cancels_dir).ok();
        for _ in 0..200 {
            let current = workspace.join(".maestro").join("runs").join("current");
            let state_path = std::fs::read_link(&current)
                .ok()
                .map(|target| workspace.join(".maestro").join("runs").join(target))
                .map(|run_dir| run_dir.join("RUN_STATE.json"));
            if let Some(state_path) = state_path {
                if let Ok(text) = std::fs::read_to_string(state_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        let first_running = json
                            .get("tasks")
                            .and_then(|v| v.get("T_first"))
                            .and_then(|v| v.get("status"))
                            .and_then(|v| v.as_str())
                            == Some("running");
                        let second_pending = json
                            .get("tasks")
                            .and_then(|v| v.get("T_second"))
                            .and_then(|v| v.get("status"))
                            .and_then(|v| v.as_str())
                            == Some("pending");
                        if first_running && second_pending {
                            std::fs::write(cancels_dir.join("current"), b"cancel").ok();
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("pending permit state never appeared");
    });

    let cfg = ExecConfig {
        max_parallel: 1,
        ..ExecConfig::default()
    };
    let final_state = tokio::time::timeout(Duration::from_secs(2), run_plan(plan, projects, cfg))
        .await
        .expect("run_plan should cancel while a later ready task is waiting for capacity")
        .expect("run_plan");
    canceller.await.ok();

    assert_eq!(final_state.status, RunStatus::Cancelled);
    assert_eq!(final_state.tasks["T_first"].status, TaskStatus::Cancelled);
    assert_eq!(final_state.tasks["T_second"].status, TaskStatus::Cancelled);
    assert!(
        !second_started.exists(),
        "second task should not start after cancellation"
    );

    clear_workspace();
}

// ─── 7. Parallelism respected by max_parallel ─────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn max_parallel_caps_concurrent_dispatch() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["a", "b", "c", "d"]);
    let projects = projects_for(dir.path(), &["a", "b", "c", "d"]);

    // Each task sleeps ~150ms. With max_parallel=2, 4 tasks should take at
    // least ~300ms wall-clock. With max_parallel=4 they'd finish in ~150ms.
    let plan = parse_plan(
        r#"
spec: parallelism
tasks:
  - id: A
    project: a
    kind: verify
    agent: shell
    command: "sleep 0.15"
  - id: B
    project: b
    kind: verify
    agent: shell
    command: "sleep 0.15"
  - id: C
    project: c
    kind: verify
    agent: shell
    command: "sleep 0.15"
  - id: D
    project: d
    kind: verify
    agent: shell
    command: "sleep 0.15"
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 2,
        ..ExecConfig::default()
    };

    let start = std::time::Instant::now();
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");
    let elapsed = start.elapsed();

    assert_eq!(final_state.status, RunStatus::Done);
    // 4 sleeps of 150ms with parallelism=2 → ~300ms minimum. We give ourselves
    // generous headroom on both sides (sometimes CI shells start slowly).
    assert!(
        elapsed >= Duration::from_millis(280),
        "expected at least ~300ms with max_parallel=2, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "took unreasonably long ({elapsed:?})"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn independent_projects_overlap_when_parallel_allowed() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    // This is stronger than a wall-clock assertion. Task A waits for a marker
    // created by task B. With true parallelism it sees the marker and exits 0;
    // with sequential dispatch A times out and fails.
    let plan = parse_plan(
        r#"
spec: overlap proof
tasks:
  - id: A
    project: api
    kind: verify
    agent: shell
    command: |
      i=0
      while [ "$i" -lt 100 ]; do
        if [ -f ../web-started ]; then
          echo "overlap observed"
          exit 0
        fi
        i=$((i + 1))
        sleep 0.01
      done
      echo "web never overlapped"
      exit 9
  - id: B
    project: web
    kind: verify
    agent: shell
    command: "touch ../web-started && sleep 0.15"
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 2,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    assert_eq!(final_state.tasks["A"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["B"].status, TaskStatus::Done);

    let evidence_path = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("evidence")
        .join("summary.json");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path).unwrap()).unwrap();
    assert_eq!(evidence["max_observed_parallelism"], 2);
    assert!(
        evidence["parallel_windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|window| {
                window["projects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|project| project == "api")
                    && window["projects"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|project| project == "web")
            }),
        "evidence should include an api/web overlap window"
    );

    clear_workspace();
}

// ─── 8.5. Dry-run renders prompts without dispatching agents ────────────

#[test]
#[serial]
fn dry_run_writes_prompts_and_does_not_create_run_state() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    std::fs::write(dir.path().join("AGENTS.md"), "Root standard.").unwrap();
    std::fs::write(
        dir.path().join("api").join("AGENTS.md"),
        "API implementation standard.",
    )
    .unwrap();
    let projects = projects_for(dir.path(), &["api"]);
    save(
        &SkillScope::Global,
        "workflow-task-guardrails",
        "---
description: keep workflow task scoped
trigger: never-auto-match-this-test
---

# Guardrails

Stay scoped.
",
    )
    .unwrap();

    let plan = parse_plan(
        r#"
spec: dry test
tasks:
  - id: T_one
    project: api
    kind: agent
    agent: shell
    skills: [workflow-task-guardrails]
    prompt: "do thing one"
  - id: T_two
    project: api
    kind: verify
    agent: shell
    depends_on: [T_one]
    command: "echo verify"
"#,
    );

    let summary = maestro::scheduler::dry_run(&plan, &projects).expect("dry_run");

    assert!(
        summary.run_id.starts_with("dry-"),
        "run id should be prefixed"
    );
    assert_eq!(summary.task_count, 2);
    assert_eq!(summary.files.len(), 2);

    // Both prompt files exist and contain their task prompt text
    for (task_id, path) in &summary.files {
        assert!(path.exists(), "{path:?} should exist");
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains(&format!("task_id: {task_id}")));
        if task_id == "T_one" {
            assert!(body.contains("do thing one"));
            assert!(body.contains("agents_instructions:"));
            assert!(body.contains("AGENTS.md"));
            assert!(body.contains("api/AGENTS.md"));
            assert!(body.contains("Root standard."));
            assert!(body.contains("API implementation standard."));
            assert!(body.contains("skills_injected:"));
            assert!(body.contains("workflow-task-guardrails"));
            assert!(body.contains("Stay scoped."));
        } else {
            assert!(body.contains("echo verify"));
        }
    }

    // Crucially: no RUN_STATE.json — dry-run doesn't pretend to be a real run.
    assert!(
        !summary.run_dir.join("RUN_STATE.json").exists(),
        "dry-run must not produce RUN_STATE.json (would confuse the watcher)"
    );

    clear_workspace();
}

// ─── 9. Git worktree isolation for parallel tasks on the same repo ──────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn parallel_tasks_on_same_git_repo_get_isolated_worktrees() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    // Initialise the project as a real git repo so worktrees can be created.
    let api_path = dir.path().join("api");
    init_git_repo(&api_path);

    let projects = projects_for(dir.path(), &["api"]);

    // Both tasks try to write a file; without isolation they'd race on the
    // working tree. With per-task worktrees they each get their own
    // checkout. The shell verify writes `pwd` into the log so we can assert
    // each task ran in a different path.
    let plan = parse_plan(
        r#"
spec: worktree iso
tasks:
  - id: T_a
    project: api
    kind: verify
    agent: shell
    command: |
      echo from-a > marker-a.txt
      pwd > $RUN_LOG_HINT_a 2>/dev/null || pwd
      sleep 0.2
  - id: T_b
    project: api
    kind: verify
    agent: shell
    command: |
      echo from-b > marker-b.txt
      pwd > $RUN_LOG_HINT_b 2>/dev/null || pwd
      sleep 0.2
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 2,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    // Both tasks done
    assert_eq!(final_state.tasks["T_a"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["T_b"].status, TaskStatus::Done);

    // Read both task log files and confirm they recorded different working
    // directories (each its own worktree path).
    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id);
    let log_a = std::fs::read_to_string(run_dir.join("logs").join("T_a.log")).unwrap();
    let log_b = std::fs::read_to_string(run_dir.join("logs").join("T_b.log")).unwrap();
    // `[shell] cd ...` line is written by the shell adapter
    let cwd = |log: &str| -> Option<String> {
        log.lines()
            .find(|l| l.starts_with("[shell] cd "))
            .map(|l| l.to_string())
    };
    let a = cwd(&log_a).expect("T_a logged its cwd");
    let b = cwd(&log_b).expect("T_b logged its cwd");
    assert_ne!(
        a, b,
        "expected each task to run in its own worktree\n  a={a}\n  b={b}"
    );
    // Both should reference the per-run worktree dir, not the original project path
    assert!(
        a.contains("worktrees") && b.contains("worktrees"),
        "worktree paths should appear in the cd line"
    );
    assert!(
        final_state.tasks["T_a"]
            .worktree_path
            .as_deref()
            .unwrap_or_default()
            .contains("worktrees"),
        "state should persist T_a worktree evidence"
    );
    assert!(
        final_state.tasks["T_b"]
            .worktree_path
            .as_deref()
            .unwrap_or_default()
            .contains("worktrees"),
        "state should persist T_b worktree evidence"
    );
    let expected_branch_a = format!("maestro/{}/T_a", final_state.run_id);
    let expected_branch_b = format!("maestro/{}/T_b", final_state.run_id);
    assert_eq!(
        final_state.tasks["T_a"].artifacts.branch.as_deref(),
        Some(expected_branch_a.as_str()),
        "state should persist T_a worktree branch for PR drafting"
    );
    assert_eq!(
        final_state.tasks["T_b"].artifacts.branch.as_deref(),
        Some(expected_branch_b.as_str()),
        "state should persist T_b worktree branch for PR drafting"
    );

    let evidence_path = run_dir.join("evidence").join("summary.json");
    let evidence: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path).unwrap()).unwrap();
    let tasks = evidence["tasks"].as_array().unwrap();
    assert!(
        tasks.iter().all(|task| task["worktree_path"]
            .as_str()
            .unwrap_or_default()
            .contains("worktrees")),
        "evidence should record per-task worktree paths"
    );

    // Worktree dirs are intentionally preserved after the run so uncommitted
    // agent edits remain inspectable and `maestro pr draft` can target the
    // per-task branch/workspace.
    let worktrees_dir = run_dir.join("worktrees");
    let leftovers: Vec<_> = std::fs::read_dir(&worktrees_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(
        leftovers.len(),
        2,
        "both per-task worktrees should be preserved"
    );
    let worktree_a = final_state.tasks["T_a"].worktree_path.as_ref().unwrap();
    let worktree_b = final_state.tasks["T_b"].worktree_path.as_ref().unwrap();
    assert!(
        std::path::Path::new(worktree_a)
            .join("marker-a.txt")
            .exists(),
        "T_a marker should remain in its preserved worktree"
    );
    assert!(
        std::path::Path::new(worktree_b)
            .join("marker-b.txt")
            .exists(),
        "T_b marker should remain in its preserved worktree"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worktree_policy_denies_secret_copy_files_before_worktree_create() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    init_git_repo(&dir.path().join("api"));
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.projects.get_mut("api").unwrap().copy_files = vec![".env".into()];

    let plan = parse_plan(
        r#"
spec: worktree policy denies
tasks:
  - id: T_denied
    project: api
    kind: verify
    agent: shell
    command: "echo should-not-run"
"#,
    );

    let final_state = run_plan(
        plan,
        projects,
        ExecConfig {
            max_parallel: 2,
            ..ExecConfig::default()
        },
    )
    .await
    .expect("run_plan");

    let task = &final_state.tasks["T_denied"];
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.error.as_deref().unwrap_or_default().contains(".env"));
    assert!(task.worktree_path.is_none());
    assert!(!dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id)
        .join("worktrees")
        .join("T_denied")
        .exists());

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worktree_policy_copies_legal_files_after_worktree_create() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let api = dir.path().join("api");
    init_git_repo(&api);
    std::fs::write(api.join(".editorconfig"), "root = true").unwrap();
    let mut projects = projects_for(dir.path(), &["api"]);
    projects.projects.get_mut("api").unwrap().copy_files = vec![".editorconfig".into()];

    let plan = parse_plan(
        r#"
spec: worktree policy copies
tasks:
  - id: T_copy
    project: api
    kind: verify
    agent: shell
    command: "test -f .editorconfig"
"#,
    );

    let final_state = run_plan(
        plan,
        projects,
        ExecConfig {
            max_parallel: 2,
            ..ExecConfig::default()
        },
    )
    .await
    .expect("run_plan");

    let task = &final_state.tasks["T_copy"];
    assert_eq!(task.status, TaskStatus::Done);
    let worktree = task.worktree_path.as_ref().expect("worktree path");
    assert!(std::path::Path::new(worktree)
        .join(".editorconfig")
        .exists());

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn monorepo_project_subdirs_run_inside_mapped_worktrees() {
    let dir = make_workspace();
    let repo = dir.path().join("repo");
    let api_path = repo.join("packages").join("api");
    std::fs::create_dir_all(api_path.join("src")).unwrap();
    std::fs::write(api_path.join("src").join("existing.txt"), "base\n").unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init monorepo"]);

    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    projects.projects.insert(
        "api".into(),
        Project {
            path: api_path.to_string_lossy().to_string(),
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
            agent_profile: None,
            review_profile: None,
            copy_files: Vec::new(),
        },
    );

    let plan = parse_plan(
        r#"
spec: monorepo worktree iso
tasks:
  - id: T_a
    project: api
    kind: verify
    agent: shell
    command: |
      echo from-a > marker-a.txt
      echo changed-a > src/a.txt
      sleep 0.2
  - id: T_b
    project: api
    kind: verify
    agent: shell
    command: |
      echo from-b > marker-b.txt
      echo changed-b > src/b.txt
      sleep 0.2
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 2,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    for task_id in ["T_a", "T_b"] {
        let task = &final_state.tasks[task_id];
        let workspace_path = task.workspace_path.as_deref().unwrap_or_default();
        let worktree_path = task.worktree_path.as_deref().unwrap_or_default();
        assert!(
            workspace_path.contains("worktrees") && workspace_path.ends_with("packages/api"),
            "{task_id} should execute from the mapped project subdir, got {workspace_path}"
        );
        assert!(
            worktree_path.contains("worktrees") && !worktree_path.ends_with("packages/api"),
            "{task_id} worktree path should record the repo-root checkout, got {worktree_path}"
        );
    }

    let workspace_a =
        std::path::Path::new(final_state.tasks["T_a"].workspace_path.as_deref().unwrap());
    let workspace_b =
        std::path::Path::new(final_state.tasks["T_b"].workspace_path.as_deref().unwrap());
    assert!(workspace_a.join("marker-a.txt").exists());
    assert!(workspace_b.join("marker-b.txt").exists());
    // F-108: artifacts.files_changed is PROJECT-relative, even for a project
    // that lives in a subdir of the monorepo. Was `packages/api/marker-a.txt`
    // (repo-root-relative) before the fix; now the project prefix is stripped.
    assert!(
        final_state.tasks["T_a"]
            .artifacts
            .files_changed
            .contains(&"marker-a.txt".to_string()),
        "T_a should infer project-relative changed files, got {:?}",
        final_state.tasks["T_a"].artifacts.files_changed
    );
    assert!(
        final_state.tasks["T_b"]
            .artifacts
            .files_changed
            .contains(&"marker-b.txt".to_string()),
        "T_b should infer project-relative changed files, got {:?}",
        final_state.tasks["T_b"].artifacts.files_changed
    );

    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id);
    // PR_BODY follows artifacts.files_changed (project-relative).
    let pr_body = std::fs::read_to_string(run_dir.join("PR_BODY.md")).unwrap();
    assert!(pr_body.contains("`marker-a.txt`"));
    assert!(pr_body.contains("`marker-b.txt`"));

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn monorepo_parallel_conflict_is_attributed_and_actionable() {
    // Two parallel tasks edit the *same line* of a shared monorepo file. One
    // integrates; the other must fail with an actionable conflict error that
    // names the file, names the task it clashed with, and tells the user to
    // serialize them with `depends_on`.
    let dir = make_workspace();
    let repo = dir.path().join("repo");
    let api_path = repo.join("packages").join("api");
    std::fs::create_dir_all(&api_path).unwrap();
    std::fs::write(api_path.join("shared.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init monorepo"]);

    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    projects.projects.insert(
        "api".into(),
        Project {
            path: api_path.to_string_lossy().to_string(),
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
            agent_profile: None,
            review_profile: None,
            copy_files: Vec::new(),
        },
    );

    // Both rewrite the middle line to a different value — a true overlap.
    let plan = parse_plan(
        r#"
spec: monorepo parallel conflict
tasks:
  - id: T_a
    project: api
    kind: verify
    agent: shell
    command: |
      printf 'alpha\nbeta-from-a\ngamma\n' > shared.txt
      sleep 0.2
  - id: T_b
    project: api
    kind: verify
    agent: shell
    command: |
      printf 'alpha\nbeta-from-b\ngamma\n' > shared.txt
      sleep 0.2
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 2,
        continue_on_error: true,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Failed);

    // Exactly one task integrated; the other lost the race and must carry the
    // actionable, attributed conflict message.
    let failed: Vec<&str> = ["T_a", "T_b"]
        .into_iter()
        .filter(|id| final_state.tasks[*id].status == TaskStatus::Failed)
        .collect();
    assert_eq!(failed.len(), 1, "exactly one task should lose the race");
    let loser = failed[0];
    let winner = if loser == "T_a" { "T_b" } else { "T_a" };
    let err = final_state.tasks[loser].error.clone().unwrap_or_default();

    assert!(err.contains("integration conflict"), "got: {err}");
    assert!(
        err.contains("shared.txt"),
        "should name the file; got: {err}"
    );
    assert!(
        err.contains("depends_on"),
        "should suggest serializing; got: {err}"
    );
    assert!(
        err.contains(winner),
        "should attribute the clash to {winner}; got: {err}"
    );
    // The conflict is recorded in the run's audit ledger.
    assert!(
        final_state
            .auto_actions
            .iter()
            .any(|a| a.kind == "integration_conflict"),
        "ledger should record the integration conflict"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn wired_contract_edges_are_recorded_in_the_audit_ledger() {
    // Edges the CLI auto-wired before the run are stamped into the ledger so
    // the user can see maestro added them.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: wired ledger
tasks:
  - id: T_only
    project: api
    kind: verify
    agent: shell
    command: "echo ok"
"#,
    );

    let cfg = ExecConfig {
        wired_contract_edges: vec![maestro::config::WiredEdge {
            consumer: "T_only".into(),
            producer: "T_producer".into(),
            contract: "types/index.d.ts".into(),
        }],
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    let wired: Vec<&str> = final_state
        .auto_actions
        .iter()
        .filter(|a| a.kind == "contract_wired")
        .map(|a| a.detail.as_str())
        .collect();
    assert_eq!(wired.len(), 1);
    assert!(wired[0].contains("T_only") && wired[0].contains("T_producer"));

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn inferred_changed_files_are_task_delta_in_shared_workspace() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let api_path = dir.path().join("api");
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&api_path)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["commit", "--allow-empty", "-q", "-m", "init"]);

    let projects = projects_for(dir.path(), &["api"]);
    let plan = parse_plan(
        r#"
spec: changed files delta
tasks:
  - id: T_a
    project: api
    kind: verify
    agent: shell
    command: "echo a > a.txt"
  - id: T_b
    project: api
    kind: verify
    agent: shell
    depends_on: [T_a]
    command: "echo b > b.txt"
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 1,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    assert_eq!(
        final_state.tasks["T_a"].artifacts.files_changed,
        vec!["a.txt"]
    );
    assert_eq!(
        final_state.tasks["T_b"].artifacts.files_changed,
        vec!["b.txt"]
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn non_git_workspace_skips_changed_file_status_probe() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);

    let fake_bin = dir.path().join("bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    let marker = dir.path().join("git-status-called");
    let fake_git = fake_bin.join("git");
    std::fs::write(
        &fake_git,
        format!(
            "#!/bin/sh\ncase \" $* \" in\n  *\" status \"*) touch '{}'; exit 1 ;;\n  *\" rev-parse \"*) echo false; exit 1 ;;\n  *) exit 1 ;;\nesac\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_git).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_git, perms).unwrap();
    }

    let old_path = std::env::var_os("PATH");
    let test_path = format!("{}:/bin:/usr/bin", fake_bin.to_string_lossy());
    unsafe {
        std::env::set_var("PATH", test_path);
    }

    let projects = projects_for(dir.path(), &["api"]);
    let plan = parse_plan(
        r#"
spec: non git changed file probe
tasks:
  - id: T_write
    project: api
    kind: verify
    agent: shell
    command: "echo ok > made.txt"
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 1,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    if let Some(path) = old_path {
        unsafe {
            std::env::set_var("PATH", path);
        }
    } else {
        unsafe {
            std::env::remove_var("PATH");
        }
    }

    assert_eq!(final_state.status, RunStatus::Done);
    assert!(final_state.tasks["T_write"]
        .artifacts
        .files_changed
        .is_empty());
    assert!(
        !marker.exists(),
        "non-git workspaces should not run git status for changed-file inference"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn integration_worktree_feeds_downstream_and_global_verification() {
    let dir = make_workspace();
    let repo = dir.path().to_path_buf();
    std::fs::create_dir_all(repo.join("pricing").join("src")).unwrap();
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("pricing").join("src").join("value.txt"), "old\n").unwrap();
    std::fs::write(repo.join("api").join("checked.txt"), "old\n").unwrap();
    std::fs::write(repo.join("docs").join("promotions.md"), "old\n").unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init monorepo"]);

    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    for name in ["pricing", "api", "docs"] {
        projects.projects.insert(
            name.into(),
            Project {
                path: repo.join(name).to_string_lossy().to_string(),
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
                agent_profile: None,
                review_profile: None,
                copy_files: Vec::new(),
            },
        );
    }

    let plan = parse_plan(
        r#"
spec: integration aggregation
goal:
  description: aggregated changes are visible to final acceptance
  acceptance:
    - describe: integration workspace contains all task changes
      check: "grep -q new pricing/src/value.txt && grep -q api-ok api/checked.txt && grep -q VIP25 docs/promotions.md"
tasks:
  - id: T_pricing
    project: pricing
    kind: verify
    agent: shell
    command: "echo new > src/value.txt"
  - id: T_docs
    project: docs
    kind: verify
    agent: shell
    command: "echo VIP25 > promotions.md"
  - id: T_api
    project: api
    kind: verify
    agent: shell
    depends_on: [T_pricing]
    command: "grep -q new ../pricing/src/value.txt && echo api-ok > checked.txt"
  - id: T_verify
    project: _global
    kind: verify
    agent: shell
    depends_on: [T_api, T_docs]
    command: "grep -q new pricing/src/value.txt && grep -q api-ok api/checked.txt && grep -q VIP25 docs/promotions.md"
"#,
    );

    let cfg = ExecConfig {
        max_parallel: 3,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    assert!(final_state.verified, "run-level acceptance should pass");
    assert_eq!(final_state.tasks["T_api"].status, TaskStatus::Done);
    assert_eq!(final_state.tasks["T_verify"].status, TaskStatus::Done);
    assert!(
        final_state.tasks["T_api"]
            .workspace_path
            .as_deref()
            .unwrap_or_default()
            .contains("worktrees"),
        "project task should still run in an isolated task worktree"
    );
    assert!(
        final_state.tasks["T_verify"]
            .workspace_path
            .as_deref()
            .unwrap_or_default()
            .contains("integration"),
        "global verify should run from the integration worktree"
    );

    assert_eq!(
        std::fs::read_to_string(repo.join("pricing").join("src").join("value.txt")).unwrap(),
        "old\n",
        "original workspace should remain untouched by isolated aggregation"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn rerun_seeded_skipped_tasks_keep_outputs_and_integration_changes() {
    let dir = make_workspace();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("contract").join("src")).unwrap();
    std::fs::create_dir_all(repo.join("consumer")).unwrap();
    std::fs::write(repo.join("contract").join("src").join("schema.txt"), "v1\n").unwrap();
    std::fs::write(repo.join("consumer").join("result.txt"), "old\n").unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init monorepo"]);

    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    for name in ["contract", "consumer"] {
        projects.projects.insert(
            name.into(),
            Project {
                path: repo.join(name).to_string_lossy().to_string(),
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
                agent_profile: None,
                review_profile: None,
                copy_files: Vec::new(),
            },
        );
    }

    let plan = parse_plan(
        r#"
spec: seeded rerun
goal:
  description: skipped upstream changes are still available
  acceptance:
    - describe: integration workspace contains upstream and downstream changes
      check: "grep -q v2 repo/contract/src/schema.txt && grep -q consumer-ok repo/consumer/result.txt"
tasks:
  - id: T_contract
    project: contract
    kind: verify
    agent: shell
    command: "printf 'v2\n' > src/schema.txt"
    outputs:
      schema:
        path: src/schema.txt
  - id: T_consumer
    project: consumer
    kind: verify
    agent: shell
    depends_on: [T_contract]
    inputs:
      schema:
        from: T_contract.schema
    command: "grep -q v2 ../contract/src/schema.txt && echo consumer-ok > result.txt"
  - id: T_verify
    project: _global
    kind: verify
    agent: shell
    depends_on: [T_consumer]
    command: "grep -q v2 repo/contract/src/schema.txt && grep -q consumer-ok repo/consumer/result.txt"
"#,
    );

    let first_state = run_plan(
        plan.clone(),
        projects.clone(),
        ExecConfig {
            max_parallel: 2,
            ..ExecConfig::default()
        },
    )
    .await
    .expect("first run");
    assert_eq!(first_state.status, RunStatus::Done);
    assert!(first_state.verified);
    assert!(first_state.tasks["T_contract"]
        .workflow_outputs
        .contains_key("schema"));

    let second_state = run_plan(
        plan,
        projects,
        ExecConfig {
            max_parallel: 2,
            skip: vec!["T_contract".into()],
            seed_skipped_from: Some(first_state),
            ..ExecConfig::default()
        },
    )
    .await
    .expect("seeded rerun");

    assert_eq!(second_state.status, RunStatus::Done);
    assert!(second_state.verified);
    assert_eq!(second_state.tasks["T_contract"].status, TaskStatus::Skipped);
    assert!(second_state.tasks["T_contract"]
        .workflow_outputs
        .contains_key("schema"));
    assert_eq!(second_state.tasks["T_consumer"].status, TaskStatus::Done);

    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&second_state.run_id);
    let consumer_log =
        std::fs::read_to_string(run_dir.join("logs").join("T_consumer.log")).unwrap();
    assert!(
        consumer_log.contains("workflow/T_contract.schema as schema"),
        "seeded skipped task output should still be injected"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn global_verification_preserves_workspace_paths_for_nested_repo() {
    let dir = make_workspace();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("schema")).unwrap();
    std::fs::create_dir_all(repo.join("service")).unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("schema").join("contract.txt"), "v1\n").unwrap();
    std::fs::write(repo.join("service").join("result.txt"), "old\n").unwrap();
    std::fs::write(repo.join("docs").join("summary.md"), "old\n").unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init nested repo"]);

    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    for name in ["schema", "service", "docs"] {
        projects.projects.insert(
            name.into(),
            Project {
                path: repo.join(name).to_string_lossy().to_string(),
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
                agent_profile: None,
                review_profile: None,
                copy_files: Vec::new(),
            },
        );
    }

    let plan = parse_plan(
        r#"
spec: nested repo global paths
goal:
  description: global checks keep workspace-root paths
  acceptance:
    - describe: nested repo files are visible under repo/
      check: "grep -q v2 repo/schema/contract.txt && grep -q service-v2 repo/service/result.txt && grep -q docs-v2 repo/docs/summary.md"
tasks:
  - id: T_schema
    project: schema
    kind: verify
    agent: shell
    command: "printf 'v2\n' > contract.txt"
  - id: T_service
    project: service
    kind: verify
    agent: shell
    depends_on: [T_schema]
    command: "grep -q v2 ../schema/contract.txt && printf 'service-v2\n' > result.txt"
  - id: T_docs
    project: docs
    kind: verify
    agent: shell
    depends_on: [T_schema]
    command: "grep -q v2 ../schema/contract.txt && printf 'docs-v2\n' > summary.md"
  - id: T_verify
    project: _global
    kind: verify
    agent: shell
    depends_on: [T_service, T_docs]
    command: "grep -q v2 repo/schema/contract.txt && grep -q service-v2 repo/service/result.txt && grep -q docs-v2 repo/docs/summary.md"
"#,
    );

    let final_state = run_plan(
        plan.clone(),
        projects.clone(),
        ExecConfig {
            max_parallel: 3,
            ..ExecConfig::default()
        },
    )
    .await
    .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    assert!(final_state.verified);
    assert_eq!(final_state.tasks["T_verify"].status, TaskStatus::Done);
    let verify_workspace = final_state.tasks["T_verify"]
        .workspace_path
        .as_deref()
        .unwrap_or_default();
    assert!(
        verify_workspace.contains("integration/workspace"),
        "global verify should run from a workspace-shaped integration bridge, got {verify_workspace}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("schema").join("contract.txt")).unwrap(),
        "v1\n",
        "original nested repo should remain untouched"
    );

    let only_state = run_plan(
        plan,
        projects,
        ExecConfig {
            max_parallel: 3,
            only: Some(vec!["T_verify".into()]),
            seed_skipped_from: Some(final_state),
            ..ExecConfig::default()
        },
    )
    .await
    .expect("only rerun");
    assert_eq!(only_state.status, RunStatus::Done);
    assert!(only_state.verified);
    assert_eq!(only_state.tasks["T_schema"].status, TaskStatus::Skipped);
    assert_eq!(only_state.tasks["T_service"].status, TaskStatus::Skipped);
    assert_eq!(only_state.tasks["T_docs"].status, TaskStatus::Skipped);
    assert_eq!(only_state.tasks["T_verify"].status, TaskStatus::Done);

    let only_run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&only_state.run_id);
    let verify_log =
        std::fs::read_to_string(only_run_dir.join("logs").join("T_verify.log")).unwrap();
    assert_eq!(
        verify_log.matches("[shell] cd ").count(),
        1,
        "fan-in from skipped tasks should enqueue T_verify exactly once"
    );

    clear_workspace();
}

// ─── 10. RUN_STATE.json is durable on disk ───────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn run_state_json_is_persisted_and_parseable() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);

    let plan = parse_plan(
        r#"
spec: persistence
tasks:
  - id: T
    project: api
    kind: verify
    agent: shell
    command: "echo hi"
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    assert_eq!(final_state.status, RunStatus::Done);

    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id);
    let state_path = run_dir.join("RUN_STATE.json");
    assert!(state_path.exists(), "RUN_STATE.json must be on disk");
    let text = std::fs::read_to_string(&state_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(parsed["status"], "done");
    assert_eq!(parsed["tasks"]["T"]["status"], "done");

    // Run report was also written
    assert!(run_dir.join("REPORT.md").exists());

    clear_workspace();
}

// ─── 11. Workflow data inputs / outputs ─────────────────────────────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn declared_outputs_are_snapshotted_and_injected_downstream() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    let plan = parse_plan(
        r#"
spec: data flow
tasks:
  - id: T_contract
    project: api
    kind: verify
    agent: shell
    command: "mkdir -p schemas && printf 'nickname: string\n' > schemas/openapi.yaml"
    outputs:
      openapi:
        path: schemas/openapi.yaml
  - id: T_web
    project: web
    agent: mock
    prompt: "Use the API contract."
    inputs:
      api_contract:
        from: T_contract.openapi
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");

    assert_eq!(final_state.status, RunStatus::Done);
    let out = &final_state.tasks["T_contract"].workflow_outputs["openapi"];
    assert!(std::path::Path::new(&out.snapshot_path).exists());
    let snapshot = std::fs::read_to_string(&out.snapshot_path).unwrap();
    assert!(snapshot.contains("nickname"));

    let run_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join(&final_state.run_id);
    let web_log = std::fs::read_to_string(run_dir.join("logs").join("T_web.log")).unwrap();
    assert!(
        web_log.contains("workflow/T_contract.openapi as api_contract"),
        "downstream mock adapter should see the workflow input context"
    );

    clear_workspace();
}

// ─── F-116 Step 2: context manifest end-to-end (N1 + N2 + privacy) ────────

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f116_manifest_records_injected_inputs_after_setup_and_is_body_free() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);

    // T_web (agent) consumes one input that IS produced + one optional input
    // whose (declared but never-written, non-required) output is absent at
    // dispatch; the missing one must not appear in the manifest (N1).
    let plan = parse_plan(
        r#"
spec: f116 manifest
tasks:
  - id: T_contract
    project: api
    kind: verify
    agent: shell
    command: "mkdir -p schemas && printf 'nickname: string\n' > schemas/openapi.yaml"
    outputs:
      openapi:
        path: schemas/openapi.yaml
      spare:
        path: schemas/spare.yaml
        required: false
  - id: T_web
    project: web
    kind: agent
    agent: mock
    prompt: "Use the API contract."
    inputs:
      api_contract:
        from: T_contract.openapi
      spare_input:
        from: T_contract.spare
        required: false
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    assert_eq!(final_state.status, RunStatus::Done);
    let run_dir = &final_state.run_dir;

    // N2: the agent task got a manifest (written after every setup gate passed).
    let manifest = maestro::scheduler::context::read_manifest(run_dir, "T_web")
        .expect("read manifest ok")
        .expect("agent task has a context manifest");
    assert_eq!(manifest.task_id, "T_web");
    assert_eq!(manifest.kind, "agent");
    assert!(manifest.layers.iter().any(|l| l.id == "task.prompt"));

    // N1: workflow.inputs reflects the actually-injected input only.
    let wf = manifest
        .layers
        .iter()
        .find(|l| l.id == "workflow.inputs")
        .expect("workflow.inputs layer present");
    assert_eq!(wf.item_count, 1, "only the injected input is counted");
    let refs: Vec<&str> = wf.refs.iter().map(|r| r.reference.as_str()).collect();
    assert!(
        refs.contains(&"api_contract"),
        "alias ref of the injected input"
    );
    assert!(
        refs.contains(&"T_contract.openapi"),
        "producer.output ref of the injected input"
    );
    assert!(
        !refs.iter().any(|r| r.contains("spare")),
        "the not-injected (absent-output) input must not be in refs: {refs:?}"
    );

    // privacy: no raw prompt body, no producer snapshot/source path in the artifact.
    let json = std::fs::read_to_string(run_dir.join("context").join("T_web.json")).unwrap();
    assert!(!json.contains("Use the API contract"), "no raw task body");
    let snapshot = &final_state.tasks["T_contract"].workflow_outputs["openapi"].snapshot_path;
    assert!(
        !json.contains(snapshot.as_str()),
        "no snapshot path in manifest"
    );

    // verify task gets no manifest (agent-only).
    assert!(
        maestro::scheduler::context::read_manifest(run_dir, "T_contract")
            .unwrap()
            .is_none()
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f116_no_orphan_manifest_when_setup_gate_fails() {
    // N2: an agent task that fails a synchronous setup gate (worktree policy)
    // BEFORE the adapter must leave no "looks-dispatched" context manifest.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    // Real git repo so worktree isolation engages (with max_parallel > 1).
    init_git_repo(&dir.path().join("api"));
    let mut projects = projects_for(dir.path(), &["api"]);
    // A denied copy_files entry (matches `**/.env*`) makes worktree-policy
    // validation fail before the adapter is ever invoked.
    projects.projects.get_mut("api").unwrap().copy_files = vec![".env".to_string()];

    let plan = parse_plan(
        r#"
spec: f116 setup deny
tasks:
  - id: T_denied
    project: api
    kind: agent
    agent: mock
    prompt: "do work"
"#,
    );
    let cfg = ExecConfig {
        max_parallel: 2,
        ..ExecConfig::default()
    };
    let final_state = run_plan(plan, projects, cfg).await.expect("run_plan");

    assert_eq!(
        final_state.tasks["T_denied"].status,
        TaskStatus::Failed,
        "the denied copy_files policy must fail the task at the setup gate"
    );
    assert!(
        maestro::scheduler::context::read_manifest(&final_state.run_dir, "T_denied")
            .unwrap()
            .is_none(),
        "a setup-failed task must not leave an orphan context manifest"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f117_fresh_run_writes_resume_descriptor_with_seeded_outputs() {
    // Boundary 1+2: a completed run leaves a valid RESUME.json that reflects all
    // task statuses, and a Done task's captured workflow output is counted in
    // seeded_tasks. Privacy: no raw prompt body / snapshot path in the artifact.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);
    let plan = parse_plan(
        r#"
spec: f117 fresh run
tasks:
  - id: T_contract
    project: api
    kind: verify
    agent: shell
    command: "mkdir -p schemas && printf 'x: 1\n' > schemas/openapi.yaml"
    outputs:
      openapi:
        path: schemas/openapi.yaml
  - id: T_web
    project: web
    kind: agent
    agent: mock
    prompt: "use the contract"
    inputs:
      api_contract:
        from: T_contract.openapi
"#,
    );

    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    assert_eq!(final_state.status, RunStatus::Done);
    let run_dir = &final_state.run_dir;

    let d = maestro::scheduler::resume::read_descriptor(run_dir)
        .expect("read descriptor ok")
        .expect("a fresh run writes a resume descriptor");
    assert_eq!(d.run_id, final_state.run_id);
    assert_eq!(d.state.task_total, 2);
    assert_eq!(d.state.done, 2);
    assert_eq!(d.state.run_status, "done");
    // both tasks done -> both seeded; the producer carries its captured output count.
    assert_eq!(d.seeded_tasks.len(), 2);
    let producer = d
        .seeded_tasks
        .iter()
        .find(|s| s.task_id == "T_contract")
        .expect("producer seeded");
    assert_eq!(
        producer.workflow_outputs, 1,
        "the producer's captured output is counted in seeded_tasks"
    );
    // event cursor advanced and settled on real task completions.
    assert!(d.event_ledger.last_seq >= 2);
    assert!(d.event_ledger.last_settled_seq >= 1);

    // privacy: no raw prompt body, no producer snapshot path in the descriptor.
    let json =
        std::fs::read_to_string(maestro::scheduler::resume::descriptor_path(run_dir)).unwrap();
    assert!(!json.contains("use the contract"), "no raw prompt body");
    let snapshot = &final_state.tasks["T_contract"].workflow_outputs["openapi"].snapshot_path;
    assert!(
        !json.contains(snapshot.as_str()),
        "no snapshot path in descriptor"
    );

    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f117_dry_run_writes_no_resume_descriptor() {
    // Boundary 5: the `--dry` preview path never calls run_plan, so it must not
    // produce a RESUME.json under any run dir it stages.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = projects_for(dir.path(), &["api"]);
    let plan = parse_plan(
        r#"
spec: f117 dry run
tasks:
  - id: T_a
    project: api
    kind: agent
    agent: mock
    prompt: "do work"
"#,
    );

    let summary =
        maestro::scheduler::dry_run_in_workspace(&plan, &projects, dir.path()).expect("dry run");
    // the dry preview stages a run dir (PLAN snapshot + prompts) but writes no
    // resume guard.
    assert!(
        maestro::scheduler::resume::read_descriptor(&summary.run_dir)
            .expect("read ok")
            .is_none(),
        "dry-run must not write a resume descriptor"
    );
    assert!(
        !maestro::scheduler::resume::descriptor_path(&summary.run_dir).exists(),
        "no RESUME.json in a dry-run dir"
    );

    clear_workspace();
}

fn resume_event(seq: u64, kind: &str, run_id: &str) -> maestro::scheduler::events::RunEvent {
    serde_json::from_value(serde_json::json!({
        "event_id": format!("e{seq}"),
        "run_id": run_id,
        "seq": seq,
        "timestamp": "2026-06-05T00:00:00Z",
        "kind": kind,
    }))
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f117_dogfood_mid_task_crash_resumes_and_seeds_done_task() {
    // Step 4 dogfood — a REAL mid-task crash (not a quiescent window): a run is
    // killed while the consumer task is running. On disk that leaves the producer
    // Done with a captured output, the consumer's `task.started` as a non-settling
    // tail in the ledger, a descriptor as-of-the-producer-settle, and a
    // still-"running" state with a dead pid. The append-only cursor fix must let
    // this resume; resume must seed the producer and run the consumer.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api", "web"]);
    let projects = projects_for(dir.path(), &["api", "web"]);
    let plan = parse_plan(
        r#"
spec: f117 dogfood
tasks:
  - id: T_contract
    project: api
    kind: verify
    agent: shell
    command: "mkdir -p schemas && printf 'x: 1\n' > schemas/openapi.yaml"
    outputs:
      openapi:
        path: schemas/openapi.yaml
  - id: T_web
    project: web
    kind: agent
    agent: mock
    prompt: "use the contract"
    inputs:
      api_contract:
        from: T_contract.openapi
"#,
    );

    // 1) Run the full plan once to produce a REAL run dir (real captured output
    //    for the producer, real PLAN.yaml snapshot).
    let done_state = run_plan(plan.clone(), projects.clone(), ExecConfig::default())
        .await
        .expect("initial run_plan");
    assert_eq!(done_state.status, RunStatus::Done);
    let run_dir = done_state.run_dir.clone();
    let run_id = done_state.run_id.clone();

    // 2) Rewind the on-disk artifacts to a mid-T_web crash.
    let mut crash = done_state.clone();
    crash.status = RunStatus::Running;
    crash.pid = 999_999; // a dead pid -> abandoned
    crash.ended_at = None;
    {
        let web = crash.tasks.get_mut("T_web").expect("T_web");
        web.status = TaskStatus::Running; // started, never settled
        web.ended_at = None;
    }
    std::fs::write(
        run_dir.join("RUN_STATE.json"),
        serde_json::to_string_pretty(&crash).unwrap(),
    )
    .unwrap();

    // The descriptor as-of the producer's settle: ledger [run.started, A settle]
    // (last_settled = 2), seeded = [T_contract].
    let desc_events = [
        resume_event(1, "run.started", &run_id),
        resume_event(2, "task.completed", &run_id),
    ];
    let descriptor = maestro::scheduler::resume::build_descriptor(
        &run_dir,
        &crash,
        &desc_events,
        crash.started_at,
        crash.started_at,
        999_999,
    )
    .expect("build descriptor");
    maestro::scheduler::resume::write_descriptor(&run_dir, &descriptor).unwrap();

    // The on-disk ledger at the crash adds T_web's non-settling `task.started`
    // tail (seq 3) beyond the descriptor's last_seq (2).
    let ledger = [
        resume_event(1, "run.started", &run_id),
        resume_event(2, "task.completed", &run_id),
        resume_event(3, "task.started", &run_id),
    ];
    let mut ndjson = String::new();
    for e in &ledger {
        ndjson.push_str(&serde_json::to_string(e).unwrap());
        ndjson.push('\n');
    }
    std::fs::write(run_dir.join("events.ndjson"), ndjson).unwrap();

    // 3) The guard must allow this real mid-task crash (the append-only tail is
    //    not staleness), seeding exactly the producer.
    let report =
        maestro::scheduler::resume::validate_resume_target(&run_dir, false).expect("validate ok");
    assert!(
        report.can_resume,
        "a real mid-task crash must be resumable; issues: {:?}",
        report.issues
    );
    assert!(
        !report
            .issues
            .iter()
            .any(|i| i.code == maestro::schema::resume::ResumeIssueCode::EventCursorStale),
        "the task.started tail is not cursor staleness: {:?}",
        report.issues
    );
    assert_eq!(report.reusable_done_tasks, vec!["T_contract"]);

    // 4) Resume: a new run seeds the producer (skipped) and runs the consumer.
    let cfg = ExecConfig {
        skip: report.reusable_done_tasks.clone(),
        seed_skipped_from: Some(crash),
        ..ExecConfig::default()
    };
    let resumed = run_plan(plan, projects, cfg)
        .await
        .expect("resume run_plan");
    assert_eq!(resumed.status, RunStatus::Done, "resumed run completes");
    // The producer was SEEDED (skipped, not re-executed) and still carries its
    // captured output; the consumer actually ran.
    assert_eq!(
        resumed.tasks["T_contract"].status,
        TaskStatus::Skipped,
        "producer was seeded, not re-run"
    );
    assert!(
        !resumed.tasks["T_contract"].workflow_outputs.is_empty(),
        "seeded producer carries its captured output"
    );
    assert_eq!(
        resumed.tasks["T_web"].status,
        TaskStatus::Done,
        "consumer ran to completion on resume"
    );

    clear_workspace();
}

/// F-122: a real run pins `PLAN_PREVIEW.json` beside `PLAN.yaml`, and the pinned
/// preview mechanically matches the on-disk plan (task count, dependency edges,
/// and the stable plan hash).
#[tokio::test(flavor = "current_thread")]
#[serial]
async fn f122_pins_plan_preview_snapshot_aligned_with_plan_yaml() {
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["core", "cli"]);
    let projects = projects_for(dir.path(), &["core", "cli"]);

    let plan = parse_plan(
        r#"
spec: F-122 pin preview
tasks:
  - id: T_core
    project: core
    kind: verify
    agent: shell
    command: "echo ok"
  - id: T_cli
    project: cli
    kind: verify
    agent: shell
    command: "echo ok"
    depends_on: [T_core]
"#,
    );
    let final_state = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    let run_dir = &final_state.run_dir;

    // The pin exists beside PLAN.yaml and parses as the v1 snapshot.
    let snap_path = run_dir.join("PLAN_PREVIEW.json");
    assert!(snap_path.exists(), "F-122 must pin PLAN_PREVIEW.json");
    let snap = maestro::schema::preview::PlanPreviewSnapshot::from_json(
        &std::fs::read_to_string(&snap_path).unwrap(),
    )
    .expect("parse PLAN_PREVIEW.json");
    assert_eq!(snap.schema_version, "maestro.plan_preview_snapshot.v1");
    assert_eq!(snap.source, "run");
    assert_eq!(snap.plan_path, "PLAN.yaml");
    assert!(snap.gate.is_none(), "no gate decision is fabricated in v1");

    // Mechanically check the pinned preview against the on-disk PLAN.yaml.
    let plan_yaml: Plan =
        serde_yaml::from_str(&std::fs::read_to_string(run_dir.join("PLAN.yaml")).unwrap()).unwrap();
    assert_eq!(snap.preview.task_count as usize, plan_yaml.tasks.len());

    let mut expected_edges: Vec<(String, String)> = plan_yaml
        .tasks
        .iter()
        .flat_map(|t| t.depends_on.iter().map(move |d| (d.clone(), t.id.clone())))
        .collect();
    let mut pinned_edges: Vec<(String, String)> = snap
        .preview
        .dependency_edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    expected_edges.sort();
    pinned_edges.sort();
    assert_eq!(
        pinned_edges, expected_edges,
        "pinned dependency edges match PLAN.yaml depends_on"
    );
    assert_eq!(
        pinned_edges,
        vec![("T_core".to_string(), "T_cli".to_string())],
        "the one declared dependency edge is pinned"
    );

    // plan_hash is the stable hash of the run dir's PLAN.yaml (drift-detectable).
    let expect_hash = maestro::file_guard::file_hash(&run_dir.join("PLAN.yaml")).unwrap();
    assert_eq!(snap.plan_hash, expect_hash);
    assert!(snap.plan_hash.starts_with("fnv1a64:"));

    clear_workspace();
}

// ─── F-126: node tool-policy violation gate (B4, opt-in) ─────────────────

/// A user role with no write capability (`git_write:false` → `fs_write:false`),
/// shell allowed with no command allowlist (allow-all) so an `echo` can change a
/// tracked file and trip the policy violation.
fn write_no_write_role(workspace: &std::path::Path) {
    let roles_dir = workspace.join(".maestro").join("roles");
    std::fs::create_dir_all(&roles_dir).unwrap();
    std::fs::write(
        roles_dir.join("restricted.md"),
        "---\nname: restricted\nallowed_tools:\n  git_write: false\n---\nno-write role\n",
    )
    .unwrap();
}

fn restricted_projects(dir: &std::path::Path) -> ProjectsConfig {
    init_git_repo(&dir.join("api"));
    write_no_write_role(dir);
    let mut projects = projects_for(dir, &["api"]);
    projects.projects.get_mut("api").unwrap().role = Some("restricted".into());
    projects
}

const POLICY_VIOLATION_PLAN: &str = r#"
spec: policy gate
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo changed > marker.txt"
"#;

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn policy_gate_off_by_default_ignores_violation() {
    // Flag default OFF: a real violation still integrates, no gate, no behavior
    // change vs the existing flow.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let projects = restricted_projects(dir.path()); // gate_on_policy_violation = false
    let st = run_plan(
        parse_plan(POLICY_VIOLATION_PLAN),
        projects,
        ExecConfig::default(),
    )
    .await
    .expect("run_plan");
    assert_eq!(
        st.tasks["T0"].status,
        TaskStatus::Done,
        "flag off: integrates, never gates"
    );
    let findings = maestro::scheduler::findings::read_findings(&st.run_dir).unwrap();
    assert!(
        !findings.iter().any(|f| f.source == "policy-gate"),
        "flag off must not record a policy-gate finding"
    );
    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn policy_gate_on_gates_violation_then_approves() {
    // Flag ON + present violation → enters approval; approve → integrates, and a
    // `policy-gate` finding proves it was policy-gated (not just risk/manual).
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = restricted_projects(dir.path());
    projects.defaults.gate_on_policy_violation = true;

    let workspace = dir.path().to_path_buf();
    let approver = tokio::spawn(async move {
        let approvals_dir = workspace.join(".maestro").join("control").join("approvals");
        for _ in 0..200 {
            if approvals_dir.exists() {
                std::fs::write(approvals_dir.join("T0"), b"ok").ok();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("approvals dir never appeared — task was not policy-gated");
    });

    let st = run_plan(
        parse_plan(POLICY_VIOLATION_PLAN),
        projects,
        ExecConfig::default(),
    )
    .await
    .expect("run_plan");
    approver.await.ok();

    assert_eq!(
        st.tasks["T0"].status,
        TaskStatus::Done,
        "approved → integrates"
    );
    let findings = maestro::scheduler::findings::read_findings(&st.run_dir).unwrap();
    let policy = findings.iter().find(|f| f.source == "policy-gate");
    assert!(policy.is_some(), "expected a policy-gate finding");
    assert!(
        policy.unwrap().summary.contains("git_write"),
        "finding should name the violated capability: {:?}",
        policy.unwrap().summary
    );
    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn policy_gate_on_compliant_does_not_gate() {
    // Flag ON + default (compliant) role: write capability is requested, so a
    // file change is within policy → integrates, no gate.
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    init_git_repo(&dir.path().join("api"));
    let mut projects = projects_for(dir.path(), &["api"]); // default role: git_write true
    projects.defaults.gate_on_policy_violation = true;
    let st = run_plan(
        parse_plan(POLICY_VIOLATION_PLAN),
        projects,
        ExecConfig::default(),
    )
    .await
    .expect("run_plan");
    assert_eq!(
        st.tasks["T0"].status,
        TaskStatus::Done,
        "compliant → integrates"
    );
    let findings = maestro::scheduler::findings::read_findings(&st.run_dir).unwrap();
    assert!(
        !findings.iter().any(|f| f.source == "policy-gate"),
        "compliant task must not be policy-gated"
    );
    clear_workspace();
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn policy_gate_on_worktree_branch_without_diff_does_not_gate() {
    // F-126-fu: flag ON + no-write role + a task with NO file diff. The executor
    // still auto-fills the per-task worktree branch, but `branch` is not a write
    // signal, so this must NOT gate (it would hang on approval if it regressed).
    let dir = make_workspace();
    ensure_dirs(dir.path(), &["api"]);
    let mut projects = restricted_projects(dir.path());
    projects.defaults.gate_on_policy_violation = true;
    let plan = parse_plan(
        r#"
spec: branch-only no gate
tasks:
  - id: T0
    project: api
    kind: verify
    agent: shell
    command: "echo hello"
"#,
    );
    let st = run_plan(plan, projects, ExecConfig::default())
        .await
        .expect("run_plan");
    assert_eq!(
        st.tasks["T0"].status,
        TaskStatus::Done,
        "no diff → not gated despite the auto worktree branch"
    );
    let findings = maestro::scheduler::findings::read_findings(&st.run_dir).unwrap();
    assert!(
        !findings.iter().any(|f| f.source == "policy-gate"),
        "a worktree-only branch must not be policy-gated"
    );
    clear_workspace();
}
