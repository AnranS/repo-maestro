use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::channel_envelope::ChannelAction;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelConfig {
    pub version: u32,
    #[serde(default)]
    pub defaults: ChannelDefaults,
    #[serde(default)]
    pub channels: BTreeMap<String, ChannelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelDefaults {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default = "default_true")]
    pub reply_to_thread: bool,
}

impl Default for ChannelDefaults {
    fn default() -> Self {
        Self {
            dry_run: true,
            reply_to_thread: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelEntry {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_open_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub botmux_session_id: Option<String>,
    #[serde(default)]
    pub allowed_senders: Vec<AllowedSender>,
    #[serde(default)]
    pub allowed_actions: Vec<ChannelAction>,
    #[serde(default)]
    pub reply_to_run_events: Vec<String>,
    #[serde(default = "default_redact_payload_over_kb")]
    pub redact_payload_over_kb: u32,
}

impl ChannelEntry {
    pub fn transport_session_id<'a>(&'a self, channel: &'a str) -> Option<&'a str> {
        match self.transport.as_str() {
            "mock" => Some(channel),
            _ => self.botmux_session_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedSender {
    pub open_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelConfigError {
    InvalidYaml(String),
    UnsupportedVersion(u32),
    RedactPayloadTooLarge { channel: String, value: u32 },
}

pub fn parse_channels_yaml(text: &str) -> Result<ChannelConfig, ChannelConfigError> {
    let config: ChannelConfig =
        serde_yaml::from_str(text).map_err(|e| ChannelConfigError::InvalidYaml(e.to_string()))?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &ChannelConfig) -> Result<(), ChannelConfigError> {
    if config.version != 1 {
        return Err(ChannelConfigError::UnsupportedVersion(config.version));
    }
    for (channel, entry) in &config.channels {
        if entry.redact_payload_over_kb > 1 {
            return Err(ChannelConfigError::RedactPayloadTooLarge {
                channel: channel.clone(),
                value: entry.redact_payload_over_kb,
            });
        }
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

fn default_redact_payload_over_kb() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
version: 1
channels:
  feishu:
    enabled: true
"#;

    #[test]
    fn parses_minimal_config_with_defaults() {
        let config = parse_channels_yaml(MINIMAL).unwrap();

        assert!(config.defaults.dry_run);
        assert!(config.defaults.reply_to_thread);
        let feishu = &config.channels["feishu"];
        assert!(feishu.enabled);
        assert_eq!(feishu.botmux_session_id, None);
        assert!(feishu.allowed_senders.is_empty());
        assert_eq!(feishu.redact_payload_over_kb, 1);
    }

    #[test]
    fn parses_full_config() {
        let config = parse_channels_yaml(
            r#"
version: 1
defaults:
  dry_run: false
  reply_to_thread: true
channels:
  feishu:
    enabled: true
    transport: botmux
    bot_open_id: ou_bot
    botmux_session_id: session-1
    allowed_senders:
      - open_id: ou_owner
        label: owner
    allowed_actions: [plan, run, approve, status]
    reply_to_run_events:
      - run.started
      - run.completed
    redact_payload_over_kb: 1
  slack:
    enabled: false
"#,
        )
        .unwrap();

        assert!(!config.defaults.dry_run);
        let feishu = &config.channels["feishu"];
        assert_eq!(feishu.transport, "botmux");
        assert_eq!(feishu.bot_open_id.as_deref(), Some("ou_bot"));
        assert_eq!(feishu.botmux_session_id.as_deref(), Some("session-1"));
        assert_eq!(feishu.allowed_senders[0].open_id, "ou_owner");
        assert_eq!(feishu.allowed_senders[0].label.as_deref(), Some("owner"));
        assert_eq!(
            feishu.allowed_actions,
            vec![
                ChannelAction::Plan,
                ChannelAction::Run,
                ChannelAction::Approve,
                ChannelAction::Status
            ]
        );
        assert_eq!(
            feishu.reply_to_run_events,
            vec!["run.started", "run.completed"]
        );
    }

    #[test]
    fn rejects_invalid_config() {
        let too_large = parse_channels_yaml(
            r#"
version: 1
channels:
  feishu:
    enabled: true
    redact_payload_over_kb: 2
"#,
        )
        .unwrap_err();
        assert!(matches!(
            too_large,
            ChannelConfigError::RedactPayloadTooLarge { .. }
        ));

        let bad_version = parse_channels_yaml("version: 2\nchannels: {}\n").unwrap_err();
        assert!(matches!(
            bad_version,
            ChannelConfigError::UnsupportedVersion(2)
        ));
    }
}
