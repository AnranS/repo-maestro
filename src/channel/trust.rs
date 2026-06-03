use crate::{config::channels::ChannelConfig, schema::channel_envelope::ChannelEnvelope};

pub fn resolve_trust(envelope: &mut ChannelEnvelope, config: &ChannelConfig) {
    envelope.sender_trusted = config.channels.get(&envelope.channel).is_some_and(|entry| {
        entry
            .allowed_senders
            .iter()
            .any(|sender| sender.open_id == envelope.sender_id)
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use crate::{
        config::channels::{AllowedSender, ChannelConfig, ChannelDefaults, ChannelEntry},
        schema::channel_envelope::ChannelEnvelope,
    };

    use super::resolve_trust;

    fn config_with_allowed_sender(open_id: &str) -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: None,
                allowed_senders: vec![AllowedSender {
                    open_id: open_id.to_string(),
                    label: None,
                }],
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

    fn envelope(sender_id: &str, sender_trusted: bool) -> ChannelEnvelope {
        ChannelEnvelope {
            schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
            channel: "feishu".to_string(),
            thread_id: "thread-1".to_string(),
            sender_id: sender_id.to_string(),
            sender_trusted,
            dry_run: true,
            message: "plan".to_string(),
            attachments: Vec::new(),
            action: None,
            run_id: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 24, 6, 0, 0).unwrap(),
        }
    }

    #[test]
    fn resolve_trust_sets_true_for_listed_sender() {
        let mut envelope = envelope("ou_trusted", false);
        resolve_trust(&mut envelope, &config_with_allowed_sender("ou_trusted"));
        assert!(envelope.sender_trusted);
    }

    #[test]
    fn resolve_trust_leaves_false_for_unlisted_sender() {
        let mut envelope = envelope("ou_unlisted", false);
        resolve_trust(&mut envelope, &config_with_allowed_sender("ou_trusted"));
        assert!(!envelope.sender_trusted);
    }

    #[test]
    fn resolve_trust_leaves_false_when_channel_missing_from_config() {
        let mut envelope = envelope("ou_trusted", false);
        resolve_trust(
            &mut envelope,
            &ChannelConfig {
                version: 1,
                defaults: ChannelDefaults::default(),
                channels: BTreeMap::new(),
            },
        );
        assert!(!envelope.sender_trusted);
    }

    #[test]
    fn resolve_trust_always_recomputes_does_not_trust_envelope_input() {
        let mut envelope = envelope("ou_unlisted", true);
        resolve_trust(&mut envelope, &config_with_allowed_sender("ou_trusted"));
        assert!(!envelope.sender_trusted);
    }
}
