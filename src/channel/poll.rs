use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::channels::ChannelConfig;

use super::feishu::FeishuInbound;
use super::route::RouteDecision;
use super::transport::{Transport, TransportError};

pub const INBOUND_CURSOR_FILE: &str = "channel_inbound_cursor.json";
pub const INBOUND_DECISIONS_FILE: &str = "channel_inbound_decisions.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PollState {
    #[serde(default)]
    pub channels: BTreeMap<String, ChannelPollState>,
}

impl PollState {
    pub fn with_last_message_id(channel: &str, message_id: &str) -> Self {
        let mut channels = BTreeMap::new();
        channels.insert(
            channel.to_string(),
            ChannelPollState {
                last_message_id: Some(message_id.to_string()),
                last_create_time: None,
                last_ids_at_boundary: Vec::new(),
            },
        );
        Self { channels }
    }

    pub fn with_cursor(channel: &str, create_time: DateTime<Utc>, message_id: &str) -> Self {
        let mut channels = BTreeMap::new();
        channels.insert(
            channel.to_string(),
            ChannelPollState {
                last_create_time: Some(create_time),
                last_message_id: Some(message_id.to_string()),
                last_ids_at_boundary: vec![message_id.to_string()],
            },
        );
        Self { channels }
    }

    pub fn last_message_id(&self, channel: &str) -> Option<&str> {
        self.channels
            .get(channel)
            .and_then(|state| state.last_message_id.as_deref())
    }

    pub fn last_create_time(&self, channel: &str) -> Option<DateTime<Utc>> {
        self.channels
            .get(channel)
            .and_then(|state| state.last_create_time)
    }

    /// The poll cursor: the newest seen `create_time` plus the SET of message
    /// ids seen AT that timestamp. The set (not a single id) is what lets us
    /// drop only the boundary messages we actually processed — same-millisecond
    /// messages have non-monotonic ids, so a single `id <= since_id` comparison
    /// would silently drop a new same-timestamp message that sorts lower.
    fn cursor(&self, channel: &str) -> Option<(DateTime<Utc>, Vec<String>)> {
        let state = self.channels.get(channel)?;
        let time = state.last_create_time?;
        let mut ids = state.last_ids_at_boundary.clone();
        if ids.is_empty() {
            // Legacy cursor (pre-seen-set): seed from the single last id.
            if let Some(id) = &state.last_message_id {
                ids.push(id.clone());
            }
        }
        Some((time, ids))
    }

    fn set_cursor(&mut self, channel: &str, create_time: DateTime<Utc>, message_id: &str) {
        let state = self.channels.entry(channel.to_string()).or_default();
        match state.last_create_time {
            // Same boundary timestamp: remember this id alongside the others.
            Some(t) if t == create_time => {
                if !state.last_ids_at_boundary.iter().any(|id| id == message_id) {
                    state.last_ids_at_boundary.push(message_id.to_string());
                }
            }
            // Older than the current boundary (shouldn't happen — inbound is
            // sorted ascending): leave the boundary set untouched.
            Some(t) if t > create_time => {}
            // A newer boundary: reset the set to just this id.
            _ => state.last_ids_at_boundary = vec![message_id.to_string()],
        }
        state.last_create_time = Some(create_time);
        state.last_message_id = Some(message_id.to_string());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ChannelPollState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_create_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<String>,
    /// Message ids seen AT `last_create_time` (the boundary). Used to drop only
    /// the same-timestamp messages already processed, since their ids are not
    /// monotonic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_ids_at_boundary: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PollOutcome {
    pub decisions: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InboundDecisionRecord {
    pub channel: String,
    pub message_id: String,
    pub decision: RouteDecision,
}

#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("inbound parse error: {0}")]
    InboundParse(String),
    #[error("channel `{0}` has no botmux_session_id")]
    MissingSession(String),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

pub fn poll_inbound(
    workspace_root: &Path,
    channel: &str,
    config: &ChannelConfig,
    transport: &dyn Transport,
    limit: usize,
) -> Result<PollOutcome, PollError> {
    let Some(entry) = config.channels.get(channel) else {
        return Ok(PollOutcome::default());
    };
    let Some(session_id) = entry.transport_session_id(channel) else {
        return Ok(PollOutcome::default());
    };

    let mut state = load_state(workspace_root)?;
    let cursor = state.cursor(channel);
    let since = cursor.as_ref().map(|(t, ids)| (*t, ids.as_slice()));
    let messages = transport.poll(session_id, since, limit)?;
    let mut outcome = PollOutcome::default();
    for message in messages {
        let inbound = FeishuInbound {
            thread_id: message.thread_id.clone(),
            sender_open_id: message.sender_open_id.clone(),
            message_id: message.message_id.clone(),
            create_time: message.create_time,
            body: message.content.clone(),
            attachments: Vec::new(),
        };
        let mut envelope = super::parse_inbound(&inbound)
            .map_err(|err| PollError::InboundParse(err.to_string()))?;
        super::resolve_trust(&mut envelope, config);
        let decision = super::route_inbound(envelope, config);
        append_decision(
            workspace_root,
            &InboundDecisionRecord {
                channel: channel.to_string(),
                message_id: message.message_id.clone(),
                decision,
            },
        )?;
        state.set_cursor(channel, message.create_time, &message.message_id);
        save_state(workspace_root, &state)?;
        outcome.decisions += 1;
    }
    Ok(outcome)
}

pub fn load_state(workspace_root: &Path) -> Result<PollState, PollError> {
    let path = state_path(workspace_root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PollState::default());
        }
        Err(source) => {
            return Err(PollError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    };
    serde_json::from_str(&raw).map_err(|source| PollError::Parse {
        path: path.display().to_string(),
        source,
    })
}

pub fn save_state(workspace_root: &Path, state: &PollState) -> Result<(), PollError> {
    let dir = workspace_root.join(".maestro");
    std::fs::create_dir_all(&dir).map_err(|source| PollError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let path = state_path(workspace_root);
    let raw = serde_json::to_string(state).map_err(|source| PollError::Parse {
        path: path.display().to_string(),
        source,
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{raw}\n")).map_err(|source| PollError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| PollError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn append_decision(workspace_root: &Path, record: &InboundDecisionRecord) -> Result<(), PollError> {
    let dir = workspace_root.join(".maestro");
    std::fs::create_dir_all(&dir).map_err(|source| PollError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let path = decisions_path(workspace_root);
    let line = serde_json::to_string(record).map_err(|source| PollError::Parse {
        path: path.display().to_string(),
        source,
    })?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| PollError::Io {
            path: path.display().to_string(),
            source,
        })?;
    writeln!(file, "{line}").map_err(|source| PollError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn state_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".maestro").join(INBOUND_CURSOR_FILE)
}

fn decisions_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".maestro").join(INBOUND_DECISIONS_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::channel::transport::{
        FileMockTransport, InboundMessage, StubTransport, Transport, TransportError,
    };
    use crate::config::channels::{AllowedSender, ChannelConfig, ChannelDefaults, ChannelEntry};
    use crate::schema::channel_envelope::ChannelAction;

    #[test]
    fn poll_inbound_returns_zero_when_transport_has_no_messages() {
        let temp = tempfile::tempdir().unwrap();
        let transport = StubTransport::new(Vec::new());

        let outcome =
            super::poll_inbound(temp.path(), "feishu", &config(), &transport, 20).unwrap();

        assert_eq!(outcome.decisions, 0);
        assert_eq!(
            super::load_state(temp.path())
                .unwrap()
                .last_message_id("feishu"),
            None
        );
    }

    #[test]
    fn poll_inbound_halts_without_advancing_cursor_on_transport_error() {
        struct FailingTransport;
        impl Transport for FailingTransport {
            fn send(
                &self,
                _session_id: &str,
                _reply: &crate::channel::OutboundReply,
            ) -> Result<(), TransportError> {
                Ok(())
            }

            fn poll(
                &self,
                _session_id: &str,
                _since: Option<(chrono::DateTime<chrono::Utc>, &[String])>,
                _limit: usize,
            ) -> Result<Vec<InboundMessage>, TransportError> {
                Err(TransportError::ParseFailed("boom".to_string()))
            }
        }
        let temp = tempfile::tempdir().unwrap();

        let err = super::poll_inbound(temp.path(), "feishu", &config(), &FailingTransport, 20)
            .unwrap_err();

        assert!(matches!(err, super::PollError::Transport(_)));
        assert_eq!(
            super::load_state(temp.path())
                .unwrap()
                .last_message_id("feishu"),
            None
        );
    }

    #[test]
    fn poll_inbound_continues_after_malformed_mock_inbox_line() {
        let temp = tempfile::tempdir().unwrap();
        let inbox_dir = temp.path().join(".maestro/channels/feishu");
        std::fs::create_dir_all(&inbox_dir).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-05-24T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut inbox = String::new();
        for index in 1..=100 {
            if index == 51 {
                inbox.push_str("{not-json\n");
                continue;
            }
            let message = InboundMessage {
                message_id: format!("om_{index:03}"),
                sender_open_id: "ou_owner".to_string(),
                sender_type: "user".to_string(),
                create_time: start + chrono::Duration::seconds(index),
                content: "status".to_string(),
                thread_id: Some("thread-1".to_string()),
            };
            inbox.push_str(&serde_json::to_string(&message).unwrap());
            inbox.push('\n');
        }
        std::fs::write(inbox_dir.join("inbox.jsonl"), inbox).unwrap();
        let mut config = config();
        config.channels.get_mut("feishu").unwrap().transport = "mock".to_string();
        config.channels.get_mut("feishu").unwrap().botmux_session_id = None;
        let transport = FileMockTransport::new(&inbox_dir);

        let outcome = super::poll_inbound(temp.path(), "feishu", &config, &transport, 100).unwrap();

        assert_eq!(outcome.decisions, 99);
        let decisions = std::fs::read_to_string(
            temp.path()
                .join(".maestro")
                .join(super::INBOUND_DECISIONS_FILE),
        )
        .unwrap();
        assert_eq!(decisions.lines().count(), 99);
        assert_eq!(
            super::load_state(temp.path())
                .unwrap()
                .last_message_id("feishu"),
            Some("om_100")
        );
    }

    fn config() -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: Some("session-1".to_string()),
                allowed_senders: vec![AllowedSender {
                    open_id: "ou_owner".to_string(),
                    label: None,
                }],
                allowed_actions: vec![ChannelAction::Plan, ChannelAction::Status],
                reply_to_run_events: Vec::new(),
                redact_payload_over_kb: 1,
            },
        );
        ChannelConfig {
            version: 1,
            defaults: ChannelDefaults::default(),
            channels,
        }
    }
}
