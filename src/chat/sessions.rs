use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::paths;

use super::actions::Action;

pub const CHAT_DIR: &str = "chat";
pub const SESSIONS_DIR: &str = "sessions";
pub const CURRENT_FILE: &str = "current.txt";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,

    /// Reasoning/thinking trace from models that emit one (Claude
    /// `-thinking-*`, cursor reasoning). Persisted so the UI can show it
    /// collapsibly under the answer instead of losing it after the stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role,
            content,
            timestamp: Utc::now(),
            actions: vec![],
            thinking: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    #[serde(default)]
    pub cursor_chat_id: Option<String>,

    #[serde(default)]
    pub messages: Vec<Message>,

    /// Free-form labels (kebab-case lowercase) for filtering and search.
    /// May be set manually or by `maestro chat auto-tag`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Per-session cursor model override. UI updates this when the user picks
    /// from the dropdown; chat uses it (with optional per-message override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_model: Option<String>,

    /// Optional chat provider pin. Missing means resolve from CLI/env/settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    pub is_current: bool,
    pub tags: Vec<String>,
}

impl Session {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: format!("New chat · {}", now.format("%m-%d %H:%M")),
            created_at: now,
            updated_at: now,
            cursor_chat_id: None,
            messages: vec![],
            tags: vec![],
            cursor_model: None,
            chat_provider: None,
        }
    }

    pub fn meta(&self, is_current: bool) -> SessionMeta {
        SessionMeta {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            message_count: self.messages.len(),
            is_current,
            tags: self.tags.clone(),
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

pub fn chat_dir() -> Result<PathBuf> {
    let p = paths::maestro_dir()?.join(CHAT_DIR);
    paths::ensure_dir(&p)?;
    paths::ensure_dir(&p.join(SESSIONS_DIR))?;
    Ok(p)
}

pub fn session_path(id: &str) -> Result<PathBuf> {
    Ok(chat_dir()?.join(SESSIONS_DIR).join(format!("{id}.json")))
}

pub fn current_path() -> Result<PathBuf> {
    Ok(chat_dir()?.join(CURRENT_FILE))
}

pub fn list_sessions() -> Result<Vec<SessionMeta>> {
    let cur = read_current().ok().flatten();
    let dir = chat_dir()?.join(SESSIONS_DIR);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(text) = std::fs::read_to_string(&p) {
                if let Ok(s) = serde_json::from_str::<Session>(&text) {
                    let is_cur = cur.as_deref() == Some(s.id.as_str());
                    out.push(s.meta(is_cur));
                }
            }
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

pub fn load(id: &str) -> Result<Session> {
    let p = session_path(id)?;
    let text = std::fs::read_to_string(&p).with_context(|| format!("read {:?}", p))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save(session: &Session) -> Result<()> {
    let p = session_path(&session.id)?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(session)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn delete(id: &str) -> Result<()> {
    let p = session_path(id)?;
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    if read_current()?.as_deref() == Some(id) {
        let _ = std::fs::remove_file(current_path()?);
    }
    Ok(())
}

pub fn read_current() -> Result<Option<String>> {
    let p = current_path()?;
    if !p.exists() {
        return Ok(None);
    }
    let id = std::fs::read_to_string(&p)?.trim().to_string();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id))
    }
}

pub fn set_current(id: &str) -> Result<()> {
    std::fs::write(current_path()?, id)?;
    Ok(())
}

/// Returns the current session, creating a fresh one if none exists.
pub fn ensure_current() -> Result<Session> {
    if let Some(id) = read_current()? {
        if let Ok(s) = load(&id) {
            return Ok(s);
        }
    }
    let s = Session::new();
    save(&s)?;
    set_current(&s.id)?;
    Ok(s)
}

pub fn update_title_from_first_message(s: &mut Session, user_text: &str) {
    if s.messages.len() <= 1 {
        // Use the user's first words as the title.
        let trimmed: String = user_text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(48)
            .collect();
        if !trimmed.trim().is_empty() {
            s.title = trimmed;
        }
    }
}
