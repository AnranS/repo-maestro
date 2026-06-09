//! F-117 resume guard descriptor (`maestro.resume_descriptor.v1`).
//!
//! A compact, privacy-safe descriptor written per run that answers ONE question:
//! "is this prior run safe to seed completed tasks from on `maestro resume`?".
//! It records run identity, plan-snapshot integrity, an F-115 event-ledger
//! cursor, a task-status summary, and the completed tasks eligible for seeding —
//! counts and symbolic refs only. It NEVER carries a raw prompt / log /
//! transcript / model output body, an absolute or `..` path, or a secret-like
//! value. The types + validation live here; the filesystem build/write/read
//! helpers live in [`crate::scheduler::resume`].

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// Run / task / project ids and run-status strings are short single-line values.
const MAX_ID_BYTES: usize = 256;

/// Run-relative plan snapshot integrity. The body is never stored — only its
/// size and a stable local hash, recomputed on resume to detect plan drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePlanRef {
    /// Must be exactly `PLAN.yaml` in v1.
    pub path: String,
    pub bytes: u64,
    /// `file_guard::file_hash` value, e.g. `fnv1a64:0123456789abcdef`.
    pub hash: String,
}

/// Cursor into the F-115 `events.ndjson` ledger as observed when the descriptor
/// was last written. Recomputed + compared on resume (a mismatch means the
/// ledger changed after the descriptor was written → refuse).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeEventCursor {
    /// Observed event schema, expected `maestro.run_event.v2`.
    pub schema_version: String,
    /// Last event sequence in `events.ndjson`, or 0 if there are no events.
    pub last_seq: u64,
    /// Last event seq whose kind settled a task or a terminal run. Computed from
    /// typed `RunEventKind` variants — `verify.completed` is a sub-lifecycle and
    /// does NOT advance it (see [`crate::scheduler::resume::compute_event_cursor`]).
    pub last_settled_seq: u64,
}

/// Compact per-status task counts. The seven buckets must add up to
/// `task_total` (a corrupt/inconsistent descriptor otherwise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeStateSummary {
    /// `running` / `done` / `failed` / `cancelled`.
    pub run_status: String,
    pub task_total: u64,
    pub done: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub skipped: u64,
    pub awaiting_approval: u64,
    pub running: u64,
    pub pending: u64,
}

impl ResumeStateSummary {
    /// Sum of the seven per-status buckets.
    pub fn bucket_sum(&self) -> u64 {
        self.done
            + self.failed
            + self.cancelled
            + self.skipped
            + self.awaiting_approval
            + self.running
            + self.pending
    }
}

/// One completed task that MAY be seeded on resume. Only `Done` tasks appear;
/// F-117 never seeds failed/cancelled/skipped tasks. The descriptor stores only
/// the output-snapshot COUNT — the actual snapshot paths are validated against
/// `RUN_STATE.json` at resume time, never duplicated (or leaked) here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeTaskSeed {
    pub task_id: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub workflow_outputs: u64,
}

/// `maestro.resume_descriptor.v1` — the resume guard artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeDescriptor {
    #[serde(default = "crate::schema::resume_descriptor_version")]
    pub schema_version: String,
    pub run_id: String,
    /// RFC3339.
    pub created_at: String,
    /// RFC3339.
    pub updated_at: String,
    /// pid of the process that last wrote the descriptor.
    pub writer_pid: u32,
    pub plan: ResumePlanRef,
    pub event_ledger: ResumeEventCursor,
    pub state: ResumeStateSummary,
    pub seeded_tasks: Vec<ResumeTaskSeed>,
}

/// Validate descriptor shape + self-consistency, run BEFORE a write AND AFTER a
/// read. A hand-edited / drifted descriptor (wrong schema, non-RFC3339
/// timestamp, non-`PLAN.yaml` path, malformed hash, buckets that don't sum,
/// seed count that disagrees with the done count, unsafe task id) is corrupt —
/// reject it, never trust it to gate a resume. Cross-file checks (recompute plan
/// hash, recompute event cursor, seed-output existence) are the resume-time
/// validator's job, not this function's.
pub fn validate_descriptor(d: &ResumeDescriptor) -> Result<()> {
    ensure!(
        d.schema_version == crate::schema::RESUME_DESCRIPTOR_V1,
        "resume descriptor schema_version {:?} is not {}",
        d.schema_version,
        crate::schema::RESUME_DESCRIPTOR_V1
    );
    validate_id("resume descriptor run_id", &d.run_id)?;
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&d.created_at).is_ok(),
        "resume descriptor created_at {:?} is not RFC3339",
        d.created_at
    );
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&d.updated_at).is_ok(),
        "resume descriptor updated_at {:?} is not RFC3339",
        d.updated_at
    );

    // Plan snapshot ref: v1 path is fixed, hash must be the stable local format.
    ensure!(
        d.plan.path == crate::paths::PLAN_SNAPSHOT,
        "resume descriptor plan.path {:?} must be {:?}",
        d.plan.path,
        crate::paths::PLAN_SNAPSHOT
    );
    ensure!(
        is_stable_hash(&d.plan.hash),
        "resume descriptor plan.hash {:?} is not a fnv1a64 hash",
        d.plan.hash
    );

    // Event cursor: observed schema is the current event schema; a settled seq
    // can never exceed the last seq.
    ensure!(
        d.event_ledger.schema_version == crate::schema::RUN_EVENT_V2,
        "resume descriptor event schema_version {:?} is not {}",
        d.event_ledger.schema_version,
        crate::schema::RUN_EVENT_V2
    );
    ensure!(
        d.event_ledger.last_settled_seq <= d.event_ledger.last_seq,
        "resume descriptor last_settled_seq {} exceeds last_seq {}",
        d.event_ledger.last_settled_seq,
        d.event_ledger.last_seq
    );

    // State summary: known run status + the seven buckets sum to the total.
    ensure!(
        matches!(
            d.state.run_status.as_str(),
            "running" | "done" | "failed" | "cancelled"
        ),
        "resume descriptor run_status {:?} is not a known run status",
        d.state.run_status
    );
    ensure!(
        d.state.bucket_sum() == d.state.task_total,
        "resume descriptor task buckets sum to {} but task_total is {}",
        d.state.bucket_sum(),
        d.state.task_total
    );

    // Seeded tasks: only Done tasks are seedable, so exactly `done` of them, each
    // with a safe id + symbolic project + RFC3339 end time.
    ensure!(
        d.seeded_tasks.len() as u64 == d.state.done,
        "resume descriptor lists {} seeded tasks but {} tasks are done",
        d.seeded_tasks.len(),
        d.state.done
    );
    for seed in &d.seeded_tasks {
        validate_slug("resume seed task_id", &seed.task_id)?;
        validate_id("resume seed project", &seed.project)?;
        if let Some(ended_at) = &seed.ended_at {
            ensure!(
                chrono::DateTime::parse_from_rfc3339(ended_at).is_ok(),
                "resume seed {:?} ended_at {:?} is not RFC3339",
                seed.task_id,
                ended_at
            );
        }
    }
    Ok(())
}

/// A non-empty, single-line, byte-capped id.
fn validate_id(field: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must be non-empty");
    ensure!(
        !value.contains(['\n', '\r']),
        "{field} must be single-line (no newline characters)"
    );
    ensure!(
        value.len() <= MAX_ID_BYTES,
        "{field} exceeds {MAX_ID_BYTES} bytes"
    );
    Ok(())
}

/// An id that is also a safe single path component: no `/`, `\`, or `..`. Used
/// for task ids, which become run-relative artifact filenames elsewhere.
fn validate_slug(field: &str, value: &str) -> Result<()> {
    validate_id(field, value)?;
    ensure!(
        !value.contains('/') && !value.contains('\\') && !value.contains(".."),
        "{field} {value:?} must be a safe path component (no '/', '\\', or '..')"
    );
    Ok(())
}

/// `file_guard::file_hash` shape: `fnv1a64:` + 16 lowercase hex digits.
fn is_stable_hash(hash: &str) -> bool {
    match hash.strip_prefix("fnv1a64:") {
        Some(tail) => tail.len() == 16 && tail.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Resume-time validation report (Step 3). The descriptor answers "is this run
// safe to seed from"; the report is the recomputed, cross-checked verdict.
// ---------------------------------------------------------------------------

/// Severity of a resume issue. `high`/`medium` block resume (medium only for the
/// two liveness-uncertainty codes, overridable by `--force`); `info` is a no-op
/// signal (already complete).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeSeverity {
    High,
    Medium,
    Info,
}

/// Closed code table for resume refusals (mirrors the design's validation table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeIssueCode {
    #[serde(rename = "resume.missing_descriptor")]
    MissingDescriptor,
    #[serde(rename = "resume.schema_mismatch")]
    SchemaMismatch,
    #[serde(rename = "resume.run_id_mismatch")]
    RunIdMismatch,
    #[serde(rename = "resume.live_run")]
    LiveRun,
    #[serde(rename = "resume.legacy_liveness_unknown")]
    LegacyLivenessUnknown,
    #[serde(rename = "resume.terminal_run")]
    TerminalRun,
    #[serde(rename = "resume.plan_drift")]
    PlanDrift,
    #[serde(rename = "resume.event_ledger_corrupt")]
    EventLedgerCorrupt,
    #[serde(rename = "resume.event_cursor_stale")]
    EventCursorStale,
    #[serde(rename = "resume.task_set_mismatch")]
    TaskSetMismatch,
    #[serde(rename = "resume.seed_output_missing")]
    SeedOutputMissing,
    #[serde(rename = "resume.seed_output_unsafe_path")]
    SeedOutputUnsafePath,
    #[serde(rename = "resume.already_complete")]
    AlreadyComplete,
}

impl ResumeIssueCode {
    pub fn as_str(self) -> &'static str {
        use ResumeIssueCode::*;
        match self {
            MissingDescriptor => "resume.missing_descriptor",
            SchemaMismatch => "resume.schema_mismatch",
            RunIdMismatch => "resume.run_id_mismatch",
            LiveRun => "resume.live_run",
            LegacyLivenessUnknown => "resume.legacy_liveness_unknown",
            TerminalRun => "resume.terminal_run",
            PlanDrift => "resume.plan_drift",
            EventLedgerCorrupt => "resume.event_ledger_corrupt",
            EventCursorStale => "resume.event_cursor_stale",
            TaskSetMismatch => "resume.task_set_mismatch",
            SeedOutputMissing => "resume.seed_output_missing",
            SeedOutputUnsafePath => "resume.seed_output_unsafe_path",
            AlreadyComplete => "resume.already_complete",
        }
    }

    pub fn severity(self) -> ResumeSeverity {
        use ResumeIssueCode::*;
        match self {
            AlreadyComplete => ResumeSeverity::Info,
            LegacyLivenessUnknown | TerminalRun => ResumeSeverity::Medium,
            _ => ResumeSeverity::High,
        }
    }

    /// `--force` overrides ONLY liveness uncertainty. It never bypasses a corrupt
    /// or missing descriptor, plan drift, a stale/corrupt event ledger, a missing
    /// or unsafe output snapshot, a task-set mismatch, or a terminal run.
    pub fn force_overridable(self) -> bool {
        matches!(self, Self::LiveRun | Self::LegacyLivenessUnknown)
    }

    /// Whether this issue blocks resume, given whether `--force` was passed.
    pub fn blocks(self, force: bool) -> bool {
        match self {
            // info-only signal; the caller treats it as a no-op, not a block.
            Self::AlreadyComplete => false,
            code if code.force_overridable() => !force,
            _ => true,
        }
    }
}

/// One recomputed refusal reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeIssue {
    pub code: ResumeIssueCode,
    pub severity: ResumeSeverity,
    pub detail: String,
}

impl ResumeIssue {
    pub fn new(code: ResumeIssueCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            severity: code.severity(),
            detail: detail.into(),
        }
    }
}

/// Recomputed verdict for a resume target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeValidationReport {
    pub run_id: String,
    /// Safe to seed completed tasks + run the rest.
    pub can_resume: bool,
    /// The run is already complete — a no-op, not a refusal.
    pub already_complete: bool,
    pub reusable_done_tasks: Vec<String>,
    pub issues: Vec<ResumeIssue>,
}

impl ResumeValidationReport {
    pub fn issue_codes(&self) -> Vec<ResumeIssueCode> {
        self.issues.iter().map(|i| i.code).collect()
    }

    pub fn has(&self, code: ResumeIssueCode) -> bool {
        self.issues.iter().any(|i| i.code == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResumeDescriptor {
        ResumeDescriptor {
            schema_version: crate::schema::resume_descriptor_version(),
            run_id: "20260605-000000_abc".into(),
            created_at: "2026-06-05T00:00:00Z".into(),
            updated_at: "2026-06-05T00:01:00Z".into(),
            writer_pid: 4242,
            plan: ResumePlanRef {
                path: "PLAN.yaml".into(),
                bytes: 1024,
                hash: "fnv1a64:0123456789abcdef".into(),
            },
            event_ledger: ResumeEventCursor {
                schema_version: crate::schema::RUN_EVENT_V2.into(),
                last_seq: 9,
                last_settled_seq: 7,
            },
            state: ResumeStateSummary {
                run_status: "running".into(),
                task_total: 3,
                done: 1,
                failed: 0,
                cancelled: 0,
                skipped: 0,
                awaiting_approval: 0,
                running: 1,
                pending: 1,
            },
            seeded_tasks: vec![ResumeTaskSeed {
                task_id: "T_change_billing".into(),
                project: "billing-service".into(),
                ended_at: Some("2026-06-05T00:00:30Z".into()),
                workflow_outputs: 2,
            }],
        }
    }

    #[test]
    fn full_descriptor_round_trips() {
        let d = sample();
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"schema_version\":\"maestro.resume_descriptor.v1\""));
        let back: ResumeDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        assert!(validate_descriptor(&d).is_ok());
    }

    #[test]
    fn buckets_must_sum_to_total() {
        let mut d = sample();
        assert!(validate_descriptor(&d).is_ok());
        d.state.pending = 5; // 1 + 1 + 5 = 7 != 3
        assert!(validate_descriptor(&d).is_err());
    }

    #[test]
    fn seeded_count_must_equal_done() {
        let mut d = sample();
        d.state.done = 2; // descriptor lists only 1 seed
                          // keep the bucket sum consistent so this isolates the seed/done check
        d.state.pending = 0;
        assert_eq!(d.state.bucket_sum(), d.state.task_total);
        assert!(validate_descriptor(&d).is_err());
    }

    #[test]
    fn wrong_schema_version_rejects() {
        let mut d = sample();
        d.schema_version = "maestro.resume_descriptor.v2".into();
        assert!(validate_descriptor(&d).is_err());
    }

    #[test]
    fn bad_plan_path_or_hash_rejects() {
        let mut d = sample();
        d.plan.path = "plan.yaml".into();
        assert!(validate_descriptor(&d).is_err());

        for bad_hash in [
            "0123456789abcdef",          // missing prefix
            "fnv1a64:",                  // empty tail
            "fnv1a64:0123456789abcde",   // 15 hex
            "fnv1a64:0123456789abcdeff", // 17 hex
            "fnv1a64:zzzzzzzzzzzzzzzz",  // non-hex
            "sha256:0123456789abcdef",   // wrong algo
        ] {
            let mut d = sample();
            d.plan.hash = bad_hash.into();
            assert!(
                validate_descriptor(&d).is_err(),
                "hash should reject: {bad_hash}"
            );
        }
    }

    #[test]
    fn bad_timestamps_reject() {
        for mutate in [
            (|d: &mut ResumeDescriptor| d.created_at = "not-a-date".into())
                as fn(&mut ResumeDescriptor),
            |d: &mut ResumeDescriptor| d.updated_at = "2026/06/05".into(),
            |d: &mut ResumeDescriptor| d.seeded_tasks[0].ended_at = Some("nope".into()),
        ] {
            let mut d = sample();
            mutate(&mut d);
            assert!(validate_descriptor(&d).is_err());
        }
    }

    #[test]
    fn unsafe_seed_task_id_rejects() {
        for bad in ["../escape", "bad/name", "..", "a\\b", "", "  "] {
            let mut d = sample();
            d.seeded_tasks[0].task_id = bad.into();
            assert!(validate_descriptor(&d).is_err(), "bad seed id {bad:?}");
        }
    }

    #[test]
    fn settled_seq_cannot_exceed_last_seq() {
        let mut d = sample();
        d.event_ledger.last_settled_seq = d.event_ledger.last_seq + 1;
        assert!(validate_descriptor(&d).is_err());
    }

    #[test]
    fn wrong_event_schema_version_rejects() {
        let mut d = sample();
        d.event_ledger.schema_version = "maestro.run_event.v1".into();
        assert!(validate_descriptor(&d).is_err());
    }

    #[test]
    fn unknown_run_status_rejects() {
        let mut d = sample();
        d.state.run_status = "paused".into();
        assert!(validate_descriptor(&d).is_err());
    }
}
