use serde::{Deserialize, Serialize};

use crate::config::channels::ChannelEntry;
use crate::scheduler::events::{RunEvent, RunEventKind};
use crate::schema::artifacts::ArtifactRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundReply {
    pub channel: String,
    pub run_id: String,
    pub event_kind: String,
    pub title: String,
    pub body: String,
    pub attachments: Vec<ArtifactRef>,
    /// F-134: the owning delivery, when this reply is a `delivery.*` write-back (so the
    /// external drainer can route a receipt back to the delivery without a reverse
    /// lookup). `None` for ordinary run-event replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    /// F-134: a stable content hash identifying THIS write-back intent — the drainer
    /// echoes it back on the receipt so a stale receipt can't be applied. Set at emit;
    /// the SAME key is persisted on `closeout.writeback.idempotency_key`. `None` for
    /// ordinary run-event replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub dry_run: bool,
}

pub fn format_event(
    event: &RunEvent,
    channel: &str,
    entry: &ChannelEntry,
    options: FormatOptions,
) -> Option<OutboundReply> {
    // Match against the canonical v2 kind AND any legacy collapsed alias, so a
    // config still subscribing to a pre-F-115 wire kind keeps receiving the
    // de-collapsed events (N1). The reply itself still reports the canonical kind.
    let subscription_keys = event.kind.subscription_keys();
    if !entry
        .reply_to_run_events
        .iter()
        .any(|allowed| subscription_keys.iter().any(|key| key == allowed))
    {
        return None;
    }

    let event_kind = event.kind.as_str();
    let title = render_title(event_kind, &event.run_id, options.dry_run);
    let body = render_body(event, entry);
    let attachments = event.refs.values().cloned().collect();

    Some(OutboundReply {
        channel: channel.to_string(),
        run_id: event.run_id.clone(),
        event_kind: event_kind.to_string(),
        title,
        body,
        attachments,
        delivery_id: None,
        idempotency_key: None,
    })
}

fn render_title(event_kind: &str, run_id: &str, dry_run: bool) -> String {
    let prefix = if dry_run { "[DRY] " } else { "" };
    format!("{prefix}{event_kind} · {run_id}")
}

fn render_body(event: &RunEvent, entry: &ChannelEntry) -> String {
    let mut lines = vec![
        format!("run_id: {}", event.run_id),
        format!("event_id: {}", event.event_id),
        format!("seq: {}", event.seq),
    ];
    if let Some(task_id) = &event.task_id {
        lines.push(format!("task_id: {task_id}"));
    }
    if let Some(message) = &event.message {
        lines.push(format!("message: {message}"));
    }
    if matches!(event.kind, RunEventKind::TaskApprovalRequested) {
        lines.push(format!(
            "approval_hint: reply approve --confirm with run_id {}",
            event.run_id
        ));
    }
    let payload_bytes = serde_json::to_vec(&event.payload).unwrap_or_default();
    let cap_bytes = entry.redact_payload_over_kb as usize * 1024;
    if payload_bytes.len() <= cap_bytes {
        let payload = String::from_utf8(payload_bytes).unwrap_or_else(|_| "null".to_string());
        lines.push(format!("payload: {payload}"));
    } else {
        lines.push(format!(
            "payload omitted ({} bytes > {} KB cap)",
            payload_bytes.len(),
            entry.redact_payload_over_kb
        ));
    }
    if !event.refs.is_empty() {
        lines.push(format!("attachments: {}", event.refs.len()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use serde_json::Value;

    use crate::config::channels::ChannelEntry;
    use crate::scheduler::events::{RunEvent, RunEventKind};
    use crate::schema::artifacts::{ArtifactRef, ArtifactSource};

    #[test]
    fn outbound_reply_round_trips_through_serde() {
        let reply = super::OutboundReply {
            channel: "feishu".to_string(),
            run_id: "run-1".to_string(),
            event_kind: "run.started".to_string(),
            title: "run.started · run-1".to_string(),
            body: "run_id: run-1".to_string(),
            attachments: Vec::new(),
            delivery_id: None,
            idempotency_key: None,
        };

        let encoded = serde_json::to_string(&reply).unwrap();
        let decoded: super::OutboundReply = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, reply);
    }

    #[test]
    fn format_event_returns_none_when_kind_not_whitelisted() {
        let event = event(RunEventKind::TaskStarted);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        );

        assert_eq!(reply, None);
    }

    #[test]
    fn format_event_returns_some_when_kind_is_whitelisted() {
        let event = event(RunEventKind::RunCreated);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert_eq!(reply.event_kind, "run.started");
    }

    #[test]
    fn legacy_collapsed_subscription_still_matches_decollapsed_events() {
        // A pre-F-115 config subscribing to the old collapsed wire kind keeps
        // receiving the de-collapsed events; the reply reports the canonical kind.
        let cases = [
            (
                RunEventKind::TaskApprovalGranted,
                "task.completed",
                "task.approval_granted",
            ),
            (
                RunEventKind::VerifyCompleted,
                "task.completed",
                "verify.completed",
            ),
            (RunEventKind::TaskSkipped, "task.cancelled", "task.skipped"),
            (RunEventKind::RunCancelled, "run.failed", "run.cancelled"),
        ];
        for (kind, legacy_sub, canonical) in cases {
            let entry = entry(&[legacy_sub]);
            let reply = super::format_event(
                &event(kind),
                "feishu",
                &entry,
                super::FormatOptions { dry_run: false },
            )
            .expect("legacy subscription should still match the de-collapsed event");
            assert_eq!(reply.event_kind, canonical);
        }
        // and a new canonical subscription matches too
        let entry = entry(&["task.approval_granted"]);
        assert!(super::format_event(
            &event(RunEventKind::TaskApprovalGranted),
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .is_some());
    }

    #[test]
    fn legacy_run_failed_subscription_matches_all_terminal_kinds() {
        // A pre-F-115 ["run.failed"] subscription matched the collapsed terminal
        // run events; it must still match RunFailed / CancelRequested / RunCancelled,
        // each reported under its own canonical v2 kind.
        let cases = [
            (RunEventKind::RunFailed, "run.failed"),
            (RunEventKind::CancelRequested, "run.cancel_requested"),
            (RunEventKind::RunCancelled, "run.cancelled"),
        ];
        for (kind, canonical) in cases {
            let entry = entry(&["run.failed"]);
            let reply = super::format_event(
                &event(kind),
                "feishu",
                &entry,
                super::FormatOptions { dry_run: false },
            )
            .expect("legacy run.failed subscription should match");
            assert_eq!(reply.event_kind, canonical);
        }
    }

    #[test]
    fn legacy_run_completed_subscription_matches_run_failed() {
        // A failed run used to be a run.completed event; a ["run.completed"]
        // subscription must still see it, reported under canonical run.failed.
        let entry = entry(&["run.completed"]);
        let reply = super::format_event(
            &event(RunEventKind::RunFailed),
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("run.completed subscription should still match a failed run");
        assert_eq!(reply.event_kind, "run.failed");
    }

    #[test]
    fn title_drops_dry_prefix_when_not_dry() {
        let event = event(RunEventKind::RunCreated);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert_eq!(reply.title, "run.started · run-1");
    }

    #[test]
    fn title_includes_dry_prefix_when_dry() {
        let event = event(RunEventKind::RunCreated);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: true },
        )
        .expect("event should format");

        assert_eq!(reply.title, "[DRY] run.started · run-1");
    }

    #[test]
    fn body_includes_run_id_event_id_seq_payload_for_small_payload() {
        let mut event = event(RunEventKind::RunCreated);
        event.payload = serde_json::json!({"foo": "bar"});
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert!(reply.body.contains("run_id: run-1"));
        assert!(reply.body.contains("event_id: event-1"));
        assert!(reply.body.contains("seq: 1"));
        assert!(reply.body.contains(r#"payload: {"foo":"bar"}"#));
    }

    #[test]
    fn body_includes_task_id_and_message_when_present() {
        let mut event = event(RunEventKind::TaskApprovalRequested);
        event.task_id = Some("task-1".to_string());
        event.message = Some("approval required".to_string());
        event.payload = serde_json::json!({"why": "human"});
        let entry = entry(&["task.approval_required"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert!(reply.body.contains("task_id: task-1"));
        assert!(reply.body.contains("message: approval required"));
        assert!(reply.body.contains(r#"payload: {"why":"human"}"#));
    }

    #[test]
    fn body_summarizes_oversize_payload() {
        let mut event = event(RunEventKind::RunCreated);
        let secret_payload = "a".repeat(1100);
        event.payload = serde_json::json!(secret_payload);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        let bytes = serde_json::to_vec(&event.payload).unwrap().len();
        assert!(reply
            .body
            .contains(&format!("payload omitted ({bytes} bytes > 1 KB cap)")));
        assert!(!reply.body.contains(&"a".repeat(1100)));
    }

    #[test]
    fn body_uses_inline_payload_at_exact_threshold() {
        let mut event = event(RunEventKind::RunCreated);
        event.payload = serde_json::json!("a".repeat(1022));
        assert_eq!(serde_json::to_vec(&event.payload).unwrap().len(), 1024);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert!(reply.body.contains(&format!(
            "payload: {}",
            serde_json::to_string(&event.payload).unwrap()
        )));
    }

    #[test]
    fn attachments_lifted_from_refs_in_order() {
        let mut event = event(RunEventKind::RunCreated);
        event
            .refs
            .insert("b".to_string(), artifact(ArtifactSource::User));
        event
            .refs
            .insert("a".to_string(), artifact(ArtifactSource::External));
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert_eq!(reply.attachments.len(), 2);
        assert_eq!(reply.attachments[0].source, ArtifactSource::External);
        assert_eq!(reply.attachments[1].source, ArtifactSource::User);
    }

    #[test]
    fn body_includes_attachment_count_line_when_refs_nonempty() {
        let mut event = event(RunEventKind::RunCreated);
        event
            .refs
            .insert("a".to_string(), artifact(ArtifactSource::External));
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert!(reply.body.contains("attachments: 1"));
    }

    #[test]
    fn body_omits_attachment_count_line_when_refs_empty() {
        let event = event(RunEventKind::RunCreated);
        let entry = entry(&["run.started"]);

        let reply = super::format_event(
            &event,
            "feishu",
            &entry,
            super::FormatOptions { dry_run: false },
        )
        .expect("event should format");

        assert!(!reply.body.contains("attachments:"));
    }

    fn event(kind: RunEventKind) -> RunEvent {
        RunEvent {
            schema_version: crate::schema::RUN_EVENT_V1.to_string(),
            event_id: "event-1".to_string(),
            run_id: "run-1".to_string(),
            seq: 1,
            timestamp: Utc::now(),
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

    fn artifact(source: ArtifactSource) -> ArtifactRef {
        ArtifactRef {
            kind: "log".to_string(),
            source,
            task_id: Some("task-1".to_string()),
            path: Some("logs/task.log".to_string()),
            uri: Some("file://logs/task.log".to_string()),
            name: Some("task.log".to_string()),
            bytes: Some(42),
        }
    }
}
