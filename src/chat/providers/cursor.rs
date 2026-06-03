use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::{ChatProvider, ChatRequest, ChatResponse};
use crate::chat::stream::StreamEvent;

pub struct CursorProvider {
    binary: String,
}

impl CursorProvider {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("MAESTRO_CURSOR_AGENT")
                .unwrap_or_else(|_| "cursor-agent".to_string()),
        }
    }
}

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ChatProvider for CursorProvider {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn binary(&self) -> &'static str {
        "cursor-agent"
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    async fn stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p")
            .arg(&req.prompt)
            .arg("--workspace")
            .arg(&req.workspace)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--stream-partial-output")
            .arg("--force")
            .arg("--trust");
        if let Some(m) = &req.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(id) = &req.provider_session_id {
            cmd.arg("--resume").arg(id);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "cursor-agent binary not found, install Cursor CLI or set MAESTRO_CURSOR_AGENT (tried `{}`)",
                self.binary
            )
        })?;

        let stdout = child.stdout.take().context("missing cursor stdout")?;
        let mut lines = BufReader::new(stdout).lines();

        let mut streamed_text = String::new();
        let mut streamed_thinking = String::new();
        let mut final_text = String::new();
        let mut cursor_session_id: Option<String> = None;

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let parsed = parse_cursor_event(&value);
            if let Some(sid) = parsed.session_id {
                cursor_session_id.get_or_insert(sid);
            }
            if let Some(result) = parsed.result {
                final_text = result;
            }
            // Cursor's `assistant` events carry a FULL snapshot of the
            // message-so-far on each tick (not an incremental delta), so
            // emitting them raw caused users to see the answer duplicated
            // on stdout — "ABAB" instead of "AB" for a two-char reply.
            // Emit only the SUFFIX that's new since the last snapshot.
            if !parsed.visible.is_empty() {
                let delta = if parsed.visible.starts_with(&streamed_text) {
                    parsed.visible[streamed_text.len()..].to_string()
                } else {
                    // The new snapshot diverged from the prior accumulator
                    // (a tool-use insertion mid-stream, or a model restart).
                    // Treat the new snapshot as the source of truth and
                    // emit only its tail; reset streamed_text to match.
                    streamed_text.clear();
                    parsed.visible.clone()
                };
                if !delta.is_empty() {
                    streamed_text.push_str(&delta);
                    tx.send(StreamEvent::Delta { text: delta }).await.ok();
                }
            }
            if req.include_thinking && !parsed.thinking.is_empty() {
                let delta = if parsed.thinking.starts_with(&streamed_thinking) {
                    parsed.thinking[streamed_thinking.len()..].to_string()
                } else {
                    streamed_thinking.clear();
                    parsed.thinking.clone()
                };
                if !delta.is_empty() {
                    streamed_thinking.push_str(&delta);
                    tx.send(StreamEvent::Thinking { text: delta }).await.ok();
                }
            }
        }

        let full_text = if !final_text.is_empty() {
            final_text
        } else {
            streamed_text
        };

        let status = child.wait().await?;
        if !status.success() {
            let mut err_text = String::new();
            if let Some(stderr) = child.stderr.take() {
                let mut reader = BufReader::new(stderr).lines();
                while let Some(line) = reader.next_line().await? {
                    err_text.push_str(&line);
                    err_text.push('\n');
                }
            }
            tx.send(StreamEvent::Error {
                message: format!("cursor-agent exited {:?}: {}", status.code(), err_text),
            })
            .await
            .ok();
        }

        Ok(ChatResponse {
            full_text,
            provider_session_id: cursor_session_id,
            thinking: streamed_thinking,
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedCursorEvent {
    visible: String,
    thinking: String,
    result: Option<String>,
    session_id: Option<String>,
}

fn parse_cursor_event(value: &serde_json::Value) -> ParsedCursorEvent {
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mut out = ParsedCursorEvent {
        session_id: value
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        ..Default::default()
    };
    match kind {
        "system" => {}
        "assistant" => {
            if let Some(content) = value.pointer("/message/content").and_then(|v| v.as_array()) {
                for piece in content {
                    let ptype = piece.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                    let bucket = match ptype {
                        "thinking" | "reasoning" | "redacted_thinking" => &mut out.thinking,
                        _ => &mut out.visible,
                    };
                    if let Some(t) = piece.get("text").and_then(|v| v.as_str()) {
                        bucket.push_str(t);
                    } else if let Some(t) = piece.get("thinking").and_then(|v| v.as_str()) {
                        bucket.push_str(t);
                    } else if let Some(t) = piece.get("data").and_then(|v| v.as_str()) {
                        bucket.push_str(t);
                    }
                }
            }
        }
        "result" => {
            out.result = value
                .get("result")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_parses_ndjson() {
        let v = serde_json::json!({
            "type": "assistant",
            "session_id": "cur-1",
            "message": {
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "thinking", "thinking": "plan"}
                ]
            }
        });
        let parsed = parse_cursor_event(&v);
        assert_eq!(parsed.visible, "hello");
        assert_eq!(parsed.thinking, "plan");
        assert_eq!(parsed.session_id.as_deref(), Some("cur-1"));
    }

    /// Cursor sends full snapshots, not deltas — pin the suffix-only
    /// diff logic used to avoid doubling stdout. This is the
    /// regression test for the "ABAB" bug seen on real workspace runs.
    #[test]
    fn snapshot_suffix_diff_emits_only_new_chars() {
        // Mirror the live loop's logic in a tiny harness.
        let mut acc = String::new();
        let mut emitted: Vec<String> = Vec::new();
        for snapshot in ["A", "AB", "ABC"] {
            let delta = if snapshot.starts_with(&acc) {
                snapshot[acc.len()..].to_string()
            } else {
                acc.clear();
                snapshot.to_string()
            };
            if !delta.is_empty() {
                acc.push_str(&delta);
                emitted.push(delta);
            }
        }
        assert_eq!(emitted, vec!["A", "B", "C"]);
        assert_eq!(acc, "ABC");
    }

    /// If a snapshot diverges (model restart / tool insertion), the
    /// accumulator resets to the new snapshot instead of stranding the
    /// old prefix in the buffer.
    #[test]
    fn snapshot_diverge_resets_accumulator() {
        let mut acc = String::from("AB");
        let snapshot = "XY"; // wholly different
        let delta = if snapshot.starts_with(&acc) {
            snapshot[acc.len()..].to_string()
        } else {
            acc.clear();
            snapshot.to_string()
        };
        acc.push_str(&delta);
        assert_eq!(delta, "XY");
        assert_eq!(acc, "XY");
    }
}
