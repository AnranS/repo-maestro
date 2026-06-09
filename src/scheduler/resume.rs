//! F-117 filesystem + builder helpers for the resume guard descriptor. The
//! types + descriptor-self-consistency validation live in
//! [`crate::schema::resume`]; this module turns a live `RunState` + F-115 event
//! ledger + `PLAN.yaml` snapshot into a descriptor, writes it atomically, and
//! reads it back (missing = `None`, corrupt/invalid = error, never silently
//! trusted). Step 1 is build/write/read only — the `resume` CLI guard is wired
//! in a later slice.

use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::file_guard;
use crate::paths;
use crate::scheduler::events::{RunEvent, RunEventKind};
use crate::scheduler::liveness::{classify_run, RunLiveness};
use crate::scheduler::state::{RunState, RunStatus, TaskStatus};
use crate::schema::resume::{
    validate_descriptor, ResumeDescriptor, ResumeEventCursor, ResumeIssue, ResumeIssueCode,
    ResumePlanRef, ResumeStateSummary, ResumeTaskSeed, ResumeValidationReport,
};

/// `.maestro/runs/<run-id>/RESUME.json`.
const RESUME_FILE: &str = "RESUME.json";

/// Path to a run's resume descriptor.
pub fn descriptor_path(run_dir: &Path) -> PathBuf {
    run_dir.join(RESUME_FILE)
}

/// Per-status task counts + run status string. The seven buckets cover every
/// `TaskStatus` variant, so they always sum to the task total.
pub fn summarize_state(state: &RunState) -> ResumeStateSummary {
    let mut s = ResumeStateSummary {
        run_status: run_status_str(state.status).to_string(),
        task_total: state.tasks.len() as u64,
        done: 0,
        failed: 0,
        cancelled: 0,
        skipped: 0,
        awaiting_approval: 0,
        running: 0,
        pending: 0,
    };
    for task in state.tasks.values() {
        match task.status {
            TaskStatus::Done => s.done += 1,
            TaskStatus::Failed => s.failed += 1,
            TaskStatus::Cancelled => s.cancelled += 1,
            TaskStatus::Skipped => s.skipped += 1,
            TaskStatus::AwaitingApproval => s.awaiting_approval += 1,
            TaskStatus::Running => s.running += 1,
            TaskStatus::Pending => s.pending += 1,
        }
    }
    s
}

/// The completed (`Done`) tasks eligible for seeding on resume, in deterministic
/// id order (state.tasks is a `BTreeMap`). Failed/cancelled/skipped tasks are
/// never seeded. Stores only the output-snapshot count — never the paths.
pub fn seed_tasks(state: &RunState) -> Vec<ResumeTaskSeed> {
    state
        .tasks
        .values()
        .filter(|t| t.status == TaskStatus::Done)
        .map(|t| ResumeTaskSeed {
            task_id: t.id.clone(),
            project: t.project.clone(),
            ended_at: t.ended_at.map(|e| e.to_rfc3339()),
            workflow_outputs: t.workflow_outputs.len() as u64,
        })
        .collect()
}

/// Compute the F-115 event-ledger cursor. `last_seq` is the max event sequence;
/// `last_settled_seq` is the max sequence among events whose **typed** kind
/// settled a task or a terminal run. Deliberately matches on `RunEventKind`
/// variants, NOT subscription-alias strings: `VerifyCompleted` aliases to
/// `task.completed` for channel routing but is a verify sub-lifecycle (F-115
/// N5) and must NOT advance the settled cursor.
pub fn compute_event_cursor(events: &[RunEvent]) -> ResumeEventCursor {
    let last_seq = events.iter().map(|e| e.seq).max().unwrap_or(0);
    let last_settled_seq = events
        .iter()
        .filter(|e| is_settling_kind(&e.kind))
        .map(|e| e.seq)
        .max()
        .unwrap_or(0);
    ResumeEventCursor {
        schema_version: crate::schema::RUN_EVENT_V2.to_string(),
        last_seq,
        last_settled_seq,
    }
}

/// Like [`compute_event_cursor`] but first enforces the F-117 ledger invariant:
/// `events.ndjson` is an append-only monotonic ledger, so its sequences must be
/// strictly contiguous `1..=N` **in file-read order** (no gap, no duplicate, no
/// reorder, starting at 1). We assert `seq == position+1` WITHOUT sorting — a
/// reordered ledger like `[2,1]` means it was rewritten/spliced/drifted, which a
/// resume guard must refuse rather than compute a plausible-looking cursor over.
/// This is the cursor path `build_descriptor` uses, so the guard never depends
/// on a later caller remembering to check.
pub fn compute_event_cursor_checked(events: &[RunEvent]) -> Result<ResumeEventCursor> {
    for (idx, e) in events.iter().enumerate() {
        let expected = idx as u64 + 1;
        ensure!(
            e.seq == expected,
            "event ledger sequence is not contiguous from 1 in read order (expected \
             {expected} at position {idx}, found {}); ledger has a gap, duplicate, \
             reorder, or wrong start",
            e.seq
        );
    }
    Ok(compute_event_cursor(events))
}

/// True only for kinds that settle a task or terminate a run. `VerifyStarted` /
/// `VerifyCompleted` / approvals / queued / started are explicitly NOT settling.
fn is_settling_kind(kind: &RunEventKind) -> bool {
    matches!(
        kind,
        RunEventKind::TaskSucceeded
            | RunEventKind::TaskFailed
            | RunEventKind::TaskCancelled
            | RunEventKind::TaskSkipped
            | RunEventKind::RunCompleted
            | RunEventKind::RunFailed
            | RunEventKind::RunCancelled
    )
}

/// Build a descriptor from a run's current state + event ledger. Reads the
/// `PLAN.yaml` snapshot for size + stable hash, and verifies every seeded
/// (`Done`) task's workflow-output snapshots are safe + present under the run
/// directory — a missing or unsafe snapshot path makes the build fail (the
/// producer treats that as warn-only; a later resume then refuses on the missing
/// descriptor rather than silently skipping a task whose output is gone).
pub fn build_descriptor(
    run_dir: &Path,
    state: &RunState,
    events: &[RunEvent],
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    writer_pid: u32,
) -> Result<ResumeDescriptor> {
    let plan_path = run_dir.join(paths::PLAN_SNAPSHOT);
    let bytes = std::fs::metadata(&plan_path)
        .with_context(|| format!("stat plan snapshot {}", plan_path.display()))?
        .len();
    let hash = file_guard::file_hash(&plan_path)?;

    // Guard seeded-task output snapshots: safe, run-relative, and a real file.
    for task in state.tasks.values() {
        if task.status != TaskStatus::Done {
            continue;
        }
        for (name, out) in &task.workflow_outputs {
            check_seed_output(run_dir, &out.snapshot_path).map_err(|problem| {
                anyhow::anyhow!(
                    "task {:?} output {:?} snapshot {:?} {}",
                    task.id,
                    name,
                    out.snapshot_path,
                    problem.detail()
                )
            })?;
        }
    }

    // A gap/duplicate/non-1-start ledger is corrupt — refuse here, in the
    // builder, not at some later caller's discretion.
    let event_ledger = compute_event_cursor_checked(events)?;

    let descriptor = ResumeDescriptor {
        schema_version: crate::schema::resume_descriptor_version(),
        run_id: state.run_id.clone(),
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
        writer_pid,
        plan: ResumePlanRef {
            path: paths::PLAN_SNAPSHOT.to_string(),
            bytes,
            hash,
        },
        event_ledger,
        state: summarize_state(state),
        seeded_tasks: seed_tasks(state),
    };
    // The helper's output must itself be self-valid, never relying on
    // write_descriptor as the only gate.
    validate_descriptor(&descriptor)?;
    Ok(descriptor)
}

/// Best-effort refresh of a run's resume descriptor at a resume-relevant seam
/// (initial setup, task settle + output capture, run terminal). Reads the
/// current `events.ndjson` (the checked cursor inside [`build_descriptor`]
/// rejects a gapped/reordered ledger) and rebuilds + atomically rewrites the
/// descriptor. It is a NO-OP (returns `Ok`) before the initial event exists, so
/// it never writes a half-baked descriptor ahead of `PLAN.yaml` + state +
/// initial event. Returns `Err` on a corrupt/gapped ledger, a missing/unsafe
/// output snapshot, or a write failure — the executor logs that as a warning
/// and leaves any prior descriptor in place, so a later `resume` fails closed on
/// the stale/missing guard rather than the active run failing.
pub fn refresh_descriptor(run_dir: &Path, state: &RunState) -> Result<()> {
    let events = crate::scheduler::events::read_events(run_dir)?;
    if events.is_empty() {
        // Boundary: no initial event on disk yet — don't write a half-baked
        // descriptor. The next refresh (after the initial event) writes it.
        return Ok(());
    }
    let now = Utc::now();
    let descriptor = build_descriptor(
        run_dir,
        state,
        &events,
        state.started_at,
        now,
        std::process::id(),
    )?;
    write_descriptor(run_dir, &descriptor)
}

/// Validate then atomically write the descriptor (tmp + fsync + rename, mirroring
/// `RunState::write_atomic`). Callers in the live run treat failure as best-effort
/// (warn + continue); resume-time validation is strict.
pub fn write_descriptor(run_dir: &Path, descriptor: &ResumeDescriptor) -> Result<()> {
    use std::io::Write;
    validate_descriptor(descriptor)?;
    let target = descriptor_path(run_dir);
    let tmp = target.with_extension(format!("json.tmp.{}", std::process::id()));
    let text = serde_json::to_string_pretty(descriptor).context("serialize resume descriptor")?;
    {
        let mut f = std::fs::File::create(&tmp).context("create tmp resume descriptor")?;
        f.write_all(text.as_bytes())
            .context("write tmp resume descriptor")?;
        f.sync_all().context("fsync tmp resume descriptor")?;
    }
    std::fs::rename(&tmp, &target).context("rename tmp resume descriptor")?;
    Ok(())
}

/// Read a run's descriptor: `Ok(None)` when missing, `Err` when the file is
/// corrupt or fails validation (a drifted/hand-edited descriptor is never
/// trusted to gate a resume).
pub fn read_descriptor(run_dir: &Path) -> Result<Option<ResumeDescriptor>> {
    let path = descriptor_path(run_dir);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let descriptor: ResumeDescriptor = serde_json::from_str(&text)
        .with_context(|| format!("parse resume descriptor {}", path.display()))?;
    validate_descriptor(&descriptor)
        .with_context(|| format!("invalid resume descriptor {}", path.display()))?;
    Ok(Some(descriptor))
}

fn run_status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

/// A workflow-output snapshot path must be run-relative: reject `file:` URIs,
/// absolutes, a leading slash/backslash, `..` traversal, and ANY Windows
/// drive-letter prefix `X:` — both drive-absolute (`C:\x`, `C:/x`) and
/// drive-relative (`C:`, `C:foo`), since the latter still resolves against the
/// drive's current directory, not the run dir.
fn snapshot_path_is_unsafe(value: &str) -> bool {
    if value.trim_start().to_ascii_lowercase().starts_with("file:") {
        return true;
    }
    if Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Why a workflow-output snapshot is not seedable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutputProblem {
    /// Escapes the run dir, contains `..`/a drive letter, or resolves to a
    /// directory or symlink (not a plain run-relative file).
    UnsafePath,
    /// No file exists at the (safe) snapshot path.
    Missing,
}

impl SeedOutputProblem {
    fn detail(self) -> &'static str {
        match self {
            Self::UnsafePath => {
                "is not a safe regular file under the run dir (escape / dir / symlink)"
            }
            Self::Missing => "is missing on disk",
        }
    }
}

/// Validate one workflow-output snapshot for seeding: normalize an
/// absolute-under-`run_dir` path to run-relative, reject escapes / `..` / drive
/// letters, and require an existing **regular file** (no symlink, no directory).
/// Shared by [`build_descriptor`] (write side) and [`validate_resume_target`]
/// (resume side) so both apply the identical rule.
fn check_seed_output(run_dir: &Path, snapshot_path: &str) -> Result<(), SeedOutputProblem> {
    if snapshot_path.trim().is_empty() {
        return Err(SeedOutputProblem::UnsafePath);
    }
    let rel = Path::new(snapshot_path)
        .strip_prefix(run_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| snapshot_path.to_string());
    if snapshot_path_is_unsafe(&rel) {
        return Err(SeedOutputProblem::UnsafePath);
    }
    match std::fs::symlink_metadata(run_dir.join(&rel)) {
        Ok(m) if m.file_type().is_symlink() => Err(SeedOutputProblem::UnsafePath),
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => Err(SeedOutputProblem::UnsafePath),
        Err(_) => Err(SeedOutputProblem::Missing),
    }
}

/// Recompute the resume verdict for a run directory. The descriptor's own
/// `validate_descriptor` only proves self-consistency; this RE-derives the
/// truth from disk and cross-checks it: liveness, terminal status, plan
/// hash/bytes, the F-115 event cursor (via the checked cursor — a gapped/
/// reordered/corrupt ledger is a hard refusal), the PLAN↔state↔seeded task sets,
/// and every Done task's output snapshot. `--force` overrides ONLY liveness
/// uncertainty (`Live` / `pid == 0`), never any data-integrity issue.
pub fn validate_resume_target(run_dir: &Path, force: bool) -> Result<ResumeValidationReport> {
    let dir_id = run_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    // Descriptor present + structurally valid. Missing or corrupt is a hard
    // refusal that `--force` cannot bypass.
    let descriptor = match read_descriptor(run_dir) {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Ok(refusal(
                dir_id,
                ResumeIssueCode::MissingDescriptor,
                "no RESUME.json — this run predates the resume guard; use `maestro rerun`",
            ))
        }
        Err(e) => {
            return Ok(refusal(
                dir_id,
                ResumeIssueCode::SchemaMismatch,
                format!("RESUME.json is corrupt or invalid: {e:#}"),
            ))
        }
    };

    // Current state must parse — a precondition for any recompute.
    let state = RunState::load(run_dir).context("load RUN_STATE.json for resume validation")?;

    let mut issues: Vec<ResumeIssue> = Vec::new();

    // Identity: state ↔ descriptor ↔ directory.
    if state.run_id != descriptor.run_id || state.run_id != dir_id {
        issues.push(ResumeIssue::new(
            ResumeIssueCode::RunIdMismatch,
            format!(
                "run id disagreement (state={}, descriptor={}, dir={})",
                state.run_id, descriptor.run_id, dir_id
            ),
        ));
    }

    // Liveness (the only `--force`-overridable issues).
    match classify_run(&state) {
        RunLiveness::Live => issues.push(ResumeIssue::new(
            ResumeIssueCode::LiveRun,
            format!("run still appears alive (pid {})", state.pid),
        )),
        RunLiveness::UnknownLegacy => issues.push(ResumeIssue::new(
            ResumeIssueCode::LegacyLivenessUnknown,
            "run predates pid tracking (pid 0); liveness is unknown",
        )),
        RunLiveness::Abandoned => {}
    }

    // Completeness / terminal.
    let done_ids: Vec<String> = state
        .tasks
        .iter()
        .filter(|(_, t)| t.status == TaskStatus::Done)
        .map(|(id, _)| id.clone())
        .collect();
    let all_done = !state.tasks.is_empty() && done_ids.len() == state.tasks.len();
    let mut already_complete = false;
    match state.status {
        RunStatus::Done if all_done => {
            already_complete = true;
            issues.push(ResumeIssue::new(
                ResumeIssueCode::AlreadyComplete,
                "run already complete; nothing to resume",
            ));
        }
        RunStatus::Failed | RunStatus::Cancelled => issues.push(ResumeIssue::new(
            ResumeIssueCode::TerminalRun,
            "run is terminal (failed/cancelled); use `maestro rerun` instead of resume",
        )),
        _ => {}
    }

    // Plan integrity — recompute hash + bytes against the descriptor.
    let plan_path = run_dir.join(paths::PLAN_SNAPSHOT);
    match (
        file_guard::file_hash(&plan_path),
        std::fs::metadata(&plan_path),
    ) {
        (Ok(hash), Ok(meta)) => {
            if hash != descriptor.plan.hash || meta.len() != descriptor.plan.bytes {
                issues.push(ResumeIssue::new(
                    ResumeIssueCode::PlanDrift,
                    "PLAN.yaml hash/size differs from the descriptor (plan drifted since the run)",
                ));
            }
        }
        _ => issues.push(ResumeIssue::new(
            ResumeIssueCode::PlanDrift,
            "PLAN.yaml snapshot is missing or unreadable",
        )),
    }

    // Event ledger — recompute via the checked cursor. A corrupt/gapped/reordered
    // ledger is a hard refusal; never silently default it.
    match crate::scheduler::events::read_events(run_dir) {
        Err(e) => issues.push(ResumeIssue::new(
            ResumeIssueCode::EventLedgerCorrupt,
            format!("events.ndjson is unreadable or corrupt: {e:#}"),
        )),
        Ok(events) => match compute_event_cursor_checked(&events) {
            Err(e) => issues.push(ResumeIssue::new(
                ResumeIssueCode::EventLedgerCorrupt,
                format!("event ledger is not contiguous: {e:#}"),
            )),
            Ok(cursor) => {
                // Append-only compatibility (not exact-match): the descriptor is
                // refreshed at task settle, so a run killed mid-task legitimately
                // has a non-settling tail (`task.started` / approval / verify.*)
                // beyond it — that is normal in-flight progress, still resumable.
                // Refuse only when (a) the ledger SHRANK (truncated/rewritten) or
                // (b) a task/run SETTLED since the descriptor (the seedable
                // done-set changed). Contiguity is already enforced above.
                if cursor.last_seq < descriptor.event_ledger.last_seq {
                    issues.push(ResumeIssue::new(
                        ResumeIssueCode::EventCursorStale,
                        format!(
                            "event ledger was truncated or rewritten (recomputed last_seq {} < descriptor last_seq {})",
                            cursor.last_seq, descriptor.event_ledger.last_seq
                        ),
                    ));
                } else if cursor.last_settled_seq != descriptor.event_ledger.last_settled_seq {
                    issues.push(ResumeIssue::new(
                        ResumeIssueCode::EventCursorStale,
                        format!(
                            "a task or run settled after the descriptor was written (recomputed last_settled_seq {} != descriptor {})",
                            cursor.last_settled_seq, descriptor.event_ledger.last_settled_seq
                        ),
                    ));
                }
            }
        },
    }

    // Task sets: PLAN ids == state ids, and descriptor.seeded == state Done set.
    if let Ok(plan) = crate::config::Plan::load(&plan_path) {
        let plan_ids: BTreeSet<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
        let state_ids: BTreeSet<&str> = state.tasks.keys().map(|s| s.as_str()).collect();
        if plan_ids != state_ids {
            issues.push(ResumeIssue::new(
                ResumeIssueCode::TaskSetMismatch,
                "PLAN.yaml task ids differ from RUN_STATE.json",
            ));
        }
    }
    let seeded_ids: BTreeSet<&str> = descriptor
        .seeded_tasks
        .iter()
        .map(|s| s.task_id.as_str())
        .collect();
    let done_set: BTreeSet<&str> = done_ids.iter().map(|s| s.as_str()).collect();
    if seeded_ids != done_set {
        issues.push(ResumeIssue::new(
            ResumeIssueCode::TaskSetMismatch,
            "descriptor seeded_tasks differ from the Done tasks in RUN_STATE.json",
        ));
    }

    // Every Done task's output snapshots must still be safe + present.
    for (tid, t) in state
        .tasks
        .iter()
        .filter(|(_, t)| t.status == TaskStatus::Done)
    {
        for (name, out) in &t.workflow_outputs {
            match check_seed_output(run_dir, &out.snapshot_path) {
                Ok(()) => {}
                Err(SeedOutputProblem::Missing) => issues.push(ResumeIssue::new(
                    ResumeIssueCode::SeedOutputMissing,
                    format!("done task {tid} output {name} snapshot is missing on disk"),
                )),
                Err(SeedOutputProblem::UnsafePath) => issues.push(ResumeIssue::new(
                    ResumeIssueCode::SeedOutputUnsafePath,
                    format!(
                        "done task {tid} output {name} snapshot is unsafe (escape / dir / symlink)"
                    ),
                )),
            }
        }
    }

    let blocking = issues.iter().any(|i| i.code.blocks(force));
    let can_resume = !already_complete && !blocking;
    Ok(ResumeValidationReport {
        run_id: state.run_id.clone(),
        can_resume,
        already_complete,
        reusable_done_tasks: done_ids,
        issues,
    })
}

fn refusal(
    run_id: String,
    code: ResumeIssueCode,
    detail: impl Into<String>,
) -> ResumeValidationReport {
    ResumeValidationReport {
        run_id,
        can_resume: false,
        already_complete: false,
        reusable_done_tasks: Vec::new(),
        issues: vec![ResumeIssue::new(code, detail)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::state::TaskState;
    use crate::schema::resume::ResumeIssueCode;
    use serde_json::json;

    /// Minimal `TaskState` via JSON (most fields are `#[serde(default)]`).
    fn task(id: &str, status: &str, outputs: &[(&str, &str)]) -> TaskState {
        let mut wo = serde_json::Map::new();
        for (name, path) in outputs {
            wo.insert(
                name.to_string(),
                json!({"name": name, "snapshot_path": path, "bytes": 4, "truncated": false}),
            );
        }
        serde_json::from_value(json!({
            "id": id,
            "project": "billing-service",
            "agent": "mock",
            "status": status,
            "log_path": format!("{id}.log"),
            "ended_at": "2026-06-05T00:00:30Z",
            "workflow_outputs": wo,
        }))
        .unwrap()
    }

    fn run(status: &str, tasks: Vec<TaskState>) -> RunState {
        let order: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let mut map = serde_json::Map::new();
        for t in &tasks {
            map.insert(t.id.clone(), serde_json::to_value(t).unwrap());
        }
        serde_json::from_value(json!({
            "run_id": "r-1",
            "spec": "demo",
            "started_at": "2026-06-05T00:00:00Z",
            "ended_at": null,
            "status": status,
            "max_parallel": 1,
            "tasks": map,
            "approvals_pending": [],
            "task_order": order,
        }))
        .unwrap()
    }

    fn ev(seq: u64, kind: &str) -> RunEvent {
        serde_json::from_value(json!({
            "event_id": format!("e{seq}"),
            "run_id": "r-1",
            "seq": seq,
            "timestamp": "2026-06-05T00:00:00Z",
            "kind": kind,
        }))
        .unwrap()
    }

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// A temp run dir with a PLAN.yaml + the given run-relative snapshot files.
    fn run_dir_with(snapshots: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(paths::PLAN_SNAPSHOT), "spec: demo\n").unwrap();
        for rel in snapshots {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{}").unwrap();
        }
        dir
    }

    #[test]
    fn summarize_buckets_sum_to_total() {
        let state = run(
            "running",
            vec![
                task("T_a", "done", &[]),
                task("T_b", "running", &[]),
                task("T_c", "pending", &[]),
                task("T_d", "failed", &[]),
            ],
        );
        let s = summarize_state(&state);
        assert_eq!(s.task_total, 4);
        assert_eq!(s.done, 1);
        assert_eq!(s.running, 1);
        assert_eq!(s.pending, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.bucket_sum(), s.task_total);
        assert_eq!(s.run_status, "running");
    }

    #[test]
    fn seed_tasks_lists_only_done_with_output_counts() {
        let state = run(
            "running",
            vec![
                task("T_done", "done", &[("api", "outputs/T_done/api.json")]),
                task("T_fail", "failed", &[]),
                task("T_run", "running", &[]),
            ],
        );
        let seeds = seed_tasks(&state);
        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].task_id, "T_done");
        assert_eq!(seeds[0].workflow_outputs, 1);
        assert_eq!(seeds[0].project, "billing-service");
    }

    #[test]
    fn event_cursor_is_deterministic() {
        let events = [
            ev(1, "run.started"),
            ev(2, "task.started"),
            ev(3, "task.completed"),
            ev(4, "task.started"),
        ];
        let cursor = compute_event_cursor(&events);
        assert_eq!(cursor.last_seq, 4);
        assert_eq!(cursor.last_settled_seq, 3);
        assert_eq!(cursor.schema_version, crate::schema::RUN_EVENT_V2);

        // empty ledger -> zeros
        let empty = compute_event_cursor(&[]);
        assert_eq!(empty.last_seq, 0);
        assert_eq!(empty.last_settled_seq, 0);
    }

    #[test]
    fn verify_completed_does_not_advance_settled_seq_but_task_succeeded_does() {
        // dali's required regression: VerifyCompleted aliases to task.completed
        // for channel routing but must NOT settle a task. Only a real
        // TaskSucceeded advances last_settled_seq.
        let only_verify = [
            ev(1, "task.started"),
            ev(2, "verify.started"),
            ev(3, "verify.completed"),
        ];
        assert_eq!(compute_event_cursor(&only_verify).last_settled_seq, 0);

        let with_success = [
            ev(1, "task.started"),
            ev(2, "task.completed"),
            ev(3, "verify.started"),
            ev(4, "verify.completed"),
        ];
        // settles at the real task success (2), not the later verify.completed (4)
        assert_eq!(compute_event_cursor(&with_success).last_settled_seq, 2);
        assert_eq!(compute_event_cursor(&with_success).last_seq, 4);
    }

    #[test]
    fn build_write_read_round_trips() {
        let dir = run_dir_with(&["outputs/T_done/api.json"]);
        let state = run(
            "running",
            vec![
                task("T_done", "done", &[("api", "outputs/T_done/api.json")]),
                task("T_next", "pending", &[]),
            ],
        );
        let events = [ev(1, "run.started"), ev(2, "task.completed")];
        let d = build_descriptor(
            dir.path(),
            &state,
            &events,
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:01:00Z"),
            4242,
        )
        .unwrap();
        assert_eq!(d.seeded_tasks.len(), 1);
        assert_eq!(d.event_ledger.last_settled_seq, 2);
        assert!(d.plan.hash.starts_with("fnv1a64:"));

        write_descriptor(dir.path(), &d).unwrap();
        let back = read_descriptor(dir.path()).unwrap();
        assert_eq!(back, Some(d));
    }

    #[test]
    fn missing_descriptor_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_descriptor(dir.path()).unwrap(), None);
    }

    #[test]
    fn corrupt_descriptor_read_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(descriptor_path(dir.path()), "{not json").unwrap();
        assert!(read_descriptor(dir.path()).is_err());
    }

    #[test]
    fn read_rejects_inconsistent_descriptor_as_corrupt() {
        // structurally-valid JSON whose buckets don't sum -> validation -> Err
        let dir = run_dir_with(&[]);
        let state = run("done", vec![task("T_a", "done", &[])]);
        let mut d = build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .unwrap();
        d.state.pending = 9; // break the bucket sum
                             // write raw (bypass write_descriptor's validation) then read
        std::fs::write(
            descriptor_path(dir.path()),
            serde_json::to_string(&d).unwrap(),
        )
        .unwrap();
        assert!(read_descriptor(dir.path()).is_err());
    }

    #[test]
    fn build_rejects_unsafe_snapshot_path() {
        let dir = run_dir_with(&[]);
        let state = run(
            "running",
            vec![task("T_done", "done", &[("api", "../escape/api.json")])],
        );
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[test]
    fn build_rejects_missing_snapshot() {
        // safe path, but the file does not exist under the run dir
        let dir = run_dir_with(&[]);
        let state = run(
            "running",
            vec![task(
                "T_done",
                "done",
                &[("api", "outputs/T_done/api.json")],
            )],
        );
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[test]
    fn build_accepts_absolute_snapshot_under_run_dir() {
        // Reality: capture_task_outputs stores snapshots as ABSOLUTE paths under
        // run_dir/outputs/. The builder normalizes to run-relative and accepts them.
        let dir = run_dir_with(&["outputs/T_done/api.json"]);
        let abs = dir
            .path()
            .join("outputs/T_done/api.json")
            .to_string_lossy()
            .into_owned();
        let state = run("running", vec![task("T_done", "done", &[("api", &abs)])]);
        let d = build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .unwrap();
        assert_eq!(d.seeded_tasks.len(), 1);
        assert_eq!(d.seeded_tasks[0].workflow_outputs, 1);
    }

    #[test]
    fn build_rejects_absolute_snapshot_outside_run_dir() {
        // An absolute path that is NOT under run_dir is still an escape -> refuse.
        let dir = run_dir_with(&[]);
        let state = run(
            "running",
            vec![task("T_done", "done", &[("api", "/etc/passwd")])],
        );
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[test]
    fn build_rejects_directory_snapshot() {
        // A directory at the snapshot path is not an output snapshot.
        let dir = run_dir_with(&[]);
        std::fs::create_dir_all(dir.path().join("outputs/T_done/api")).unwrap();
        let state = run(
            "running",
            vec![task("T_done", "done", &[("api", "outputs/T_done/api")])],
        );
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_symlink_snapshot() {
        // A symlink under run_dir could point at an external file -> refuse.
        let dir = run_dir_with(&["outputs/T_done/real.json"]);
        std::os::unix::fs::symlink(
            dir.path().join("outputs/T_done/real.json"),
            dir.path().join("outputs/T_done/link.json"),
        )
        .unwrap();
        let state = run(
            "running",
            vec![task(
                "T_done",
                "done",
                &[("api", "outputs/T_done/link.json")],
            )],
        );
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[test]
    fn build_requires_plan_snapshot() {
        // no PLAN.yaml in the run dir -> build fails
        let dir = tempfile::tempdir().unwrap();
        let state = run("running", vec![task("T_a", "pending", &[])]);
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    #[test]
    fn snapshot_path_guard_rejects_windows_drive_relative() {
        // N1: any `X:` prefix is unsafe — drive-absolute AND drive-relative.
        for bad in ["C:foo", "C:", "z:outputs/x", r"C:\x", "C:/x", "D:bar/baz"] {
            assert!(
                snapshot_path_is_unsafe(bad),
                "drive path should be unsafe: {bad}"
            );
        }
        for ok in ["outputs/T/x.json", "a/b/c", "x"] {
            assert!(
                !snapshot_path_is_unsafe(ok),
                "run-relative should be safe: {ok}"
            );
        }
    }

    #[test]
    fn build_rejects_drive_relative_snapshot_path() {
        // N1 regression at the builder boundary.
        for bad in ["C:foo", "C:", "z:outputs/x"] {
            let dir = run_dir_with(&[]);
            let state = run("running", vec![task("T_done", "done", &[("api", bad)])]);
            assert!(
                build_descriptor(
                    dir.path(),
                    &state,
                    &[],
                    ts("2026-06-05T00:00:00Z"),
                    ts("2026-06-05T00:00:00Z"),
                    1,
                )
                .is_err(),
                "drive-relative snapshot should fail build: {bad}"
            );
        }
    }

    #[test]
    fn event_cursor_checked_rejects_gap_duplicate_and_nonstart() {
        // N2: gap / duplicate / wrong-start ledgers are refused; empty + contiguous are ok.
        assert!(
            compute_event_cursor_checked(&[ev(1, "run.started"), ev(3, "task.completed")]).is_err(),
            "gap"
        );
        assert!(
            compute_event_cursor_checked(&[ev(1, "run.started"), ev(1, "task.started")]).is_err(),
            "duplicate"
        );
        assert!(
            compute_event_cursor_checked(&[ev(2, "task.started"), ev(3, "task.completed")])
                .is_err(),
            "wrong start"
        );
        // N3: a reordered ledger (right multiset, wrong read order) is refused —
        // append-only seqs must be contiguous in file order, not after sorting.
        assert!(
            compute_event_cursor_checked(&[ev(2, "task.completed"), ev(1, "run.started")]).is_err(),
            "reorder [2,1]"
        );
        assert_eq!(
            compute_event_cursor_checked(&[]).unwrap().last_seq,
            0,
            "empty ok"
        );
        let ok =
            compute_event_cursor_checked(&[ev(1, "run.started"), ev(2, "task.completed")]).unwrap();
        assert_eq!(ok.last_seq, 2);
        assert_eq!(ok.last_settled_seq, 2);
    }

    #[test]
    fn build_rejects_gap_ledger() {
        // N2 regression at the builder boundary.
        let dir = run_dir_with(&[]);
        let state = run("running", vec![task("T_a", "pending", &[])]);
        assert!(build_descriptor(
            dir.path(),
            &state,
            &[ev(1, "run.started"), ev(3, "task.started")],
            ts("2026-06-05T00:00:00Z"),
            ts("2026-06-05T00:00:00Z"),
            1,
        )
        .is_err());
    }

    fn write_events(dir: &std::path::Path, evs: &[RunEvent]) {
        let mut body = String::new();
        for e in evs {
            body.push_str(&serde_json::to_string(e).unwrap());
            body.push('\n');
        }
        std::fs::write(dir.join(crate::paths::RUN_EVENTS_FILE), body).unwrap();
    }

    #[test]
    fn refresh_descriptor_noop_before_initial_event() {
        // PLAN.yaml present, no events.ndjson yet -> no half-baked descriptor.
        let dir = run_dir_with(&[]);
        let state = run("running", vec![task("T_a", "pending", &[])]);
        assert!(refresh_descriptor(dir.path(), &state).is_ok());
        assert!(
            !descriptor_path(dir.path()).exists(),
            "no descriptor before the initial event"
        );
    }

    #[test]
    fn refresh_descriptor_writes_when_events_present() {
        let dir = run_dir_with(&["outputs/T_done/api.json"]);
        write_events(dir.path(), &[ev(1, "run.started"), ev(2, "task.completed")]);
        let state = run(
            "running",
            vec![
                task("T_done", "done", &[("api", "outputs/T_done/api.json")]),
                task("T_next", "pending", &[]),
            ],
        );
        refresh_descriptor(dir.path(), &state).unwrap();
        let d = read_descriptor(dir.path()).unwrap().unwrap();
        assert_eq!(d.seeded_tasks.len(), 1);
        assert_eq!(d.event_ledger.last_seq, 2);
        assert_eq!(d.event_ledger.last_settled_seq, 2);
    }

    #[test]
    fn refresh_descriptor_errs_on_gapped_ledger_without_writing() {
        // A gapped/corrupt ledger -> refresh returns Err and writes nothing, so
        // the executor's warn-only wrap leaves no/stale guard and resume fails
        // closed — never failing the active run.
        let dir = run_dir_with(&[]);
        write_events(dir.path(), &[ev(1, "run.started"), ev(3, "task.completed")]);
        let state = run("running", vec![task("T_a", "pending", &[])]);
        assert!(refresh_descriptor(dir.path(), &state).is_err());
        assert!(
            !descriptor_path(dir.path()).exists(),
            "gapped ledger -> no descriptor written"
        );
    }

    // ----- validate_resume_target (Step 3) -----

    const RUN_ID: &str = "20260605-000000_abc";

    fn run_with_id(run_id: &str, status: &str, pid: u32, tasks: Vec<TaskState>) -> RunState {
        let order: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let mut map = serde_json::Map::new();
        for t in &tasks {
            map.insert(t.id.clone(), serde_json::to_value(t).unwrap());
        }
        serde_json::from_value(json!({
            "run_id": run_id,
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

    /// A complete, internally-consistent run dir for resume validation:
    /// PLAN.yaml + an output snapshot + RUN_STATE.json + a contiguous
    /// events.ndjson + a matching RESUME.json, in a dir named after the run id.
    fn build_resume_run_dir(pid: u32, status: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(RUN_ID);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join(crate::paths::PLAN_SNAPSHOT),
            "spec: demo\ntasks:\n  - id: T_done\n  - id: T_next\n",
        )
        .unwrap();
        std::fs::create_dir_all(run_dir.join("outputs/T_done")).unwrap();
        std::fs::write(run_dir.join("outputs/T_done/api.json"), "{}").unwrap();
        let abs = run_dir
            .join("outputs/T_done/api.json")
            .to_string_lossy()
            .into_owned();
        // "done" status -> both tasks done (a complete run); otherwise T_done is
        // done (with an output) and T_next is still pending.
        let tasks = if status == "done" {
            vec![
                task("T_done", "done", &[("api", &abs)]),
                task("T_next", "done", &[]),
            ]
        } else {
            vec![
                task("T_done", "done", &[("api", &abs)]),
                task("T_next", "pending", &[]),
            ]
        };
        let state = run_with_id(RUN_ID, status, pid, tasks);
        std::fs::write(
            run_dir.join(crate::paths::RUN_STATE_FILE),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        let events = [ev(1, "run.started"), ev(2, "task.completed")];
        write_events(&run_dir, &events);
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
        (tmp, run_dir)
    }

    fn good_run_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        build_resume_run_dir(999_999, "running")
    }

    #[test]
    fn validate_happy_path_abandoned_running_with_valid_descriptor() {
        let (_t, dir) = good_run_dir();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.can_resume, "should resume; issues: {:?}", r.issues);
        assert!(!r.already_complete);
        assert_eq!(r.reusable_done_tasks, vec!["T_done"]);
        assert!(r.issues.is_empty(), "no issues: {:?}", r.issues);
    }

    #[test]
    fn validate_live_run_refused_without_force_allowed_with() {
        let (_t, dir) = build_resume_run_dir(std::process::id(), "running");
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(!r.can_resume);
        assert!(r.has(ResumeIssueCode::LiveRun));
        let forced = validate_resume_target(&dir, true).unwrap();
        assert!(
            forced.can_resume,
            "force overrides liveness: {:?}",
            forced.issues
        );
    }

    #[test]
    fn validate_pid0_refused_without_force_allowed_with() {
        let (_t, dir) = build_resume_run_dir(0, "running");
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(!r.can_resume);
        assert!(r.has(ResumeIssueCode::LegacyLivenessUnknown));
        assert!(validate_resume_target(&dir, true).unwrap().can_resume);
    }

    #[test]
    fn validate_missing_descriptor_refused_force_cannot_override() {
        let (_t, dir) = good_run_dir();
        std::fs::remove_file(descriptor_path(&dir)).unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(!r.can_resume);
        assert!(r.has(ResumeIssueCode::MissingDescriptor));
        assert!(
            !validate_resume_target(&dir, true).unwrap().can_resume,
            "force must not bypass a missing descriptor"
        );
    }

    #[test]
    fn validate_corrupt_descriptor_refused() {
        let (_t, dir) = good_run_dir();
        std::fs::write(descriptor_path(&dir), "{not json").unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(!r.can_resume);
        assert!(r.has(ResumeIssueCode::SchemaMismatch));
    }

    #[test]
    fn validate_plan_drift_refused_force_cannot_override() {
        let (_t, dir) = good_run_dir();
        // Change the plan body (same task ids) so only the hash drifts.
        std::fs::write(
            dir.join(crate::paths::PLAN_SNAPSHOT),
            "spec: drifted\ntasks:\n  - id: T_done\n  - id: T_next\n",
        )
        .unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::PlanDrift));
        assert!(!r.can_resume);
        assert!(
            !validate_resume_target(&dir, true).unwrap().can_resume,
            "force must not bypass plan drift"
        );
    }

    #[test]
    fn validate_corrupt_events_refused() {
        let (_t, dir) = good_run_dir();
        std::fs::write(dir.join(crate::paths::RUN_EVENTS_FILE), "{not json\n").unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::EventLedgerCorrupt));
        assert!(!r.can_resume);
    }

    #[test]
    fn validate_gapped_events_refused() {
        let (_t, dir) = good_run_dir();
        write_events(&dir, &[ev(1, "run.started"), ev(3, "task.completed")]);
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::EventLedgerCorrupt));
        assert!(!r.can_resume);
    }

    #[test]
    fn validate_mid_task_non_settling_tail_is_resumable() {
        // A run killed while the NEXT task is running: a `task.started` tail
        // beyond the descriptor is normal in-flight progress, NOT staleness.
        let (_t, dir) = good_run_dir();
        write_events(
            &dir,
            &[
                ev(1, "run.started"),
                ev(2, "task.completed"),
                ev(3, "task.started"),
            ],
        );
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(
            !r.has(ResumeIssueCode::EventCursorStale),
            "a non-settling tail is not stale: {:?}",
            r.issues
        );
        assert!(
            r.can_resume,
            "mid-task interruption is resumable: {:?}",
            r.issues
        );
    }

    #[test]
    fn validate_settling_tail_after_descriptor_refused() {
        // A task that SETTLED after the descriptor changes the seedable done-set.
        for kind in [
            "task.completed",
            "task.failed",
            "task.cancelled",
            "task.skipped",
        ] {
            let (_t, dir) = good_run_dir();
            write_events(
                &dir,
                &[ev(1, "run.started"), ev(2, "task.completed"), ev(3, kind)],
            );
            let r = validate_resume_target(&dir, false).unwrap();
            assert!(
                r.has(ResumeIssueCode::EventCursorStale),
                "settling tail {kind} must refuse: {:?}",
                r.issues
            );
            assert!(!r.can_resume, "{kind}");
        }
    }

    #[test]
    fn validate_truncated_ledger_refused() {
        // recomputed last_seq < descriptor last_seq -> truncated/rewritten ledger.
        let (_t, dir) = good_run_dir(); // descriptor built from [1,2]
        write_events(&dir, &[ev(1, "run.started")]); // shrink to [1]
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::EventCursorStale));
        assert!(!r.can_resume);
    }

    #[test]
    fn validate_verify_completed_tail_is_resumable() {
        // F-115 N5: verify.* is a sub-lifecycle and does NOT advance the settled
        // cursor, so a verify tail after the descriptor stays resumable.
        let (_t, dir) = good_run_dir();
        write_events(
            &dir,
            &[
                ev(1, "run.started"),
                ev(2, "task.completed"),
                ev(3, "verify.started"),
                ev(4, "verify.completed"),
            ],
        );
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(
            !r.has(ResumeIssueCode::EventCursorStale),
            "verify tail allowed: {:?}",
            r.issues
        );
        assert!(r.can_resume);
    }

    #[test]
    fn validate_missing_output_snapshot_refused() {
        let (_t, dir) = good_run_dir();
        std::fs::remove_file(dir.join("outputs/T_done/api.json")).unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::SeedOutputMissing));
        assert!(!r.can_resume);
    }

    #[test]
    fn validate_task_set_mismatch_refused() {
        let (_t, dir) = good_run_dir();
        // Mark T_next done in state too, so the Done set no longer matches the
        // descriptor's seeded_tasks ({T_done}).
        let abs = dir
            .join("outputs/T_done/api.json")
            .to_string_lossy()
            .into_owned();
        let state2 = run_with_id(
            RUN_ID,
            "running",
            999_999,
            vec![
                task("T_done", "done", &[("api", &abs)]),
                task("T_next", "done", &[]),
            ],
        );
        std::fs::write(
            dir.join(crate::paths::RUN_STATE_FILE),
            serde_json::to_string_pretty(&state2).unwrap(),
        )
        .unwrap();
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.has(ResumeIssueCode::TaskSetMismatch));
        assert!(!r.can_resume);
    }

    #[test]
    fn validate_terminal_failed_and_cancelled_refused() {
        for status in ["failed", "cancelled"] {
            let (_t, dir) = build_resume_run_dir(999_999, status);
            let r = validate_resume_target(&dir, false).unwrap();
            assert!(r.has(ResumeIssueCode::TerminalRun), "{status}");
            assert!(!r.can_resume, "{status}");
            assert!(
                !validate_resume_target(&dir, true).unwrap().can_resume,
                "force must not bypass a terminal run ({status})"
            );
        }
    }

    #[test]
    fn validate_complete_run_is_a_noop() {
        let (_t, dir) = build_resume_run_dir(999_999, "done");
        let r = validate_resume_target(&dir, false).unwrap();
        assert!(r.already_complete);
        assert!(!r.can_resume, "complete run is a no-op, not a resume");
        assert!(r.has(ResumeIssueCode::AlreadyComplete));
        // a clean complete run has ONLY the info issue (so the CLI no-ops).
        assert!(
            !r.issues.iter().any(|i| i.code.blocks(false)),
            "clean complete run has no blocking issue"
        );
    }

    #[test]
    fn validate_complete_run_with_blocking_issue_is_not_a_clean_noop() {
        // N1: a Done+all-done run that ALSO has plan drift / corrupt events must
        // carry a blocking issue alongside already_complete — the CLI then
        // refuses instead of silently no-op'ing.
        for break_it in [
            (|dir: &std::path::Path| {
                std::fs::write(
                    dir.join(crate::paths::PLAN_SNAPSHOT),
                    "spec: drifted\ntasks:\n  - id: T_done\n  - id: T_next\n",
                )
                .unwrap()
            }) as fn(&std::path::Path),
            |dir: &std::path::Path| {
                std::fs::write(dir.join(crate::paths::RUN_EVENTS_FILE), "{not json\n").unwrap()
            },
        ] {
            let (_t, dir) = build_resume_run_dir(999_999, "done");
            break_it(&dir);
            let r = validate_resume_target(&dir, false).unwrap();
            assert!(r.already_complete, "still complete");
            assert!(
                r.issues.iter().any(|i| i.code.blocks(false)),
                "a blocking issue co-exists with already_complete"
            );
            assert!(!r.can_resume);
        }
    }
}
