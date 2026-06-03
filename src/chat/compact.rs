//! `/compact`: summarize the older half of a session into a single system
//! message and discard the originals.
//!
//! Privacy note (different from `tagger`): unlike auto-tagging, which
//! sends only user messages, compaction needs the full transcript to
//! produce a useful summary. This is OK because the user explicitly asks
//! for it via the `/compact` command — they're consenting to ship that
//! one summarization request to the model API.
//!
//! Real token savings: we clear `cursor_chat_id` after compacting, so the
//! NEXT message starts a fresh cursor-agent chat. Until that happens,
//! cursor-agent's server still holds the old transcript; only the maestro
//! UI's view of the session is compacted.

use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use super::sessions::{save, Message, Role, Session};
use crate::config::ProjectsConfig;
use crate::paths;

/// Keep this many most-recent messages verbatim; everything older gets
/// collapsed into the summary. 6 is a sweet spot — usually one or two
/// recent question/answer pairs, enough that the user doesn't feel like
/// the conversation was reset under them.
const KEEP_RECENT: usize = 6;

/// Skip the work if there's not enough history to be worth compressing.
const MIN_TO_COMPACT: usize = KEEP_RECENT + 4;

/// Compact one session in place. Returns the number of messages that were
/// folded into the summary (0 if the session was already short enough).
pub async fn compact_session(session: &mut Session) -> Result<usize> {
    if session.messages.len() < MIN_TO_COMPACT {
        return Ok(0);
    }
    let cutoff = session.messages.len() - KEEP_RECENT;
    let to_summarize = &session.messages[..cutoff];
    let blob = render_transcript(to_summarize);
    let summary = ask_for_summary(&blob).await?;

    let summary_msg = Message {
        id: format!("compact-{}", chrono::Utc::now().timestamp_millis()),
        role: Role::System,
        content: format!(
            "## Conversation summary (compacted {} earlier message{})\n\n{}",
            cutoff,
            if cutoff == 1 { "" } else { "s" },
            summary.trim()
        ),
        timestamp: chrono::Utc::now(),
        actions: vec![],
        thinking: None,
    };

    let kept = session.messages[cutoff..].to_vec();
    session.messages = std::iter::once(summary_msg).chain(kept).collect();
    // Forget the cursor-side chat id so the next user message starts a
    // fresh cursor-agent chat — that's what actually saves tokens.
    session.cursor_chat_id = None;
    session.updated_at = chrono::Utc::now();
    save(session)?;
    Ok(cutoff)
}

fn render_transcript(msgs: &[Message]) -> String {
    let mut s = String::new();
    for m in msgs {
        let tag = match m.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::System => "SYSTEM",
        };
        s.push_str(&format!("[{tag}] {}\n\n", m.content.trim()));
    }
    s
}

async fn ask_for_summary(transcript: &str) -> Result<String> {
    let prompt = format!(
        "Below is a chat transcript between a developer and an AI orchestrator. Summarize it in 6-10 dense bullet points capturing:\n\
         - the goal the user is working on\n\
         - decisions made and *why* (with file/function names where possible)\n\
         - any unresolved questions or pending actions\n\n\
         Output Markdown bullets only — no preamble, no closing line.\n\n\
         Transcript:\n\n{transcript}"
    );

    let binary = std::env::var("MAESTRO_CURSOR_AGENT").unwrap_or_else(|_| "cursor-agent".into());
    let mut cmd = Command::new(&binary);
    cmd.arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--force")
        .arg("--trust");
    // Reuse the tagger model — it's the cheap one for exactly this kind
    // of "boil text down" task.
    if let Some(m) = resolved_tagger_model() {
        cmd.arg("--model").arg(m);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(90), cmd.output())
        .await
        .context("cursor-agent compact timeout")?
        .context("spawn cursor-agent")?;
    if !output.status.success() {
        anyhow::bail!(
            "cursor-agent compact exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse cursor-agent json")?;
    let body = value
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if body.is_empty() {
        anyhow::bail!("cursor-agent returned an empty summary");
    }
    Ok(body)
}

fn resolved_tagger_model() -> Option<String> {
    let pfile = paths::projects_file().ok()?;
    if !pfile.exists() {
        return None;
    }
    let cfg = ProjectsConfig::load(&pfile).ok()?;
    cfg.resolved_tagger_model()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::sessions::{Message, Role, Session};

    fn msg(role: Role, content: &str) -> Message {
        Message {
            id: format!(
                "m-{}-{}",
                &content[..content.len().min(3)],
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ),
            role,
            content: content.into(),
            timestamp: chrono::Utc::now(),
            actions: vec![],
            thinking: None,
        }
    }

    #[tokio::test]
    async fn short_session_is_left_alone() {
        let mut s = Session::new();
        // 4 messages → far below MIN_TO_COMPACT
        s.messages.push(msg(Role::User, "hi"));
        s.messages.push(msg(Role::Assistant, "hello"));
        s.messages.push(msg(Role::User, "do thing"));
        s.messages.push(msg(Role::Assistant, "ok"));
        let before = s.messages.clone();
        // Don't bother with the cursor-agent call — this branch returns
        // before any IO. We just need to confirm we get 0 back and the
        // session is untouched.
        let n = compact_session(&mut s).await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(s.messages.len(), before.len());
    }

    #[test]
    fn render_transcript_tags_roles_explicitly() {
        let msgs = vec![
            msg(Role::User, "what is X"),
            msg(Role::Assistant, "X is Y"),
            msg(Role::System, "(internal)"),
        ];
        let out = render_transcript(&msgs);
        assert!(out.contains("[USER] what is X"));
        assert!(out.contains("[ASSISTANT] X is Y"));
        assert!(out.contains("[SYSTEM] (internal)"));
    }
}
