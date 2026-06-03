use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::{ChatProvider, ChatRequest, ChatResponse};
use crate::chat::stream::StreamEvent;

pub struct ClaudeProvider {
    binary: String,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("MAESTRO_CLAUDE").unwrap_or_else(|_| "claude".to_string()),
        }
    }
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ChatProvider for ClaudeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn binary(&self) -> &'static str {
        "claude"
    }

    fn supports_thinking(&self) -> bool {
        // Claude's `stream-json` exposes `thinking_delta` blocks when the
        // selected model is a `*-thinking-*` / extended-thinking variant. We
        // always parse them; whether any arrive depends on the model the user
        // pinned (or the CLI's default).
        true
    }

    async fn stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        let mut cmd = Command::new(&self.binary);
        // Stream the structured JSONL event log so we get text + thinking
        // deltas separately. `--verbose` is required by the CLI when stream-json
        // is paired with `--print`; `--include-partial-messages` gives us
        // delta-level events instead of just whole content blocks.
        cmd.arg("--print")
            .arg("-")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--verbose")
            .current_dir(&req.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(model) = req.model.as_deref().filter(|m| !m.trim().is_empty()) {
            cmd.arg("--model").arg(model);
        }

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "claude binary not found, install Claude Code CLI or set MAESTRO_CLAUDE (tried `{}`)",
                self.binary
            )
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(req.prompt.as_bytes()).await?;
            stdin.shutdown().await.ok();
        }

        let stdout = child.stdout.take().context("missing claude stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut streamed_text = String::new();
        let mut streamed_thinking = String::new();
        let mut session_id: Option<String> = None;

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

            // System init event carries the CLI's session id — capture it
            // (parallel to how the cursor provider stashes its own id) so a
            // follow-up turn could resume.
            if kind == "system" && v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                    session_id = Some(sid.to_string());
                }
                continue;
            }

            // The deltas we care about live inside `stream_event.event`.
            if kind != "stream_event" {
                continue;
            }
            let Some(ev) = v.get("event") else { continue };
            let ev_type = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ev_type != "content_block_delta" {
                continue;
            }
            let Some(delta) = ev.get("delta") else {
                continue;
            };
            let dtype = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match dtype {
                "text_delta" => {
                    if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                        streamed_text.push_str(t);
                        tx.send(StreamEvent::Delta {
                            text: t.to_string(),
                        })
                        .await
                        .ok();
                    }
                }
                "thinking_delta" if req.include_thinking => {
                    // Extended thinking: only emitted by `*-thinking-*` models.
                    if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                        streamed_thinking.push_str(t);
                        tx.send(StreamEvent::Thinking {
                            text: t.to_string(),
                        })
                        .await
                        .ok();
                    }
                }
                _ => {}
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            tx.send(StreamEvent::Error {
                message: format!("claude exited {:?}", status.code()),
            })
            .await
            .ok();
        }
        Ok(ChatResponse {
            // Strip claude's self-emitted startup blurbs (e.g. "Using
            // `superpowers:using-superpowers` to satisfy the required
            // startup workflow.") before persisting. They're noise from
            // the CLI's skill auto-mentions, not part of the answer the
            // user asked for. We don't filter deltas as they arrive —
            // the in-flight stream may briefly show them, but the saved
            // message stays clean (and the dashboard re-renders from the
            // saved message on reload).
            full_text: strip_self_noise(&streamed_text),
            provider_session_id: session_id,
            thinking: streamed_thinking,
        })
    }
}

/// Strip claude CLI's startup-skill auto-mentions from the front of a
/// reply. These leak into our chat history as raw "Using
/// \`superpowers:...\` to satisfy the required startup workflow." lines
/// that have nothing to do with the user's question.
///
/// Conservative — only matches the exact "Using \`...\` to satisfy ..."
/// preamble at the start of the text, and stops at the first newline
/// or sentence boundary. Won't touch any other body content.
pub(crate) fn strip_self_noise(text: &str) -> String {
    // Most common shape: "Using `<skill-id>` to satisfy the required
    // startup workflow." possibly followed (no newline) by the real
    // answer. Match the prefix; advance past the trailing period.
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("Using `") {
        if let Some(end) = rest.find("` to satisfy the required startup workflow.") {
            let body_start = end + "` to satisfy the required startup workflow.".len();
            return rest[body_start..].trim_start().to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_self_noise_removes_startup_prefix() {
        let raw = "Using `superpowers:using-superpowers` to satisfy the required startup workflow.AuthCore\nQueueForge";
        assert_eq!(strip_self_noise(raw), "AuthCore\nQueueForge");
    }

    #[test]
    fn strip_self_noise_handles_trailing_newline() {
        let raw = "Using `superpowers:foo` to satisfy the required startup workflow.\nHere's your answer.";
        assert_eq!(strip_self_noise(raw), "Here's your answer.");
    }

    #[test]
    fn strip_self_noise_no_match_returns_unchanged() {
        let raw = "Just a normal answer with no preamble.";
        assert_eq!(strip_self_noise(raw), raw);
    }

    #[test]
    fn strip_self_noise_does_not_match_mid_text() {
        // Only matches if the noise is at the START — a body that
        // happens to mention "Using `foo`" later must stay intact.
        let raw = "Here we go. Using `bar` to satisfy the required startup workflow. Done.";
        assert_eq!(strip_self_noise(raw), raw);
    }
}
