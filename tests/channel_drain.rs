use std::collections::BTreeMap;
use std::sync::Mutex;

use maestro::channel::{
    drain_once, OutboundReply, Transport, TransportError, OUTBOUND_REPLIES_FILE,
};
use maestro::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};

#[test]
fn channel_drain_sends_new_replies_and_respects_cursor() {
    let temp = tempfile::tempdir().unwrap();
    write_replies(
        temp.path(),
        &[reply("run-1"), reply("run-2"), reply("run-3")],
    );
    let transport = TestTransport::default();

    let first = drain_once(temp.path(), &config(), &transport).unwrap();
    let second = drain_once(temp.path(), &config(), &transport).unwrap();

    assert_eq!(first.sent, 3);
    assert_eq!(second.sent, 0);
    let sent = transport.sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].0, "session-abc");
    assert_eq!(sent[2].1.run_id, "run-3");
}

#[derive(Default)]
struct TestTransport {
    sent: Mutex<Vec<(String, OutboundReply)>>,
}

impl Transport for TestTransport {
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
        _since: Option<(chrono::DateTime<chrono::Utc>, &[String])>,
        _limit: usize,
    ) -> Result<Vec<maestro::channel::InboundMessage>, TransportError> {
        Ok(Vec::new())
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
    std::fs::write(run_dir.join(OUTBOUND_REPLIES_FILE), format!("{raw}\n")).unwrap();
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
