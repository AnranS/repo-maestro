//! F-111: `--json` paths must emit a single `PlanPreview` JSON object on stdout
//! and NOTHING else — all human progress (fresh-workspace init, `--root`
//! discovery, goal-matched) goes to stderr — so a caller can `JSON.parse(stdout)`.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

/// Two neutral demo projects under `root`, enough for discovery + synthesis.
fn write_demo_projects(root: &std::path::Path) {
    fs::create_dir_all(root.join("billing-service/src")).unwrap();
    fs::write(
        root.join("billing-service/Cargo.toml"),
        "[package]\nname = \"billing-service\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("billing-service/src/main.rs"), "fn main() {}").unwrap();
    fs::create_dir_all(root.join("web-frontend")).unwrap();
    fs::write(
        root.join("web-frontend/package.json"),
        "{ \"name\": \"web-frontend\", \"version\": \"0.1.0\", \"scripts\": { \"build\": \"echo b\", \"test\": \"echo t\" } }",
    )
    .unwrap();
}

#[test]
fn work_dry_json_stdout_is_pure_json_in_fresh_root_workspace() {
    // Fresh (uninitialized) workspace + `--root`: exercises BOTH the
    // ensure_initialized prints and the discover_and_apply prints, which used to
    // land on stdout before the JSON.
    let tmp = TempDir::new().unwrap();
    write_demo_projects(tmp.path());

    let out = maestro()
        .current_dir(tmp.path())
        .args([
            "work",
            "update the billing service",
            "--root",
            ".",
            "--dry",
            "--json",
        ])
        .output()
        .expect("run maestro");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );

    // stdout is exactly one valid PlanPreview JSON object.
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not pure JSON: {e}\nstdout: {stdout}"));
    assert_eq!(v["schema_version"], "maestro.plan_preview.v1");
    assert!(v["task_count"].as_u64().unwrap() >= 1);
    assert!(
        v.get("goal_matched").is_some(),
        "goal_matched present in work --dry"
    );

    // The human init / discovery lines went to stderr, NOT stdout.
    assert!(
        stderr.contains("discovered") || stderr.contains("initialized"),
        "init/discovery human lines must be on stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("discovered") && !stdout.contains("initialized"),
        "stdout must be pure JSON, found human text:\n{stdout}"
    );
}

#[test]
fn work_dry_json_stdout_is_pure_json_in_initialized_workspace() {
    // Already-initialized workspace, no `--root`: the goal-matched line still
    // routes to stderr; stdout stays a single JSON object.
    let tmp = TempDir::new().unwrap();
    write_demo_projects(tmp.path());
    let init = maestro()
        .current_dir(tmp.path())
        .args(["init", "--no-analyze"])
        .output()
        .expect("init");
    assert!(init.status.success());

    let out = maestro()
        .current_dir(tmp.path())
        .args(["work", "update the billing service", "--dry", "--json"])
        .output()
        .expect("run maestro");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not pure JSON: {e}\nstdout: {stdout}"));
    assert_eq!(v["schema_version"], "maestro.plan_preview.v1");
    assert_eq!(v["goal"], "update the billing service");
}

#[test]
fn work_dry_json_emits_error_envelope_on_synthesis_failure() {
    // Fresh, EMPTY workspace, no --root: synthesis bails (no registered
    // projects). The --json contract says stdout is STILL a valid PlanPreview
    // envelope with the problem in errors[]; the human guidance goes to stderr.
    let tmp = TempDir::new().unwrap();

    let out = maestro()
        .current_dir(tmp.path())
        .args(["work", "do something", "--dry", "--json"])
        .output()
        .expect("run maestro");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "synthesis failure should exit non-zero"
    );

    // stdout is still a single, parseable PlanPreview envelope.
    assert_eq!(stdout.trim().lines().count(), 1, "stdout is one JSON line");
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be a valid envelope: {e}\nstdout: {stdout}"));
    assert_eq!(v["schema_version"], "maestro.plan_preview.v1");
    assert_eq!(v["task_count"], 0);
    let errors = v["errors"].as_array().expect("errors[] array");
    assert!(!errors.is_empty(), "errors[] must carry the failure");
    assert_eq!(errors[0]["code"], "plan.invalid");

    // the full human guidance ("no projects … next: --root …") is on stderr.
    assert!(
        stderr.contains("project") || stderr.contains("next"),
        "human guidance must be on stderr:\n{stderr}"
    );
}
