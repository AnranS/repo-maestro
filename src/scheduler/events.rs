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
use crate::schema::artifacts::ArtifactRef;

const MAX_EVENT_PAYLOAD_BYTES: usize = 1024;

static EVENT_APPEND_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEventKind {
    RunCreated,
    RunCompleted,
    RunCancelled,
    CancelRequested,
    TaskSkipped,
    TaskStarted,
    TaskSucceeded,
    TaskFailed,
    TaskCancelled,
    TaskApprovalRequested,
    TaskApprovalGranted,
    VerifyStarted,
    VerifyCompleted,
    ReplanWritten,
}

impl RunEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunCreated => "run.started",
            Self::RunCompleted => "run.completed",
            Self::RunCancelled | Self::CancelRequested => "run.failed",
            Self::TaskSkipped | Self::TaskCancelled => "task.cancelled",
            Self::TaskStarted | Self::VerifyStarted => "task.started",
            Self::TaskSucceeded | Self::TaskApprovalGranted | Self::VerifyCompleted => {
                "task.completed"
            }
            Self::TaskFailed => "task.failed",
            Self::TaskApprovalRequested => "task.approval_required",
            Self::ReplanWritten => "evidence.captured",
        }
    }

    fn from_wire(value: &str) -> Option<Self> {
        Some(match value {
            "run.started" | "run_created" => Self::RunCreated,
            "run.completed" | "run_completed" => Self::RunCompleted,
            "run.failed" | "run_cancelled" => Self::RunCancelled,
            "cancel_requested" => Self::CancelRequested,
            "task.queued" | "task_skipped" => Self::TaskSkipped,
            "task.started" | "task_started" => Self::TaskStarted,
            "task.completed" | "task_succeeded" => Self::TaskSucceeded,
            "task.failed" | "task_failed" => Self::TaskFailed,
            "task.cancelled" | "task_cancelled" => Self::TaskCancelled,
            "task.approval_required" | "task_approval_requested" => Self::TaskApprovalRequested,
            "task_approval_granted" => Self::TaskApprovalGranted,
            "verify_started" => Self::VerifyStarted,
            "verify_completed" => Self::VerifyCompleted,
            "evidence.captured" | "pr.drafted" | "replan_written" => Self::ReplanWritten,
            _ => return None,
        })
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
        Self::from_wire(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown run event kind {raw:?}")))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "value_is_null")]
    pub payload: Value,
    #[serde(default)]
    pub refs: BTreeMap<String, ArtifactRef>,
}

pub fn events_path(run_dir: &Path) -> PathBuf {
    run_dir.join(paths::RUN_EVENTS_FILE)
}

pub fn append_event(
    run_dir: &Path,
    run_id: &str,
    kind: RunEventKind,
    task_id: Option<&str>,
    message: Option<String>,
    payload: Value,
) -> Result<RunEvent> {
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
    let payload_bytes = serde_json::to_vec(&payload)
        .context("measure run event payload")?
        .len();
    anyhow::ensure!(
        payload_bytes <= MAX_EVENT_PAYLOAD_BYTES,
        "run event payload exceeds {MAX_EVENT_PAYLOAD_BYTES} bytes; store large data in refs"
    );
    let event = RunEvent {
        schema_version: crate::schema::RUN_EVENT_V1.to_string(),
        event_id: format!("{run_id}-{seq}"),
        run_id: run_id.to_string(),
        seq,
        timestamp: Utc::now(),
        kind,
        task_id: task_id.map(ToOwned::to_owned),
        message,
        payload,
        refs: BTreeMap::new(),
    };
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
    let event = append_event(run_dir, run_id, kind, task_id, message, payload)?;
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

    use super::{append_event_with_subscribe, read_events, RunEvent, RunEventKind};
    use crate::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};
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

        assert_eq!(event.schema_version, crate::schema::RUN_EVENT_V1);
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
}
