use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::mpsc;

use super::stream::StreamEvent;

pub mod claude;
pub mod codex;
pub mod cursor;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub session_id: String,
    pub provider_session_id: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
    pub include_thinking: bool,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub full_text: String,
    pub provider_session_id: Option<String>,
    /// Accumulated reasoning tokens for models that emit them (Claude
    /// `-thinking-*`, codex/cursor reasoning). Empty when the provider
    /// doesn't emit thinking or `include_thinking` was false. The server
    /// persists this on the assistant message so the UI can show the
    /// trace alongside the answer instead of losing it after the stream.
    pub thinking: String,
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn binary(&self) -> &'static str;
    fn supports_thinking(&self) -> bool;
    async fn stream(&self, req: ChatRequest, tx: mpsc::Sender<StreamEvent>)
        -> Result<ChatResponse>;
}

pub fn resolve(id: &str) -> Option<Box<dyn ChatProvider>> {
    match id {
        "cursor" => Some(Box::new(cursor::CursorProvider::new())),
        "codex" => Some(Box::new(codex::CodexProvider::new())),
        "claude" | "claude-code" => Some(Box::new(claude::ClaudeProvider::new())),
        _ => None,
    }
}

pub fn default() -> Box<dyn ChatProvider> {
    let configured = std::env::var("MAESTRO_CHAT_PROVIDER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| crate::config::Settings::load().chat.default_provider)
        .unwrap_or_else(|| "cursor".to_string());
    resolve(configured.trim()).unwrap_or_else(|| Box::new(cursor::CursorProvider::new()))
}

pub fn installed_ids() -> Vec<&'static str> {
    ["cursor", "codex", "claude"]
        .into_iter()
        .filter(|id| {
            resolve(id)
                .map(|provider| {
                    let binary = provider.binary();
                    crate::providers::find_binary(binary).is_some()
                })
                .unwrap_or(false)
        })
        .collect()
}
