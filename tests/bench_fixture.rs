use maestro::{bench::fixture, paths};
use serial_test::serial;
use std::{fs, path::Path};
use tempfile::TempDir;

fn with_workspace() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
    }
    dir
}

fn clear_workspace_env() {
    unsafe {
        std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
    }
}

fn write_fixture(root: &Path, scenario: &str, id: &str) {
    let dir = root.join("bench/scenarios").join(scenario);
    fs::create_dir_all(&dir).expect("scenario dir");
    fs::write(
        dir.join("fixture.yaml"),
        format!(
            r#"id: {id}
kind: oss-replay
goal: Add JSON output to the calculator CLI
upstream:
  url: https://github.com/example/project
  parent_sha: abc123
  pr_url: https://github.com/example/project/pull/42
expected:
  touched_files:
    - packages/cli/src/main.rs
  required_projects: [cli]
  required_contract_consumers: []
  forbidden_files: []
budget:
  max_runtime_secs: 120
  max_tasks: 8
"#
        ),
    )
    .expect("fixture yaml");
}

#[test]
#[serial]
fn bench_paths_resolve_under_workspace() {
    let workspace = with_workspace();

    assert_eq!(
        paths::bench_scenarios_dir().unwrap(),
        workspace.path().join("bench/scenarios")
    );
    assert_eq!(
        paths::bench_cache_dir().unwrap(),
        workspace.path().join(".maestro/bench/cache")
    );
    assert_eq!(
        paths::bench_runs_dir().unwrap(),
        workspace.path().join(".maestro/bench/runs")
    );

    clear_workspace_env();
}

#[test]
#[serial]
fn loads_all_scenarios_returns_empty_when_dir_missing() {
    let _workspace = with_workspace();

    let fixtures = fixture::load_all().expect("load fixtures");

    assert!(fixtures.is_empty());
    clear_workspace_env();
}

#[test]
#[serial]
fn loads_all_scenarios_from_fixture_yaml_files() {
    let workspace = with_workspace();
    write_fixture(workspace.path(), "example-one", "example-one");
    write_fixture(workspace.path(), "example-two", "example-two");

    let fixtures = fixture::load_all().expect("load fixtures");

    let ids: Vec<_> = fixtures.into_iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["example-one", "example-two"]);
    clear_workspace_env();
}

#[test]
#[serial]
fn load_all_rejects_duplicate_fixture_ids() {
    let workspace = with_workspace();
    write_fixture(workspace.path(), "first", "duplicate-id");
    write_fixture(workspace.path(), "second", "duplicate-id");

    let err = fixture::load_all().expect_err("duplicate ids should fail");

    assert!(err.to_string().contains("duplicate fixture id"));
    clear_workspace_env();
}
