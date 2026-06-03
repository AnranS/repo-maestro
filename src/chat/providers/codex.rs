use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::{ChatProvider, ChatRequest, ChatResponse};
use crate::chat::stream::StreamEvent;

pub struct CodexProvider {
    binary: String,
}

impl CodexProvider {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("MAESTRO_CODEX").unwrap_or_else(|_| "codex".to_string()),
        }
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ChatProvider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn binary(&self) -> &'static str {
        "codex"
    }

    fn supports_thinking(&self) -> bool {
        // The codex CLI doesn't stream the reasoning trace as text (see
        // openai/codex#5276) — it only reports `reasoning_output_tokens` in
        // the turn.completed usage. We capture that count and surface a one-
        // line synthetic note so the UI's thinking affordance still signals
        // "the model reasoned" even though the trace itself isn't exposed.
        true
    }

    async fn stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("exec")
            .arg("--cd")
            .arg(&req.workspace)
            .arg("--sandbox")
            .arg("workspace-write")
            .arg("--skip-git-repo-check")
            .arg("--json")
            .arg("-");
        if let Some(model) = req.model.as_deref().filter(|m| !m.trim().is_empty()) {
            cmd.arg("--model").arg(model);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "codex binary not found, install OpenAI Codex CLI or set MAESTRO_CODEX (tried `{}`)",
                self.binary
            )
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(req.prompt.as_bytes()).await?;
            stdin.shutdown().await.ok();
        }

        let stdout = child.stdout.take().context("missing codex stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut streamed_text = String::new();
        let mut final_text = String::new();
        let mut reasoning_tokens: u64 = 0;

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            match parse_event(&line) {
                ParsedCodexEvent::Delta(text) => {
                    streamed_text.push_str(&text);
                    tx.send(StreamEvent::Delta { text }).await.ok();
                }
                ParsedCodexEvent::Final(text) => final_text = text,
                ParsedCodexEvent::ReasoningTokens(n) => reasoning_tokens = reasoning_tokens.max(n),
                ParsedCodexEvent::Error(message) => {
                    tx.send(StreamEvent::Error { message }).await.ok();
                }
                ParsedCodexEvent::Ignore => {}
            }
        }

        // Codex doesn't stream the reasoning trace itself; surface the token
        // count it DID report so the user knows the model reasoned silently.
        let thinking = if req.include_thinking && reasoning_tokens > 0 {
            let note = format!(
                "(codex reasoned silently for {} token(s) — the CLI doesn't expose the trace; \
                 follow https://github.com/openai/codex/issues/5276 for updates.)",
                reasoning_tokens,
            );
            tx.send(StreamEvent::Thinking { text: note.clone() })
                .await
                .ok();
            note
        } else {
            String::new()
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
                message: format!("codex exited {:?}: {}", status.code(), err_text),
            })
            .await
            .ok();
        }

        Ok(ChatResponse {
            full_text: if final_text.is_empty() {
                streamed_text
            } else {
                final_text
            },
            provider_session_id: None,
            thinking,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedCodexEvent {
    Delta(String),
    Final(String),
    ReasoningTokens(u64),
    Error(String),
    Ignore,
}

fn parse_event(line: &str) -> ParsedCodexEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        tracing::warn!("ignore malformed codex json line: {line}");
        return ParsedCodexEvent::Ignore;
    };
    let kind = value
        .get("type")
        .or_else(|| value.get("event"))
        .or_else(|| value.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if kind.to_ascii_lowercase().contains("error") {
        return ParsedCodexEvent::Error(
            find_string(&value, &["message", "error"])
                .unwrap_or_else(|| "codex returned an error event".to_string()),
        );
    }
    if matches!(kind, "result" | "final" | "done") {
        if let Some(text) = find_string(&value, &["result", "text", "content", "message"]) {
            return ParsedCodexEvent::Final(text);
        }
    }
    // `turn.completed` carries usage.reasoning_output_tokens — the only
    // signal codex emits about silent reasoning. Capture it so we can show
    // a "model reasoned for N tokens" badge in the chat.
    if kind == "turn.completed" {
        if let Some(n) = value
            .get("usage")
            .and_then(|u| u.get("reasoning_output_tokens"))
            .and_then(|v| v.as_u64())
        {
            return ParsedCodexEvent::ReasoningTokens(n);
        }
    }
    if let Some(text) = find_string(&value, &["delta", "text", "content", "message"]) {
        if !text.trim().is_empty() {
            return ParsedCodexEvent::Delta(text);
        }
    }
    ParsedCodexEvent::Ignore
}

fn find_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(serde_json::Value::String(s)) = map.get(*key) {
                    return Some(s.clone());
                }
            }
            map.values().find_map(|child| find_string(child, keys))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_string(v, keys)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_delta() {
        let parsed = parse_event(r#"{"type":"message_delta","delta":"hi"}"#);
        assert_eq!(parsed, ParsedCodexEvent::Delta("hi".to_string()));
    }

    #[test]
    fn parse_event_error() {
        let parsed = parse_event(r#"{"type":"error","message":"bad"}"#);
        assert_eq!(parsed, ParsedCodexEvent::Error("bad".to_string()));
    }

    #[test]
    fn parse_event_final() {
        let parsed = parse_event(r#"{"type":"result","result":"done"}"#);
        assert_eq!(parsed, ParsedCodexEvent::Final("done".to_string()));
    }

    #[test]
    fn parse_event_reasoning_tokens_from_turn_completed() {
        // The only thing codex emits about silent reasoning — capture it so
        // the chat thinking badge can at least show a count.
        let parsed = parse_event(
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":3,"reasoning_output_tokens":138}}"#,
        );
        assert_eq!(parsed, ParsedCodexEvent::ReasoningTokens(138));
    }
}
