use chrono::{TimeZone, Utc};
use maestro::{
    channel::{persist, resolve, thread_for_run, ThreadOrigin, ENVELOPES_FILE},
    schema::channel_envelope::{ChannelAction, ChannelEnvelope},
};

#[test]
fn channel_origin_persists_and_resolves_thread_origin() {
    let temp = tempfile::tempdir().unwrap();
    let envelope = envelope("thread-integration");

    persist(temp.path(), &envelope).unwrap();

    assert!(temp.path().join(ENVELOPES_FILE).is_file());
    assert_eq!(resolve(temp.path()).unwrap(), Some(envelope));
    assert_eq!(
        thread_for_run(temp.path()).unwrap(),
        Some(ThreadOrigin {
            channel: "feishu".to_string(),
            thread_id: "thread-integration".to_string(),
        })
    );
}

fn envelope(thread_id: &str) -> ChannelEnvelope {
    ChannelEnvelope {
        schema_version: maestro::schema::CHANNEL_ENVELOPE_V1.to_string(),
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
