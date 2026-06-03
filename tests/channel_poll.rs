use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use maestro::channel::{
    poll_inbound, InboundMessage, OutboundReply, Transport, TransportError, INBOUND_DECISIONS_FILE,
};
use maestro::config::channels::{AllowedSender, ChannelConfig, ChannelDefaults, ChannelEntry};
use maestro::schema::channel_envelope::ChannelAction;

#[test]
fn channel_poll_writes_decisions_and_resumes_from_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let transport = TestTransport::new(vec![message("om_1", "plan"), message("om_2", "status")]);

    let first = poll_inbound(temp.path(), "feishu", &config(), &transport, 20).unwrap();
    transport
        .messages
        .lock()
        .unwrap()
        .push(message("om_3", "plan"));
    let second = poll_inbound(temp.path(), "feishu", &config(), &transport, 20).unwrap();

    assert_eq!(first.decisions, 2);
    assert_eq!(second.decisions, 1);
    let raw =
        std::fs::read_to_string(temp.path().join(".maestro").join(INBOUND_DECISIONS_FILE)).unwrap();
    assert_eq!(raw.lines().count(), 3);
    assert!(raw.contains(r#""message_id":"om_3""#));
}

#[test]
fn channel_poll_uses_create_time_not_lexical_message_id_for_resume() {
    let temp = tempfile::tempdir().unwrap();
    let t1 = Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap();
    maestro::channel::poll::save_state(
        temp.path(),
        &maestro::channel::PollState::with_cursor("feishu", t1, "om_zzz"),
    )
    .unwrap();
    let transport = TestTransport::new(vec![message_at(
        "om_aaa",
        "plan",
        Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 1).unwrap(),
    )]);

    let outcome = poll_inbound(temp.path(), "feishu", &config(), &transport, 20).unwrap();

    assert_eq!(outcome.decisions, 1);
    let raw =
        std::fs::read_to_string(temp.path().join(".maestro").join(INBOUND_DECISIONS_FILE)).unwrap();
    assert!(raw.contains(r#""message_id":"om_aaa""#));
}

struct TestTransport {
    messages: Mutex<Vec<InboundMessage>>,
}

impl TestTransport {
    fn new(messages: Vec<InboundMessage>) -> Self {
        Self {
            messages: Mutex::new(messages),
        }
    }
}

impl Transport for TestTransport {
    fn send(&self, _session_id: &str, _reply: &OutboundReply) -> Result<(), TransportError> {
        Ok(())
    }

    fn poll(
        &self,
        _session_id: &str,
        since: Option<(chrono::DateTime<Utc>, &[String])>,
        limit: usize,
    ) -> Result<Vec<InboundMessage>, TransportError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|message| {
                since
                    .map(|(since_time, seen)| {
                        message.create_time > since_time
                            || (message.create_time == since_time
                                && !seen.iter().any(|id| id == &message.message_id))
                    })
                    .unwrap_or(true)
            })
            .take(limit)
            .cloned()
            .collect())
    }
}

fn config() -> ChannelConfig {
    let mut channels = BTreeMap::new();
    channels.insert(
        "feishu".to_string(),
        ChannelEntry {
            enabled: true,
            transport: "botmux".to_string(),
            bot_open_id: None,
            botmux_session_id: Some("session-abc".to_string()),
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

fn message(message_id: &str, content: &str) -> InboundMessage {
    message_at(
        message_id,
        content,
        Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
    )
}

fn message_at(
    message_id: &str,
    content: &str,
    create_time: chrono::DateTime<Utc>,
) -> InboundMessage {
    InboundMessage {
        message_id: message_id.to_string(),
        sender_open_id: "ou_owner".to_string(),
        sender_type: "user".to_string(),
        create_time,
        content: content.to_string(),
        thread_id: Some("thread-1".to_string()),
    }
}
