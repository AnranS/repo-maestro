use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::channels::ChannelEntry;

use super::OutboundReply;

pub trait Transport: Send + Sync {
    fn send(&self, session_id: &str, reply: &OutboundReply) -> Result<(), TransportError>;

    fn poll(
        &self,
        session_id: &str,
        since: Option<(DateTime<Utc>, &[String])>,
        limit: usize,
    ) -> Result<Vec<InboundMessage>, TransportError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboundMessage {
    pub message_id: String,
    pub sender_open_id: String,
    pub sender_type: String,
    pub create_time: DateTime<Utc>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("botmux binary not found")]
    BotmuxNotFound,
    #[error("botmux send failed with exit code {exit_code}: {stderr_excerpt}")]
    SendFailed {
        exit_code: i32,
        stderr_excerpt: String,
    },
    #[error("botmux output parse failed: {0}")]
    ParseFailed(String),
    #[error("unknown channel transport `{0}`")]
    UnknownTransport(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct FileMockTransport {
    root: PathBuf,
}

impl FileMockTransport {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn outbox_path(&self) -> PathBuf {
        self.root.join("outbox.jsonl")
    }

    fn inbox_path(&self) -> PathBuf {
        self.root.join("inbox.jsonl")
    }
}

impl Transport for FileMockTransport {
    fn send(&self, _session_id: &str, reply: &OutboundReply) -> Result<(), TransportError> {
        std::fs::create_dir_all(&self.root)?;
        let line = outbound_reply_jsonl_line(reply)?;
        let mut outbox = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.outbox_path())?;
        outbox.write_all(line.as_bytes())?;
        Ok(())
    }

    fn poll(
        &self,
        _session_id: &str,
        since: Option<(DateTime<Utc>, &[String])>,
        limit: usize,
    ) -> Result<Vec<InboundMessage>, TransportError> {
        let inbox = match std::fs::File::open(self.inbox_path()) {
            Ok(inbox) => inbox,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut messages = Vec::new();
        for (index, line) in BufReader::new(inbox).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let message: InboundMessage = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(err) => {
                    tracing::warn!(
                        line = index + 1,
                        error = %err,
                        "skipping malformed mock inbox line"
                    );
                    continue;
                }
            };
            if message_is_at_or_before_cursor(message.create_time, &message.message_id, since) {
                continue;
            }
            messages.push(message);
            if messages.len() >= limit {
                break;
            }
        }
        Ok(messages)
    }
}

fn outbound_reply_jsonl_line(reply: &OutboundReply) -> Result<String, TransportError> {
    let mut line =
        serde_json::to_string(reply).map_err(|err| TransportError::ParseFailed(err.to_string()))?;
    line.push('\n');
    Ok(line)
}

pub fn select_transport(
    channel: &str,
    entry: &ChannelEntry,
    workspace_root: &Path,
) -> Result<Box<dyn Transport>, TransportError> {
    match entry.transport.as_str() {
        "" | "botmux" | "feishu" => Ok(Box::new(BotmuxTransport::new())),
        "mock" => Ok(Box::new(FileMockTransport::new(
            workspace_root.join(".maestro/channels").join(channel),
        ))),
        other => Err(TransportError::UnknownTransport(other.to_string())),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BotmuxTransport;

impl BotmuxTransport {
    pub fn new() -> Self {
        Self
    }
}

impl Transport for BotmuxTransport {
    fn send(&self, session_id: &str, reply: &OutboundReply) -> Result<(), TransportError> {
        let mut child = Command::new("botmux")
            .args(["send", "--session-id", session_id])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(map_spawn_error)?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(reply.body.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(TransportError::SendFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr_excerpt: excerpt(&output.stderr),
            })
        }
    }

    fn poll(
        &self,
        session_id: &str,
        since: Option<(DateTime<Utc>, &[String])>,
        limit: usize,
    ) -> Result<Vec<InboundMessage>, TransportError> {
        let output = Command::new("botmux")
            .args(["history", "--session-id", session_id, "--limit"])
            .arg(limit.to_string())
            .output()
            .map_err(map_spawn_error)?;
        if !output.status.success() {
            return Err(TransportError::SendFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr_excerpt: excerpt(&output.stderr),
            });
        }
        parse_history(&output.stdout, since, limit)
    }
}

fn parse_history(
    stdout: &[u8],
    since: Option<(DateTime<Utc>, &[String])>,
    limit: usize,
) -> Result<Vec<InboundMessage>, TransportError> {
    let value: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|err| TransportError::ParseFailed(err.to_string()))?;
    let messages = value
        .as_array()
        .or_else(|| {
            value
                .get("messages")
                .and_then(|messages| messages.as_array())
        })
        .ok_or_else(|| TransportError::ParseFailed("history output is not an array".to_string()))?;
    let mut inbound = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let Some(message) = (match parse_history_message(message) {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!(
                    message_index = index,
                    error = %err,
                    "skipping malformed botmux history message"
                );
                continue;
            }
        }) else {
            continue;
        };
        if message_is_at_or_before_cursor(message.create_time, &message.message_id, since) {
            continue;
        }
        inbound.push(message);
    }
    // botmux does not guarantee ordering (chat history APIs commonly return
    // newest-first). Sort ascending by (create_time, message_id) so the caller's
    // per-message cursor advance lands on the NEWEST message rather than whatever
    // botmux happened to emit last — otherwise the cursor can stick on the oldest
    // of a batch and re-emit / drop messages. Sort BEFORE truncating so the
    // newest-N window is the tail we keep.
    inbound.sort_by(|a, b| {
        a.create_time
            .cmp(&b.create_time)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });
    if inbound.len() > limit {
        inbound.drain(0..inbound.len() - limit);
    }
    Ok(inbound)
}

fn parse_history_message(
    message: &serde_json::Value,
) -> Result<Option<InboundMessage>, TransportError> {
    let sender_type = string_field(message, "senderType").unwrap_or_default();
    if sender_type != "user" {
        return Ok(None);
    }
    let message_id = string_field(message, "messageId")
        .or_else(|| string_field(message, "message_id"))
        .ok_or_else(|| TransportError::ParseFailed("message missing messageId".to_string()))?;
    let create_time = parse_create_time(
        string_field(message, "createTime")
            .or_else(|| string_field(message, "create_time"))
            .as_deref(),
    )?;
    let root_id = string_field(message, "rootId").filter(|root| !root.is_empty());
    Ok(Some(InboundMessage {
        message_id: message_id.clone(),
        sender_open_id: string_field(message, "senderId")
            .or_else(|| string_field(message, "sender_open_id"))
            .ok_or_else(|| TransportError::ParseFailed("message missing senderId".to_string()))?,
        sender_type,
        create_time,
        content: string_field(message, "content").unwrap_or_default(),
        thread_id: root_id.or(Some(message_id)),
    }))
}

fn message_is_at_or_before_cursor(
    create_time: DateTime<Utc>,
    message_id: &str,
    since: Option<(DateTime<Utc>, &[String])>,
) -> bool {
    since
        .map(|(since_time, seen_at_boundary)| {
            // Strictly older → already processed. At the boundary timestamp,
            // skip ONLY the ids we actually saw (Feishu `om_*` ids are not
            // monotonic, so a `<=` comparison would drop a new same-timestamp
            // message that happens to sort lower).
            create_time < since_time
                || (create_time == since_time && seen_at_boundary.iter().any(|id| id == message_id))
        })
        .unwrap_or(false)
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn parse_create_time(raw: Option<&str>) -> Result<DateTime<Utc>, TransportError> {
    let Some(raw) = raw else {
        return Ok(Utc::now());
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&Utc));
    }
    let millis = raw
        .parse::<i64>()
        .map_err(|err| TransportError::ParseFailed(format!("invalid createTime `{raw}`: {err}")))?;
    DateTime::<Utc>::from_timestamp_millis(millis)
        .ok_or_else(|| TransportError::ParseFailed(format!("invalid createTime `{raw}`")))
}

fn map_spawn_error(source: std::io::Error) -> TransportError {
    if source.kind() == std::io::ErrorKind::NotFound {
        TransportError::BotmuxNotFound
    } else {
        TransportError::Io(source)
    }
}

fn excerpt(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.chars().take(500).collect()
}

#[cfg(test)]
#[derive(Debug)]
pub struct StubTransport {
    sent: std::sync::Mutex<Vec<(String, OutboundReply)>>,
    inbound: std::sync::Mutex<Vec<InboundMessage>>,
}

#[cfg(test)]
impl StubTransport {
    pub fn new(inbound: Vec<InboundMessage>) -> Self {
        Self {
            sent: std::sync::Mutex::new(Vec::new()),
            inbound: std::sync::Mutex::new(inbound),
        }
    }

    pub fn sent(&self) -> Vec<(String, OutboundReply)> {
        self.sent.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Transport for StubTransport {
    fn send(&self, session_id: &str, reply: &OutboundReply) -> Result<(), TransportError> {
        self.sent
            .lock()
            .unwrap()
            .push((session_id.to_string(), reply.clone()));
        Ok(())
    }

    fn poll(
        &self,
        _session_id: &str,
        since: Option<(DateTime<Utc>, &[String])>,
        limit: usize,
    ) -> Result<Vec<InboundMessage>, TransportError> {
        let inbound = self.inbound.lock().unwrap();
        Ok(inbound
            .iter()
            .filter(|message| {
                since
                    .map(|(since_time, seen_at_boundary)| {
                        message.create_time > since_time
                            || (message.create_time == since_time
                                && !seen_at_boundary.iter().any(|id| id == &message.message_id))
                    })
                    .unwrap_or(true)
            })
            .take(limit)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::Transport;

    #[test]
    fn stub_transport_captures_send_and_replays_poll() {
        let transport = super::StubTransport::new(vec![super::InboundMessage {
            message_id: "om_1".to_string(),
            sender_open_id: "ou_sender".to_string(),
            sender_type: "user".to_string(),
            create_time: chrono::Utc::now(),
            content: "plan".to_string(),
            thread_id: Some("thread-1".to_string()),
        }]);
        let reply = crate::channel::OutboundReply {
            channel: "feishu".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "run.started".to_string(),
            body: "hello".to_string(),
            attachments: Vec::new(),
        };

        super::Transport::send(&transport, "session-1", &reply).unwrap();
        let messages = super::Transport::poll(&transport, "session-1", None, 10).unwrap();

        assert_eq!(transport.sent().len(), 1);
        assert_eq!(transport.sent()[0].0, "session-1");
        assert_eq!(transport.sent()[0].1, reply);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "om_1");
    }

    #[test]
    fn file_mock_transport_persists_outbound_and_polls_inbound() {
        let temp = tempfile::tempdir().unwrap();
        let transport = super::FileMockTransport::new(temp.path());
        let reply = crate::channel::OutboundReply {
            channel: "demo".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "title".to_string(),
            body: "body".to_string(),
            attachments: Vec::new(),
        };

        super::Transport::send(&transport, "demo", &reply).unwrap();

        let outbox = temp.path().join("outbox.jsonl");
        let raw = std::fs::read_to_string(outbox).unwrap();
        let persisted: crate::channel::OutboundReply = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(persisted, reply);

        let old_time = chrono::DateTime::parse_from_rfc3339("2026-05-24T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let new_time = chrono::DateTime::parse_from_rfc3339("2026-05-24T08:01:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let inbox = temp.path().join("inbox.jsonl");
        let old = super::InboundMessage {
            message_id: "om_old".to_string(),
            sender_open_id: "ou_old".to_string(),
            sender_type: "user".to_string(),
            create_time: old_time,
            content: "old".to_string(),
            thread_id: Some("thread-1".to_string()),
        };
        let new = super::InboundMessage {
            message_id: "om_new".to_string(),
            sender_open_id: "ou_new".to_string(),
            sender_type: "user".to_string(),
            create_time: new_time,
            content: "new".to_string(),
            thread_id: Some("thread-1".to_string()),
        };
        std::fs::write(
            &inbox,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&old).unwrap(),
                serde_json::to_string(&new).unwrap()
            ),
        )
        .unwrap();

        let seen = ["om_old".to_string()];
        let messages =
            super::Transport::poll(&transport, "demo", Some((old_time, &seen)), 10).unwrap();

        assert_eq!(messages, vec![new]);
    }

    #[test]
    fn file_mock_transport_serializes_outbound_as_one_jsonl_line() {
        let reply = crate::channel::OutboundReply {
            channel: "demo".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "title".to_string(),
            body: "body".to_string(),
            attachments: Vec::new(),
        };

        let line = super::outbound_reply_jsonl_line(&reply).unwrap();

        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let persisted: crate::channel::OutboundReply =
            serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(persisted, reply);
    }

    #[test]
    fn select_transport_dispatches_mock_to_file_transport() {
        let temp = tempfile::tempdir().unwrap();
        let entry = crate::config::channels::ChannelEntry {
            enabled: true,
            transport: "mock".to_string(),
            bot_open_id: None,
            botmux_session_id: None,
            allowed_senders: Vec::new(),
            allowed_actions: Vec::new(),
            reply_to_run_events: Vec::new(),
            redact_payload_over_kb: 1,
        };
        let reply = crate::channel::OutboundReply {
            channel: "demo".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "title".to_string(),
            body: "body".to_string(),
            attachments: Vec::new(),
        };

        let transport = super::select_transport("demo", &entry, temp.path()).unwrap();
        transport.send("ignored-for-mock", &reply).unwrap();

        let outbox = temp
            .path()
            .join(".maestro")
            .join("channels")
            .join("demo")
            .join("outbox.jsonl");
        let raw = std::fs::read_to_string(outbox).unwrap();
        let persisted: crate::channel::OutboundReply = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(persisted, reply);
    }

    #[test]
    fn file_mock_transport_skips_malformed_inbox_lines_and_continues() {
        let temp = tempfile::tempdir().unwrap();
        let transport = super::FileMockTransport::new(temp.path());
        let first = super::InboundMessage {
            message_id: "om_1".to_string(),
            sender_open_id: "ou_1".to_string(),
            sender_type: "user".to_string(),
            create_time: chrono::DateTime::parse_from_rfc3339("2026-05-24T08:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            content: "first".to_string(),
            thread_id: Some("thread-1".to_string()),
        };
        let second = super::InboundMessage {
            message_id: "om_2".to_string(),
            sender_open_id: "ou_2".to_string(),
            sender_type: "user".to_string(),
            create_time: chrono::DateTime::parse_from_rfc3339("2026-05-24T08:01:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            content: "second".to_string(),
            thread_id: Some("thread-1".to_string()),
        };
        std::fs::write(
            temp.path().join("inbox.jsonl"),
            format!(
                "{}\n{{not-json\n{}\n",
                serde_json::to_string(&first).unwrap(),
                serde_json::to_string(&second).unwrap()
            ),
        )
        .unwrap();

        let messages = super::Transport::poll(&transport, "demo", None, 10).unwrap();

        assert_eq!(messages, vec![first, second]);
    }

    #[test]
    #[serial_test::serial]
    fn botmux_transport_returns_error_shape_for_missing_or_unavailable_binary() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_path(temp.path());
        let transport = super::BotmuxTransport::new();

        let err = super::Transport::send(
            &transport,
            "session-1",
            &crate::channel::OutboundReply {
                channel: "feishu".to_string(),
                run_id: "run-1".to_string(),
                event_kind: "run.started".to_string(),
                title: "title".to_string(),
                body: "body".to_string(),
                attachments: Vec::new(),
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            super::TransportError::BotmuxNotFound | super::TransportError::Io(_)
        ));
    }

    #[test]
    #[serial_test::serial]
    fn botmux_transport_send_shells_out_with_session_and_body() {
        let temp = tempfile::tempdir().unwrap();
        write_fake_botmux(
            temp.path(),
            r#"#!/bin/sh
printf "%s\n" "$@" > "$BOTMUX_ARGS_OUT"
cat > "$BOTMUX_STDIN_OUT"
exit 0
"#,
        );
        let _guard = EnvGuard::prepend_path(temp.path());
        let args_out = temp.path().join("args.txt");
        let stdin_out = temp.path().join("stdin.txt");
        std::env::set_var("BOTMUX_ARGS_OUT", &args_out);
        std::env::set_var("BOTMUX_STDIN_OUT", &stdin_out);
        let reply = crate::channel::OutboundReply {
            channel: "feishu".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "title".to_string(),
            body: "body line".to_string(),
            attachments: Vec::new(),
        };

        super::BotmuxTransport::new()
            .send("session-1", &reply)
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(args_out).unwrap(),
            "send\n--session-id\nsession-1\n"
        );
        assert_eq!(std::fs::read_to_string(stdin_out).unwrap(), "body line");
    }

    #[test]
    #[serial_test::serial]
    fn botmux_transport_poll_parses_history_and_filters_since() {
        let temp = tempfile::tempdir().unwrap();
        write_fake_botmux(
            temp.path(),
            r#"#!/bin/sh
cat <<'JSON'
{"messages":[
  {"messageId":"om_1","senderId":"ou_1","senderType":"user","createTime":"2026-05-24T08:00:00Z","content":"old","rootId":"thread-1"},
  {"messageId":"om_2","senderId":"bot","senderType":"app","createTime":"2026-05-24T08:00:00Z","content":"skip","rootId":"thread-1"},
  {"messageId":"om_3","senderId":"ou_2","senderType":"user","createTime":"2026-05-24T08:00:00Z","content":"new","rootId":"thread-1"}
]}
JSON
exit 0
"#,
        );
        let _guard = EnvGuard::prepend_path(temp.path());

        let cursor_time = chrono::DateTime::parse_from_rfc3339("2026-05-24T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let seen = ["om_1".to_string()];
        let messages = super::BotmuxTransport::new()
            .poll("session-1", Some((cursor_time, &seen)), 20)
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "om_3");
        assert_eq!(messages[0].sender_open_id, "ou_2");
        assert_eq!(messages[0].content, "new");
        assert_eq!(messages[0].thread_id.as_deref(), Some("thread-1"));
    }

    #[test]
    fn botmux_history_parser_skips_malformed_user_messages_and_continues() {
        let raw = br#"{"messages":[
  {"messageId":"om_1","senderId":"ou_1","senderType":"user","createTime":"2026-05-24T08:00:00Z","content":"first","rootId":"thread-1"},
  {"messageId":"om_bad","senderId":"ou_bad","senderType":"user","createTime":"not-a-time","content":"bad","rootId":"thread-1"},
  {"messageId":"om_2","senderId":"ou_2","senderType":"user","createTime":"2026-05-24T08:01:00Z","content":"second","rootId":"thread-1"}
]}"#;

        let messages = super::parse_history(raw, None, 20).unwrap();

        assert_eq!(
            messages
                .iter()
                .map(|message| message.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["om_1", "om_2"]
        );
    }

    #[test]
    fn parse_history_sorts_newest_first_input_to_ascending() {
        // botmux may return newest-first; the result must be ascending so the
        // caller's per-message cursor advance lands on the NEWEST message.
        let raw = br#"{"messages":[
  {"messageId":"om_3","senderId":"ou_3","senderType":"user","createTime":"2026-05-24T08:03:00Z","content":"newest","rootId":"t"},
  {"messageId":"om_2","senderId":"ou_2","senderType":"user","createTime":"2026-05-24T08:02:00Z","content":"mid","rootId":"t"},
  {"messageId":"om_1","senderId":"ou_1","senderType":"user","createTime":"2026-05-24T08:01:00Z","content":"oldest","rootId":"t"}
]}"#;
        let messages = super::parse_history(raw, None, 20).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, ["om_1", "om_2", "om_3"]);
        assert_eq!(messages.last().unwrap().content, "newest");
    }

    #[test]
    fn same_timestamp_lower_id_is_not_dropped() {
        // Cursor already saw {om_500} at T. A NEW message (T, om_300) — same
        // timestamp, lexicographically smaller id — must be kept. The old
        // `message_id <= since_id` rule silently dropped it (Feishu ids are not
        // monotonic); the boundary seen-set keeps it.
        let raw = br#"{"messages":[
  {"messageId":"om_300","senderId":"ou_1","senderType":"user","createTime":"2026-05-24T08:00:00Z","content":"new","rootId":"t"},
  {"messageId":"om_500","senderId":"ou_2","senderType":"user","createTime":"2026-05-24T08:00:00Z","content":"already seen","rootId":"t"}
]}"#;
        let t = chrono::DateTime::parse_from_rfc3339("2026-05-24T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let seen = ["om_500".to_string()];
        let messages = super::parse_history(raw, Some((t, &seen)), 20).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, ["om_300"]);
    }

    #[cfg(unix)]
    fn write_fake_botmux(dir: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("botmux");
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    struct EnvGuard {
        old_path: Option<String>,
    }

    impl EnvGuard {
        fn set_path(path: &std::path::Path) -> Self {
            let old_path = std::env::var("PATH").ok();
            std::env::set_var("PATH", path);
            Self { old_path }
        }

        fn prepend_path(path: &std::path::Path) -> Self {
            let old_path = std::env::var("PATH").ok();
            let mut paths = vec![path.to_path_buf()];
            if let Some(old_path) = &old_path {
                paths.extend(std::env::split_paths(old_path));
            }
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            Self { old_path }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(old_path) = &self.old_path {
                std::env::set_var("PATH", old_path);
            }
        }
    }
}
