use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::channels::ChannelConfig;

use super::outbound::OutboundReply;
use super::subscribe::OUTBOUND_REPLIES_FILE;
use super::transport::{Transport, TransportError};

pub const DRAIN_CURSOR_FILE: &str = "channel_drain_cursor.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DrainState {
    pub lines_sent: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainOutcome {
    pub sent: usize,
    pub skipped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum DrainError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path} line {line}: {source}")]
    Parse {
        path: String,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Transport(#[from] TransportError),
}

pub fn drain_once(
    run_dir: &Path,
    config: &ChannelConfig,
    transport: &dyn Transport,
) -> Result<DrainOutcome, DrainError> {
    let mut state = load_state(run_dir)?;
    let outbound_path = run_dir.join(OUTBOUND_REPLIES_FILE);
    let raw = match std::fs::read_to_string(&outbound_path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            save_state(run_dir, &state)?;
            return Ok(DrainOutcome::default());
        }
        Err(source) => {
            return Err(DrainError::Io {
                path: outbound_path.display().to_string(),
                source,
            });
        }
    };

    let mut outcome = DrainOutcome::default();
    for (index, line) in raw.lines().enumerate().skip(state.lines_sent) {
        let reply: OutboundReply =
            serde_json::from_str(line).map_err(|source| DrainError::Parse {
                path: outbound_path.display().to_string(),
                line: index + 1,
                source,
            })?;
        let Some(session_id) = config
            .channels
            .get(&reply.channel)
            .and_then(|entry| entry.transport_session_id(&reply.channel))
        else {
            outcome.skipped += 1;
            state.lines_sent += 1;
            save_state(run_dir, &state)?;
            continue;
        };

        transport.send(session_id, &reply)?;
        outcome.sent += 1;
        state.lines_sent += 1;
        save_state(run_dir, &state)?;
    }

    Ok(outcome)
}

pub fn load_state(run_dir: &Path) -> Result<DrainState, DrainError> {
    let path = state_path(run_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DrainState::default());
        }
        Err(source) => {
            return Err(DrainError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    };
    serde_json::from_str(&raw).map_err(|source| DrainError::Parse {
        path: path.display().to_string(),
        line: 1,
        source,
    })
}

pub fn save_state(run_dir: &Path, state: &DrainState) -> Result<(), DrainError> {
    std::fs::create_dir_all(run_dir).map_err(|source| DrainError::Io {
        path: run_dir.display().to_string(),
        source,
    })?;
    let path = state_path(run_dir);
    let raw = serde_json::to_string(state).map_err(|source| DrainError::Parse {
        path: path.display().to_string(),
        line: 1,
        source,
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{raw}\n")).map_err(|source| DrainError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| DrainError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn state_path(run_dir: &Path) -> PathBuf {
    run_dir.join(DRAIN_CURSOR_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::channel::transport::{StubTransport, TransportError};
    use crate::channel::OutboundReply;
    use crate::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};

    #[test]
    fn drain_once_returns_zero_when_outbound_file_missing() {
        let temp = tempfile::tempdir().unwrap();
        let transport = StubTransport::new(Vec::new());

        let outcome = super::drain_once(temp.path(), &config(Some("session-1")), &transport)
            .expect("missing file is not an error");

        assert_eq!(outcome.sent, 0);
        assert_eq!(super::load_state(temp.path()).unwrap().lines_sent, 0);
    }

    #[test]
    fn drain_once_skips_missing_session_and_advances_cursor() {
        let temp = tempfile::tempdir().unwrap();
        write_replies(temp.path(), &[reply("run-1")]);
        let transport = StubTransport::new(Vec::new());

        let outcome = super::drain_once(temp.path(), &config(None), &transport)
            .expect("missing session is a skip");

        assert_eq!(outcome.sent, 0);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(super::load_state(temp.path()).unwrap().lines_sent, 1);
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn drain_once_halts_without_advancing_cursor_on_send_error() {
        struct FailingTransport;
        impl crate::channel::transport::Transport for FailingTransport {
            fn send(
                &self,
                _session_id: &str,
                _reply: &OutboundReply,
            ) -> Result<(), TransportError> {
                Err(TransportError::SendFailed {
                    exit_code: 1,
                    stderr_excerpt: "boom".to_string(),
                })
            }

            fn poll(
                &self,
                _session_id: &str,
                _since: Option<(chrono::DateTime<chrono::Utc>, &[String])>,
                _limit: usize,
            ) -> Result<Vec<crate::channel::transport::InboundMessage>, TransportError>
            {
                Ok(Vec::new())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        write_replies(temp.path(), &[reply("run-1")]);

        let err = super::drain_once(temp.path(), &config(Some("session-1")), &FailingTransport)
            .unwrap_err();

        assert!(matches!(err, super::DrainError::Transport(_)));
        assert_eq!(super::load_state(temp.path()).unwrap().lines_sent, 0);
    }

    fn config(session_id: Option<&str>) -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: session_id.map(str::to_string),
                allowed_senders: Vec::new(),
                allowed_actions: Vec::new(),
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

    fn write_replies(run_dir: &std::path::Path, replies: &[OutboundReply]) {
        let raw = replies
            .iter()
            .map(|reply| serde_json::to_string(reply).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            run_dir.join(crate::channel::OUTBOUND_REPLIES_FILE),
            format!("{raw}\n"),
        )
        .unwrap();
    }

    fn reply(run_id: &str) -> OutboundReply {
        OutboundReply {
            channel: "feishu".to_string(),
            run_id: run_id.to_string(),
            event_kind: "run.started".to_string(),
            title: "title".to_string(),
            body: "body".to_string(),
            attachments: Vec::new(),
        }
    }
}
