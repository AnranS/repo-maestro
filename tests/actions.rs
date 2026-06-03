//! Pin the verb → CLI argv translation. The agent-emitted YAML block is
//! parsed into an `Action` (covered in `smoke.rs`); from there `build_argv`
//! produces the exact argument vector we hand to `maestro`. If anyone tweaks
//! a flag name in `cli/`, these tests catch the drift.

use std::collections::BTreeMap;

use maestro::chat::actions::{build_argv, parse_actions, Action, ActionVerb};

fn action(verb: ActionVerb, args: &[(&str, &str)]) -> Action {
    let mut map = BTreeMap::new();
    for (k, v) in args {
        map.insert((*k).to_string(), (*v).to_string());
    }
    Action {
        id: "a-id".into(),
        verb,
        args: map,
        status: None,
        label: Action::label_for(verb, &Default::default()),
        output: None,
    }
}

#[test]
fn run_with_plan_only() {
    let argv = build_argv(&action(ActionVerb::Run, &[("plan", "plans/foo.yaml")])).unwrap();
    assert_eq!(argv, vec!["run", "plans/foo.yaml"]);
}

#[test]
fn run_with_only_and_skip_flags() {
    let argv = build_argv(&action(
        ActionVerb::Run,
        &[
            ("plan", "plans/foo.yaml"),
            ("only", "T1,T2"),
            ("skip", "T3"),
        ],
    ))
    .unwrap();
    assert_eq!(
        argv,
        vec!["run", "plans/foo.yaml", "--only", "T1,T2", "--skip", "T3"]
    );
}

#[test]
fn run_missing_plan_arg_is_error() {
    let err = build_argv(&action(ActionVerb::Run, &[])).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("missing"), "msg = {msg}");
}

#[test]
fn approve_requires_task() {
    let argv = build_argv(&action(ActionVerb::Approve, &[("task", "T_design_review")])).unwrap();
    assert_eq!(argv, vec!["approve", "T_design_review"]);

    let err = build_argv(&action(ActionVerb::Approve, &[])).unwrap_err();
    assert!(format!("{err:#}").contains("missing"));
}

#[test]
fn status_takes_no_args() {
    let argv = build_argv(&action(ActionVerb::Status, &[])).unwrap();
    assert_eq!(argv, vec!["status"]);
}

#[test]
fn rerun_with_from_flag() {
    let argv = build_argv(&action(
        ActionVerb::Rerun,
        &[("plan", "20260516-x"), ("from", "T2")],
    ))
    .unwrap();
    assert_eq!(argv, vec!["rerun", "20260516-x", "--from", "T2"]);
}

#[test]
fn plan_validate_takes_plan() {
    let argv = build_argv(&action(
        ActionVerb::PlanValidate,
        &[("plan", "plans/x.yaml")],
    ))
    .unwrap();
    assert_eq!(argv, vec!["plan", "validate", "plans/x.yaml"]);
}

#[test]
fn run_with_matching_plan_hash_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("p.yaml");
    std::fs::write(&plan, "spec: guarded\ntasks: []\n").unwrap();
    let plan_str = plan.to_string_lossy().to_string();
    let hash = maestro::file_guard::file_hash(&plan).unwrap();

    let argv = build_argv(&action(
        ActionVerb::Run,
        &[("plan", &plan_str), ("plan_hash", &hash)],
    ))
    .unwrap();

    assert_eq!(argv, vec!["run".to_string(), plan_str]);
}

#[test]
fn run_with_stale_plan_hash_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("p.yaml");
    std::fs::write(&plan, "spec: guarded\ntasks: []\n").unwrap();
    let plan_str = plan.to_string_lossy().to_string();

    let err = build_argv(&action(
        ActionVerb::Run,
        &[
            ("plan", &plan_str),
            ("plan_hash", "fnv1a64:0000000000000000"),
        ],
    ))
    .unwrap_err();

    assert!(format!("{err:#}").contains("plan_hash mismatch"));
}

#[test]
fn work_scans_plans_and_optionally_runs() {
    let argv = build_argv(&action(
        ActionVerb::Work,
        &[
            ("spec", "Add JSON output"),
            ("root", "~/work/projects"),
            ("agent", "codex"),
            ("out", "plans/json.yaml"),
            ("run", "true"),
        ],
    ))
    .unwrap();
    assert_eq!(
        argv,
        vec![
            "work",
            "Add JSON output",
            "--root",
            "~/work/projects",
            "--agent",
            "codex",
            "--out",
            "plans/json.yaml",
            "--run",
        ]
    );
}

#[test]
fn label_for_uses_verb_specific_format() {
    let mut args = BTreeMap::new();
    args.insert("plan".into(), "foo.yaml".into());
    assert_eq!(
        Action::label_for(ActionVerb::Run, &args),
        "maestro run foo.yaml"
    );

    args.clear();
    args.insert("task".into(), "T_x".into());
    assert_eq!(
        Action::label_for(ActionVerb::Approve, &args),
        "maestro approve T_x"
    );

    assert_eq!(
        Action::label_for(ActionVerb::Status, &Default::default()),
        "maestro status"
    );
}

/// Sanity: the parser + build_argv round-trip a realistic chat message into
/// the exact CLI command we'd run.
#[test]
fn parse_then_build_argv_round_trip() {
    let chat = r#"
Sure, here's the plan:

```maestro-action
verb: run
plan: plans/login.yaml
only: T1,T2
```

Let me know if anything looks off.
"#;
    let actions = parse_actions(chat);
    assert_eq!(actions.len(), 1);
    let argv = build_argv(&actions[0]).unwrap();
    assert_eq!(argv, vec!["run", "plans/login.yaml", "--only", "T1,T2"]);
}
