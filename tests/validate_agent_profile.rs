//! F-114: `maestro validate` must fail on an `agent_profile` whose `role` or
//! `skill` does not exist on disk (design lines 300-306), resolve unscoped
//! skills against a pinning project's scope (project-first, then `_global`),
//! and stay read-only (never create the role/skill registry dirs). These are
//! existence checks against the on-disk registries, so they're exercised
//! end-to-end through the binary in a throwaway workspace.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

/// Write `.maestro/projects.yaml` from a `defaults.agent_profiles` block and a
/// `projects` block under a fresh workspace, and return it.
fn workspace(profiles_yaml: &str, projects_yaml: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".maestro")).unwrap();
    let yaml = format!(
        "version: 1\ndefaults:\n  agent_profiles:\n{profiles_yaml}projects:\n{projects_yaml}"
    );
    fs::write(tmp.path().join(".maestro/projects.yaml"), yaml).unwrap();
    tmp
}

fn write_skill(tmp: &TempDir, scope: &str, name: &str) {
    let dir = tmp.path().join(".maestro/skills").join(scope);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n---\nbody\n"),
    )
    .unwrap();
}

fn run_validate(tmp: &TempDir) -> std::process::Output {
    maestro()
        .current_dir(tmp.path())
        .arg("validate")
        .output()
        .expect("run maestro validate")
}

// a project that pins `reviewer` via review_profile; `.` path always exists.
const API_PINS_REVIEWER: &str = "  api:\n    path: .\n    review_profile: reviewer\n";
// a project that pins nothing.
const API_PLAIN: &str = "  api:\n    path: .\n";

#[test]
fn validate_fails_on_missing_role() {
    // `refuter` is a builtin role; `definitely_missing_role` is not.
    let tmp = workspace(
        "    reviewer:\n      role: definitely_missing_role\n",
        API_PLAIN,
    );
    let out = run_validate(&tmp);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "validate should fail:\n{stderr}");
    assert!(
        stderr.contains("agent_profile 'reviewer': role 'definitely_missing_role' is not defined"),
        "missing role must be reported:\n{stderr}"
    );
}

#[test]
fn validate_fails_on_missing_explicit_global_skill() {
    // explicit `_global/<missing>` fails regardless of project pinning.
    let tmp = workspace(
        "    reviewer:\n      role: refuter\n      skills: [_global/definitely_missing_skill]\n",
        API_PLAIN,
    );
    let out = run_validate(&tmp);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "validate should fail:\n{stderr}");
    assert!(
        stderr.contains("skill '_global/definitely_missing_skill' is not defined"),
        "explicit-scope missing skill must be reported:\n{stderr}"
    );
}

#[test]
fn validate_fails_on_missing_unscoped_skill_for_pinned_profile() {
    // N2: an unscoped skill on a project-pinned profile is checked against that
    // project's scope, then global — missing in both → fail, naming the project.
    let tmp = workspace(
        "    reviewer:\n      role: refuter\n      skills: [also_missing]\n",
        API_PINS_REVIEWER,
    );
    let out = run_validate(&tmp);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "validate should fail:\n{stderr}");
    assert!(
        stderr.contains("skill 'also_missing' is not defined for project 'api'"),
        "unscoped missing skill must name the pinning project:\n{stderr}"
    );
}

#[test]
fn validate_passes_with_project_local_skill() {
    // N2 main path (dali's repro): unscoped `contract-first` resolves in the
    // pinning project's own scope `.maestro/skills/api/`.
    let tmp = workspace(
        "    reviewer:\n      role: refuter\n      skills: [contract-first]\n",
        API_PINS_REVIEWER,
    );
    write_skill(&tmp, "api", "contract-first");
    let out = run_validate(&tmp);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "validate should pass; stdout: {stdout} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("ok"), "expected ok summary:\n{stdout}");
}

#[test]
fn validate_passes_with_existing_global_skill() {
    let tmp = workspace(
        "    reviewer:\n      role: refuter\n      skills: [_global/contract-first]\n",
        API_PINS_REVIEWER,
    );
    write_skill(&tmp, "_global", "contract-first");
    let out = run_validate(&tmp);
    assert!(
        out.status.success(),
        "validate should pass; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn validate_defers_unscoped_skill_for_unreferenced_profile() {
    // A profile pinned by no project (would only ever auto-match by trigger):
    // its unscoped skill has no project context, so Step 2 defers — validate
    // passes even though `floating` exists nowhere. Role still must exist.
    let tmp = workspace(
        "    reviewer:\n      role: refuter\n      skills: [floating]\n",
        API_PLAIN,
    );
    let out = run_validate(&tmp);
    assert!(
        out.status.success(),
        "unscoped skill on an unreferenced profile should be deferred, not failed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn validate_is_read_only_and_creates_no_registry_dirs() {
    // N2b: existence checks must not create `.maestro/roles` or `.maestro/skills`.
    let tmp = workspace(
        "    reviewer:\n      role: definitely_missing_role\n      skills: [_global/definitely_missing_skill]\n",
        API_PINS_REVIEWER,
    );
    let _ = run_validate(&tmp); // fails on the missing role/skill — that's fine
    assert!(
        !tmp.path().join(".maestro/roles").exists(),
        "validate must not create .maestro/roles"
    );
    assert!(
        !tmp.path().join(".maestro/skills").exists(),
        "validate must not create .maestro/skills"
    );
}
