//! F-117 Step 3 — `maestro resume` real-entry guard. These drive the actual
//! binary so the exit code + coded stderr/stdout of the resume guard are locked,
//! not just the `validate_resume_target` unit layer. All cases hit a guard path
//! (refuse / no-op) before any task would run, so no real run is executed.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

use maestro::scheduler::events::RunEvent;
use maestro::scheduler::resume::{build_descriptor, write_descriptor};
use maestro::scheduler::state::RunState;
use serde_json::json;

const RUN_ID: &str = "20260605-000000_cli";

fn maestro() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maestro"))
}

fn ev(seq: u64, kind: &str) -> RunEvent {
    serde_json::from_value(json!({
        "event_id": format!("e{seq}"),
        "run_id": RUN_ID,
        "seq": seq,
        "timestamp": "2026-06-05T00:00:00Z",
        "kind": kind,
    }))
    .unwrap()
}

fn task_json(id: &str, status: &str, outputs: &[(&str, &str)]) -> serde_json::Value {
    let mut wo = serde_json::Map::new();
    for (name, path) in outputs {
        wo.insert(
            (*name).to_string(),
            json!({"name": name, "snapshot_path": path, "bytes": 2, "truncated": false}),
        );
    }
    json!({
        "id": id,
        "project": "billing-service",
        "agent": "mock",
        "status": status,
        "log_path": format!("{id}.log"),
        "ended_at": "2026-06-05T00:00:30Z",
        "workflow_outputs": wo,
    })
}

fn run_state(status: &str, pid: u32, tasks: Vec<serde_json::Value>) -> RunState {
    let order: Vec<String> = tasks
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    let mut map = serde_json::Map::new();
    for t in &tasks {
        map.insert(t["id"].as_str().unwrap().to_string(), t.clone());
    }
    serde_json::from_value(json!({
        "run_id": RUN_ID,
        "spec": "demo",
        "started_at": "2026-06-05T00:00:00Z",
        "ended_at": null,
        "status": status,
        "max_parallel": 1,
        "pid": pid,
        "tasks": map,
        "approvals_pending": [],
        "task_order": order,
    }))
    .unwrap()
}

/// Stage `ws/.maestro/runs/RUN_ID` with a consistent descriptor. `pid` 999999 is
/// a dead pid (abandoned).
fn build_workspace(status: &str, pid: u32) -> TempDir {
    let ws = TempDir::new().unwrap();
    let run_dir = ws.path().join(".maestro/runs").join(RUN_ID);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("PLAN.yaml"),
        "spec: demo\ntasks:\n  - id: T_done\n  - id: T_next\n",
    )
    .unwrap();
    fs::create_dir_all(run_dir.join("outputs/T_done")).unwrap();
    fs::write(run_dir.join("outputs/T_done/api.json"), "{}").unwrap();
    let abs = run_dir
        .join("outputs/T_done/api.json")
        .to_string_lossy()
        .into_owned();
    let tasks = if status == "done" {
        vec![
            task_json("T_done", "done", &[("api", &abs)]),
            task_json("T_next", "done", &[]),
        ]
    } else {
        vec![
            task_json("T_done", "done", &[("api", &abs)]),
            task_json("T_next", "pending", &[]),
        ]
    };
    let state = run_state(status, pid, tasks);
    fs::write(
        run_dir.join("RUN_STATE.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();
    let events = [ev(1, "run.started"), ev(2, "task.completed")];
    let mut ndjson = String::new();
    for e in &events {
        ndjson.push_str(&serde_json::to_string(e).unwrap());
        ndjson.push('\n');
    }
    fs::write(run_dir.join("events.ndjson"), ndjson).unwrap();
    let d = build_descriptor(
        &run_dir,
        &state,
        &events,
        state.started_at,
        state.started_at,
        pid,
    )
    .unwrap();
    write_descriptor(&run_dir, &d).unwrap();
    ws
}

fn resume(ws: &Path) -> Output {
    maestro()
        .env("MAESTRO_WORKSPACE_ROOT", ws)
        .args(["resume", "--run", RUN_ID])
        .output()
        .expect("run maestro resume")
}

#[test]
fn resume_missing_descriptor_exits_nonzero_with_code() {
    // An empty run dir (no RESUME.json) is a pre-F-117 run -> refuse.
    let ws = TempDir::new().unwrap();
    fs::create_dir_all(ws.path().join(".maestro/runs").join(RUN_ID)).unwrap();
    let out = resume(ws.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "should refuse; stderr: {stderr}");
    assert!(
        stderr.contains("resume.missing_descriptor"),
        "stderr: {stderr}"
    );
}

#[test]
fn resume_terminal_failed_points_to_rerun() {
    let ws = build_workspace("failed", 999_999);
    let out = resume(ws.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "should refuse; stderr: {stderr}");
    assert!(stderr.contains("resume.terminal_run"), "stderr: {stderr}");
    assert!(stderr.contains("maestro rerun"), "stderr: {stderr}");
}

#[test]
fn resume_terminal_cancelled_points_to_rerun() {
    let ws = build_workspace("cancelled", 999_999);
    let out = resume(ws.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "should refuse; stderr: {stderr}");
    assert!(stderr.contains("resume.terminal_run"), "stderr: {stderr}");
}

#[test]
fn resume_complete_clean_run_is_noop_exit0() {
    // Locks N1's normal no-op: a cleanly-complete run exits 0 and says so.
    let ws = build_workspace("done", 999_999);
    let out = resume(ws.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean complete run is a no-op (exit 0); stderr: {stderr}"
    );
    assert!(stdout.contains("already complete"), "stdout: {stdout}");
}
