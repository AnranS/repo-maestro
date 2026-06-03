use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::schema::channel_envelope::ChannelEnvelope;

pub const ENVELOPES_FILE: &str = "channel_envelopes.ndjson";

#[derive(Debug, thiserror::Error)]
pub enum OriginError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("serialize envelope: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("parse envelope at {path} line {line}: {source}")]
    Parse {
        path: String,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadOrigin {
    pub channel: String,
    pub thread_id: String,
}

pub fn persist(run_dir: &Path, envelope: &ChannelEnvelope) -> Result<(), OriginError> {
    std::fs::create_dir_all(run_dir).map_err(|source| OriginError::Io {
        path: run_dir.display().to_string(),
        source,
    })?;
    let path = envelopes_path(run_dir);
    let line = serde_json::to_string(envelope).map_err(OriginError::Serialize)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| OriginError::Io {
            path: path.display().to_string(),
            source,
        })?;
    writeln!(file, "{line}").map_err(|source| OriginError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

pub fn resolve(run_dir: &Path) -> Result<Option<ChannelEnvelope>, OriginError> {
    let path = envelopes_path(run_dir);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(OriginError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    };

    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let raw = line.map_err(|source| OriginError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if raw.trim().is_empty() {
            continue;
        }
        let envelope = serde_json::from_str(&raw).map_err(|source| OriginError::Parse {
            path: path.display().to_string(),
            line: idx + 1,
            source,
        })?;
        return Ok(Some(envelope));
    }

    Ok(None)
}

pub fn thread_for_run(run_dir: &Path) -> Result<Option<ThreadOrigin>, OriginError> {
    Ok(resolve(run_dir)?.map(|envelope| ThreadOrigin {
        channel: envelope.channel,
        thread_id: envelope.thread_id,
    }))
}

fn envelopes_path(run_dir: &Path) -> PathBuf {
    run_dir.join(ENVELOPES_FILE)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::schema::channel_envelope::{ChannelAction, ChannelEnvelope};

    #[test]
    fn envelopes_file_constant_matches_doctor_path() {
        assert_eq!(super::ENVELOPES_FILE, "channel_envelopes.ndjson");
    }

    #[test]
    fn persist_appends_one_line_to_run_dir() {
        let temp = tempfile::tempdir().unwrap();
        let envelope = envelope("thread-1");

        super::persist(temp.path(), &envelope).unwrap();

        let raw = std::fs::read_to_string(temp.path().join(super::ENVELOPES_FILE)).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec![serde_json::to_string(&envelope).unwrap()]);
    }

    #[test]
    fn persist_appends_subsequent_lines() {
        let temp = tempfile::tempdir().unwrap();
        let first = envelope("thread-1");
        let second = envelope("thread-2");

        super::persist(temp.path(), &first).unwrap();
        super::persist(temp.path(), &second).unwrap();

        let raw = std::fs::read_to_string(temp.path().join(super::ENVELOPES_FILE)).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                serde_json::to_string(&first).unwrap(),
                serde_json::to_string(&second).unwrap()
            ]
        );
    }

    #[test]
    fn persist_creates_run_dir_if_missing() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("missing-run-dir");
        let envelope = envelope("thread-1");

        super::persist(&run_dir, &envelope).unwrap();

        assert!(run_dir.is_dir());
        assert!(run_dir.join(super::ENVELOPES_FILE).is_file());
    }

    #[test]
    fn resolve_returns_first_envelope_when_file_exists() {
        let temp = tempfile::tempdir().unwrap();
        let first = envelope("thread-1");
        let second = envelope("thread-2");
        super::persist(temp.path(), &first).unwrap();
        super::persist(temp.path(), &second).unwrap();

        let resolved = super::resolve(temp.path()).unwrap();

        assert_eq!(resolved, Some(first));
    }

    #[test]
    fn resolve_returns_none_when_file_missing() {
        let temp = tempfile::tempdir().unwrap();

        let resolved = super::resolve(temp.path()).unwrap();

        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_returns_parse_error_on_malformed_first_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(super::ENVELOPES_FILE);
        std::fs::write(&path, "not json\n").unwrap();

        let err = super::resolve(temp.path()).unwrap_err();

        match err {
            super::OriginError::Parse { path, line, .. } => {
                assert!(path.ends_with(super::ENVELOPES_FILE));
                assert_eq!(line, 1);
            }
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn thread_for_run_returns_channel_and_thread_id() {
        let temp = tempfile::tempdir().unwrap();
        let envelope = envelope("thread-x");
        super::persist(temp.path(), &envelope).unwrap();

        let origin = super::thread_for_run(temp.path()).unwrap();

        assert_eq!(
            origin,
            Some(super::ThreadOrigin {
                channel: "feishu".to_string(),
                thread_id: "thread-x".to_string()
            })
        );
    }

    #[test]
    fn thread_for_run_returns_none_when_file_missing() {
        let temp = tempfile::tempdir().unwrap();

        let origin = super::thread_for_run(temp.path()).unwrap();

        assert_eq!(origin, None);
    }

    fn envelope(thread_id: &str) -> ChannelEnvelope {
        ChannelEnvelope {
            schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
            channel: "feishu".to_string(),
            thread_id: thread_id.to_string(),
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
