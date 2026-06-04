//! Role registry — opinionated personas the orchestrator can adopt per task.
//!
//! A *role* is a single markdown file with YAML frontmatter:
//!
//! ```markdown
//! ---
//! name: backend_rust
//! display: 后端工程师 · Backend (Rust)
//! summary: Tokio + async; contract-first; migration-safe
//! source: rust-async-development-rules
//! tags: [backend, rust]
//! ---
//!
//! # Role · backend_rust
//!
//! <prelude body…>
//! ```
//!
//! Roles live in two places:
//!
//!   - **Builtin** — embedded into the binary from `src/roles/builtin/*.md`.
//!     Ships 9 seeds covering the user's day-to-day functions: design,
//!     architecture, backend (Rust + Python), frontend, mobile (RN/Expo),
//!     game (Unity + Cocos), and QA.
//!   - **User** — `.maestro/roles/<name>.md` in the workspace.
//!     Overrides the builtin by the same name and wins for that workspace.
//!
//! Resolution priority when running a task is **task.role → project.role →
//! none**. "None" is a meaningful state: `_global` tasks shouldn't accidentally
//! borrow a builder role.
//!
//! This module is deliberately thin — no triggers, no skill matching, no
//! mirror to `.cursor/rules`. The role's `prelude` body is concatenated
//! into the agent prompt by the executor; cursor's own rule system stays
//! its own source of truth.

use anyhow::{Context, Result};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::paths;

/// Embeds every file under `src/roles/builtin/`. Markdown only.
#[derive(RustEmbed)]
#[folder = "src/roles/builtin/"]
struct RoleAsset;

pub const ROLES_DIR: &str = "roles";

/// YAML frontmatter on every role file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleFront {
    /// Canonical kebab-case id. Defaults to the filename stem when omitted.
    #[serde(default)]
    pub name: Option<String>,
    /// Human-readable label for UI rendering. Bilingual is fine.
    #[serde(default)]
    pub display: Option<String>,
    /// One-line summary; used in `maestro role ls` and the picker UI.
    #[serde(default)]
    pub summary: Option<String>,
    /// Provenance — where this role's content was adapted from (e.g.
    /// `rust-async-development-rules`, `metagpt/architect`, `cocos-creator-3.x`).
    #[serde(default)]
    pub source: Option<String>,
    /// Free-form labels. Used by the UI for grouping (`backend`, `mobile`,
    /// `game`, `cross-cutting`).
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_tools: Option<crate::modes::AllowedTools>,
}

/// In-memory representation of a loaded role.
#[derive(Debug, Clone, Serialize)]
pub struct Role {
    pub name: String,
    pub display: String,
    pub summary: String,
    pub source: Option<String>,
    pub tags: Vec<String>,
    /// Markdown body that gets injected into the agent prompt.
    pub prelude: String,
    /// `true` when this role came from the embedded builtin bundle;
    /// `false` when it was overridden by a user file in `.maestro/roles/`.
    pub builtin: bool,
    /// On-disk location if user-defined. `None` for pure builtins.
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<crate::modes::AllowedTools>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleSummary {
    pub name: String,
    pub display: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub builtin: bool,
}

/// Workspace directory for user-defined roles. Created lazily.
pub fn roles_root() -> Result<PathBuf> {
    let p = paths::maestro_dir()?.join(ROLES_DIR);
    paths::ensure_dir(&p)?;
    Ok(p)
}

fn role_filename(name: &str) -> String {
    format!("{}.md", sanitize(name))
}

fn sanitize(name: &str) -> String {
    name.trim()
        .trim_end_matches(".md")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Load a role by name. Resolution order:
///   1. `.maestro/roles/<name>.md` (user override)
///   2. embedded `src/roles/builtin/<name>.md`
///   3. Cursor plugin personas at
///      `~/.cursor/plugins/cache/cursor-public/<plugin>/agents/<name>.agent.md`
///      — lets the user use Compound Engineering and other community
///      reviewer personas as first-class maestro roles without copying
///      anything in.
///   4. error
pub fn load(name: &str) -> Result<Role> {
    let sanitized = sanitize(name);
    let user_path = roles_root()?.join(role_filename(&sanitized));
    if user_path.exists() {
        let raw = std::fs::read_to_string(&user_path)
            .with_context(|| format!("read role {:?}", user_path))?;
        return Ok(parse(&sanitized, &raw, false, Some(user_path)));
    }

    let key = format!("{sanitized}.md");
    if let Some(f) = RoleAsset::get(&key) {
        let raw = std::str::from_utf8(f.data.as_ref())
            .with_context(|| format!("builtin role {key} not utf-8"))?
            .to_string();
        return Ok(parse(&sanitized, &raw, true, None));
    }

    if let Some(path) = find_cursor_plugin_persona(&sanitized) {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read plugin persona {:?}", path))?;
        return Ok(parse(&sanitized, &raw, false, Some(path)));
    }

    anyhow::bail!(
        "role {:?} not found (looked in .maestro/roles, builtins, and ~/.cursor/plugins)",
        name
    )
}

/// Scan `~/.cursor/plugins/cache/cursor-public/*/agents/` for a file
/// named `<name>.agent.md`. Returns the first match. Cheap because the
/// directory layout is shallow and stable.
fn find_cursor_plugin_persona(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let root = PathBuf::from(home)
        .join(".cursor")
        .join("plugins")
        .join("cache")
        .join("cursor-public");
    if !root.is_dir() {
        return None;
    }
    let needle = format!("{name}.agent.md");
    for plugin in std::fs::read_dir(&root).ok()? {
        let Ok(plugin) = plugin else { continue };
        // <plugin>/<commit-sha>/agents/<persona>.agent.md
        for inner in std::fs::read_dir(plugin.path()).ok()?.flatten() {
            let agents = inner.path().join("agents");
            if !agents.is_dir() {
                continue;
            }
            let candidate = agents.join(&needle);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Like [`load`] but returns `None` instead of erroring when the role is
/// missing. Useful in the executor where a missing role should be a
/// warning, not a hard failure.
pub fn try_load(name: &str) -> Option<Role> {
    match load(name) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!("role lookup failed for {name:?}: {e:#}");
            None
        }
    }
}

/// Read-only existence check mirroring [`load`]'s lookup order (workspace role
/// file → builtin → cursor-plugin persona) but **without** creating
/// `.maestro/roles` (so callers like `maestro validate` stay non-mutating).
pub fn exists(name: &str) -> bool {
    let sanitized = sanitize(name);
    // workspace role file — compute the path directly, never ensure_dir.
    if let Ok(dir) = paths::maestro_dir() {
        if dir.join(ROLES_DIR).join(role_filename(&sanitized)).exists() {
            return true;
        }
    }
    if RoleAsset::get(&format!("{sanitized}.md")).is_some() {
        return true;
    }
    find_cursor_plugin_persona(&sanitized).is_some()
}

/// Enumerate every role visible to this workspace: builtins first, then
/// user-defined (which shadow builtins of the same name).
pub fn list_all() -> Result<Vec<RoleSummary>> {
    let mut by_name: BTreeMap<String, RoleSummary> = BTreeMap::new();

    for key in RoleAsset::iter() {
        if !key.ends_with(".md") {
            continue;
        }
        let name = key.trim_end_matches(".md").to_string();
        if let Ok(role) = load(&name) {
            by_name.insert(
                role.name.clone(),
                RoleSummary {
                    name: role.name,
                    display: role.display,
                    summary: role.summary,
                    tags: role.tags,
                    builtin: true,
                },
            );
        }
    }

    // User-defined roles (may override builtins).
    let root = roles_root()?;
    if root.exists() {
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().map(|e| e == "md").unwrap_or(false) {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Ok(role) = load(&name) {
                    by_name.insert(
                        role.name.clone(),
                        RoleSummary {
                            name: role.name,
                            display: role.display,
                            summary: role.summary,
                            tags: role.tags,
                            builtin: false,
                        },
                    );
                }
            }
        }
    }

    Ok(by_name.into_values().collect())
}

/// Copy a builtin role to the workspace so the user can edit it. Returns
/// the new on-disk path. Errors if the role isn't a builtin (nothing to
/// export) or if a user file with that name already exists.
pub fn export_to_workspace(name: &str) -> Result<PathBuf> {
    let sanitized = sanitize(name);
    let target = roles_root()?.join(role_filename(&sanitized));
    if target.exists() {
        anyhow::bail!("user role already exists at {:?}", target);
    }
    let key = format!("{sanitized}.md");
    let asset = RoleAsset::get(&key)
        .ok_or_else(|| anyhow::anyhow!("builtin role {sanitized:?} not found"))?;
    std::fs::write(&target, asset.data.as_ref()).with_context(|| format!("write {:?}", target))?;
    Ok(target)
}

/// Render the role as it appears in a prompt: a markdown section the
/// agent reads BEFORE the user-authored task instruction. Returns an
/// empty string when prelude is blank so the executor can skip the
/// `# Role` header altogether for header-only roles.
pub fn render_section(role: &Role) -> String {
    if role.prelude.trim().is_empty() {
        return String::new();
    }
    let mut s = String::new();
    s.push_str(&format!("# Role · {}\n\n", role.name));
    s.push_str(role.prelude.trim_end());
    s.push_str("\n\n");
    s
}

fn parse(name: &str, raw: &str, builtin: bool, path: Option<PathBuf>) -> Role {
    let (front, body) = split_frontmatter(raw);
    let f = front.unwrap_or_default();
    Role {
        name: f.name.unwrap_or_else(|| name.to_string()),
        display: f.display.unwrap_or_else(|| name.to_string()),
        summary: f.summary.unwrap_or_default(),
        source: f.source,
        tags: f.tags,
        skills: f.skills,
        allowed_tools: f.allowed_tools,
        prelude: body,
        builtin,
        path,
    }
}

fn split_frontmatter(raw: &str) -> (Option<RoleFront>, String) {
    let stripped = raw.trim_start_matches('\u{feff}');
    if !stripped.starts_with("---") {
        return (None, stripped.to_string());
    }
    let rest = &stripped[3..];
    let mut iter = rest.splitn(2, "\n---");
    let (fm, body) = match (iter.next(), iter.next()) {
        (Some(fm), Some(body)) => (fm, body),
        _ => return (None, stripped.to_string()),
    };
    let body = body.trim_start_matches('\n').to_string();
    let front: Option<RoleFront> = serde_yaml::from_str(fm.trim()).ok();
    (front, body)
}

/// Helper for the executor: pick the effective role for one task, given
/// the projects config. Returns `None` if neither task nor project named
/// a role.
pub fn resolve_for_task(task_role: Option<&str>, project_role: Option<&str>) -> Option<String> {
    // Treat empty / whitespace-only strings as if the caller had passed
    // None, so an explicit `role: ""` in PLAN.yaml falls through to the
    // project default rather than blocking it.
    let pick = |s: Option<&str>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    pick(task_role).or_else(|| pick(project_role))
}

/// Used by `maestro role ls` to show which projects each role covers.
/// Returns a map `role_name -> [project names]`. A project shows up only
/// once even if its `role:` matches several builtins by tag.
pub fn projects_by_role(projects: &crate::config::ProjectsConfig) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, p) in &projects.projects {
        if let Some(role) = &p.role {
            out.entry(role.clone()).or_default().push(name.clone());
        }
    }
    out
}

/// Convenience for the path resolution used in tests. Not for production.
pub fn maestro_roles_path(name: &str) -> Result<PathBuf> {
    Ok(roles_root()?.join(role_filename(name)))
}

/// Re-export so callers don't need to depend on rust-embed types.
pub fn builtin_names() -> Vec<String> {
    RoleAsset::iter()
        .filter_map(|k| {
            if k.ends_with(".md") {
                Some(k.trim_end_matches(".md").to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_pulls_yaml() {
        let raw = "---\nname: foo\nsummary: hi\n---\n\nbody here\n";
        let (front, body) = split_frontmatter(raw);
        let f = front.expect("frontmatter parsed");
        assert_eq!(f.name.as_deref(), Some("foo"));
        assert_eq!(f.summary.as_deref(), Some("hi"));
        assert_eq!(body.trim(), "body here");
    }

    #[test]
    fn split_frontmatter_handles_missing_block() {
        let (front, body) = split_frontmatter("just markdown\n");
        assert!(front.is_none());
        assert_eq!(body.trim(), "just markdown");
    }

    #[test]
    fn parse_falls_back_to_filename_for_name() {
        let role = parse("my_role", "hello body", false, None);
        assert_eq!(role.name, "my_role");
        assert_eq!(role.display, "my_role");
        assert_eq!(role.prelude.trim(), "hello body");
    }

    #[test]
    fn resolve_prefers_task_over_project() {
        assert_eq!(resolve_for_task(Some("a"), Some("b")).as_deref(), Some("a"));
        assert_eq!(resolve_for_task(None, Some("b")).as_deref(), Some("b"));
        assert_eq!(resolve_for_task(Some(""), Some("b")).as_deref(), Some("b"));
        assert!(resolve_for_task(None, None).is_none());
    }

    #[test]
    fn render_section_emits_header_when_prelude_present() {
        let role = Role {
            name: "x".into(),
            display: "x".into(),
            summary: "".into(),
            source: None,
            tags: vec![],
            prelude: "do the thing".into(),
            builtin: true,
            path: None,
            skills: None,
            allowed_tools: None,
        };
        let rendered = render_section(&role);
        assert!(rendered.contains("# Role · x"));
        assert!(rendered.contains("do the thing"));
    }

    #[test]
    fn render_section_skips_empty_prelude() {
        let role = Role {
            name: "x".into(),
            display: "x".into(),
            summary: "".into(),
            source: None,
            tags: vec![],
            prelude: "  \n  ".into(),
            builtin: true,
            path: None,
            skills: None,
            allowed_tools: None,
        };
        assert!(render_section(&role).is_empty());
    }

    /// Belt-and-suspenders: every role file we ship MUST parse cleanly
    /// and have non-empty frontmatter + body. Catches typos at build time.
    #[test]
    fn every_builtin_role_loads_and_has_content() {
        for name in builtin_names() {
            let r = load(&name).unwrap_or_else(|e| panic!("load {name}: {e:#}"));
            assert!(!r.display.is_empty(), "{name} missing display");
            assert!(!r.summary.is_empty(), "{name} missing summary");
            assert!(
                r.prelude.trim().len() > 200,
                "{name} prelude too short ({} bytes) — role files must be substantive",
                r.prelude.trim().len()
            );
        }
    }

    #[test]
    fn sanitize_strips_unsafe_chars() {
        // `..`, `/` and any other non-[A-Za-z0-9_-] become `-`. The point
        // isn't a particular replacement, it's that the name can never
        // escape `.maestro/roles/` via path traversal.
        let cleaned = sanitize("../etc/passwd");
        assert_eq!(cleaned.len(), "../etc/passwd".len());
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('.'));
        assert_eq!(sanitize("backend_rust"), "backend_rust");
        assert_eq!(sanitize("backend-rust.md"), "backend-rust");
    }
}
