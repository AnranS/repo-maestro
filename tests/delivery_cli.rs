//! F-127a — `maestro delivery` end-to-end. Drives the real binary so the exit
//! codes + coded output of intake / show / ls are locked, not just the store +
//! parser unit layer. The headline contract: a corrupt DELIVERY.json is an
//! explicit non-zero error, never a silent empty.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use maestro::scheduler::state::RunState;
use serde_json::json;
use tempfile::TempDir;

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

fn delivery(ws: &Path, args: &[&str]) -> Output {
    let mut c = maestro();
    c.env("MAESTRO_WORKSPACE_ROOT", ws)
        .current_dir(ws)
        .arg("delivery");
    c.args(args);
    c.output().expect("run maestro delivery")
}

/// A workspace with one registered project on the offline `mock` agent — enough
/// for `delivery plan`/`run` to synthesize + execute a plan with no provider.
fn ws_with_project() -> TempDir {
    let ws = TempDir::new().unwrap();
    fs::create_dir_all(ws.path().join(".maestro")).unwrap();
    fs::create_dir_all(ws.path().join("proj-a")).unwrap();
    fs::write(
        ws.path().join(".maestro/projects.yaml"),
        "projects:\n  proj-a:\n    path: ./proj-a\n    agent: mock\ndefaults:\n  agent: mock\n  max_parallel: 1\n",
    )
    .unwrap();
    // a git root mirrors a real workspace (the smoke path); run still completes.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(ws.path())
        .status()
        .ok();
    ws
}

/// Count actual runs under `.maestro/runs` (dirs with a RUN_STATE.json).
fn run_count(ws: &Path) -> usize {
    match fs::read_dir(ws.join(".maestro/runs")) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("RUN_STATE.json").exists())
            .count(),
        Err(_) => 0,
    }
}

fn run_id_of(ws: &Path, id: &str) -> String {
    let out = delivery(ws, &["show", id, "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["run_id"].as_str().unwrap().to_string()
}

/// Overwrite a run's RUN_STATE status/verified (and clear any pending_gate) so the
/// accept-source classification can be exercised without a real verified run.
fn set_run_state(ws: &Path, run_id: &str, status: &str, verified: bool) {
    let p = ws.join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("status".into(), json!(status));
    o.insert("verified".into(), json!(verified));
    o.remove("pending_gate");
    fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
}

/// Drive a delivery to Execute (real mock run) and return its run_id. Optional
/// `source_uri` records a doc uri on the intake source (the write-back target).
fn drive_to_execute(ws: &Path, id: &str, source_uri: Option<&str>) -> String {
    let file = write_req(ws, &format!("{id}.md"), GOOD);
    let mut args: Vec<&str> = vec!["intake", "--file", file.as_str(), "--id", id];
    if let Some(u) = source_uri {
        args.push("--source-uri");
        args.push(u);
    }
    assert!(delivery(ws, &args).status.success());
    assert!(delivery(
        ws,
        &[
            "spec",
            id,
            "--prd",
            "Add a healthcheck endpoint",
            "--project",
            "proj-a",
            "--accept",
            "ok :: test 1 = 1",
        ],
    )
    .status
    .success());
    assert!(delivery(ws, &["confirm-spec", id, "--by", "alice"])
        .status
        .success());
    assert!(delivery(ws, &["plan", id]).status.success());
    assert!(delivery(ws, &["run", id]).status.success());
    run_id_of(ws, id)
}

/// Drive a delivery from intake to a confirmed spec (the F-127b precondition).
fn intake_spec_confirm(ws: &Path, id: &str) {
    let file = write_req(ws, "req.md", GOOD);
    assert!(delivery(ws, &["intake", "--file", &file, "--id", id])
        .status
        .success());
    assert!(delivery(
        ws,
        &[
            "spec",
            id,
            "--prd",
            "Add a healthcheck endpoint",
            "--project",
            "proj-a",
            "--accept",
            "returns 200 :: test 1 = 1",
        ],
    )
    .status
    .success());
    let c = delivery(ws, &["confirm-spec", id, "--by", "alice"]);
    assert!(
        c.status.success(),
        "confirm should pass: {}",
        String::from_utf8_lossy(&c.stderr)
    );
}

fn write_req(ws: &Path, name: &str, body: &str) -> String {
    let p = ws.join(name);
    fs::write(&p, body).unwrap();
    p.to_string_lossy().into_owned()
}

/// Hand-edit a persisted DELIVERY.json (the "tampered file" attack surface).
fn tamper(path: &Path, mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) {
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    mutate(v.as_object_mut().unwrap());
    fs::write(path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

const GOOD: &str = "\
## Objective
Ship CSV export for the analytics dashboard.

## Target Users
Data analysts.

## Acceptance
- CSV downloads from the dashboard
- exported numbers match the on-screen table

## Constraints
- no DB schema change
";

#[test]
fn intake_well_formed_lands_in_intake_stage() {
    let ws = TempDir::new().unwrap();
    let file = write_req(ws.path(), "req-good.md", GOOD);
    let out = delivery(ws.path(), &["intake", "--file", &file, "--id", "d-good"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "intake should succeed: {stdout}");
    assert!(stdout.contains("stage: Intake"), "stdout: {stdout}");
    assert!(
        stdout.contains("no blocking clarifications"),
        "stdout: {stdout}"
    );
    // The record actually landed at the contracted path.
    let landed = ws.path().join(".maestro/deliveries/d-good/DELIVERY.json");
    assert!(landed.exists(), "DELIVERY.json should exist at {landed:?}");
    // refs-first: structured fields are extracted, but the raw markdown doc is
    // NOT pasted wholesale into the record (no `## ...` headings survive).
    let body = fs::read_to_string(&landed).unwrap();
    assert!(
        !body.contains("## Objective") && !body.contains("## Acceptance"),
        "raw markdown headings must not be pasted into the record; body={body}"
    );
}

#[test]
fn intake_missing_required_fields_become_blocking_clarify_not_silent() {
    let ws = TempDir::new().unwrap();
    let file = write_req(ws.path(), "req-thin.md", "We want faster onboarding.\n");
    let out = delivery(ws.path(), &["intake", "--file", &file, "--id", "d-thin"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "intake still succeeds: {stdout}");
    assert!(stdout.contains("stage: Clarify"), "stdout: {stdout}");
    assert!(
        stdout.contains("blocking clarify question"),
        "stdout: {stdout}"
    );
    // The thin doc supplied no target users / acceptance -> both blocked.
    assert!(stdout.contains("target users"), "stdout: {stdout}");
    assert!(
        stdout.contains("acceptance criteria") || stdout.contains("success metric"),
        "stdout: {stdout}"
    );
}

#[test]
fn show_projection_reports_present_absent_and_corrupt() {
    let ws = TempDir::new().unwrap();
    let file = write_req(ws.path(), "req-good.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &file, "--id", "d-x"]);

    // present
    let show = delivery(ws.path(), &["show", "d-x", "--json"]);
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(show.status.success(), "show should succeed: {stdout}");
    assert!(
        stdout.contains("\"delivery_id\": \"d-x\""),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("\"stage\": \"intake\""), "stdout: {stdout}");

    // absent -> exit 0, "not found" (NOT an error, NOT a silent empty record)
    let missing = delivery(ws.path(), &["show", "d-nope"]);
    let mstdout = String::from_utf8_lossy(&missing.stdout);
    assert!(missing.status.success(), "absent is not an error");
    assert!(mstdout.contains("not found"), "stdout: {mstdout}");

    // corrupt -> explicit non-zero error, never silent empty
    let landed = ws.path().join(".maestro/deliveries/d-x/DELIVERY.json");
    fs::write(&landed, "not json {{{").unwrap();
    let corrupt = delivery(ws.path(), &["show", "d-x"]);
    let stderr = String::from_utf8_lossy(&corrupt.stderr);
    assert!(
        !corrupt.status.success(),
        "corrupt file must be an explicit error, stderr: {stderr}"
    );
    assert!(stderr.contains("corrupt DeliverySpec"), "stderr: {stderr}");
}

#[test]
fn ls_lists_intaken_deliveries() {
    let ws = TempDir::new().unwrap();
    let good = write_req(ws.path(), "a.md", GOOD);
    let thin = write_req(ws.path(), "b.md", "Just an idea.\n");
    delivery(ws.path(), &["intake", "--file", &good, "--id", "d-aaa"]);
    delivery(ws.path(), &["intake", "--file", &thin, "--id", "d-bbb"]);
    let out = delivery(ws.path(), &["ls"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("d-aaa"), "stdout: {stdout}");
    assert!(stdout.contains("d-bbb"), "stdout: {stdout}");
    assert!(stdout.contains("Intake"), "stdout: {stdout}");
    assert!(stdout.contains("Clarify"), "stdout: {stdout}");
}

// ── blocker repros (大力 F-127a review) ──────────────────────────────────────

#[test]
fn intake_twice_same_id_refuses_overwrite() {
    // Blocker 3: a PM requirement is durable — a same-id re-intake must NOT
    // silently overwrite the first record.
    let ws = TempDir::new().unwrap();
    let good = write_req(ws.path(), "a.md", GOOD);
    let thin = write_req(ws.path(), "b.md", "Some other idea.\n");
    let first = delivery(ws.path(), &["intake", "--file", &good, "--id", "d-same"]);
    assert!(first.status.success(), "first intake succeeds");
    let landed = ws.path().join(".maestro/deliveries/d-same/DELIVERY.json");
    let before = fs::read_to_string(&landed).unwrap();

    let second = delivery(ws.path(), &["intake", "--file", &thin, "--id", "d-same"]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        !second.status.success(),
        "re-intake of an existing id must fail, stderr: {stderr}"
    );
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
    // The original record is untouched (no data loss).
    assert_eq!(
        fs::read_to_string(&landed).unwrap(),
        before,
        "record unchanged"
    );
}

#[test]
fn tampered_plan_ref_is_rejected_on_read() {
    // Blocker 1: a non-DeliveryRef string ref (plan.plan_path) smuggling an
    // absolute path must be rejected on read, not projected out.
    let ws = TempDir::new().unwrap();
    let good = write_req(ws.path(), "a.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &good, "--id", "d-ref"]);
    let landed = ws.path().join(".maestro/deliveries/d-ref/DELIVERY.json");
    tamper(&landed, |o| {
        o.insert(
            "plan".into(),
            json!({"plan_path": "/abs/PLAN.yaml", "plan_hash": "x", "preview_ref": "file:///etc/passwd"}),
        );
    });
    let out = delivery(ws.path(), &["show", "d-ref", "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "tampered plan ref must be an explicit error, stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stderr.contains("run-relative") || stderr.contains("file:"),
        "stderr: {stderr}"
    );
    assert!(
        !stdout.contains("/abs/PLAN.yaml"),
        "must NOT project the smuggled path; stdout: {stdout}"
    );
}

#[test]
fn unsupported_schema_version_is_rejected() {
    // Blocker 2: a wrong/unsupported schema_version must not be projected as v1.
    let ws = TempDir::new().unwrap();
    let good = write_req(ws.path(), "a.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &good, "--id", "d-ver"]);
    let landed = ws.path().join(".maestro/deliveries/d-ver/DELIVERY.json");
    tamper(&landed, |o| {
        o.insert("schema_version".into(), json!("maestro.delivery_spec.v999"));
    });
    let out = delivery(ws.path(), &["show", "d-ver"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "unsupported schema_version must be an explicit error, stderr: {stderr}"
    );
    assert!(
        stderr.contains("unsupported schema_version"),
        "stderr: {stderr}"
    );
}

// ── F-127b: spec shaping → confirm → plan → run linkage ──────────────────────

#[test]
fn confirm_spec_refuses_incomplete_and_records_blocking_clarify() {
    let ws = ws_with_project();
    let file = write_req(ws.path(), "req.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &file, "--id", "d1"]);
    // unknown project + an acceptance with no check → both are gaps, not silent.
    delivery(
        ws.path(),
        &[
            "spec",
            "d1",
            "--prd",
            "Add healthcheck",
            "--project",
            "ghost",
            "--accept",
            "works",
        ],
    );
    let c = delivery(ws.path(), &["confirm-spec", "d1", "--by", "alice"]);
    let stderr = String::from_utf8_lossy(&c.stderr);
    assert!(!c.status.success(), "incomplete spec must refuse: {stderr}");
    assert!(stderr.contains("incomplete"), "stderr: {stderr}");
    assert!(
        stderr.contains("not registered"),
        "unknown project gap: {stderr}"
    );
    // The gaps are recorded as BLOCKING clarify questions in the record itself.
    let body = fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    let rec: serde_json::Value = serde_json::from_str(&body).unwrap();
    let qs = rec["clarify"]["questions"].as_array().unwrap();
    assert!(
        qs.iter()
            .any(|q| q["blocking"] == true
                && q["q"].as_str().unwrap_or("").contains("not registered")),
        "unknown-project gap recorded as a blocking clarify question: {body}"
    );
    // No confirmed-but-unplannable record: spec_confirm absent.
    assert!(
        rec.get("spec_confirm").is_none(),
        "must not be confirmed: {body}"
    );
}

#[test]
fn plan_refused_until_confirmed_then_reuses_synthesize() {
    let ws = ws_with_project();
    let file = write_req(ws.path(), "req.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &file, "--id", "d1"]);
    delivery(
        ws.path(),
        &[
            "spec",
            "d1",
            "--prd",
            "Add a healthcheck endpoint",
            "--project",
            "proj-a",
            "--accept",
            "returns 200 :: test 1 = 1",
        ],
    );
    // Not confirmed yet → plan refused (hard guard).
    let p = delivery(ws.path(), &["plan", "d1"]);
    assert!(!p.status.success(), "plan before confirm must refuse");
    assert!(
        String::from_utf8_lossy(&p.stderr).contains("not confirmed"),
        "{}",
        String::from_utf8_lossy(&p.stderr)
    );
    // Confirm, then plan generates a PLAN with the delivery's own goal block.
    assert!(
        delivery(ws.path(), &["confirm-spec", "d1", "--by", "alice"])
            .status
            .success()
    );
    let p = delivery(ws.path(), &["plan", "d1"]);
    assert!(p.status.success(), "{}", String::from_utf8_lossy(&p.stderr));
    let plan_yaml = fs::read_to_string(ws.path().join("plans/delivery-d1.yaml")).unwrap();
    assert!(
        plan_yaml.contains("goal:"),
        "reuses Goal block: {plan_yaml}"
    );
    assert!(
        plan_yaml.contains("returns 200"),
        "injects the delivery's acceptance"
    );
    assert!(
        plan_yaml.contains("check: test 1 = 1"),
        "with its runnable check"
    );
    // Stage advanced + PlanRef recorded.
    let show = delivery(ws.path(), &["show", "d1", "--json"]);
    let sout = String::from_utf8_lossy(&show.stdout);
    assert!(sout.contains("\"stage\": \"plan\""), "{sout}");
    assert!(sout.contains("plans/delivery-d1.yaml"), "{sout}");
}

#[test]
fn editing_confirmed_spec_resets_confirmation() {
    // B3: a human confirms a SPECIFIC spec version. Editing a confirmed spec must
    // reset spec_confirm so `delivery plan` can't run a changed spec under a stale
    // confirmation.
    let ws = ws_with_project();
    let file = write_req(ws.path(), "req.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &file, "--id", "d1"]);
    delivery(
        ws.path(),
        &[
            "spec",
            "d1",
            "--prd",
            "Original PRD",
            "--project",
            "proj-a",
            "--accept",
            "ok :: test 1 = 1",
        ],
    );
    assert!(
        delivery(ws.path(), &["confirm-spec", "d1", "--by", "alice"])
            .status
            .success()
    );

    // Edit the confirmed spec → confirmation must reset (+ audit).
    assert!(delivery(
        ws.path(),
        &[
            "spec",
            "d1",
            "--prd",
            "Changed PRD",
            "--project",
            "proj-a",
            "--accept",
            "ok :: test 2 = 2",
        ],
    )
    .status
    .success());
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    assert!(
        body.get("spec_confirm").is_none(),
        "editing a confirmed spec must reset the confirmation: {body}"
    );
    assert!(
        body["audit"].as_array().unwrap().iter().any(|a| a["reason"]
            .as_str()
            .unwrap_or("")
            .contains("confirmation reset")),
        "reset is audited: {body}"
    );

    // `plan` must refuse the reset spec and generate NO PLAN.
    let p = delivery(ws.path(), &["plan", "d1"]);
    assert!(!p.status.success(), "plan must refuse a reset spec");
    assert!(
        String::from_utf8_lossy(&p.stderr).contains("not confirmed"),
        "{}",
        String::from_utf8_lossy(&p.stderr)
    );
    assert!(
        !ws.path().join("plans/delivery-d1.yaml").exists(),
        "no PLAN generated for an unconfirmed spec"
    );

    // Re-confirm → plan succeeds with the NEW spec.
    assert!(
        delivery(ws.path(), &["confirm-spec", "d1", "--by", "alice"])
            .status
            .success()
    );
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());
    let plan_yaml = fs::read_to_string(ws.path().join("plans/delivery-d1.yaml")).unwrap();
    assert!(
        plan_yaml.contains("Changed PRD"),
        "PLAN reflects the new spec: {plan_yaml}"
    );
    assert!(!plan_yaml.contains("Original PRD"), "old spec must be gone");
}

#[test]
fn delivery_run_links_run_with_backref_and_is_idempotent() {
    let ws = ws_with_project();
    intake_spec_confirm(ws.path(), "d1");
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());

    let r = delivery(ws.path(), &["run", "d1"]);
    assert!(
        r.status.success(),
        "delivery run should link: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    // Linkage recorded: stage execute, run_id, preview_ref.
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["stage"], "execute");
    let run_id = body["execute"]["run_id"].as_str().unwrap().to_string();
    assert!(!run_id.is_empty());
    assert_eq!(body["plan"]["preview_ref"], "PLAN_PREVIEW.json");

    // Run side carries the delivery_id back-reference.
    let run_state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            ws.path()
                .join(format!(".maestro/runs/{run_id}/RUN_STATE.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(run_state["delivery_id"], "d1");
    // Honors the workspace's configured default concurrency (projects.yaml
    // max_parallel: 1), not the bare ExecConfig::default() of 4.
    assert_eq!(
        run_state["max_parallel"], 1,
        "configured max_parallel honored"
    );

    // Same-source: PlanRef.plan_hash == the F-122 pin.
    let pin: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            ws.path()
                .join(format!(".maestro/runs/{run_id}/PLAN_PREVIEW.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["plan"]["plan_hash"], pin["plan_hash"],
        "same-source hash"
    );

    // Idempotent: a second `run` refuses, does not start a second run.
    let again = delivery(ws.path(), &["run", "d1"]);
    assert!(!again.status.success(), "second run must refuse");
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already linked"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn plan_hash_drift_rejected_at_run() {
    let ws = ws_with_project();
    intake_spec_confirm(ws.path(), "d1");
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());
    // Tamper the generated PLAN after the hash was recorded.
    let plan_path = ws.path().join("plans/delivery-d1.yaml");
    let tampered = fs::read_to_string(&plan_path)
        .unwrap()
        .replace("Add a healthcheck endpoint", "Tampered objective");
    fs::write(&plan_path, tampered).unwrap();
    // PREFLIGHT: a drifted plan is refused BEFORE any run is created — no side
    // effects, no orphan run, only an explicit error.
    let r = delivery(ws.path(), &["run", "d1"]);
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(!r.status.success(), "drift must error: {stderr}");
    assert!(
        stderr.contains("drift") || stderr.contains("plan_hash"),
        "{stderr}"
    );
    // No run was started: `.maestro/runs` has zero runs (no tampered-plan exec,
    // no orphan run stamped with delivery_id).
    assert_eq!(
        run_count(ws.path()),
        0,
        "a drifted plan must not create a run"
    );
    // Stage NOT advanced to execute (no successful linkage written).
    let body = fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    assert!(
        !body.contains("\"execute\""),
        "no execute linkage on drift: {body}"
    );
}

#[test]
fn run_refuses_project_drift_without_creating_orphan_run() {
    // B2: `delivery run` must mirror `maestro run`'s pre-run project-drift guard —
    // a plan whose target project vanished from projects.yaml is refused BEFORE
    // any run is created (no orphan run, no stamped delivery_id).
    let ws = ws_with_project();
    intake_spec_confirm(ws.path(), "d1");
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());
    // Remove the target project after planning.
    fs::write(
        ws.path().join(".maestro/projects.yaml"),
        "projects: {}\ndefaults:\n  agent: mock\n  max_parallel: 1\n",
    )
    .unwrap();
    let r = delivery(ws.path(), &["run", "d1"]);
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(!r.status.success(), "project drift must refuse: {stderr}");
    assert!(stderr.contains("not in projects.yaml"), "{stderr}");
    assert_eq!(run_count(ws.path()), 0, "drift must not create a run");
}

#[test]
fn run_refuses_task_cap_without_creating_orphan_run() {
    // B2: the task-cap preflight also fires before any run is created. A two-project
    // plan (≥2 tasks) with max_total_tasks lowered to 1 after planning leaves the
    // plan (hash) intact but trips the cap → refuse, no orphan run.
    let ws = TempDir::new().unwrap();
    fs::create_dir_all(ws.path().join(".maestro")).unwrap();
    fs::create_dir_all(ws.path().join("proj-a")).unwrap();
    fs::create_dir_all(ws.path().join("proj-b")).unwrap();
    fs::write(
        ws.path().join(".maestro/projects.yaml"),
        "projects:\n  proj-a:\n    path: ./proj-a\n    agent: mock\n  proj-b:\n    path: ./proj-b\n    agent: mock\ndefaults:\n  agent: mock\n  max_parallel: 1\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(ws.path())
        .status()
        .ok();

    let file = write_req(ws.path(), "req.md", GOOD);
    delivery(ws.path(), &["intake", "--file", &file, "--id", "d1"]);
    delivery(
        ws.path(),
        &[
            "spec",
            "d1",
            "--prd",
            "Touch both",
            "--project",
            "proj-a",
            "--project",
            "proj-b",
            "--accept",
            "ok :: test 1 = 1",
        ],
    );
    assert!(
        delivery(ws.path(), &["confirm-spec", "d1", "--by", "alice"])
            .status
            .success()
    );
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());

    // Lower the cap below the (≥2) task count, plan unchanged.
    fs::write(
        ws.path().join(".maestro/projects.yaml"),
        "projects:\n  proj-a:\n    path: ./proj-a\n    agent: mock\n  proj-b:\n    path: ./proj-b\n    agent: mock\ndefaults:\n  agent: mock\n  max_parallel: 1\n  max_total_tasks: 1\n",
    )
    .unwrap();
    let r = delivery(ws.path(), &["run", "d1"]);
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(!r.status.success(), "task cap must refuse: {stderr}");
    assert!(stderr.contains("over the cap"), "{stderr}");
    assert_eq!(run_count(ws.path()), 0, "cap breach must not create a run");
}

#[test]
fn legacy_run_state_without_delivery_id_is_none() {
    // A pre-F-127b RUN_STATE.json has no delivery_id field → deserializes to None.
    let legacy = json!({
        "run_id": "r1", "spec": "demo", "started_at": "2026-06-08T00:00:00Z",
        "status": "running", "max_parallel": 1, "tasks": {},
        "approvals_pending": [], "task_order": []
    });
    let st: RunState = serde_json::from_value(legacy).unwrap();
    assert!(st.delivery_id.is_none(), "legacy state → None");

    let with = json!({
        "run_id": "r1", "spec": "demo", "started_at": "2026-06-08T00:00:00Z",
        "status": "running", "max_parallel": 1, "tasks": {},
        "approvals_pending": [], "task_order": [], "delivery_id": "d1"
    });
    let st: RunState = serde_json::from_value(with).unwrap();
    assert_eq!(st.delivery_id.as_deref(), Some("d1"));
}

// ── F-127c: accept + closeout + write-back ───────────────────────────────────

#[test]
fn accept_failed_run_only_changes_or_rejected() {
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);
    set_run_state(ws.path(), &run_id, "failed", false);
    // accepted / partial refused on a failed run (stage stays execute).
    for v in ["accepted", "partial"] {
        let r = delivery(ws.path(), &["accept", "d1", "--verdict", v, "--by", "pm"]);
        assert!(!r.status.success(), "{v} on failed run must refuse");
        assert!(
            String::from_utf8_lossy(&r.stderr).contains("failed/cancelled"),
            "{}",
            String::from_utf8_lossy(&r.stderr)
        );
    }
    // rejected is allowed → lands at Rejected.
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "rejected", "--by", "pm"],
    );
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let show = delivery(ws.path(), &["show", "d1", "--json"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("\"stage\": \"rejected\""));
}

#[test]
fn accept_unverified_run_requires_explicit_debt() {
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);
    set_run_state(ws.path(), &run_id, "done", false); // finished, acceptance failed

    // accepted with no flag → refuse.
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "accepted", "--by", "pm"],
    );
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("accept-failed-with-debt"));
    // accepted with flag but no debt → refuse.
    let r = delivery(
        ws.path(),
        &[
            "accept",
            "d1",
            "--verdict",
            "accepted",
            "--by",
            "pm",
            "--accept-failed-with-debt",
        ],
    );
    assert!(!r.status.success(), "flag without debt must refuse");
    // partial with no debt → refuse.
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "partial", "--by", "pm"],
    );
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("non-empty --debt"));
    // accepted with flag + debt → OK, debt recorded.
    let r = delivery(
        ws.path(),
        &[
            "accept",
            "d1",
            "--verdict",
            "accepted",
            "--by",
            "pm",
            "--accept-failed-with-debt",
            "--debt",
            "follow up on the flaky check",
        ],
    );
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["stage"], "accept");
    assert_eq!(body["accept"]["debt"][0], "follow up on the flaky check");
    assert_eq!(body["pm_accept"]["by"], "pm");
}

#[test]
fn no_auto_accept_then_accept_and_closeout() {
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);
    set_run_state(ws.path(), &run_id, "done", true); // verified clean pass

    // A verified run does NOT auto-advance — still at execute until `accept`.
    let show = delivery(ws.path(), &["show", "d1", "--json"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("\"stage\": \"execute\""));

    assert!(delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "accepted", "--by", "pm"]
    )
    .status
    .success());
    // empty closeout refused.
    let r = delivery(ws.path(), &["closeout", "d1"]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("at least one evidence"));
    // closeout with evidence → Closeout; absolute evidence path rejected.
    let bad = delivery(ws.path(), &["closeout", "d1", "--evidence", "/abs/log.txt"]);
    assert!(!bad.status.success(), "absolute evidence path rejected");
    let ok = delivery(
        ws.path(),
        &[
            "closeout",
            "d1",
            "--commit",
            "abc1234",
            "--ci",
            "https://ci/run/1",
        ],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["stage"], "closeout");
    assert_eq!(body["closeout"]["writeback"]["status"], "skipped");
    // re-closeout refused.
    let again = delivery(ws.path(), &["closeout", "d1", "--commit", "z"]);
    assert!(!again.status.success(), "re-closeout must refuse");
}

#[test]
fn closeout_writeback_emits_intent_and_requires_doc_uri() {
    // With a doc uri on the intake source → write-back emits a delivery.closeout
    // intent and records intent_emitted.
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", Some("https://example.feishu.cn/docx/abc"));
    set_run_state(ws.path(), &run_id, "done", true);
    assert!(delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "accepted", "--by", "pm"]
    )
    .status
    .success());
    let r = delivery(
        ws.path(),
        &["closeout", "d1", "--commit", "abc1234", "--writeback"],
    );
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let raw = fs::read_to_string(
        ws.path()
            .join(format!(".maestro/runs/{run_id}/outbound_replies.ndjson")),
    )
    .unwrap();
    assert!(
        raw.contains("delivery.closeout"),
        "intent line written: {raw}"
    );
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["closeout"]["writeback"]["status"], "intent_emitted");

    // Without a doc uri → --writeback refuses and does NOT close out.
    let ws2 = ws_with_project();
    let rid2 = drive_to_execute(ws2.path(), "d2", None);
    set_run_state(ws2.path(), &rid2, "done", true);
    assert!(delivery(
        ws2.path(),
        &["accept", "d2", "--verdict", "accepted", "--by", "pm"]
    )
    .status
    .success());
    let r = delivery(
        ws2.path(),
        &["closeout", "d2", "--commit", "abc", "--writeback"],
    );
    assert!(!r.status.success(), "no doc uri → writeback refuses");
    assert!(String::from_utf8_lossy(&r.stderr).contains("Feishu/doc uri"));
    let show = delivery(ws2.path(), &["show", "d2", "--json"]);
    assert!(
        String::from_utf8_lossy(&show.stdout).contains("\"stage\": \"accept\""),
        "must NOT be closed out when write-back fails"
    );
}

#[test]
fn accept_refuses_missing_or_corrupt_run_state() {
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);
    // corrupt RUN_STATE → explicit error.
    let rs = ws
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    fs::write(&rs, "not json {{{").unwrap();
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "rejected", "--by", "pm"],
    );
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("corrupt"));
    // missing RUN_STATE → explicit error.
    fs::remove_file(&rs).unwrap();
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "rejected", "--by", "pm"],
    );
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("missing"));
}

#[test]
fn show_projects_pm_accept_and_writeback_status() {
    // B1: the read-only projection must expose pm_accepted_by + writeback_status.
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);
    set_run_state(ws.path(), &run_id, "done", true);
    assert!(delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "accepted", "--by", "alice-pm"]
    )
    .status
    .success());
    let show = delivery(ws.path(), &["show", "d1", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(v["accept_verdict"], "accepted");
    assert_eq!(v["pm_accepted_by"], "alice-pm", "projection exposes the PM");

    assert!(
        delivery(ws.path(), &["closeout", "d1", "--commit", "abc1234"])
            .status
            .success()
    );
    let show = delivery(ws.path(), &["show", "d1", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(
        v["writeback_status"], "skipped",
        "projection exposes the write-back status"
    );
}

#[test]
fn accept_refuses_running_or_outcome_gated_run() {
    let ws = ws_with_project();
    let run_id = drive_to_execute(ws.path(), "d1", None);

    // Still running → refuse (can't accept an unfinished run).
    set_run_state(ws.path(), &run_id, "running", false);
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "rejected", "--by", "pm"],
    );
    assert!(!r.status.success(), "running run must refuse");
    assert!(
        String::from_utf8_lossy(&r.stderr).contains("still running"),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );

    // Paused at the outcome gate → refuse (resolve the gate first).
    let p = ws
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("status".into(), json!("done"));
    o.insert("pending_gate".into(), json!("outcome"));
    fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    let r = delivery(
        ws.path(),
        &["accept", "d1", "--verdict", "rejected", "--by", "pm"],
    );
    assert!(!r.status.success(), "outcome-gated run must refuse");
    assert!(
        String::from_utf8_lossy(&r.stderr).contains("outcome gate"),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
}

#[test]
fn delivery_run_records_actor_in_execute_audit() {
    // B1 (F-129): the Web POST spawns `delivery run <id> --by web-ui`; the actor must
    // land in the Execute audit row (not null), or Web-run provenance is lost.
    let ws = ws_with_project();
    intake_spec_confirm(ws.path(), "d1");
    assert!(delivery(ws.path(), &["plan", "d1"]).status.success());
    let r = delivery(ws.path(), &["run", "d1", "--by", "web-ui"]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let body: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ws.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap(),
    )
    .unwrap();
    let exec_audit = body["audit"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["stage"] == "execute")
        .expect("an execute audit row");
    assert_eq!(
        exec_audit["by"], "web-ui",
        "the run actor must be recorded in the Execute audit row"
    );
}
