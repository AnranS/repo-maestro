use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::channels::ChannelConfig;
use crate::scheduler::events::RunEvent;

use super::origin;
use super::outbound::{format_event, FormatOptions, OutboundReply};

pub const OUTBOUND_REPLIES_FILE: &str = "outbound_replies.ndjson";

#[derive(Debug, thiserror::Error)]
pub enum SubscribeError {
    #[error(transparent)]
    Origin(#[from] origin::OriginError),
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("serialize reply: {0}")]
    Serialize(#[source] serde_json::Error),
}

pub fn handle_event(
    run_dir: &Path,
    event: &RunEvent,
    config: &ChannelConfig,
    dry_run: bool,
) -> Result<Option<OutboundReply>, SubscribeError> {
    let Some(envelope) = origin::resolve(run_dir)? else {
        return Ok(None);
    };
    let Some(entry) = config.channels.get(&envelope.channel) else {
        return Ok(None);
    };
    let Some(reply) = format_event(event, &envelope.channel, entry, FormatOptions { dry_run })
    else {
        return Ok(None);
    };
    append_reply(run_dir, &reply)?;
    Ok(Some(reply))
}

fn append_reply(run_dir: &Path, reply: &OutboundReply) -> Result<(), SubscribeError> {
    let path = outbound_replies_path(run_dir);
    let line = serde_json::to_string(reply).map_err(SubscribeError::Serialize)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| SubscribeError::Io {
            path: path.display().to_string(),
            source,
        })?;
    writeln!(file, "{line}").map_err(|source| SubscribeError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn outbound_replies_path(run_dir: &Path) -> PathBuf {
    run_dir.join(OUTBOUND_REPLIES_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    use crate::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};
    use crate::scheduler::events::{RunEvent, RunEventKind};
    use crate::schema::channel_envelope::{ChannelAction, ChannelEnvelope};

    #[test]
    fn outbound_replies_file_constant_is_stable() {
        assert_eq!(super::OUTBOUND_REPLIES_FILE, "outbound_replies.ndjson");
    }

    #[test]
    fn handle_event_returns_none_when_no_origin_envelope() {
        let temp = tempfile::tempdir().unwrap();
        let reply = handle(&temp, RunEventKind::RunCreated, &config(), false).unwrap();
        assert_eq!(reply, None);
        assert!(!temp.path().join(super::OUTBOUND_REPLIES_FILE).exists());
    }

    #[test]
    fn handle_event_returns_none_when_channel_missing_from_config() {
        let temp = tempfile::tempdir().unwrap();
        super::origin::persist(temp.path(), &envelope()).unwrap();
        let reply = handle(&temp, RunEventKind::RunCreated, &config(), false).unwrap();
        assert_eq!(reply, None);
        assert!(!temp.path().join(super::OUTBOUND_REPLIES_FILE).exists());
    }

    #[test]
    fn handle_event_returns_none_when_event_kind_not_in_reply_to_run_events() {
        let temp = tempfile::tempdir().unwrap();
        super::origin::persist(temp.path(), &envelope()).unwrap();
        let reply = handle(
            &temp,
            RunEventKind::TaskStarted,
            &config_with_events(&["run.started"]),
            false,
        )
        .unwrap();
        assert_eq!(reply, None);
        assert!(!temp.path().join(super::OUTBOUND_REPLIES_FILE).exists());
    }

    #[test]
    fn handle_event_writes_reply_when_event_is_whitelisted() {
        let temp = tempfile::tempdir().unwrap();
        super::origin::persist(temp.path(), &envelope()).unwrap();
        let reply = handle(
            &temp,
            RunEventKind::RunCreated,
            &config_with_events(&["run.started"]),
            false,
        )
        .unwrap()
        .expect("reply should be produced");
        assert_eq!(reply.event_kind, "run.started");
        let raw = std::fs::read_to_string(temp.path().join(super::OUTBOUND_REPLIES_FILE)).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec![serde_json::to_string(&reply).unwrap()]);
    }

    #[test]
    fn handle_event_propagates_dry_run_to_format_options() {
        let temp = tempfile::tempdir().unwrap();
        super::origin::persist(temp.path(), &envelope()).unwrap();

        let reply = handle(
            &temp,
            RunEventKind::RunCreated,
            &config_with_events(&["run.started"]),
            true,
        )
        .unwrap()
        .expect("reply should be produced");

        assert!(reply.title.starts_with("[DRY] "));
    }

    fn handle(
        temp: &tempfile::TempDir,
        kind: RunEventKind,
        config: &ChannelConfig,
        dry_run: bool,
    ) -> Result<Option<super::OutboundReply>, super::SubscribeError> {
        super::handle_event(temp.path(), &event(kind), config, dry_run)
    }

    #[test]
    fn handle_event_appends_subsequent_lines() {
        let temp = tempfile::tempdir().unwrap();
        super::origin::persist(temp.path(), &envelope()).unwrap();
        let first = event_with_seq(RunEventKind::RunCreated, 1);
        let second = event_with_seq(RunEventKind::RunCompleted, 2);

        super::handle_event(
            temp.path(),
            &first,
            &config_with_events(&["run.started", "run.completed"]),
            false,
        )
        .unwrap();
        super::handle_event(
            temp.path(),
            &second,
            &config_with_events(&["run.started", "run.completed"]),
            false,
        )
        .unwrap();

        let raw = std::fs::read_to_string(temp.path().join(super::OUTBOUND_REPLIES_FILE)).unwrap();
        let replies = raw
            .lines()
            .map(|line| serde_json::from_str::<super::OutboundReply>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            replies
                .iter()
                .map(|reply| reply.event_kind.as_str())
                .collect::<Vec<_>>(),
            vec!["run.started", "run.completed"]
        );
    }

    fn config() -> ChannelConfig {
        ChannelConfig {
            version: 1,
            defaults: ChannelDefaults::default(),
            channels: BTreeMap::new(),
        }
    }

    fn config_with_events(reply_to_run_events: &[&str]) -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert("feishu".to_string(), entry(reply_to_run_events));
        ChannelConfig {
            version: 1,
            defaults: ChannelDefaults::default(),
            channels,
        }
    }

    fn entry(reply_to_run_events: &[&str]) -> ChannelEntry {
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
        }
    }

    fn event(kind: RunEventKind) -> RunEvent {
        event_with_seq(kind, 1)
    }

    fn event_with_seq(kind: RunEventKind, seq: u64) -> RunEvent {
        RunEvent {
            schema_version: crate::schema::RUN_EVENT_V1.to_string(),
            event_id: format!("event-{seq}"),
            run_id: "run-1".to_string(),
            seq,
            timestamp: Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
            kind,
            task_id: None,
            message: None,
            payload: Value::Null,
            refs: BTreeMap::new(),
        }
    }

    fn envelope() -> ChannelEnvelope {
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
