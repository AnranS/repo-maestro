//! F-114 Step 6: `maestro agent-profile` CLI — exercised end-to-end through the
//! binary in a throwaway workspace. `new` writes a disabled draft, `eval`
//! dry-runs trigger matching, and bad input is rejected.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".maestro")).unwrap();
    fs::write(
        tmp.path().join(".maestro/projects.yaml"),
        "version: 1\ndefaults:\n  agent: cursor\nprojects:\n  api:\n    path: .\n",
    )
    .unwrap();
    tmp
}

fn run(tmp: &TempDir, args: &[&str]) -> std::process::Output {
    maestro()
        .current_dir(tmp.path())
        .args(args)
        .output()
        .expect("run maestro agent-profile")
}

#[test]
fn new_writes_a_disabled_draft() {
    let tmp = workspace();
    let out = run(
        &tmp,
        &[
            "agent-profile",
            "new",
            "reviewer",
            "--template",
            "contract-reviewer",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // persisted as a disabled draft
    let yaml = fs::read_to_string(tmp.path().join(".maestro/projects.yaml")).unwrap();
    assert!(yaml.contains("agent_profiles:"));
    assert!(yaml.contains("reviewer:"));
    assert!(yaml.contains("enabled: false"));

    // ls + show see it
    let ls = run(&tmp, &["agent-profile", "ls"]);
    assert!(String::from_utf8_lossy(&ls.stdout).contains("reviewer"));
    let show = run(&tmp, &["agent-profile", "show", "reviewer"]);
    let body = String::from_utf8_lossy(&show.stdout);
    assert!(body.contains("role: refuter") && body.contains("contract_changed"));
}

#[test]
fn new_rejects_unknown_template_and_duplicate() {
    let tmp = workspace();
    let bad = run(&tmp, &["agent-profile", "new", "x", "--template", "nope"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown template"));

    assert!(run(&tmp, &["agent-profile", "new", "dup"]).status.success());
    let dup = run(&tmp, &["agent-profile", "new", "dup"]);
    assert!(!dup.status.success());
    assert!(String::from_utf8_lossy(&dup.stderr).contains("already exists"));
}

#[test]
fn eval_reports_match_against_a_fixture() {
    let tmp = workspace();
    assert!(run(
        &tmp,
        &[
            "agent-profile",
            "new",
            "rev",
            "--template",
            "contract-reviewer"
        ]
    )
    .status
    .success());

    // a contract-touching fixture matches the post-task triggers
    fs::write(
        tmp.path().join("hit.yaml"),
        "contract_changed: true\nchanged_paths: [idl/user.thrift]\n",
    )
    .unwrap();
    let hit = run(
        &tmp,
        &["agent-profile", "eval", "rev", "--fixture", "hit.yaml"],
    );
    assert!(hit.status.success());
    let out = String::from_utf8_lossy(&hit.stdout);
    assert!(out.contains("MATCHES as reviewer"), "{out}");

    // an unrelated change matches nothing
    fs::write(
        tmp.path().join("miss.yaml"),
        "changed_paths: [src/main.rs]\n",
    )
    .unwrap();
    let miss = run(
        &tmp,
        &["agent-profile", "eval", "rev", "--fixture", "miss.yaml"],
    );
    assert!(miss.status.success());
    assert!(String::from_utf8_lossy(&miss.stdout).contains("no trigger matches"));
}

#[test]
fn eval_does_not_claim_reviewer_for_a_non_review_capable_profile() {
    // N1: a profile with BOTH a pre-dispatch and a post-task trigger but no
    // `review_verdict` output must report as a writer only — never "and
    // reviewer".
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".maestro")).unwrap();
    fs::write(
        tmp.path().join(".maestro/projects.yaml"),
        r#"version: 1
defaults:
  agent_profiles:
    mixed:
      role: refuter
      triggers:
        - on: task_kind
          kind: agent
        - on: high_risk
      outputs:
        - finding_kind: doctor
projects:
  api:
    path: .
"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("facts.yaml"),
        "task_kind: agent\nhigh_risk: true\n",
    )
    .unwrap();

    let out = run(
        &tmp,
        &["agent-profile", "eval", "mixed", "--fixture", "facts.yaml"],
    );
    assert!(out.status.success());
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("MATCHES as writer"), "{body}");
    assert!(
        !body.contains("and reviewer"),
        "must not claim reviewer: {body}"
    );
    assert!(body.contains("NOT review-capable"), "{body}");
}

#[test]
fn promote_gates_on_usability_then_enables() {
    let tmp = workspace();

    // a blank draft has no trigger and no binding → promotion is refused.
    assert!(run(&tmp, &["agent-profile", "new", "bare"])
        .status
        .success());
    let refused = run(&tmp, &["agent-profile", "promote", "bare"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("would never be used"));

    // a triggered draft promotes, and the change is persisted as enabled.
    assert!(run(
        &tmp,
        &[
            "agent-profile",
            "new",
            "rev",
            "--template",
            "contract-reviewer"
        ],
    )
    .status
    .success());
    let ok = run(&tmp, &["agent-profile", "promote", "rev"]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("now enabled"));
    let show = run(&tmp, &["agent-profile", "show", "rev"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("enabled: true"));

    // promoting again is idempotent (no error).
    let again = run(&tmp, &["agent-profile", "promote", "rev"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("already enabled"));
}

#[test]
fn promote_requires_unscoped_skill_in_every_pinning_project() {
    // N1: a profile pinned by api AND web with an unscoped skill that exists
    // only in api's scope must NOT promote — it would fail on web at dispatch.
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".maestro")).unwrap();
    fs::write(
        tmp.path().join(".maestro/projects.yaml"),
        r#"version: 1
defaults:
  agent_profiles:
    rev:
      role: refuter
      skills: [house-style]
      enabled: false
projects:
  api:
    path: .
    review_profile: rev
  web:
    path: .
    review_profile: rev
"#,
    )
    .unwrap();
    // skill exists only under api's scope
    let api_skill = tmp.path().join(".maestro/skills/api");
    fs::create_dir_all(&api_skill).unwrap();
    fs::write(
        api_skill.join("house-style.md"),
        "---\nname: house-style\n---\nx\n",
    )
    .unwrap();

    let refused = run(&tmp, &["agent-profile", "promote", "rev"]);
    assert!(!refused.status.success());
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("not defined for project 'web'"),
        "must name the missing project: {err}"
    );

    // make it resolvable for web too (global scope) → promote succeeds.
    let global = tmp.path().join(".maestro/skills/_global");
    fs::create_dir_all(&global).unwrap();
    fs::write(
        global.join("house-style.md"),
        "---\nname: house-style\n---\nx\n",
    )
    .unwrap();
    let ok = run(&tmp, &["agent-profile", "promote", "rev"]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

/// A minimal `RUN_STATE.json` with one high-risk agent task that triggered a
/// skill, plus a `refute` finding — enough for `train` to distill from.
const RUN_STATE_JSON: &str = r#"{"run_id":"r-test","spec":"x","started_at":"2026-06-04T00:00:00Z","ended_at":null,"status":"done","max_parallel":1,"pid":0,"tasks":{"T0":{"id":"T0","project":"api","agent":"mock","status":"done","started_at":null,"ended_at":null,"chat_id":null,"error":null,"attempts":0,"risk_level":"high","artifacts":{},"permission":null,"workflow_outputs":{},"log_path":"x.log","trajectory_path":null,"depends_on":[],"parallel_group":null,"requires_approval_after":false,"kind":"agent","memory_used":[],"context_bytes":null,"skills_triggered":["customer-portal/house-style","_global/contract-first"],"usage":null,"steps":null,"role":"backend_rust","resolved_agent_profile":null,"workspace_path":null,"worktree_path":null}},"approvals_pending":[],"task_order":["T0"],"session_id":null,"usage":{},"budget_tokens":null,"pending_gate":null,"goal":null,"acceptance_results":[],"verified":false,"auto_actions":[],"run_dir":"."}"#;

#[test]
fn train_distills_a_sanitized_disabled_draft_from_a_run() {
    let tmp = workspace();
    let run_dir = tmp.path().join(".maestro/runs/r-test");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(run_dir.join("RUN_STATE.json"), RUN_STATE_JSON).unwrap();
    fs::write(
        run_dir.join("findings.ndjson"),
        r#"{"schema_version":"maestro.finding.v1","finding_id":"refute-1","run_id":"r-test","seq":1,"task_id":"T0","kind":"refute","severity":"high","summary":"x","evidence_refs":[],"source":"refuter","status":"open","created_at":"2026-06-04T00:00:00Z","provenance":{"producer":"refuter"}}
"#,
    )
    .unwrap();

    let out = run(
        &tmp,
        &[
            "agent-profile",
            "train",
            "distilled",
            "--from-run",
            "r-test",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary = String::from_utf8_lossy(&out.stdout);
    // summary is sanitized: counts + kinds only, never the spec / task paths /
    // the dropped project-scoped skill name.
    assert!(summary.contains("1 task(s)") && summary.contains("refute"));
    assert!(
        summary.contains("dropped 1 project-scoped skill"),
        "{summary}"
    );
    assert!(
        !summary.contains("customer-portal"),
        "leaked project name: {summary}"
    );

    // the draft is disabled, carries the distilled shape, keeps the _global
    // skill, and never embeds the project-scoped skill / project name.
    let show = run(&tmp, &["agent-profile", "show", "distilled"]);
    let body = String::from_utf8_lossy(&show.stdout);
    assert!(body.contains("enabled: false"));
    assert!(body.contains("role: backend_rust"));
    assert!(body.contains("_global/contract-first"));
    assert!(
        !body.contains("customer-portal") && !body.contains("house-style"),
        "trained draft must not embed a project-scoped skill: {body}"
    );
    assert!(body.contains("on: high_risk") && body.contains("on: finding_kind"));
}

#[test]
fn train_errors_on_unknown_run() {
    let tmp = workspace();
    let out = run(
        &tmp,
        &[
            "agent-profile",
            "train",
            "p",
            "--from-run",
            "does-not-exist",
        ],
    );
    assert!(!out.status.success());
}
