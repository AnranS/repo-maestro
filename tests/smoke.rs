//! Tier-B smoke tests for maestro internals. These don't aim for full coverage —
//! they're guardrails on the five things we've burned the most time on.

use std::collections::BTreeMap;

use maestro::chat::actions::{parse_actions, ActionVerb};
use maestro::config::{analyze, Plan, Project, ProjectsConfig};
use maestro::scheduler::dag::TaskGraph;
use maestro::scheduler::{append_event, read_events, RunEventKind};

fn yaml_plan(yaml: &str) -> Plan {
    let mut plan: Plan = serde_yaml::from_str(yaml).expect("parse plan yaml");
    plan = plan.expand_for_each();
    plan.validate().expect("plan validates");
    plan
}

// ---------- action parsing ----------

#[test]
fn parse_actions_picks_up_run_block() {
    let text =
        "Some prose.\n\n```maestro-action\nverb: run\nplan: plans/foo.yaml\n```\n\nMore prose.";
    let actions = parse_actions(text);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].verb, ActionVerb::Run);
    assert_eq!(
        actions[0].args.get("plan").map(|s| s.as_str()),
        Some("plans/foo.yaml")
    );
}

#[test]
fn parse_actions_skips_unknown_verb() {
    let text = "```maestro-action\nverb: nuke\nplan: foo.yaml\n```\n";
    assert!(parse_actions(text).is_empty());
}

#[test]
fn parse_actions_handles_multiple_blocks() {
    let text = r#"
```maestro-action
verb: status
```

```maestro-action
verb: approve
task: T1
```
"#;
    let actions = parse_actions(text);
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].verb, ActionVerb::Status);
    assert_eq!(actions[1].verb, ActionVerb::Approve);
    assert_eq!(actions[1].args.get("task").map(|s| s.as_str()), Some("T1"));
}

// ---------- plan: for-each expansion + dep rewrite ----------

#[test]
fn for_each_expands_into_concrete_tasks_and_rewrites_deps() {
    let yaml = r#"
spec: test
tasks:
  - id: T_setup
    project: api
    prompt: setup
  - id: T_lint
    project_each: [api, web, cli]
    prompt: "lint {{project}}"
    depends_on: [T_setup]
  - id: T_final
    project: api
    prompt: final
    depends_on: [T_lint]
"#;
    let plan = yaml_plan(yaml);
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"T_lint__api"));
    assert!(ids.contains(&"T_lint__web"));
    assert!(ids.contains(&"T_lint__cli"));

    let final_task = plan.task("T_final").expect("T_final");
    assert_eq!(final_task.depends_on.len(), 3);
    assert!(final_task.depends_on.contains(&"T_lint__api".to_string()));

    // {{project}} placeholder is substituted
    let web = plan.task("T_lint__web").unwrap();
    assert!(web.prompt.contains("web"));
    assert!(!web.prompt.contains("{{project}}"));
}

#[test]
fn workflow_input_refs_add_ordering_dependency() {
    let yaml = r#"
spec: data dependency
tasks:
  - id: T_contract
    project: api
    prompt: write contract
    outputs:
      openapi:
        path: schemas/openapi.yaml
  - id: T_web
    project: web
    prompt: consume contract
    inputs:
      api_contract:
        from: T_contract.openapi
"#;
    let plan = yaml_plan(yaml);
    let web = plan.task("T_web").unwrap();
    assert_eq!(web.depends_on, vec!["T_contract".to_string()]);
}

#[test]
fn workflow_input_refs_must_point_at_declared_outputs() {
    let yaml = r#"
spec: bad data dependency
tasks:
  - id: T_contract
    project: api
    prompt: write contract
  - id: T_web
    project: web
    prompt: consume contract
    inputs:
      api_contract:
        from: T_contract.openapi
"#;
    let mut plan: Plan = serde_yaml::from_str(yaml).unwrap();
    plan = plan.expand_for_each();
    let err = plan.validate().expect_err("validation error");
    assert!(format!("{err:#}").contains("unknown output openapi"));
}

#[test]
fn plan_tasks_accept_explicit_skill_dependencies() {
    let yaml = r#"
spec: skill deps
tasks:
  - id: T_impl
    project: api
    skills: [workflow-task-guardrails, contract-first]
    prompt: implement endpoint
"#;
    let plan = yaml_plan(yaml);
    let task = plan.task("T_impl").unwrap();
    assert_eq!(
        task.skills,
        vec![
            "workflow-task-guardrails".to_string(),
            "contract-first".to_string()
        ]
    );
}

#[test]
fn run_events_append_and_read_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    append_event(
        tmp.path(),
        "run-1",
        RunEventKind::RunCreated,
        None,
        Some("started".into()),
        serde_json::json!({ "task_count": 1 }),
    )
    .unwrap();
    append_event(
        tmp.path(),
        "run-1",
        RunEventKind::TaskStarted,
        Some("T1"),
        None,
        serde_json::json!({}),
    )
    .unwrap();

    let events = read_events(tmp.path()).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[0].event_id, "run-1-1");
    assert_eq!(events[1].kind, RunEventKind::TaskStarted);
    assert_eq!(events[1].task_id.as_deref(), Some("T1"));
    assert_eq!(events[0].schema_version, "maestro.run_event.v1");
    assert_eq!(events[0].kind.as_str(), "run.started");
    assert_eq!(events[1].kind.as_str(), "task.started");
    assert!(events[0].refs.is_empty());

    let raw = std::fs::read_to_string(tmp.path().join("events.ndjson")).unwrap();
    let first: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    assert_eq!(first["schema_version"], "maestro.run_event.v1");
    assert_eq!(first["kind"], "run.started");
    assert!(first["refs"].as_object().unwrap().is_empty());

    let too_large = "x".repeat(1_025);
    let err = append_event(
        tmp.path(),
        "run-1",
        RunEventKind::TaskStarted,
        Some("T2"),
        None,
        serde_json::json!({ "body": too_large }),
    )
    .expect_err("payload over 1KB must be rejected");
    assert!(format!("{err:#}").contains("payload"));
}

// ---------- DAG: cycle detection + initial-ready ----------

#[test]
fn dag_detects_cycle() {
    let yaml = r#"
spec: cycle test
tasks:
  - id: A
    project: api
    prompt: a
    depends_on: [B]
  - id: B
    project: api
    prompt: b
    depends_on: [A]
"#;
    // Plan::validate doesn't check for cycles (just dep existence), so it
    // parses; TaskGraph rejects it.
    let plan: Plan = serde_yaml::from_str(yaml).unwrap();
    let err = TaskGraph::from_plan(&plan).err().expect("cycle error");
    assert!(format!("{err:#}").contains("cycle"));
}

#[test]
fn dag_initial_ready_contains_root_tasks() {
    let yaml = r#"
spec: ready test
tasks:
  - id: root1
    project: api
    prompt: r1
  - id: root2
    project: web
    prompt: r2
  - id: child
    project: cli
    prompt: c
    depends_on: [root1, root2]
"#;
    let plan = yaml_plan(yaml);
    let graph = TaskGraph::from_plan(&plan).unwrap();
    let mut ready = graph.initial_ready();
    ready.sort();
    assert_eq!(ready, vec!["root1".to_string(), "root2".to_string()]);
}

// ---------- contracts analyzer: warn when consumers don't depend on producers ----------

#[test]
fn analyzer_warns_on_missing_contract_edge() {
    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    let mut api = Project {
        path: "/tmp/api".into(),
        r#type: Some("backend".into()),
        stack: vec![],
        commands: BTreeMap::new(),
        contracts: Default::default(),
        dependencies: Vec::new(),
        memory_scope: vec![],
        agent: None,
        agent_model: None,
        cursor_model: None,
        model_profile: None,
        role: None,
        agent_profile: None,
        review_profile: None,
        copy_files: Vec::new(),
    };
    api.contracts.provides = Some("schemas/openapi.yaml".into());
    let mut web = api.clone();
    web.path = "/tmp/web".into();
    web.r#type = Some("frontend".into());
    web.contracts.provides = None;
    web.contracts.consumes = Some("schemas/openapi.yaml".into());
    projects.projects.insert("api".into(), api);
    projects.projects.insert("web".into(), web);

    let yaml = r#"
spec: bad plan
tasks:
  - id: T_api
    project: api
    prompt: change schema
  - id: T_web
    project: web
    prompt: render
"#;
    let plan = yaml_plan(yaml);
    let report = analyze(&plan, &projects);
    assert!(
        report.warning_count() >= 1,
        "expected race-warning for web→api edge"
    );
    assert!(!report.has_errors());
}

#[test]
fn analyzer_passes_when_consumer_depends_on_producer() {
    let mut projects = ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects: BTreeMap::new(),
    };
    let mut api = Project {
        path: "/tmp/api".into(),
        r#type: Some("backend".into()),
        stack: vec![],
        commands: BTreeMap::new(),
        contracts: Default::default(),
        dependencies: Vec::new(),
        memory_scope: vec![],
        agent: None,
        agent_model: None,
        cursor_model: None,
        model_profile: None,
        role: None,
        agent_profile: None,
        review_profile: None,
        copy_files: Vec::new(),
    };
    api.contracts.provides = Some("schemas/openapi.yaml".into());
    let mut web = api.clone();
    web.path = "/tmp/web".into();
    web.contracts.provides = None;
    web.contracts.consumes = Some("schemas/openapi.yaml".into());
    projects.projects.insert("api".into(), api);
    projects.projects.insert("web".into(), web);

    let yaml = r#"
spec: good plan
tasks:
  - id: T_api
    project: api
    prompt: change schema
  - id: T_web
    project: web
    prompt: render
    depends_on: [T_api]
"#;
    let plan = yaml_plan(yaml);
    let report = analyze(&plan, &projects);
    let race_warnings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| matches!(f, maestro::config::Finding::Warning { message, .. } if message.contains("contract")))
        .collect();
    assert!(
        race_warnings.is_empty(),
        "expected no contract race warnings, got {} ({:?})",
        race_warnings.len(),
        race_warnings
    );
}
