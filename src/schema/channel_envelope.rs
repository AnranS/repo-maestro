use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema::artifacts::ArtifactRef;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAction {
    Plan,
    Run,
    Approve,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelEnvelope {
    #[serde(default = "crate::schema::channel_envelope_version")]
    pub schema_version: String,
    pub channel: String,
    pub thread_id: String,
    pub sender_id: String,
    pub sender_trusted: bool,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ChannelAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    SchemaVersionMismatch,
    ThreadIdMissing,
    ActionNotAllowed {
        action: ChannelAction,
        allowed: Vec<ChannelAction>,
    },
    InvalidEnvelope(String),
}

pub fn parse_envelope(raw: &str) -> Result<ChannelEnvelope, EnvelopeError> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| EnvelopeError::InvalidEnvelope(e.to_string()))?;
    let found_version = value
        .get("schema_version")
        .and_then(|version| version.as_str());
    if let Some(version) = found_version {
        if version != crate::schema::CHANNEL_ENVELOPE_V1 {
            return Err(EnvelopeError::SchemaVersionMismatch);
        }
    }
    let thread_id = value
        .get("thread_id")
        .and_then(|thread_id| thread_id.as_str());
    if thread_id.is_none_or(|thread_id| thread_id.trim().is_empty()) {
        return Err(EnvelopeError::ThreadIdMissing);
    }
    serde_json::from_value(value).map_err(|e| EnvelopeError::InvalidEnvelope(e.to_string()))
}

pub fn validate_action(
    envelope: &ChannelEnvelope,
    allowed: &[ChannelAction],
) -> Result<(), EnvelopeError> {
    let Some(action) = envelope.action else {
        return Ok(());
    };
    if allowed.contains(&action) {
        return Ok(());
    }
    Err(EnvelopeError::ActionNotAllowed {
        action,
        allowed: allowed.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::artifacts::{ArtifactRef, ArtifactSource};
    use chrono::{TimeZone, Utc};

    fn envelope(action: Option<ChannelAction>) -> ChannelEnvelope {
        ChannelEnvelope {
            schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
            channel: "feishu".to_string(),
            thread_id: "thread-1".to_string(),
            sender_id: "ou_sender".to_string(),
            sender_trusted: true,
            dry_run: true,
            message: "status".to_string(),
            attachments: Vec::new(),
            action,
            run_id: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 24, 5, 0, 0).unwrap(),
        }
    }

    #[test]
    fn channel_envelope_schema_version_is_stable() {
        assert_eq!(
            crate::schema::CHANNEL_ENVELOPE_V1,
            "maestro.channel_envelope.v1"
        );
    }

    #[test]
    fn channel_action_serializes_as_v1_values() {
        for (action, value) in [
            (ChannelAction::Plan, "plan"),
            (ChannelAction::Run, "run"),
            (ChannelAction::Approve, "approve"),
            (ChannelAction::Status, "status"),
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), value);
        }
        let err = serde_json::from_str::<ChannelAction>(r#""foo""#).unwrap_err();
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn envelope_round_trips_with_attachments_and_action() {
        let mut envelope = envelope(Some(ChannelAction::Run));
        envelope.dry_run = false;
        envelope.message = "run --run".to_string();
        envelope.run_id = Some("run-1".to_string());
        envelope.attachments = vec![ArtifactRef {
            kind: "attachment".to_string(),
            source: ArtifactSource::User,
            task_id: None,
            path: None,
            uri: Some("feishu://msg/om_x/file_1".to_string()),
            name: Some("context.txt".to_string()),
            bytes: Some(128),
        }];

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: ChannelEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, envelope);
    }

    #[test]
    fn parse_envelope_rejects_v2_and_missing_thread_id() {
        let raw = serde_json::to_string(&serde_json::json!({
            "schema_version": "maestro.channel_envelope.v2",
            "thread_id": "thread-1",
        }))
        .unwrap();
        assert!(matches!(
            parse_envelope(&raw).unwrap_err(),
            EnvelopeError::SchemaVersionMismatch
        ));

        let mut missing_thread = serde_json::to_value(envelope(None)).unwrap();
        missing_thread.as_object_mut().unwrap().remove("thread_id");
        assert!(matches!(
            parse_envelope(&missing_thread.to_string()).unwrap_err(),
            EnvelopeError::ThreadIdMissing
        ));
    }

    #[test]
    fn validate_action_enforces_allowed_actions() {
        let err = validate_action(
            &envelope(Some(ChannelAction::Run)),
            &[ChannelAction::Plan, ChannelAction::Status],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            EnvelopeError::ActionNotAllowed {
                action: ChannelAction::Run,
                ..
            }
        ));
        validate_action(&envelope(None), &[ChannelAction::Plan]).unwrap();
    }
}
