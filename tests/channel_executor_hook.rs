use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use maestro::channel::{persist, OutboundReply, OUTBOUND_REPLIES_FILE};
use maestro::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};
use maestro::scheduler::events::{append_event_with_subscribe, RunEventKind};
use maestro::schema::channel_envelope::{ChannelAction, ChannelEnvelope};

#[test]
fn append_event_with_subscribe_writes_outbound_reply_for_run_origin() {
    let temp = tempfile::tempdir().unwrap();
    persist(temp.path(), &origin_envelope()).unwrap();

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

    let raw = std::fs::read_to_string(temp.path().join(OUTBOUND_REPLIES_FILE)).unwrap();
    let reply: OutboundReply = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    assert_eq!(reply.channel, "feishu");
    assert_eq!(reply.event_kind, "run.started");
    assert_eq!(reply.run_id, "run-1");
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
        schema_version: maestro::schema::CHANNEL_ENVELOPE_V1.to_string(),
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
