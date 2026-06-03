use std::collections::BTreeMap;

use maestro::{
    channel::{
        parse_inbound, resolve_trust, route_inbound, FeishuInbound, RejectReason, RouteDecision,
    },
    config::channels::{AllowedSender, ChannelConfig, ChannelDefaults, ChannelEntry},
    schema::{
        artifacts::ArtifactSource,
        channel_envelope::{ChannelAction, ChannelEnvelope},
    },
};

fn load_fixture(name: &str) -> FeishuInbound {
    let path = format!("tests/fixtures/channels/feishu/{name}");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn test_config(allowed_actions: Vec<ChannelAction>) -> ChannelConfig {
    let mut channels = BTreeMap::new();
    channels.insert(
        "feishu".to_string(),
        ChannelEntry {
            enabled: true,
            transport: "botmux".to_string(),
            bot_open_id: None,
            botmux_session_id: None,
            allowed_senders: vec![AllowedSender {
                open_id: "ou_trusted_sender".to_string(),
                label: Some("trusted".to_string()),
            }],
            allowed_actions,
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

fn all_actions_config() -> ChannelConfig {
    test_config(vec![
        ChannelAction::Plan,
        ChannelAction::Run,
        ChannelAction::Approve,
        ChannelAction::Status,
    ])
}

fn route_fixture(name: &str, config: &ChannelConfig) -> RouteDecision {
    let mut envelope = parse_inbound(&load_fixture(name)).unwrap();
    resolve_trust(&mut envelope, config);
    route_inbound(envelope, config)
}

fn assert_dispatch(
    decision: RouteDecision,
    expected_action: ChannelAction,
    expected_dry_run: bool,
) -> ChannelEnvelope {
    match decision {
        RouteDecision::Dispatch {
            envelope,
            action,
            dry_run,
        } => {
            assert_eq!(action, expected_action);
            assert_eq!(dry_run, expected_dry_run);
            assert_eq!(envelope.action, Some(expected_action));
            assert_eq!(envelope.dry_run, expected_dry_run);
            envelope
        }
        other => panic!("expected Dispatch, got {other:?}"),
    }
}

fn assert_reject(decision: RouteDecision, expected_reason: RejectReason) -> String {
    match decision {
        RouteDecision::RejectWithReply { reason, reply_text } => {
            assert_eq!(reason, expected_reason);
            reply_text
        }
        other => panic!("expected RejectWithReply, got {other:?}"),
    }
}

#[test]
fn valid_plan_action_dispatches_dry_run() {
    assert_dispatch(
        route_fixture("valid_plan_action.json", &all_actions_config()),
        ChannelAction::Plan,
        true,
    );
}

#[test]
fn valid_status_action_dispatches_dry_run_for_untrusted_sender() {
    assert_dispatch(
        route_fixture("valid_status_action.json", &all_actions_config()),
        ChannelAction::Status,
        true,
    );
}

#[test]
fn valid_run_action_trusted_dry_dispatches_preview() {
    assert_dispatch(
        route_fixture("valid_run_action_trusted_dry.json", &all_actions_config()),
        ChannelAction::Run,
        true,
    );
}

#[test]
fn valid_run_action_trusted_real_dispatches_real() {
    assert_dispatch(
        route_fixture("valid_run_action_trusted_real.json", &all_actions_config()),
        ChannelAction::Run,
        false,
    );
}

#[test]
fn valid_approve_action_trusted_dry_dispatches_preview() {
    assert_dispatch(
        route_fixture(
            "valid_approve_action_trusted_dry.json",
            &all_actions_config(),
        ),
        ChannelAction::Approve,
        true,
    );
}

#[test]
fn valid_approve_action_trusted_real_dispatches_real() {
    assert_dispatch(
        route_fixture(
            "valid_approve_action_trusted_real.json",
            &all_actions_config(),
        ),
        ChannelAction::Approve,
        false,
    );
}

#[test]
fn run_action_untrusted_rejected() {
    let reply = assert_reject(
        route_fixture("run_action_untrusted_rejected.json", &all_actions_config()),
        RejectReason::UntrustedMutating,
    );
    assert!(reply.contains("untrusted sender"));
}

#[test]
fn approve_action_untrusted_rejected() {
    let reply = assert_reject(
        route_fixture(
            "approve_action_untrusted_rejected.json",
            &all_actions_config(),
        ),
        RejectReason::UntrustedMutating,
    );
    assert!(reply.contains("untrusted sender"));
}

#[test]
fn unparsable_action_usage_reply() {
    match route_fixture("unparsable_action_usage_reply.json", &all_actions_config()) {
        RouteDecision::UsageReply { reply_text } => {
            assert!(reply_text.contains("plan / run / approve / status"));
        }
        other => panic!("expected UsageReply, got {other:?}"),
    }
}

#[test]
fn action_disabled_by_config_rejected() {
    let reply = assert_reject(
        route_fixture(
            "action_disabled_by_config_rejected.json",
            &test_config(vec![ChannelAction::Plan, ChannelAction::Status]),
        ),
        RejectReason::ActionNotAllowed,
    );
    assert!(reply.contains("plan"));
    assert!(reply.contains("status"));
}

#[test]
fn inbound_attachment_artifact_ref_dispatches_with_user_attachment() {
    let envelope = assert_dispatch(
        route_fixture(
            "inbound_attachment_artifact_ref.json",
            &all_actions_config(),
        ),
        ChannelAction::Plan,
        true,
    );
    assert_eq!(envelope.attachments.len(), 1);
    let attachment = &envelope.attachments[0];
    assert_eq!(attachment.source, ArtifactSource::User);
    assert_eq!(attachment.task_id, None);
    assert_eq!(
        attachment.uri.as_deref(),
        Some("feishu://msg/om_xxx/file_1")
    );
    assert_eq!(attachment.path, None);
}
