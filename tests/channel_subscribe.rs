use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use maestro::{
    channel::{handle_event, persist, OutboundReply, OUTBOUND_REPLIES_FILE},
    config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry},
    scheduler::events::{RunEvent, RunEventKind},
    schema::channel_envelope::{ChannelAction, ChannelEnvelope},
};
use serde_json::Value;

#[test]
fn channel_subscribe_persists_whitelisted_reply_and_filters_other_events() {
    let temp = tempfile::tempdir().unwrap();
    persist(temp.path(), &envelope()).unwrap();

    let reply = handle_event(
        temp.path(),
        &event(RunEventKind::RunCreated),
        &config(&["run.started"]),
        false,
    )
    .unwrap()
    .expect("whitelisted event should produce reply");

    assert_eq!(reply.event_kind, "run.started");
    assert!(handle_event(
        temp.path(),
        &event(RunEventKind::TaskStarted),
        &config(&["run.started"]),
        false,
    )
    .unwrap()
    .is_none());

    let raw = std::fs::read_to_string(temp.path().join(OUTBOUND_REPLIES_FILE)).unwrap();
    let replies = raw
        .lines()
        .map(|line| serde_json::from_str::<OutboundReply>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies, vec![reply]);
}

fn config(reply_to_run_events: &[&str]) -> ChannelConfig {
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

fn envelope() -> ChannelEnvelope {
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

fn event(kind: RunEventKind) -> RunEvent {
    RunEvent {
        schema_version: maestro::schema::RUN_EVENT_V1.to_string(),
        event_id: "event-1".to_string(),
        run_id: "run-1".to_string(),
        seq: 1,
        timestamp: Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
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
