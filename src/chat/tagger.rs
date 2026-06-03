//! AI auto-tagger for chat sessions.
//!
//! Privacy contract — borrowed straight from Reunion:
//! - Only `Role::User` messages are forwarded to the upstream model.
//! - Assistant replies (which may contain code, internal discussion, etc.) are
//!   never sent for tagging.
//!
//! The tagger asks cursor-agent for 1-3 short, lowercase, hyphen-separated
//! tags and parses them out. Anything that doesn't look like a sane tag is
//! discarded.

use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use super::sessions::{save, Role, Session};

const MAX_TAGS: usize = 3;
const MAX_USER_MSGS: usize = 12;
const MAX_TAG_LEN: usize = 32;

/// Compute tags for a single session and persist them. Returns the new tags.
pub async fn auto_tag(session: &mut Session) -> Result<Vec<String>> {
    let user_blob = collect_user_messages(session);
    if user_blob.trim().is_empty() {
        return Ok(vec![]);
    }
    let tags = ask_for_tags(&user_blob).await?;
    if tags.is_empty() {
        return Ok(vec![]);
    }
    session.tags = merge_tags(&session.tags, &tags);
    session.updated_at = chrono::Utc::now();
    save(session)?;
    Ok(tags)
}

fn collect_user_messages(session: &Session) -> String {
    let mut s = String::new();
    let mut count = 0;
    for m in &session.messages {
        if m.role != Role::User {
            continue;
        }
        if count >= MAX_USER_MSGS {
            break;
        }
        count += 1;
        s.push_str("- ");
        for line in m.content.lines().take(6) {
            s.push_str(line);
            s.push(' ');
        }
        s.push('\n');
    }
    s
}

async fn ask_for_tags(user_blob: &str) -> Result<Vec<String>> {
    let prompt = format!(
        "You are a librarian. Given these user messages from a chat session, output 1-3 short tags that describe what the conversation is about. Each tag MUST be lowercase, kebab-case, and ≤ 24 chars. Output ONLY the tags as a comma-separated list, nothing else, no quotes, no explanation.\n\nExamples of good output:\n  webpack-debug, ssr-bug, vite-config\n  rust-async, tokio, error-handling\n\nMessages:\n\n{user_blob}\n\nTags:"
    );

    let binary = std::env::var("MAESTRO_CURSOR_AGENT").unwrap_or_else(|_| "cursor-agent".into());
    let mut cmd = Command::new(&binary);
    cmd.arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--force")
        .arg("--trust");
    if let Some(m) = resolved_tagger_model() {
        cmd.arg("--model").arg(m);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .context("cursor-agent tagger timeout")?
        .context("spawn cursor-agent")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "cursor-agent tagger exited {:?}: {stderr}",
            output.status.code()
        );
    }

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse cursor-agent json")?;
    let raw = value
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(sanitize(&raw))
}

fn sanitize(raw: &str) -> Vec<String> {
    let mut out = vec![];
    // Pick the last non-empty line so we ignore any chain-of-thought noise above
    // the actual answer.
    let candidate_line = raw
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    for piece in candidate_line.split([',', ';', '\n']) {
        let t = piece
            .trim()
            .trim_matches(|c: char| "`'\"#*[]()".contains(c))
            .to_ascii_lowercase();
        let t: String = t
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c
                } else if c == '-' || c == '_' || c.is_whitespace() {
                    '-'
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-");
        let t = t.trim_matches('-').to_string();
        if t.is_empty() || t.len() > MAX_TAG_LEN {
            continue;
        }
        if !out.iter().any(|x: &String| x == &t) {
            out.push(t);
        }
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    out
}

fn resolved_tagger_model() -> Option<String> {
    use crate::config::ProjectsConfig;
    use crate::paths;
    let pfile = paths::projects_file().ok()?;
    if !pfile.exists() {
        return None;
    }
    let cfg = ProjectsConfig::load(&pfile).ok()?;
    cfg.resolved_tagger_model()
}

fn merge_tags(existing: &[String], fresh: &[String]) -> Vec<String> {
    let mut out: Vec<String> = existing.to_vec();
    for t in fresh {
        if !out.iter().any(|e| e == t) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_normalizes_case() {
        let raw = "\"webpack-debug\", 'SSR Bug', vite_config";
        let tags = sanitize(raw);
        assert_eq!(
            tags,
            vec![
                "webpack-debug".to_string(),
                "ssr-bug".to_string(),
                "vite-config".to_string(),
            ]
        );
    }

    #[test]
    fn sanitize_caps_at_max_tags() {
        let raw = "a, b, c, d, e";
        assert_eq!(sanitize(raw).len(), MAX_TAGS);
    }

    #[test]
    fn sanitize_picks_last_line() {
        let raw = "thinking...\nlet me look\ntag1, tag2";
        assert_eq!(sanitize(raw), vec!["tag1", "tag2"]);
    }
}
