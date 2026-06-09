//! F-118 Step 2 review (大力 N1/N2): exercise `maestro doctor runtime` end-to-end
//! through the binary in a throwaway workspace.
//!
//! N1 — a user who passes `--json` must get JSON on stdout, whichever side of the
//! subcommand it sits on: `doctor --json`, `doctor runtime --json`, and
//! `doctor --json runtime` all emit JSON (the last one used to print the human
//! report because the parent `--json` was ignored).
//!
//! N2 — `profiles.valid` / `skills.visible` reuse the same F-114 role/skill
//! visibility rules as `maestro validate`: a missing role must not pass, and a
//! legitimate project-local unscoped skill must not false-warn.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

/// A minimal workspace with a raw `.maestro/projects.yaml` body.
fn workspace(projects_yaml: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".maestro")).unwrap();
    fs::write(tmp.path().join(".maestro/projects.yaml"), projects_yaml).unwrap();
    tmp
}

fn run(tmp: &TempDir, args: &[&str]) -> std::process::Output {
    maestro()
        .current_dir(tmp.path())
        .args(args)
        .output()
        .expect("run maestro")
}

/// stdout parsed as JSON (the report is printed before any non-zero exit).
fn stdout_json(out: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"))
}

fn check_status(report: &serde_json::Value, id: &str) -> String {
    report["checks"]
        .as_array()
        .expect("checks array")
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("check {id} missing"))["status"]
        .as_str()
        .expect("status string")
        .to_string()
}

// --- N1: --json always yields JSON, on either side of `runtime` ---------------

#[test]
fn doctor_json_still_emits_legacy_doctor_json() {
    let tmp = workspace("version: 1\nprojects: {}\n");
    let out = run(&tmp, &["doctor", "--json"]);
    let json = stdout_json(&out);
    // the legacy doctor report shape, not the runtime schema.
    assert!(
        json.get("workspace_root").is_some(),
        "legacy doctor JSON expected"
    );
    assert!(json.get("schema_version").is_none());
}

#[test]
fn doctor_runtime_json_emits_runtime_schema() {
    let tmp = workspace("version: 1\nprojects: {}\n");
    let out = run(&tmp, &["doctor", "runtime", "--json"]);
    let json = stdout_json(&out);
    assert_eq!(json["schema_version"], "maestro.runtime_health.v1");
    assert_eq!(json["checks"].as_array().unwrap().len(), 6);
}

#[test]
fn doctor_json_before_runtime_still_emits_runtime_json() {
    // the regression: `--json` before the subcommand must NOT print the human report.
    let tmp = workspace("version: 1\nprojects: {}\n");
    let out = run(&tmp, &["doctor", "--json", "runtime"]);
    let json = stdout_json(&out);
    assert_eq!(json["schema_version"], "maestro.runtime_health.v1");
}

#[test]
fn doctor_runtime_without_json_is_human_text() {
    let tmp = workspace("version: 1\nprojects: {}\n");
    let out = run(&tmp, &["doctor", "runtime"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("maestro runtime health"),
        "human header expected:\n{stdout}"
    );
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_err());
}

// --- N2: F-114 role/skill visibility parity ----------------------------------

#[test]
fn runtime_health_flags_missing_role_and_accepts_project_local_skill() {
    // dali's repro: a profile with a missing role + an unscoped project-local skill
    // that a project pins. Role missing -> profiles.valid must NOT pass; the
    // project-local skill resolves in the pinning project's scope -> skills.visible
    // must NOT false-warn.
    let tmp = workspace(
        "version: 1\n\
         defaults:\n\
        \x20 agent_profiles:\n\
        \x20   reviewer:\n\
        \x20     role: definitely_missing_role\n\
        \x20     skills: [house-style]\n\
         projects:\n\
        \x20 api:\n\
        \x20   path: .\n\
        \x20   review_profile: reviewer\n",
    );
    // the project-local skill exists under the project's own scope.
    let skills = tmp.path().join(".maestro/skills/api");
    fs::create_dir_all(&skills).unwrap();
    fs::write(
        skills.join("house-style.md"),
        "---\nname: house-style\n---\nbody\n",
    )
    .unwrap();

    let out = run(&tmp, &["doctor", "runtime", "--json"]);
    let json = stdout_json(&out);
    assert_eq!(
        check_status(&json, "profiles.valid"),
        "warn",
        "missing role must not pass:\n{json:#}"
    );
    assert_eq!(
        check_status(&json, "skills.visible"),
        "pass",
        "project-local skill must resolve, not false-warn:\n{json:#}"
    );
}
