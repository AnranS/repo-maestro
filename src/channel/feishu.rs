use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::{
    artifacts::{ArtifactRef, ArtifactSource},
    channel_envelope::ChannelEnvelope,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuInbound {
    pub thread_id: Option<String>,
    pub sender_open_id: String,
    pub message_id: String,
    pub create_time: DateTime<Utc>,
    pub body: String,
    #[serde(default)]
    pub attachments: Vec<FeishuAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuAttachment {
    pub file_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeishuParseError {
    #[error("thread_id missing")]
    ThreadIdMissing,
    #[error("message body empty")]
    MessageBodyEmpty,
}

pub fn parse_inbound(raw: &FeishuInbound) -> Result<ChannelEnvelope, FeishuParseError> {
    let thread_id = raw
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .ok_or(FeishuParseError::ThreadIdMissing)?;
    let message = strip_feishu_body(&raw.body);
    if message.is_empty() {
        return Err(FeishuParseError::MessageBodyEmpty);
    }

    Ok(ChannelEnvelope {
        schema_version: crate::schema::CHANNEL_ENVELOPE_V1.to_string(),
        channel: "feishu".to_string(),
        thread_id: thread_id.to_string(),
        sender_id: raw.sender_open_id.clone(),
        sender_trusted: false,
        dry_run: true,
        message,
        attachments: raw
            .attachments
            .iter()
            .map(|attachment| ArtifactRef {
                kind: "attachment".to_string(),
                source: ArtifactSource::User,
                task_id: None,
                path: None,
                uri: Some(format!(
                    "feishu://msg/{}/{}",
                    raw.message_id, attachment.file_id
                )),
                name: attachment.name.clone(),
                bytes: attachment.bytes,
            })
            .collect(),
        action: None,
        run_id: None,
        created_at: raw.create_time,
    })
}

fn strip_feishu_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    // Anchor the botmux footer to the END: strip from the LAST line containing
    // `[botmux](`, not the first. The old first-occurrence match truncated a
    // legitimate multi-line user message that merely mentioned the link before
    // its real trailing footer.
    let footer_line = lines.iter().rposition(|line| line.contains("[botmux]("));
    let mut kept: Vec<&str> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if Some(i) == footer_line {
            let index = line.find("[botmux](").unwrap();
            let before_footer = &line[..index];
            if !before_footer.trim().is_empty() {
                kept.push(before_footer);
            }
            break;
        }
        kept.push(line);
    }
    let without_footer = kept.join("\n");

    let mut stripped = without_footer.trim().to_string();
    loop {
        let next = strip_one_leading_mention(&stripped);
        if next == stripped {
            break;
        }
        stripped = next.trim_start().to_string();
    }
    stripped.trim().to_string()
}

fn strip_one_leading_mention(input: &str) -> String {
    let trimmed = input.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<at ") {
        if let Some(end) = rest.find("</at>") {
            let end_index = "<at ".len() + end + "</at>".len();
            return trimmed[end_index..].to_string();
        }
    }

    if let Some(rest) = trimmed.strip_prefix('@') {
        let token_len = rest
            .char_indices()
            .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
            .unwrap_or(rest.len());
        return rest[token_len..].to_string();
    }

    input.to_string()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::schema::artifacts::ArtifactSource;

    use super::{parse_inbound, FeishuAttachment, FeishuInbound, FeishuParseError};

    fn inbound(body: &str) -> FeishuInbound {
        FeishuInbound {
            thread_id: Some("thread-1".to_string()),
            sender_open_id: "ou_sender".to_string(),
            message_id: "om_xxx".to_string(),
            create_time: Utc.with_ymd_and_hms(2026, 5, 24, 6, 0, 0).unwrap(),
            body: body.to_string(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn feishu_inbound_round_trips_through_serde() {
        let raw = r#"
{
  "thread_id": "thread-1",
  "sender_open_id": "ou_sender",
  "message_id": "om_xxx",
  "create_time": "2026-05-24T06:00:00Z",
  "body": "plan",
  "attachments": [
    { "file_id": "file_1", "name": "context.txt", "bytes": 128 }
  ]
}
"#;

        let parsed: FeishuInbound = serde_json::from_str(raw).unwrap();

        assert_eq!(parsed.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(parsed.sender_open_id, "ou_sender");
        assert_eq!(parsed.message_id, "om_xxx");
        assert_eq!(
            parsed.create_time,
            Utc.with_ymd_and_hms(2026, 5, 24, 6, 0, 0).unwrap()
        );
        assert_eq!(parsed.body, "plan");
        assert_eq!(
            parsed.attachments,
            vec![FeishuAttachment {
                file_id: "file_1".to_string(),
                name: Some("context.txt".to_string()),
                bytes: Some(128),
            }]
        );
    }

    #[test]
    fn parse_inbound_maps_minimal_fields() {
        let mut raw = inbound("plan");
        raw.attachments = vec![FeishuAttachment {
            file_id: "file_1".to_string(),
            name: Some("context.txt".to_string()),
            bytes: Some(128),
        }];

        let envelope = parse_inbound(&raw).unwrap();

        assert_eq!(envelope.schema_version, crate::schema::CHANNEL_ENVELOPE_V1);
        assert_eq!(envelope.channel, "feishu");
        assert_eq!(envelope.thread_id, "thread-1");
        assert_eq!(envelope.sender_id, "ou_sender");
        assert!(!envelope.sender_trusted);
        assert!(envelope.dry_run);
        assert_eq!(envelope.message, "plan");
        assert_eq!(envelope.created_at, raw.create_time);
        assert_eq!(envelope.action, None);
        assert_eq!(envelope.run_id, None);
        assert_eq!(envelope.attachments.len(), 1);
        let attachment = &envelope.attachments[0];
        assert_eq!(attachment.source, ArtifactSource::User);
        assert_eq!(attachment.task_id, None);
        assert_eq!(
            attachment.uri.as_deref(),
            Some("feishu://msg/om_xxx/file_1")
        );
        assert_eq!(attachment.path, None);
        assert_eq!(attachment.name.as_deref(), Some("context.txt"));
        assert_eq!(attachment.bytes, Some(128));
    }

    #[test]
    fn parse_inbound_rejects_missing_thread_id() {
        let mut raw = inbound("plan");
        raw.thread_id = None;

        assert!(matches!(
            parse_inbound(&raw).unwrap_err(),
            FeishuParseError::ThreadIdMissing
        ));
    }

    #[test]
    fn parse_inbound_strips_bot_mention_and_footer() {
        let raw = inbound(
            r#"<at user_id="ou_bot"></at> @大力
plan
[botmux](https://github.com/deepcoldy/botmux)<font color='grey'> footer</font>"#,
        );

        let envelope = parse_inbound(&raw).unwrap();

        assert_eq!(envelope.message, "plan");
    }

    #[test]
    fn parse_inbound_handles_empty_attachments() {
        let envelope = parse_inbound(&inbound("status")).unwrap();

        assert!(envelope.attachments.is_empty());
    }

    #[test]
    fn parse_inbound_rejects_empty_message_after_stripping() {
        let raw =
            inbound(r#"<at user_id="ou_bot"></at> [botmux](https://github.com/deepcoldy/botmux)"#);

        assert!(matches!(
            parse_inbound(&raw).unwrap_err(),
            FeishuParseError::MessageBodyEmpty
        ));
    }
}
