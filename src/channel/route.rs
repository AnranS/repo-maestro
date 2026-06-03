use crate::{
    config::channels::ChannelConfig,
    schema::channel_envelope::{ChannelAction, ChannelEnvelope},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RouteDecision {
    Dispatch {
        envelope: ChannelEnvelope,
        action: ChannelAction,
        dry_run: bool,
    },
    RejectWithReply {
        reason: RejectReason,
        reply_text: String,
    },
    UsageReply {
        reply_text: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    ChannelDisabled,
    ActionNotAllowed,
    UntrustedMutating,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("route unavailable")]
    Unavailable,
}

pub fn parse_action(message: &str) -> Option<ChannelAction> {
    let action = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .split_whitespace()
        .next()?
        .to_ascii_lowercase();
    match action.as_str() {
        "plan" => Some(ChannelAction::Plan),
        "run" => Some(ChannelAction::Run),
        "approve" => Some(ChannelAction::Approve),
        "status" => Some(ChannelAction::Status),
        _ => None,
    }
}

pub fn has_confirm_token(action: ChannelAction, message: &str) -> bool {
    match action {
        ChannelAction::Run => message.contains("--run"),
        ChannelAction::Approve => message.contains("--confirm"),
        ChannelAction::Plan | ChannelAction::Status => false,
    }
}

pub fn route_inbound(mut envelope: ChannelEnvelope, config: &ChannelConfig) -> RouteDecision {
    let Some(entry) = config.channels.get(&envelope.channel) else {
        return RouteDecision::RejectWithReply {
            reason: RejectReason::ChannelDisabled,
            reply_text: format!("channel `{}` is disabled", envelope.channel),
        };
    };
    if !entry.enabled {
        return RouteDecision::RejectWithReply {
            reason: RejectReason::ChannelDisabled,
            reply_text: format!("channel `{}` is disabled", envelope.channel),
        };
    }

    let Some(action) = parse_action(&envelope.message) else {
        return RouteDecision::UsageReply {
            reply_text: "usage: plan / run / approve / status".to_string(),
        };
    };

    if !entry.allowed_actions.contains(&action) {
        return RouteDecision::RejectWithReply {
            reason: RejectReason::ActionNotAllowed,
            reply_text: format!(
                "action disabled by channel config; allowed actions: {}",
                format_actions(&entry.allowed_actions)
            ),
        };
    }

    let dry_run = match action {
        ChannelAction::Plan | ChannelAction::Status => true,
        ChannelAction::Run | ChannelAction::Approve => {
            if !envelope.sender_trusted {
                return RouteDecision::RejectWithReply {
                    reason: RejectReason::UntrustedMutating,
                    reply_text: "untrusted sender cannot run mutating channel actions".to_string(),
                };
            }
            !has_confirm_token(action, &envelope.message)
        }
    };

    envelope.action = Some(action);
    envelope.dry_run = dry_run;
    RouteDecision::Dispatch {
        envelope,
        action,
        dry_run,
    }
}

fn format_actions(actions: &[ChannelAction]) -> String {
    actions
        .iter()
        .map(action_name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn action_name(action: &ChannelAction) -> &'static str {
    match action {
        ChannelAction::Plan => "plan",
        ChannelAction::Run => "run",
        ChannelAction::Approve => "approve",
        ChannelAction::Status => "status",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use crate::config::channels::{ChannelConfig, ChannelDefaults, ChannelEntry};
    use crate::schema::channel_envelope::ChannelAction;

    use super::{has_confirm_token, parse_action, route_inbound, RejectReason, RouteDecision};

    fn test_config(enabled: bool, allowed_actions: Vec<ChannelAction>) -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: None,
                allowed_senders: Vec::new(),
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
        test_config(
            true,
            vec![
                ChannelAction::Plan,
                ChannelAction::Run,
                ChannelAction::Approve,
                ChannelAction::Status,
            ],
        )
    }

    fn envelope(
        message: &str,
        sender_trusted: bool,
    ) -> crate::schema::channel_envelope::ChannelEnvelope {
        crate::schema::channel_envelope::ChannelEnvelope {
            schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
            channel: "feishu".to_string(),
            thread_id: "thread-1".to_string(),
            sender_id: "ou_sender".to_string(),
            sender_trusted,
            dry_run: true,
            message: message.to_string(),
            attachments: Vec::new(),
            action: None,
            run_id: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 24, 6, 0, 0).unwrap(),
        }
    }

    fn expect_dispatch(
        decision: RouteDecision,
        expected_action: ChannelAction,
        expected_dry_run: bool,
    ) {
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
            }
            other => panic!("expected Dispatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_action_recognizes_all_four_actions() {
        assert_eq!(parse_action("plan"), Some(ChannelAction::Plan));
        assert_eq!(parse_action("RUN"), Some(ChannelAction::Run));
        assert_eq!(parse_action("approve"), Some(ChannelAction::Approve));
        assert_eq!(parse_action("status"), Some(ChannelAction::Status));
    }

    #[test]
    fn parse_action_returns_none_for_unrecognized() {
        assert_eq!(parse_action("please do the thing"), None);
        assert_eq!(parse_action(""), None);
    }

    #[test]
    fn parse_action_uses_first_nonempty_line() {
        assert_eq!(
            parse_action("\n\n  status\nrun --run"),
            Some(ChannelAction::Status)
        );
    }

    #[test]
    fn has_confirm_token_only_matches_corresponding_action() {
        assert!(has_confirm_token(ChannelAction::Run, "run --run"));
        assert!(has_confirm_token(
            ChannelAction::Approve,
            "approve --confirm"
        ));
        assert!(!has_confirm_token(ChannelAction::Run, "run --confirm"));
        assert!(!has_confirm_token(ChannelAction::Approve, "approve --run"));
        assert!(!has_confirm_token(ChannelAction::Plan, "plan --run"));
        assert!(!has_confirm_token(
            ChannelAction::Status,
            "status --confirm"
        ));
    }

    #[test]
    fn route_dispatches_plan_action_as_dry_run() {
        expect_dispatch(
            route_inbound(envelope("plan", false), &all_actions_config()),
            ChannelAction::Plan,
            true,
        );
    }

    #[test]
    fn route_dispatches_status_action_as_dry_run() {
        expect_dispatch(
            route_inbound(envelope("status", false), &all_actions_config()),
            ChannelAction::Status,
            true,
        );
    }

    #[test]
    fn route_dispatches_run_trusted_no_token_as_dry() {
        expect_dispatch(
            route_inbound(envelope("run", true), &all_actions_config()),
            ChannelAction::Run,
            true,
        );
    }

    #[test]
    fn route_dispatches_run_trusted_with_token_as_real() {
        expect_dispatch(
            route_inbound(envelope("run --run", true), &all_actions_config()),
            ChannelAction::Run,
            false,
        );
    }

    #[test]
    fn route_dispatches_approve_trusted_no_token_as_dry() {
        expect_dispatch(
            route_inbound(envelope("approve", true), &all_actions_config()),
            ChannelAction::Approve,
            true,
        );
    }

    #[test]
    fn route_dispatches_approve_trusted_with_token_as_real() {
        expect_dispatch(
            route_inbound(envelope("approve --confirm", true), &all_actions_config()),
            ChannelAction::Approve,
            false,
        );
    }

    #[test]
    fn route_rejects_run_untrusted() {
        match route_inbound(envelope("run --run", false), &all_actions_config()) {
            RouteDecision::RejectWithReply { reason, reply_text } => {
                assert_eq!(reason, RejectReason::UntrustedMutating);
                assert!(reply_text.contains("untrusted sender"));
            }
            other => panic!("expected RejectWithReply, got {other:?}"),
        }
    }

    #[test]
    fn route_rejects_approve_untrusted() {
        match route_inbound(envelope("approve --confirm", false), &all_actions_config()) {
            RouteDecision::RejectWithReply { reason, reply_text } => {
                assert_eq!(reason, RejectReason::UntrustedMutating);
                assert!(reply_text.contains("untrusted sender"));
            }
            other => panic!("expected RejectWithReply, got {other:?}"),
        }
    }

    #[test]
    fn route_rejects_channel_disabled() {
        match route_inbound(
            envelope("plan", true),
            &test_config(false, vec![ChannelAction::Plan]),
        ) {
            RouteDecision::RejectWithReply { reason, .. } => {
                assert_eq!(reason, RejectReason::ChannelDisabled);
            }
            other => panic!("expected RejectWithReply, got {other:?}"),
        }
    }

    #[test]
    fn route_rejects_action_not_in_allowed_actions() {
        match route_inbound(
            envelope("run", true),
            &test_config(true, vec![ChannelAction::Plan, ChannelAction::Status]),
        ) {
            RouteDecision::RejectWithReply { reason, reply_text } => {
                assert_eq!(reason, RejectReason::ActionNotAllowed);
                assert!(reply_text.contains("plan"));
                assert!(reply_text.contains("status"));
            }
            other => panic!("expected RejectWithReply, got {other:?}"),
        }
    }

    #[test]
    fn route_replies_usage_for_unparsable_action() {
        match route_inbound(envelope("please do the thing", true), &all_actions_config()) {
            RouteDecision::UsageReply { reply_text } => {
                assert!(reply_text.contains("plan / run / approve / status"));
            }
            other => panic!("expected UsageReply, got {other:?}"),
        }
    }
}
