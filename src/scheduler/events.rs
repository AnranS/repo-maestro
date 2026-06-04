use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::config::channels::ChannelConfig;
use crate::paths;
use crate::scheduler::findings::{Finding, Severity};
use crate::schema::artifacts::{ArtifactRef, ArtifactSource};

const MAX_EVENT_PAYLOAD_BYTES: usize = 1024;
/// `message` and `display.label` are short structured summaries, never raw model
/// output / prompt / transcript. F-115: `message` is lossy-sanitized to this bound
/// at write (newlines folded to spaces, truncated on a UTF-8 boundary) so a
/// failure event is never dropped just because its error string is long/multiline;
/// `display.label` (a new field with no legacy producer) is hard-rejected instead.
/// Step 2 may flip `message` from sanitize to hard-reject once producers only pass
/// short, single-line summaries.
const MAX_EVENT_MESSAGE_BYTES: usize = 256;

static EVENT_APPEND_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

/// Closed event taxonomy (F-115 v2). Each variant maps 1:1 to a distinct wire
/// kind — v2 de-collapses the lossy v1 strings (v1 folded approval-granted /
/// verify / skipped / run-cancelled into `task.completed` / `task.cancelled` /
/// `run.failed`). Producers may only emit the named variants; `Other` is a
/// **reader-only** tolerance for a well-formed *future* kind, so an unknown kind
/// never errors the whole ledger. The writer rejects `Other` (see `append_event`),
/// and a structurally corrupt line still hard-errors at JSON parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEventKind {
    RunCreated,
    RunCompleted,
    RunFailed,
    RunCancelled,
    CancelRequested,
    TaskQueued,
    TaskSkipped,
    TaskStarted,
    TaskSucceeded,
    TaskFailed,
    TaskCancelled,
    TaskApprovalRequested,
    TaskApprovalGranted,
    VerifyStarted,
    VerifyCompleted,
    FindingRecorded,
    ReplanWritten,
    /// Reader-only: an unknown but well-formed wire kind, preserved verbatim.
    /// Never produced by a writer.
    Other(String),
}

impl RunEventKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::RunCreated => "run.started",
            Self::RunCompleted => "run.completed",
            Self::RunFailed => "run.failed",
            Self::RunCancelled => "run.cancelled",
            Self::CancelRequested => "run.cancel_requested",
            Self::TaskQueued => "task.queued",
            Self::TaskSkipped => "task.skipped",
            Self::TaskStarted => "task.started",
            Self::TaskSucceeded => "task.completed",
            Self::TaskFailed => "task.failed",
            Self::TaskCancelled => "task.cancelled",
            Self::TaskApprovalRequested => "task.approval_required",
            Self::TaskApprovalGranted => "task.approval_granted",
            Self::VerifyStarted => "verify.started",
            Self::VerifyCompleted => "verify.completed",
            Self::FindingRecorded => "finding.recorded",
            Self::ReplanWritten => "evidence.captured",
            Self::Other(raw) => raw.as_str(),
        }
    }

    /// True for the reader-only `Other` tolerance variant. The writer uses this
    /// to refuse to persist an out-of-taxonomy kind.
    pub fn is_other(&self) -> bool {
        matches!(self, Self::Other(_))
    }

    /// Wire kinds a channel subscription may match this event by. Always the
    /// canonical v2 `as_str()`, plus the legacy *collapsed* v1 wire kind for the
    /// variants F-115 de-collapsed — so a config still subscribing to the old
    /// `task.completed` / `task.cancelled` / `run.failed` keeps receiving the
    /// now-de-collapsed events. The ledger and `OutboundReply` keep the canonical
    /// v2 kind; this is purely a subscription-matching compatibility shim.
    pub fn subscription_keys(&self) -> Vec<&str> {
        let mut keys = vec![self.as_str()];
        let legacy = match self {
            // Before F-115 a failed run was a run.completed event with a Failed
            // payload, so a ["run.completed"] subscription must still see it.
            Self::RunFailed => Some("run.completed"),
            // v1 collapsed both the cancelled run and the cancel request into
            // run.failed; keep matching it so old subscriptions don't miss them.
            Self::RunCancelled | Self::CancelRequested => Some("run.failed"),
            Self::TaskSkipped => Some("task.cancelled"),
            Self::VerifyStarted => Some("task.started"),
            Self::TaskApprovalGranted | Self::VerifyCompleted => Some("task.completed"),
            _ => None,
        };
        if let Some(alias) = legacy {
            keys.push(alias);
        }
        keys
    }

    /// The F-112 7-bucket lifecycle status this kind implies, or `None` for kinds
    /// that are not a bucket transition (verify sub-lifecycle, cancel request,
    /// finding/evidence projection, unknown). `append_event_draft` auto-fills the
    /// event `status` from this unless the producer set one explicitly.
    pub fn default_status(&self) -> Option<RunEventStatus> {
        Some(match self {
            Self::RunCreated => RunEventStatus::Running,
            Self::RunCompleted => RunEventStatus::Done,
            Self::RunFailed => RunEventStatus::Failed,
            Self::RunCancelled => RunEventStatus::Cancelled,
            Self::TaskQueued => RunEventStatus::Pending,
            Self::TaskStarted => RunEventStatus::Running,
            Self::TaskSucceeded => RunEventStatus::Done,
            Self::TaskFailed => RunEventStatus::Failed,
            Self::TaskCancelled => RunEventStatus::Cancelled,
            Self::TaskSkipped => RunEventStatus::Skipped,
            Self::TaskApprovalRequested => RunEventStatus::AwaitingApproval,
            Self::TaskApprovalGranted => RunEventStatus::Running,
            // No bucket transition: verify sub-lifecycle, the cancel request
            // signal, the finding/evidence projections, and unknown kinds.
            Self::VerifyStarted
            | Self::VerifyCompleted
            | Self::CancelRequested
            | Self::FindingRecorded
            | Self::ReplanWritten
            | Self::Other(_) => return None,
        })
    }

    /// Reader. Never fails: a known canonical/legacy string maps to its variant,
    /// anything else is preserved as `Other`. (Corruption is caught earlier, when
    /// the surrounding JSON / required fields fail to deserialize.)
    fn from_wire(value: &str) -> Self {
        match value {
            "run.started" | "run_created" => Self::RunCreated,
            "run.completed" | "run_completed" => Self::RunCompleted,
            "run.failed" => Self::RunFailed,
            "run.cancelled" | "run_cancelled" => Self::RunCancelled,
            // cancel-requested is a control/request event, not a terminal state;
            // `cancel_requested` is the legacy read alias.
            "run.cancel_requested" | "cancel_requested" => Self::CancelRequested,
            // task.queued is its own kind (pending/queued), NOT skipped — a v1
            // reader mis-aliased it; v2 reads it as TaskQueued.
            "task.queued" => Self::TaskQueued,
            "task.skipped" | "task_skipped" => Self::TaskSkipped,
            "task.started" | "task_started" => Self::TaskStarted,
            "task.completed" | "task_succeeded" => Self::TaskSucceeded,
            "task.failed" | "task_failed" => Self::TaskFailed,
            "task.cancelled" | "task_cancelled" => Self::TaskCancelled,
            "task.approval_required" | "task_approval_requested" => Self::TaskApprovalRequested,
            "task.approval_granted" | "task_approval_granted" => Self::TaskApprovalGranted,
            "verify.started" | "verify_started" => Self::VerifyStarted,
            "verify.completed" | "verify_completed" => Self::VerifyCompleted,
            "finding.recorded" => Self::FindingRecorded,
            "evidence.captured" | "pr.drafted" | "replan_written" => Self::ReplanWritten,
            other => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for RunEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for RunEventKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunEventKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_wire(&raw))
    }
}

/// Event lifecycle status — the F-112 7-bucket vocabulary, expressed as a
/// dedicated enum. NOT `scheduler::state::RunStatus` (which only has the four
/// run-level values running/done/failed/cancelled); the 7 buckets come from task
/// progress, so a stream folded into a snapshot agrees with a fresh F-112
/// projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventStatus {
    Pending,
    Running,
    AwaitingApproval,
    Done,
    Failed,
    Cancelled,
    Skipped,
}

/// Small, structured display hint. `label` is a short single-line summary
/// (≤ `MAX_EVENT_MESSAGE_BYTES`), never raw output; large content travels only as
/// a run-relative artifact ref.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventDisplay {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Structured builder for an event with the F-115 v2 fields. Producers that need
/// `status`/`severity`/`display`/`refs` use this via `append_event_draft`; the
/// legacy `append_event` is a thin wrapper over it, so existing callsites keep
/// working. `status` is auto-filled from the kind unless set explicitly.
#[derive(Debug, Clone)]
pub struct RunEventDraft {
    kind: RunEventKind,
    task_id: Option<String>,
    status: Option<RunEventStatus>,
    severity: Option<Severity>,
    message: Option<String>,
    display: Option<RunEventDisplay>,
    payload: Value,
    refs: BTreeMap<String, ArtifactRef>,
}

impl RunEventDraft {
    pub fn new(kind: RunEventKind) -> Self {
        Self {
            kind,
            task_id: None,
            status: None,
            severity: None,
            message: None,
            display: None,
            payload: Value::Null,
            refs: BTreeMap::new(),
        }
    }

    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn status(mut self, status: RunEventStatus) -> Self {
        self.status = Some(status);
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = Some(severity);
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn display(mut self, display: RunEventDisplay) -> Self {
        self.display = Some(display);
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn refs(mut self, refs: BTreeMap<String, ArtifactRef>) -> Self {
        self.refs = refs;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunEvent {
    #[serde(default = "crate::schema::run_event_version")]
    pub schema_version: String,
    pub event_id: String,
    pub run_id: String,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: RunEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    // F-115 v2 additions — all optional, so a v1 line deserializes with these as
    // None and an old reader ignores them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunEventStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<RunEventDisplay>,
    #[serde(default, skip_serializing_if = "value_is_null")]
    pub payload: Value,
    #[serde(default)]
    pub refs: BTreeMap<String, ArtifactRef>,
}

pub fn events_path(run_dir: &Path) -> PathBuf {
    run_dir.join(paths::RUN_EVENTS_FILE)
}

/// Legacy entry point — a thin wrapper over `append_event_draft` so existing
/// callsites keep working. `status` is auto-filled from the kind.
pub fn append_event(
    run_dir: &Path,
    run_id: &str,
    kind: RunEventKind,
    task_id: Option<&str>,
    message: Option<String>,
    payload: Value,
) -> Result<RunEvent> {
    let mut draft = RunEventDraft::new(kind).payload(payload);
    if let Some(task) = task_id {
        draft = draft.task(task);
    }
    if let Some(message) = message {
        draft = draft.message(message);
    }
    append_event_draft(run_dir, run_id, draft)
}

/// Structured append: the core writer for F-115 v2 events. Allocates the next
/// `seq` under the append lock, auto-fills `status` from the kind (unless the
/// draft set one), enforces the write invariants (Other rejected, payload/refs
/// guards, message sanitized, display.label validated), and appends one line.
pub fn append_event_draft(run_dir: &Path, run_id: &str, draft: RunEventDraft) -> Result<RunEvent> {
    paths::ensure_dir(run_dir)?;
    let path = events_path(run_dir);
    let append_lock = event_append_lock(&path)?;
    let _append_guard = append_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("run event append lock poisoned for {}", path.display()))?;
    let seq = read_events(run_dir)?
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
        + 1;
    let status = draft.status.or_else(|| draft.kind.default_status());
    let mut event = RunEvent {
        schema_version: crate::schema::RUN_EVENT_V2.to_string(),
        event_id: format!("{run_id}-{seq}"),
        run_id: run_id.to_string(),
        seq,
        timestamp: Utc::now(),
        kind: draft.kind,
        task_id: draft.task_id,
        status,
        severity: draft.severity,
        message: draft.message,
        display: draft.display,
        payload: draft.payload,
        refs: draft.refs,
    };
    enforce_write_invariants(&mut event)?;
    let mut line = serde_json::to_string(&event).context("serialize run event")?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open run events file {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("append run event {}", path.display()))?;
    Ok(event)
}

/// Best-effort live projection of a durably-written F-110 finding into the event
/// ledger AND (when `channels` is set) the channel outbound queue. Call ONLY after
/// `append_finding` succeeded — a failure here is logged and swallowed (it must
/// never roll back the finding or break the run). The event carries only
/// `severity`, `task_id`, a small `{kind,id,seq}` payload, and a run-relative ref
/// to `findings.ndjson`; never the summary / evidence / body. Off-`RunCtx`
/// producers (liveness/doctor) pass `channels = None`.
pub fn project_finding_event(
    run_dir: &Path,
    finding: &Finding,
    channels: Option<&ChannelConfig>,
    dry_run: bool,
) {
    let mut refs = BTreeMap::new();
    refs.insert(
        "findings".to_string(),
        ArtifactRef {
            kind: "findings".to_string(),
            source: ArtifactSource::Verification,
            task_id: finding.task_id.clone(),
            path: Some(paths::RUN_FINDINGS_FILE.to_string()),
            uri: None,
            name: None,
            bytes: None,
        },
    );
    let mut draft = RunEventDraft::new(RunEventKind::FindingRecorded)
        .severity(finding.severity)
        .payload(serde_json::json!({
            "finding_kind": finding.kind.as_str(),
            "finding_id": finding.finding_id,
            "finding_seq": finding.seq,
        }))
        .refs(refs);
    if let Some(task) = &finding.task_id {
        draft = draft.task(task.clone());
    }
    if let Err(e) =
        append_event_draft_with_subscribe(run_dir, &finding.run_id, draft, channels, dry_run)
    {
        tracing::warn!(
            "could not project finding.recorded event for {}: {e:#}",
            finding.finding_id
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn append_event_with_subscribe(
    run_dir: &Path,
    run_id: &str,
    kind: RunEventKind,
    task_id: Option<&str>,
    message: Option<String>,
    payload: Value,
    channels: Option<&ChannelConfig>,
    dry_run: bool,
) -> Result<RunEvent> {
    let mut draft = RunEventDraft::new(kind).payload(payload);
    if let Some(task) = task_id {
        draft = draft.task(task);
    }
    if let Some(message) = message {
        draft = draft.message(message);
    }
    append_event_draft_with_subscribe(run_dir, run_id, draft, channels, dry_run)
}

/// Structured append that also notifies channel subscribers — the draft-based
/// mirror of `append_event_with_subscribe`, so producers setting
/// `status/severity/display/refs` (e.g. the finding projection) are not cut off
/// from `reply_to_run_events` channels.
pub fn append_event_draft_with_subscribe(
    run_dir: &Path,
    run_id: &str,
    draft: RunEventDraft,
    channels: Option<&ChannelConfig>,
    dry_run: bool,
) -> Result<RunEvent> {
    let event = append_event_draft(run_dir, run_id, draft)?;
    if let Some(config) = channels {
        if let Err(e) = crate::channel::handle_event(run_dir, &event, config, dry_run) {
            tracing::warn!("could not append outbound channel reply: {e:#}");
        }
    }
    Ok(event)
}

pub fn read_events(run_dir: &Path) -> Result<Vec<RunEvent>> {
    let path = events_path(run_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut events = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let event: RunEvent = serde_json::from_str(raw)
            .with_context(|| format!("parse {} line {}", path.display(), idx + 1))?;
        events.push(event);
    }
    Ok(events)
}

fn value_is_null(value: &Value) -> bool {
    value.is_null()
}

/// F-115 write invariants, enforced before an event is appended. Some checks
/// hard-error (the event is refused); `message` is sanitized in place instead, so
/// a long/multiline failure message can never *drop* the event:
/// - `kind == Other`        → hard error (writer set is closed; `Other` is reader-only)
/// - `payload` > 1 KB        → hard error ("store large data in refs")
/// - `refs` absolute / `..`  → hard error (only run-relative artifact refs)
/// - `display.label` bad     → hard error (new field, no legacy producer)
/// - `message`               → lossy-sanitized (single-line, ≤256 B)
fn enforce_write_invariants(event: &mut RunEvent) -> Result<()> {
    anyhow::ensure!(
        !event.kind.is_other(),
        "refusing to write reader-only Other run event kind {:?}",
        event.kind.as_str()
    );
    let payload_bytes = serde_json::to_vec(&event.payload)
        .context("measure run event payload")?
        .len();
    anyhow::ensure!(
        payload_bytes <= MAX_EVENT_PAYLOAD_BYTES,
        "run event payload exceeds {MAX_EVENT_PAYLOAD_BYTES} bytes; store large data in refs"
    );
    validate_artifact_refs(&event.refs)?;
    if let Some(display) = &event.display {
        validate_single_line_capped("display.label", &display.label)?;
    }
    if let Some(message) = &event.message {
        let sanitized = sanitize_message(message);
        event.message = Some(sanitized);
    }
    Ok(())
}

/// Lossy: fold `\n`/`\r` to spaces and truncate to `MAX_EVENT_MESSAGE_BYTES` on a
/// UTF-8 boundary. Used for `message` (F-115 option B) so the on-disk value is
/// always single-line and bounded without ever dropping the event.
fn sanitize_message(raw: &str) -> String {
    let single_line: String = raw
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    truncate_on_char_boundary(&single_line, MAX_EVENT_MESSAGE_BYTES)
}

fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Strict single-line + length validator. This is the canonical reject semantics
/// for short text fields; `display.label` uses it now, and Step 2 may switch
/// `message` from sanitize to this hard reject once producers only pass short,
/// single-line summaries.
fn validate_single_line_capped(field: &str, value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.contains(['\n', '\r']),
        "{field} must be single-line (no newline characters)"
    );
    anyhow::ensure!(
        value.len() <= MAX_EVENT_MESSAGE_BYTES,
        "{field} exceeds {MAX_EVENT_MESSAGE_BYTES} bytes; store large content in refs"
    );
    Ok(())
}

/// Artifact refs must stay run-local. `path` must be run-relative (no absolute /
/// UNC / Windows drive-letter / `..`); `uri` must not reference the local
/// filesystem via a `file:` scheme (external PR/platform links over https or an
/// opaque channel scheme are fine).
fn validate_artifact_refs(refs: &BTreeMap<String, ArtifactRef>) -> Result<()> {
    for (key, artifact) in refs {
        if let Some(path) = &artifact.path {
            anyhow::ensure!(
                !ref_path_is_unsafe(path),
                "artifact ref {key:?} path must be run-relative \
                 (no absolute / UNC / drive-letter / '..'): {path:?}"
            );
        }
        if let Some(uri) = &artifact.uri {
            anyhow::ensure!(
                !ref_uri_is_unsafe(uri),
                "artifact ref {key:?} uri must not use a file: scheme: {uri:?}"
            );
        }
    }
    Ok(())
}

/// A run-relative path: reject Unix-absolute, a leading slash/backslash (covers
/// UNC `\\server` / `//server`), a Windows drive-letter root (`C:\`, `C:/`), and
/// any `..` traversal component.
fn ref_path_is_unsafe(path: &str) -> bool {
    if Path::new(path).is_absolute() {
        return true;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    if path.split(['/', '\\']).any(|component| component == "..") {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

/// Reject `file:` URIs (any case) — a run-local artifact uses `path`, never a
/// filesystem URI; only remote/opaque schemes belong in `uri`.
fn ref_uri_is_unsafe(uri: &str) -> bool {
    uri.trim_start().to_ascii_lowercase().starts_with("file:")
}

/// Pure read-side projection of a run's event ledger (F-115). No filesystem, no
/// clock, no transport — it sorts by `seq`, filters to `seq > since_seq`, and is
/// the unit-tested core a future SSE endpoint serializes. `next_seq` is the
/// highest `seq` present (the cursor a client passes back as `since_seq`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventStream {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
    pub next_seq: u64,
    pub events: Vec<RunEvent>,
}

impl RunEventStream {
    /// Build a stream view from a slice of events. `since_seq = None` returns all
    /// events; `Some(n)` returns only events with `seq > n`. Events are sorted by
    /// `seq` defensively (the ledger is already append-ordered).
    pub fn from_events(events: &[RunEvent], since_seq: Option<u64>) -> Self {
        let run_id = events.first().map(|e| e.run_id.clone()).unwrap_or_default();
        let mut filtered: Vec<RunEvent> = events
            .iter()
            .filter(|e| since_seq.is_none_or(|cursor| e.seq > cursor))
            .cloned()
            .collect();
        filtered.sort_by_key(|e| e.seq);
        let next_seq = events.iter().map(|e| e.seq).max().unwrap_or(0);
        Self {
            run_id,
            since_seq,
            next_seq,
            events: filtered,
        }
    }
}

fn event_append_lock(path: &Path) -> Result<Arc<Mutex<()>>> {
    let mut locks = EVENT_APPEND_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("run event append lock registry poisoned"))?;
    Ok(locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Barrier};

    use chrono::{TimeZone, Utc};

    use super::{
        append_event, append_event_draft, append_event_with_subscribe, enforce_write_invariants,
        project_finding_event, read_events, sanitize_message, validate_artifact_refs,
        validate_single_line_capped, RunEvent, RunEventDisplay, RunEventDraft, RunEventKind,
        RunEventStatus, RunEventStream,
    };
    use crate::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};
    use crate::scheduler::findings::{append_finding, Finding, FindingKind, Severity};
    use crate::schema::artifacts::{ArtifactRef, ArtifactSource};
    use crate::schema::channel_envelope::{ChannelAction, ChannelEnvelope};

    #[test]
    fn append_event_with_subscribe_writes_event_like_append_event() {
        let temp = tempfile::tempdir().unwrap();

        let event = append_event_with_subscribe(
            temp.path(),
            "run-1",
            RunEventKind::RunCreated,
            None,
            Some("started".to_string()),
            serde_json::json!({ "task_count": 1 }),
            None,
            false,
        )
        .unwrap();

        assert_eq!(event.schema_version, crate::schema::RUN_EVENT_V2);
        assert_eq!(event.event_id, "run-1-1");
        assert_eq!(event.run_id, "run-1");
        assert_eq!(event.seq, 1);
        assert_eq!(event.kind, RunEventKind::RunCreated);
        assert_eq!(event.task_id, None);
        assert_eq!(event.message.as_deref(), Some("started"));
        assert_eq!(event.payload, serde_json::json!({ "task_count": 1 }));
        assert!(event.refs.is_empty());

        let events = read_events(temp.path()).unwrap();
        assert_eq!(events, vec![event.clone()]);

        let raw = std::fs::read_to_string(temp.path().join(crate::paths::RUN_EVENTS_FILE)).unwrap();
        let decoded: RunEvent = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn append_event_with_subscribe_invokes_handle_event_when_channels_present() {
        let temp = tempfile::tempdir().unwrap();
        crate::channel::persist(temp.path(), &origin_envelope()).unwrap();

        append_event_with_subscribe(
            temp.path(),
            "run-1",
            RunEventKind::RunCreated,
            None,
            Some("started".to_string()),
            serde_json::json!({}),
            Some(&channel_config(&["run.started"])),
            false,
        )
        .unwrap();

        let raw = std::fs::read_to_string(temp.path().join(crate::channel::OUTBOUND_REPLIES_FILE))
            .unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        let reply: crate::channel::OutboundReply = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(reply.channel, "feishu");
        assert_eq!(reply.run_id, "run-1");
        assert_eq!(reply.event_kind, "run.started");
    }

    #[test]
    fn append_event_with_subscribe_is_noop_when_channels_absent() {
        let temp = tempfile::tempdir().unwrap();
        crate::channel::persist(temp.path(), &origin_envelope()).unwrap();

        append_event_with_subscribe(
            temp.path(),
            "run-1",
            RunEventKind::RunCreated,
            None,
            None,
            serde_json::json!({}),
            None,
            false,
        )
        .unwrap();

        assert!(!temp
            .path()
            .join(crate::channel::OUTBOUND_REPLIES_FILE)
            .exists());
        assert_eq!(read_events(temp.path()).unwrap().len(), 1);
    }

    #[test]
    fn append_event_with_subscribe_swallows_handle_event_errors_when_origin_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(crate::channel::ENVELOPES_FILE),
            "not json\n",
        )
        .unwrap();

        let event = append_event_with_subscribe(
            temp.path(),
            "run-1",
            RunEventKind::RunCreated,
            None,
            None,
            serde_json::json!({}),
            Some(&channel_config(&["run.started"])),
            false,
        )
        .unwrap();

        assert_eq!(event.seq, 1);
        assert_eq!(read_events(temp.path()).unwrap(), vec![event]);
        assert!(!temp
            .path()
            .join(crate::channel::OUTBOUND_REPLIES_FILE)
            .exists());
    }

    #[test]
    fn append_event_concurrent_writes_are_parseable_with_unique_sequence_numbers() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = Arc::new(temp.path().to_path_buf());
        let thread_count = 8;
        let events_per_thread = 50;
        let barrier = Arc::new(Barrier::new(thread_count));
        let mut handles = Vec::new();

        for idx in 0..thread_count {
            let run_dir = Arc::clone(&run_dir);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for iter in 0..events_per_thread {
                    super::append_event(
                        &run_dir,
                        "run-1",
                        RunEventKind::TaskStarted,
                        Some(&format!("task-{idx}-{iter}")),
                        None,
                        serde_json::json!({ "idx": idx, "iter": iter }),
                    )
                    .unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let events = read_events(temp.path()).unwrap();
        let expected_len = thread_count * events_per_thread;
        assert_eq!(events.len(), expected_len);
        let seqs = events.iter().map(|event| event.seq).collect::<Vec<_>>();
        assert_eq!(
            seqs,
            (1..=expected_len as u64).collect::<Vec<_>>(),
            "event seqs must be unique and strictly increasing"
        );
    }

    fn channel_config(reply_to_run_events: &[&str]) -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: None,
                allowed_senders: Vec::new(),
                allowed_actions: Vec::new(),
                reply_to_run_events: reply_to_run_events
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
                redact_payload_over_kb: 1,
            },
        );
        ChannelConfig {
            version: 1,
            defaults: ChannelDefaults::default(),
            channels,
        }
    }

    fn origin_envelope() -> ChannelEnvelope {
        ChannelEnvelope {
            schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
            channel: "feishu".to_string(),
            thread_id: "thread-1".to_string(),
            sender_id: "ou_sender".to_string(),
            sender_trusted: true,
            dry_run: false,
            message: "run --run".to_string(),
            attachments: Vec::new(),
            action: Some(ChannelAction::Run),
            run_id: Some("run-1".to_string()),
            created_at: Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
        }
    }

    // ---- F-115 Step 1: schema / compat reader / guards / projector ----

    fn sample_event_seq(kind: RunEventKind, seq: u64) -> RunEvent {
        RunEvent {
            schema_version: crate::schema::RUN_EVENT_V2.to_string(),
            event_id: format!("run-1-{seq}"),
            run_id: "run-1".to_string(),
            seq,
            timestamp: Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
            kind,
            task_id: None,
            status: None,
            severity: None,
            message: None,
            display: None,
            payload: serde_json::Value::Null,
            refs: BTreeMap::new(),
        }
    }

    fn ref_with_path(path: &str) -> BTreeMap<String, ArtifactRef> {
        let mut refs = BTreeMap::new();
        refs.insert(
            "log".to_string(),
            ArtifactRef {
                kind: "log".to_string(),
                source: ArtifactSource::AgentTask,
                task_id: None,
                path: Some(path.to_string()),
                uri: None,
                name: None,
                bytes: None,
            },
        );
        refs
    }

    fn ref_with_uri(uri: &str) -> BTreeMap<String, ArtifactRef> {
        let mut refs = BTreeMap::new();
        refs.insert(
            "link".to_string(),
            ArtifactRef {
                kind: "link".to_string(),
                source: ArtifactSource::External,
                task_id: None,
                path: None,
                uri: Some(uri.to_string()),
                name: None,
                bytes: None,
            },
        );
        refs
    }

    #[test]
    fn v1_line_reads_under_v2_with_defaulted_fields() {
        let line = r#"{"schema_version":"maestro.run_event.v1","event_id":"r-1","run_id":"r","seq":1,"timestamp":"2026-05-24T08:00:00Z","kind":"task.completed"}"#;
        let ev: RunEvent = serde_json::from_str(line).unwrap();
        assert_eq!(ev.schema_version, crate::schema::RUN_EVENT_V1);
        assert_eq!(ev.kind, RunEventKind::TaskSucceeded);
        assert!(ev.status.is_none());
        assert!(ev.severity.is_none());
        assert!(ev.display.is_none());
        assert!(ev.message.is_none());
    }

    #[test]
    fn v2_event_with_new_fields_round_trips() {
        let mut ev = sample_event_seq(RunEventKind::FindingRecorded, 7);
        ev.status = Some(RunEventStatus::AwaitingApproval);
        ev.severity = Some(Severity::High);
        ev.display = Some(RunEventDisplay {
            label: "needs approval".to_string(),
            tone: Some("warn".to_string()),
            icon: None,
        });
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"status\":\"awaiting_approval\""));
        assert!(json.contains("\"severity\":\"high\""));
        let back: RunEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn new_v2_kinds_round_trip_and_evidence_stays_canonical() {
        let cases = [
            (RunEventKind::RunCancelled, "run.cancelled"),
            (RunEventKind::TaskSkipped, "task.skipped"),
            (RunEventKind::TaskApprovalGranted, "task.approval_granted"),
            (RunEventKind::VerifyStarted, "verify.started"),
            (RunEventKind::VerifyCompleted, "verify.completed"),
            (RunEventKind::FindingRecorded, "finding.recorded"),
            // canonical wire kind preserved (NOT renamed to artifact.captured)
            (RunEventKind::ReplanWritten, "evidence.captured"),
        ];
        for (kind, wire) in cases {
            assert_eq!(kind.as_str(), wire);
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            let back: RunEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn unknown_kind_reads_as_other_and_round_trips() {
        let line = r#"{"schema_version":"maestro.run_event.v2","event_id":"r-1","run_id":"r","seq":1,"timestamp":"2026-05-24T08:00:00Z","kind":"usage.sampled"}"#;
        let ev: RunEvent = serde_json::from_str(line).unwrap();
        assert_eq!(ev.kind, RunEventKind::Other("usage.sampled".to_string()));
        assert!(ev.kind.is_other());
        assert_eq!(ev.kind.as_str(), "usage.sampled");
        // round-trips back to the same wire string
        let json = serde_json::to_string(&ev.kind).unwrap();
        assert_eq!(json, "\"usage.sampled\"");
    }

    #[test]
    fn corrupt_line_is_hard_error_not_other() {
        // malformed JSON
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(crate::paths::RUN_EVENTS_FILE),
            "{not valid json\n",
        )
        .unwrap();
        assert!(read_events(temp.path()).is_err());
        // structurally valid JSON but missing required fields (run_id/seq/...)
        let temp2 = tempfile::tempdir().unwrap();
        std::fs::write(
            temp2.path().join(crate::paths::RUN_EVENTS_FILE),
            r#"{"schema_version":"maestro.run_event.v2","kind":"run.started"}"#,
        )
        .unwrap();
        assert!(read_events(temp2.path()).is_err());
    }

    #[test]
    fn writer_rejects_other_kind_and_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let err = append_event(
            temp.path(),
            "run-1",
            RunEventKind::Other("custom.kind".to_string()),
            None,
            None,
            serde_json::json!({}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Other"), "got: {err}");
        assert!(read_events(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn message_is_sanitized_single_line_capped_without_dropping_event() {
        let temp = tempfile::tempdir().unwrap();
        let multiline_long = format!("err line1\nline2\r\ncause: {}", "x".repeat(400));
        let ev = append_event(
            temp.path(),
            "run-1",
            RunEventKind::TaskFailed,
            Some("t1"),
            Some(multiline_long),
            serde_json::json!({}),
        )
        .unwrap();
        let msg = ev.message.clone().unwrap();
        assert!(!msg.contains('\n') && !msg.contains('\r'), "single line");
        assert!(
            msg.len() <= super::MAX_EVENT_MESSAGE_BYTES,
            "len {}",
            msg.len()
        );
        // the failure event is NOT dropped
        let persisted = read_events(temp.path()).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].message.as_deref(), Some(msg.as_str()));
    }

    #[test]
    fn sanitize_message_folds_newlines_and_truncates_on_char_boundary() {
        assert_eq!(sanitize_message("a\nb\r\nc"), "a b  c");
        // multibyte truncation never splits a char
        let s = "é".repeat(200); // 400 bytes
        let out = sanitize_message(&s);
        assert!(out.len() <= super::MAX_EVENT_MESSAGE_BYTES);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn display_label_strict_validator_rejects_multiline_and_oversize() {
        assert!(validate_single_line_capped("display.label", "ok label").is_ok());
        assert!(validate_single_line_capped("display.label", "bad\nlabel").is_err());
        assert!(validate_single_line_capped("display.label", "bad\rlabel").is_err());
        assert!(validate_single_line_capped("display.label", &"x".repeat(257)).is_err());
        // and through the write-path enforcer (hard reject, new field)
        let mut ev = sample_event_seq(RunEventKind::TaskStarted, 1);
        ev.display = Some(RunEventDisplay {
            label: "x".repeat(300),
            tone: None,
            icon: None,
        });
        assert!(enforce_write_invariants(&mut ev).is_err());
    }

    #[test]
    fn refs_reject_absolute_and_traversal_accept_run_relative() {
        assert!(validate_artifact_refs(&ref_with_path("/etc/passwd")).is_err());
        assert!(validate_artifact_refs(&ref_with_path("../escape/x.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_path("logs/t1.log")).is_ok());
        assert!(validate_artifact_refs(&ref_with_path("trajectories/t1.ndjson")).is_ok());
    }

    #[test]
    fn refs_reject_windows_unc_paths_and_file_uris() {
        // Windows drive-letter + UNC roots (not caught by unix is_absolute)
        assert!(validate_artifact_refs(&ref_with_path(r"C:\data\a.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_path("C:/data/a.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_path(r"\\server\share\a.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_path("//server/share/a.log")).is_err());
        // file: URIs rejected (any case); remote / opaque URIs allowed
        assert!(validate_artifact_refs(&ref_with_uri("file:///opt/a.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_uri("file://logs/a.log")).is_err());
        assert!(validate_artifact_refs(&ref_with_uri("FILE:///x")).is_err());
        assert!(validate_artifact_refs(&ref_with_uri("https://example.invalid/pr/1")).is_ok());
        assert!(validate_artifact_refs(&ref_with_uri("botmux:thread/42")).is_ok());
    }

    #[test]
    fn task_queued_reads_as_its_own_kind_not_skipped() {
        let line = r#"{"schema_version":"maestro.run_event.v2","event_id":"r-1","run_id":"r","seq":1,"timestamp":"2026-05-24T08:00:00Z","kind":"task.queued"}"#;
        let ev: RunEvent = serde_json::from_str(line).unwrap();
        assert_eq!(ev.kind, RunEventKind::TaskQueued);
        assert_ne!(ev.kind, RunEventKind::TaskSkipped);
        assert_eq!(ev.kind.as_str(), "task.queued");
        assert_eq!(
            serde_json::to_string(&RunEventKind::TaskQueued).unwrap(),
            "\"task.queued\""
        );
    }

    #[test]
    fn subscription_keys_include_legacy_collapsed_aliases() {
        assert_eq!(
            RunEventKind::RunCancelled.subscription_keys(),
            vec!["run.cancelled", "run.failed"]
        );
        assert_eq!(
            RunEventKind::TaskSkipped.subscription_keys(),
            vec!["task.skipped", "task.cancelled"]
        );
        assert_eq!(
            RunEventKind::VerifyStarted.subscription_keys(),
            vec!["verify.started", "task.started"]
        );
        assert_eq!(
            RunEventKind::TaskApprovalGranted.subscription_keys(),
            vec!["task.approval_granted", "task.completed"]
        );
        assert_eq!(
            RunEventKind::VerifyCompleted.subscription_keys(),
            vec!["verify.completed", "task.completed"]
        );
        // kinds that were never collapsed expose only the canonical key
        assert_eq!(
            RunEventKind::TaskFailed.subscription_keys(),
            vec!["task.failed"]
        );
        assert_eq!(
            RunEventKind::TaskQueued.subscription_keys(),
            vec!["task.queued"]
        );
    }

    #[test]
    fn payload_over_cap_is_hard_rejected() {
        let mut ev = sample_event_seq(RunEventKind::TaskStarted, 1);
        ev.payload = serde_json::json!({ "blob": "x".repeat(2048) });
        let err = enforce_write_invariants(&mut ev).unwrap_err();
        assert!(err.to_string().contains("payload exceeds"), "got: {err}");
    }

    #[test]
    fn projector_filters_since_seq_and_orders_deterministically() {
        let events = vec![
            sample_event_seq(RunEventKind::RunCreated, 1),
            sample_event_seq(RunEventKind::TaskStarted, 3),
            sample_event_seq(RunEventKind::TaskSucceeded, 2),
        ];
        let all = RunEventStream::from_events(&events, None);
        assert_eq!(
            all.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(all.next_seq, 3);
        assert_eq!(all.run_id, "run-1");

        let since = RunEventStream::from_events(&events, Some(2));
        assert_eq!(
            since.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(since.since_seq, Some(2));
        assert_eq!(since.next_seq, 3);

        let empty = RunEventStream::from_events(&[], None);
        assert_eq!(empty.next_seq, 0);
        assert!(empty.events.is_empty());
        assert_eq!(empty.run_id, "");
    }

    #[test]
    fn run_event_status_serializes_to_the_seven_buckets() {
        let pairs = [
            (RunEventStatus::Pending, "pending"),
            (RunEventStatus::Running, "running"),
            (RunEventStatus::AwaitingApproval, "awaiting_approval"),
            (RunEventStatus::Done, "done"),
            (RunEventStatus::Failed, "failed"),
            (RunEventStatus::Cancelled, "cancelled"),
            (RunEventStatus::Skipped, "skipped"),
        ];
        for (status, wire) in pairs {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{wire}\"")
            );
        }
    }

    // ---- F-115 Step 2: producer status fill + finding projection ----

    #[test]
    fn run_terminal_canonical_wire_kinds_are_distinct() {
        assert_eq!(RunEventKind::RunFailed.as_str(), "run.failed");
        assert_eq!(
            RunEventKind::CancelRequested.as_str(),
            "run.cancel_requested"
        );
        assert_eq!(RunEventKind::RunCancelled.as_str(), "run.cancelled");
        let distinct: std::collections::HashSet<&str> = [
            RunEventKind::RunFailed.as_str(),
            RunEventKind::CancelRequested.as_str(),
            RunEventKind::RunCancelled.as_str(),
        ]
        .into_iter()
        .collect();
        assert_eq!(distinct.len(), 3);
        assert_eq!(
            serde_json::from_str::<RunEventKind>("\"run.failed\"").unwrap(),
            RunEventKind::RunFailed
        );
        assert_eq!(
            serde_json::from_str::<RunEventKind>("\"run.cancel_requested\"").unwrap(),
            RunEventKind::CancelRequested
        );
        assert_eq!(
            serde_json::from_str::<RunEventKind>("\"cancel_requested\"").unwrap(),
            RunEventKind::CancelRequested
        );
    }

    #[test]
    fn default_status_maps_lifecycle_to_seven_buckets() {
        use RunEventStatus::*;
        let cases = [
            (RunEventKind::RunCreated, Some(Running)),
            (RunEventKind::RunCompleted, Some(Done)),
            (RunEventKind::RunFailed, Some(Failed)),
            (RunEventKind::RunCancelled, Some(Cancelled)),
            (RunEventKind::TaskQueued, Some(Pending)),
            (RunEventKind::TaskStarted, Some(Running)),
            (RunEventKind::TaskSucceeded, Some(Done)),
            (RunEventKind::TaskFailed, Some(Failed)),
            (RunEventKind::TaskCancelled, Some(Cancelled)),
            (RunEventKind::TaskSkipped, Some(Skipped)),
            (RunEventKind::TaskApprovalRequested, Some(AwaitingApproval)),
            (RunEventKind::TaskApprovalGranted, Some(Running)),
            (RunEventKind::VerifyStarted, None),
            (RunEventKind::VerifyCompleted, None),
            (RunEventKind::CancelRequested, None),
            (RunEventKind::FindingRecorded, None),
            (RunEventKind::ReplanWritten, None),
        ];
        for (kind, want) in cases {
            assert_eq!(kind.default_status(), want, "kind {}", kind.as_str());
        }
    }

    #[test]
    fn append_autofills_lifecycle_status_and_verify_omits_it() {
        let temp = tempfile::tempdir().unwrap();
        let started = append_event(
            temp.path(),
            "run-1",
            RunEventKind::TaskStarted,
            Some("t1"),
            None,
            serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(started.status, Some(RunEventStatus::Running));
        let verify = append_event(
            temp.path(),
            "run-1",
            RunEventKind::VerifyStarted,
            Some("t1"),
            None,
            serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(verify.status, None);
        let overridden = append_event_draft(
            temp.path(),
            "run-1",
            RunEventDraft::new(RunEventKind::TaskStarted)
                .task("t2")
                .status(RunEventStatus::Pending),
        )
        .unwrap();
        assert_eq!(overridden.status, Some(RunEventStatus::Pending));
    }

    #[test]
    fn project_finding_event_writes_minimal_projection_without_body() {
        let temp = tempfile::tempdir().unwrap();
        let finding = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::High,
            "risk-gate",
            "VERYSECRET summary that must not leak into the event",
            "2026-05-24T08:00:00Z",
        )
        .task("t1");
        let written = append_finding(temp.path(), finding).unwrap();

        project_finding_event(temp.path(), &written, None, false);

        let events = read_events(temp.path()).unwrap();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.kind, RunEventKind::FindingRecorded);
        assert_eq!(ev.severity, Some(Severity::High));
        assert_eq!(ev.task_id.as_deref(), Some("t1"));
        assert_eq!(ev.status, None);
        let payload = ev.payload.to_string();
        assert!(payload.contains("finding_kind"));
        assert!(payload.contains(&written.finding_id));
        assert!(
            !payload.contains("VERYSECRET"),
            "no finding body in payload"
        );
        assert!(
            ev.message
                .as_deref()
                .map_or(true, |m| !m.contains("VERYSECRET")),
            "no finding body in message"
        );
        let r = ev.refs.get("findings").expect("findings ref");
        assert_eq!(r.path.as_deref(), Some("findings.ndjson"));
    }

    #[test]
    fn no_finding_projection_when_durable_write_fails() {
        let temp = tempfile::tempdir().unwrap();
        let bad = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::High,
            "risk-gate",
            "x",
            "2026-05-24T08:00:00Z",
        )
        .confidence(2.0);
        match append_finding(temp.path(), bad) {
            Ok(written) => project_finding_event(temp.path(), &written, None, false),
            Err(_) => {}
        }
        let events = read_events(temp.path()).unwrap();
        assert!(events
            .iter()
            .all(|e| e.kind != RunEventKind::FindingRecorded));
    }

    #[test]
    fn run_failed_subscription_keys_include_legacy_run_completed() {
        // N1: failed runs used to be run.completed; keep matching that subscription.
        assert_eq!(
            RunEventKind::RunFailed.subscription_keys(),
            vec!["run.failed", "run.completed"]
        );
    }

    #[test]
    fn finding_projection_reaches_channel_subscribers_without_body() {
        // N2: a structured (draft) event must still reach reply_to_run_events
        // channels — finding.recorded should produce an outbound reply.
        let temp = tempfile::tempdir().unwrap();
        crate::channel::persist(temp.path(), &origin_envelope()).unwrap();
        let finding = Finding::new(
            "run-1",
            FindingKind::Risk,
            Severity::High,
            "risk-gate",
            "VERYSECRET summary must not reach the channel",
            "2026-05-24T08:00:00Z",
        )
        .task("t1");
        let written = append_finding(temp.path(), finding).unwrap();

        project_finding_event(
            temp.path(),
            &written,
            Some(&channel_config(&["finding.recorded"])),
            false,
        );

        let raw = std::fs::read_to_string(temp.path().join(crate::channel::OUTBOUND_REPLIES_FILE))
            .unwrap();
        let lines: Vec<_> = raw.lines().collect();
        assert_eq!(lines.len(), 1);
        let reply: crate::channel::OutboundReply = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(reply.event_kind, "finding.recorded");
        assert!(
            !reply.body.contains("VERYSECRET"),
            "no finding body in the channel reply"
        );
    }
}
