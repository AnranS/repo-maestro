//! Per-run finding ledger (`findings.ndjson`) — F-110.
//!
//! One append-only, newline-delimited JSON record per finding, unifying the
//! signals a human reviewer cares about (risk / refuter / doctor / …) into a
//! single evidence ledger the HOTL dashboard reads. Mirrors the append / lock /
//! read-last-seq discipline of [`super::events`].
//!
//! Design: `docs/experience/F-110-FINDING-LEDGER-DESIGN.md`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::paths;

/// Max characters in a finding `summary`. Larger detail must live as a run
/// artifact referenced by path, never inlined — keeps the ledger cheap to scan.
const SUMMARY_MAX_CHARS: usize = 500;
/// Max number of `evidence_refs` on a single finding.
const EVIDENCE_REFS_MAX: usize = 32;

static FINDING_APPEND_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

/// What kind of signal a finding records. Closed set in v1 so the dashboard can
/// render per-kind affordances; extend deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    Risk,
    Refute,
    Approval,
    Learn,
    Doctor,
    Channel,
}

impl FindingKind {
    /// Every variant, in declaration order. Single source of truth for callers
    /// that need to enumerate or validate kinds (e.g. F-114 profile outputs).
    pub const ALL: [FindingKind; 6] = [
        Self::Risk,
        Self::Refute,
        Self::Approval,
        Self::Learn,
        Self::Doctor,
        Self::Channel,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Risk => "risk",
            Self::Refute => "refute",
            Self::Approval => "approval",
            Self::Learn => "learn",
            Self::Doctor => "doctor",
            Self::Channel => "channel",
        }
    }

    /// Parse a kind name (the inverse of [`as_str`]). `None` for an unknown
    /// string — callers (e.g. an F-114 profile's declared `finding_kind`) treat
    /// that as "no marker" rather than erroring.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// v1 only ever writes `Open`. The ledger is append-only, so adoption /
/// dismissal — if ever added — is a *separate* transition record, never an
/// in-place rewrite of this file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    #[default]
    Open,
}

/// Where a finding came from, for audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub producer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default = "crate::schema::finding_version")]
    pub schema_version: String,
    /// `<kind>-<seq>`; minted by [`append_finding`] inside the append lock.
    pub finding_id: String,
    pub run_id: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub kind: FindingKind,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub summary: String,
    // Required by the schema: always serialized, even when empty (a reader
    // should never have to distinguish "no evidence" from "field omitted").
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub source: String,
    #[serde(default)]
    pub status: FindingStatus,
    /// RFC3339, supplied by the caller (not minted here, so tests stay
    /// deterministic and each producer controls its own clock).
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

impl Finding {
    /// A finding ready to hand to [`append_finding`]. `finding_id` and `seq` are
    /// placeholders — the writer overwrites them under the append lock.
    pub fn new(
        run_id: impl Into<String>,
        kind: FindingKind,
        severity: Severity,
        source: impl Into<String>,
        summary: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: crate::schema::finding_version(),
            finding_id: String::new(),
            run_id: run_id.into(),
            seq: 0,
            task_id: None,
            kind,
            severity,
            confidence: None,
            summary: summary.into(),
            evidence_refs: Vec::new(),
            source: source.into(),
            status: FindingStatus::Open,
            created_at: created_at.into(),
            provenance: None,
        }
    }

    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn evidence(mut self, refs: impl IntoIterator<Item = String>) -> Self {
        self.evidence_refs = refs.into_iter().collect();
        self
    }

    pub fn provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }
}

pub fn findings_path(run_dir: &Path) -> PathBuf {
    run_dir.join(paths::RUN_FINDINGS_FILE)
}

/// Validate caller-controlled fields before taking the append lock. A bad
/// evidence path or oversized field is a *writer* error — surfaced, never
/// silently normalized or truncated.
fn validate(finding: &Finding) -> Result<()> {
    anyhow::ensure!(
        finding.summary.chars().count() <= SUMMARY_MAX_CHARS,
        "finding summary exceeds {SUMMARY_MAX_CHARS} chars; keep it one line and reference detail via evidence_refs"
    );
    // One-line human-readable: a newline would corrupt the NDJSON list view and
    // any bot/channel echo of the summary.
    anyhow::ensure!(
        !finding.summary.contains(['\n', '\r']),
        "finding summary must be a single line (no newlines)"
    );
    if let Some(c) = finding.confidence {
        anyhow::ensure!(
            c.is_finite() && (0.0..=1.0).contains(&c),
            "finding confidence {c} is out of the 0.0..=1.0 contract"
        );
    }
    // created_at must be a real RFC3339 timestamp so the dashboard / API can
    // sort and render it without choking on free-form strings.
    anyhow::ensure!(
        chrono::DateTime::parse_from_rfc3339(&finding.created_at).is_ok(),
        "finding created_at {:?} is not RFC3339",
        finding.created_at
    );
    anyhow::ensure!(
        finding.evidence_refs.len() <= EVIDENCE_REFS_MAX,
        "finding has more than {EVIDENCE_REFS_MAX} evidence_refs; aggregate into a single run artifact"
    );
    for r in &finding.evidence_refs {
        anyhow::ensure!(!r.is_empty(), "evidence ref is empty");
        let p = Path::new(r);
        anyhow::ensure!(
            !p.is_absolute(),
            "evidence ref {r:?} is absolute; use a run-relative artifact path"
        );
        anyhow::ensure!(
            !p.components().any(|c| matches!(c, Component::ParentDir)),
            "evidence ref {r:?} escapes the run dir with `..`"
        );
    }
    Ok(())
}

/// Append one finding to `<run_dir>/findings.ndjson`. Mints `seq`
/// (read-last-seq + 1) and `finding_id` (`<kind>-<seq>`) inside a per-file lock,
/// then writes a single `line + "\n"`, mirroring [`super::events::append_event`].
/// Returns the finalized record.
pub fn append_finding(run_dir: &Path, finding: Finding) -> Result<Finding> {
    validate(&finding)?;
    paths::ensure_dir(run_dir)?;
    let path = findings_path(run_dir);
    let append_lock = finding_append_lock(&path)?;
    let _guard = append_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("finding append lock poisoned for {}", path.display()))?;

    let seq = read_findings(run_dir)?.last().map(|f| f.seq).unwrap_or(0) + 1;
    let mut finding = finding;
    finding.seq = seq;
    finding.finding_id = format!("{}-{}", finding.kind.as_str(), seq);
    // Writer owns these regardless of what the caller passed.
    finding.schema_version = crate::schema::finding_version();
    finding.status = FindingStatus::Open;

    let mut line = serde_json::to_string(&finding).context("serialize finding")?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open findings file {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("append finding {}", path.display()))?;
    Ok(finding)
}

pub fn read_findings(run_dir: &Path) -> Result<Vec<Finding>> {
    let path = findings_path(run_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut findings = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let finding: Finding = serde_json::from_str(raw)
            .with_context(|| format!("parse {} line {}", path.display(), idx + 1))?;
        findings.push(finding);
    }
    Ok(findings)
}

fn finding_append_lock(path: &Path) -> Result<Arc<Mutex<()>>> {
    let mut locks = FINDING_APPEND_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("finding append lock registry poisoned"))?;
    Ok(locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn sample(run: &str) -> Finding {
        Finding::new(
            run,
            FindingKind::Risk,
            Severity::High,
            "risk",
            "task touches a generated client and its producer",
            "2026-06-03T00:00:00Z",
        )
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let finding = sample("run-1")
            .task("t-1")
            .confidence(0.8)
            .evidence(["tasks/t-1/diff.patch".to_string()])
            .provenance(Provenance {
                producer: "risk-gate".to_string(),
                producer_version: Some("1".to_string()),
                inputs_digest: None,
            });
        let line = serde_json::to_string(&finding).unwrap();
        let back: Finding = serde_json::from_str(&line).unwrap();
        assert_eq!(back, finding);
        assert_eq!(back.schema_version, crate::schema::FINDING_V1);
        // optional fields absent still round-trips
        let bare = sample("run-1");
        let bare_back: Finding =
            serde_json::from_str(&serde_json::to_string(&bare).unwrap()).unwrap();
        assert_eq!(bare_back, bare);
        assert_eq!(bare_back.task_id, None);
        assert!(bare_back.evidence_refs.is_empty());
    }

    #[test]
    fn append_mints_id_and_seq_and_reparses() {
        let temp = tempfile::tempdir().unwrap();
        let first = append_finding(temp.path(), sample("run-1")).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(first.finding_id, "risk-1");
        assert_eq!(first.status, FindingStatus::Open);

        let second = append_finding(
            temp.path(),
            Finding::new(
                "run-1",
                FindingKind::Doctor,
                Severity::Medium,
                "doctor",
                "run abandoned with an in-flight task",
                "2026-06-03T00:01:00Z",
            ),
        )
        .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.finding_id, "doctor-2");

        let all = read_findings(temp.path()).unwrap();
        assert_eq!(all, vec![first, second]);

        // file is valid NDJSON
        let raw = std::fs::read_to_string(findings_path(temp.path())).unwrap();
        assert_eq!(raw.lines().count(), 2);
    }

    #[test]
    fn rejects_bad_evidence_paths() {
        let temp = tempfile::tempdir().unwrap();
        for bad in ["/etc/passwd", "../escape.txt", ""] {
            let f = sample("run-1").evidence([bad.to_string()]);
            let err = append_finding(temp.path(), f).unwrap_err().to_string();
            assert!(
                err.contains("evidence ref"),
                "expected an evidence-ref rejection for {bad:?}, got: {err}"
            );
        }
        // nothing was written
        assert!(read_findings(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn rejects_oversized_summary_and_too_many_refs() {
        let temp = tempfile::tempdir().unwrap();
        let long = "x".repeat(SUMMARY_MAX_CHARS + 1);
        let f = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::Low,
            "risk",
            long,
            "2026-06-03T00:00:00Z",
        );
        assert!(append_finding(temp.path(), f)
            .unwrap_err()
            .to_string()
            .contains("summary"));

        let many: Vec<String> = (0..=EVIDENCE_REFS_MAX).map(|i| format!("a/{i}")).collect();
        let f = sample("run-1").evidence(many);
        assert!(append_finding(temp.path(), f)
            .unwrap_err()
            .to_string()
            .contains("evidence_refs"));
        assert!(read_findings(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn enforces_confidence_contract() {
        let temp = tempfile::tempdir().unwrap();
        for bad in [2.0, -0.1, f64::NAN, f64::INFINITY] {
            let f = sample("run-1").confidence(bad);
            assert!(
                append_finding(temp.path(), f)
                    .unwrap_err()
                    .to_string()
                    .contains("confidence"),
                "expected a confidence rejection for {bad}"
            );
        }
        // boundary + interior values are accepted
        for ok in [0.0, 0.5, 1.0] {
            append_finding(temp.path(), sample("run-1").confidence(ok)).unwrap();
        }
        assert_eq!(read_findings(temp.path()).unwrap().len(), 3);
    }

    #[test]
    fn rejects_non_rfc3339_created_at() {
        let temp = tempfile::tempdir().unwrap();
        let f = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::Low,
            "risk",
            "ok",
            "yesterday",
        );
        assert!(append_finding(temp.path(), f)
            .unwrap_err()
            .to_string()
            .contains("RFC3339"));
        assert!(read_findings(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn rejects_multiline_summary() {
        let temp = tempfile::tempdir().unwrap();
        let f = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::Low,
            "risk",
            "line one\nline two",
            "2026-06-03T00:00:00Z",
        );
        assert!(append_finding(temp.path(), f)
            .unwrap_err()
            .to_string()
            .contains("single line"));
        assert!(read_findings(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn empty_evidence_refs_is_serialized_not_skipped() {
        // evidence_refs is required by the schema — an empty list must appear in
        // the JSON, so a reader never confuses "no evidence" with "field absent".
        let line = serde_json::to_string(&sample("run-1")).unwrap();
        assert!(
            line.contains("\"evidence_refs\":[]"),
            "empty evidence_refs must serialize, got: {line}"
        );
    }

    #[test]
    fn concurrent_appends_are_parseable_with_unique_sequence_numbers() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = Arc::new(temp.path().to_path_buf());
        let thread_count = 8;
        let per_thread = 50;
        let barrier = Arc::new(Barrier::new(thread_count));
        let mut handles = Vec::new();

        for idx in 0..thread_count {
            let run_dir = Arc::clone(&run_dir);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for iter in 0..per_thread {
                    append_finding(
                        &run_dir,
                        Finding::new(
                            "run-1",
                            FindingKind::Risk,
                            Severity::Info,
                            "risk",
                            format!("f-{idx}-{iter}"),
                            "2026-06-03T00:00:00Z",
                        ),
                    )
                    .unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let findings = read_findings(temp.path()).unwrap();
        let expected = thread_count * per_thread;
        assert_eq!(findings.len(), expected);
        let seqs = findings.iter().map(|f| f.seq).collect::<Vec<_>>();
        assert_eq!(
            seqs,
            (1..=expected as u64).collect::<Vec<_>>(),
            "finding seqs must be unique and strictly increasing"
        );
        // every finding_id matches its seq
        for f in &findings {
            assert_eq!(f.finding_id, format!("{}-{}", f.kind.as_str(), f.seq));
        }
    }
}
