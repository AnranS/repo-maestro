//! Read-only view onto the other AI tools' local session stores. We do **not**
//! touch their data; we only list what's on disk so the dashboard can show
//! "yes, you also have 87 Cursor IDE chats for this workspace".
//!
//! Currently we support `~/.cursor/chats/<workspace-hash>/<chat-uuid>`. Codex
//! and Claude Code formats are placeholders until we have a real sample.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize)]
pub struct ExternalSession {
    pub source: String,
    /// e.g. workspace hash for Cursor, identifier from the source.
    pub workspace_id: String,
    pub id: String,
    pub path: String,
    pub modified_at: Option<DateTime<Utc>>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalSummary {
    pub source: String,
    pub session_count: usize,
    pub workspace_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalListing {
    pub summaries: Vec<ExternalSummary>,
    pub sessions: Vec<ExternalSession>,
}

pub async fn list_sessions(source_filter: Option<&str>) -> Result<ExternalListing> {
    let want = |s: &str| source_filter.map(|f| f == s).unwrap_or(true);

    let mut sessions = vec![];
    if want("cursor") {
        sessions.extend(list_cursor());
    }
    if want("claude") {
        sessions.extend(list_claude());
    }
    if want("codex") {
        sessions.extend(list_codex());
    }
    sessions.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));

    let mut by_source: std::collections::BTreeMap<
        String,
        (usize, std::collections::HashSet<String>),
    > = Default::default();
    for s in &sessions {
        let entry = by_source
            .entry(s.source.clone())
            .or_insert((0, Default::default()));
        entry.0 += 1;
        entry.1.insert(s.workspace_id.clone());
    }
    let summaries = by_source
        .into_iter()
        .map(|(source, (n, ws))| ExternalSummary {
            source,
            session_count: n,
            workspace_count: ws.len(),
        })
        .collect();

    Ok(ExternalListing {
        summaries,
        sessions,
    })
}

fn list_cursor() -> Vec<ExternalSession> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let root = home.join(".cursor").join("chats");
    if !root.is_dir() {
        return vec![];
    }
    let mut out = vec![];
    let Ok(top) = std::fs::read_dir(&root) else {
        return out;
    };
    for ws_entry in top.flatten() {
        if !ws_entry.path().is_dir() {
            continue;
        }
        let workspace_id = ws_entry.file_name().to_string_lossy().to_string();
        let Ok(inner) = std::fs::read_dir(ws_entry.path()) else {
            continue;
        };
        for chat in inner.flatten() {
            let p = chat.path();
            // Cursor stores each chat as a dir with a UUID name; in some
            // older versions it's a flat file. Handle both.
            let id = chat.file_name().to_string_lossy().to_string();
            let modified = chat
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(systime_to_chrono);
            let bytes = chat.metadata().ok().map(|m| m.len()).unwrap_or(0);
            out.push(ExternalSession {
                source: "cursor".into(),
                workspace_id: workspace_id.clone(),
                id,
                path: p.to_string_lossy().to_string(),
                modified_at: modified,
                bytes,
            });
        }
    }
    out
}

fn list_claude() -> Vec<ExternalSession> {
    // Claude Code stores conversations under ~/.claude/projects/<workspace>/
    // (as of this writing). Format will likely change; treat very defensively.
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let root = home.join(".claude").join("projects");
    if !root.is_dir() {
        return vec![];
    }
    walk_simple(&root, "claude")
}

fn list_codex() -> Vec<ExternalSession> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let root = home.join(".codex").join("sessions");
    if !root.is_dir() {
        return vec![];
    }
    walk_simple(&root, "codex")
}

fn walk_simple(root: &PathBuf, source: &str) -> Vec<ExternalSession> {
    let mut out = vec![];
    let Ok(top) = std::fs::read_dir(root) else {
        return out;
    };
    for ws_entry in top.flatten() {
        if !ws_entry.path().is_dir() {
            continue;
        }
        let workspace_id = ws_entry.file_name().to_string_lossy().to_string();
        let Ok(inner) = std::fs::read_dir(ws_entry.path()) else {
            continue;
        };
        for chat in inner.flatten() {
            let p = chat.path();
            let id = chat.file_name().to_string_lossy().to_string();
            let modified = chat
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(systime_to_chrono);
            let bytes = chat.metadata().ok().map(|m| m.len()).unwrap_or(0);
            out.push(ExternalSession {
                source: source.into(),
                workspace_id: workspace_id.clone(),
                id,
                path: p.to_string_lossy().to_string(),
                modified_at: modified,
                bytes,
            });
        }
    }
    out
}

fn systime_to_chrono(t: SystemTime) -> Option<DateTime<Utc>> {
    let dur = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    DateTime::<Utc>::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
}
