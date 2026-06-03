//! Filesystem browse endpoint used by the dashboard's path picker.
//!
//! `GET /api/fs/list?path=<dir>&hidden=0|1`
//!
//! Returns a directory listing for the picker UI. Designed to be tiny —
//! the dashboard is local-only and the user already has shell access to
//! everything we'd serve, so there's no sandbox here. We do, however,
//! refuse anything that's not a directory and we resolve symlinks so the
//! response is unambiguous about the actual on-disk path.

use std::path::PathBuf;

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Maximum number of children returned in one response. Most directories
/// are small; this cap exists so a stray `/nix/store` or similar can't
/// freeze the picker.
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Deserialize)]
pub struct FsListQuery {
    /// Directory to list. `~` is expanded. Defaults to `$HOME` (or the
    /// workspace root if `$HOME` is unset).
    #[serde(default)]
    path: Option<String>,
    /// Show dotfiles when set to a truthy value. Default: hidden.
    #[serde(default)]
    hidden: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FsListResponse {
    /// The resolved absolute path the listing is for.
    path: String,
    /// Parent path, or `None` when at the filesystem root.
    parent: Option<String>,
    /// `$HOME` expanded — handy so the UI can render a "Home" shortcut.
    home: Option<String>,
    /// Workspace root (where `.maestro/` lives) — same idea.
    workspace_root: Option<String>,
    /// Child entries. Directories sort first, then files, both
    /// case-insensitive alphabetically. Capped at MAX_ENTRIES.
    entries: Vec<FsEntry>,
    /// True iff the listing was truncated to MAX_ENTRIES.
    truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct FsEntry {
    name: String,
    kind: EntryKind,
    /// Whether the name starts with a dot.
    hidden: bool,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

pub async fn fs_list_handler(Query(q): Query<FsListQuery>) -> Response {
    let show_hidden = q
        .hidden
        .as_deref()
        .map(|v| matches!(v, "1" | "true" | "yes" | "on"))
        .unwrap_or(false);

    let requested = q.path.unwrap_or_else(default_path);
    let resolved = match resolve(&requested) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    if !resolved.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("path does not exist: {}", resolved.display()),
        )
            .into_response();
    }
    if !resolved.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            format!("not a directory: {}", resolved.display()),
        )
            .into_response();
    }

    let read_dir = match std::fs::read_dir(&resolved) {
        Ok(it) => it,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                format!("cannot read {}: {}", resolved.display(), e),
            )
                .into_response();
        }
    };

    let mut entries: Vec<FsEntry> = Vec::new();
    let mut truncated = false;
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let hidden = name.starts_with('.');
        if hidden && !show_hidden {
            continue;
        }
        let kind = classify(&entry);
        entries.push(FsEntry { name, kind, hidden });
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
    }

    // Dirs first, then files. Within each group, case-insensitive sort.
    entries.sort_by(|a, b| {
        let ka = kind_sort_key(a.kind);
        let kb = kind_sort_key(b.kind);
        ka.cmp(&kb)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let body = FsListResponse {
        path: resolved.to_string_lossy().to_string(),
        parent: resolved
            .parent()
            .filter(|p| *p != resolved.as_path())
            .map(|p| p.to_string_lossy().to_string()),
        home: home_dir().map(|p| p.to_string_lossy().to_string()),
        workspace_root: paths::workspace_root()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        entries,
        truncated,
    };
    Json(body).into_response()
}

fn classify(entry: &std::fs::DirEntry) -> EntryKind {
    match entry.file_type() {
        Ok(t) if t.is_symlink() => EntryKind::Symlink,
        Ok(t) if t.is_dir() => EntryKind::Dir,
        Ok(t) if t.is_file() => EntryKind::File,
        _ => EntryKind::Other,
    }
}

fn kind_sort_key(k: EntryKind) -> u8 {
    match k {
        EntryKind::Dir => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
        EntryKind::Other => 3,
    }
}

fn default_path() -> String {
    home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            paths::workspace_root()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/".into())
        })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Expand `~` and resolve relative paths against the workspace root.
/// Returns the canonical (symlink-resolved) absolute path when possible.
fn resolve(input: &str) -> Result<PathBuf, String> {
    let expanded = shellexpand::full(input).map_err(|e| format!("expand: {e}"))?;
    let p = PathBuf::from(expanded.as_ref());
    let abs = if p.is_absolute() {
        p
    } else {
        paths::workspace_root()
            .map_err(|e| format!("resolve workspace_root: {e:#}"))?
            .join(p)
    };
    // canonicalize when possible to give a stable, symlink-resolved
    // path; fall back to the lexical path if canonicalize fails (e.g.
    // path doesn't exist yet — caller will handle that case).
    Ok(std::fs::canonicalize(&abs).unwrap_or(abs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_returns_dir_for_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("sub");
        std::fs::create_dir(&child).unwrap();
        let entry = std::fs::read_dir(tmp.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(classify(&entry), EntryKind::Dir);
    }

    #[test]
    fn classify_returns_file_for_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hi").unwrap();
        let entry = std::fs::read_dir(tmp.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(classify(&entry), EntryKind::File);
    }

    #[test]
    fn kind_sort_orders_dir_before_file() {
        let mut a = vec![EntryKind::File, EntryKind::Dir, EntryKind::Symlink];
        a.sort_by_key(|k| kind_sort_key(*k));
        assert_eq!(a, vec![EntryKind::Dir, EntryKind::Symlink, EntryKind::File]);
    }

    #[test]
    fn resolve_expands_tilde() {
        if let Some(home) = home_dir() {
            let r = resolve("~").unwrap();
            // canonicalize may add /private on macOS; just ensure
            // the resolved path ends with the home suffix.
            assert!(r.ends_with(home.file_name().unwrap_or_default()));
        }
    }

    #[test]
    fn resolve_returns_relative_against_workspace_root() {
        let r = resolve(".").unwrap();
        assert!(r.is_absolute());
    }
}
