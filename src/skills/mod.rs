//! Skill registry. A *skill* is a markdown file the orchestrator can advertise
//! to agents (chat or task adapters) so they know the established play-by-play
//! for a recurring task. Scopes:
//!
//!   .maestro/skills/_global/<name>.md      — visible to every agent
//!   .maestro/skills/<project>/<name>.md    — only when that project's adapter runs
//!
//! Each file is parsed for a simple YAML frontmatter block:
//!
//!   ---
//!   description: one-line summary shown to the agent
//!   trigger:     hint phrase that should call this skill in
//!   ---
//!   <markdown body…>

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::paths;

pub mod inventory;

pub const SKILLS_DIR: &str = "skills";
pub const GLOBAL_SCOPE_DIR: &str = "_global";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SkillScope {
    Global,
    Project(String),
}

impl SkillScope {
    pub fn dir_name(&self) -> String {
        match self {
            SkillScope::Global => GLOBAL_SCOPE_DIR.to_string(),
            SkillScope::Project(p) => p.clone(),
        }
    }

    pub fn from_dir(dir: &str) -> Self {
        if dir == GLOBAL_SCOPE_DIR {
            SkillScope::Global
        } else {
            SkillScope::Project(dir.to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFront {
    /// Anthropic Agent Skills standard field. maestro already *writes* it when
    /// exporting to `.claude/skills/*/SKILL.md`; reading it here lets a skill
    /// authored in the standard format (or a re-imported export) keep its name
    /// instead of falling back to the filename.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub scope: SkillScope,
    pub description: Option<String>,
    pub trigger: Option<String>,
    pub content: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub scope: SkillScope,
    pub description: Option<String>,
    pub trigger: Option<String>,
}

pub fn skills_root() -> Result<PathBuf> {
    let p = paths::maestro_dir()?.join(SKILLS_DIR);
    paths::ensure_dir(&p)?;
    paths::ensure_dir(&p.join(GLOBAL_SCOPE_DIR))?;
    Ok(p)
}

pub fn scope_dir(scope: &SkillScope) -> Result<PathBuf> {
    // A project scope becomes a directory name verbatim, so a traversal payload
    // (`../../tmp`) would otherwise escape the skills root and be create_dir_all'd.
    if let SkillScope::Project(p) = scope {
        paths::validate_path_component("skill scope", p)?;
    }
    let d = skills_root()?.join(scope.dir_name());
    paths::ensure_dir(&d)?;
    Ok(d)
}

pub fn skill_path(scope: &SkillScope, name: &str) -> Result<PathBuf> {
    Ok(scope_dir(scope)?.join(format!("{}.md", sanitize(name))))
}

pub(crate) fn sanitize(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches(".md");
    trimmed
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

pub fn parse_skill(scope: SkillScope, path: &Path) -> Result<Skill> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read skill {:?}", path))?;
    let (front, body) = split_frontmatter(&raw);
    // Prefer the standard frontmatter `name`; fall back to the filename.
    let name = front
        .as_ref()
        .and_then(|f| f.name.clone())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "skill".to_string());
    Ok(Skill {
        name,
        scope,
        description: front.as_ref().and_then(|f| f.description.clone()),
        trigger: front.and_then(|f| f.trigger),
        content: body,
        path: path.to_string_lossy().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> (Option<SkillFront>, String) {
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
    let body_trimmed = body.trim_start_matches('\n').to_string();
    let front: Option<SkillFront> = serde_yaml::from_str(fm.trim()).ok();
    (front, body_trimmed)
}

pub fn list_scope(scope: &SkillScope) -> Result<Vec<Skill>> {
    let dir = scope_dir(scope)?;
    let mut out = vec![];
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().map(|e| e == "md").unwrap_or(false) {
            if let Ok(s) = parse_skill(scope.clone(), &p) {
                out.push(s);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Every skill, grouped by scope.
pub fn list_all() -> Result<BTreeMap<String, Vec<Skill>>> {
    let mut by_scope: BTreeMap<String, Vec<Skill>> = BTreeMap::new();
    let root = skills_root()?;
    if !root.exists() {
        return Ok(by_scope);
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().to_string();
        let scope = SkillScope::from_dir(&dir);
        let skills = list_scope(&scope)?;
        by_scope.insert(dir, skills);
    }
    Ok(by_scope)
}

/// Every skill, grouped by scope, as body-free [`SkillSummary`]s. The sidebar
/// listing never needs skill bodies — only the per-skill editor route does — so
/// this keeps playbook content off the wire (F-121 N1).
pub fn list_all_summaries() -> Result<BTreeMap<String, Vec<SkillSummary>>> {
    let mut out = BTreeMap::new();
    for (scope, skills) in list_all()? {
        out.insert(scope, skills.into_iter().map(summarize).collect());
    }
    Ok(out)
}

pub fn load(scope: &SkillScope, name: &str) -> Result<Skill> {
    let p = skill_path(scope, name)?;
    parse_skill(scope.clone(), &p)
}

pub fn save(scope: &SkillScope, name: &str, content: &str) -> Result<PathBuf> {
    let p = skill_path(scope, name)?;
    if let Some(parent) = p.parent() {
        paths::ensure_dir(parent)?;
    }
    std::fs::write(&p, content).with_context(|| format!("write {:?}", p))?;
    // Mirror into IDE-native locations on every save. Best-effort: if the
    // workspace root doesn't have a .cursor or .claude folder yet, we still
    // create it so the IDE picks the skill up immediately.
    let _ = sync_to_ide(scope, name, content);
    Ok(p)
}

pub fn delete(scope: &SkillScope, name: &str) -> Result<()> {
    let p = skill_path(scope, name)?;
    if p.exists() {
        std::fs::remove_file(&p).with_context(|| format!("remove {:?}", p))?;
    }
    let _ = remove_from_ide(scope, name);
    Ok(())
}

/// Mirror one skill into `.cursor/rules/<n>.mdc` and `.claude/skills/<n>/SKILL.md`
/// so the IDE-native agents pick it up automatically.
pub fn sync_to_ide(scope: &SkillScope, name: &str, content: &str) -> Result<()> {
    let root = paths::workspace_root()?;
    let prefix = match scope {
        SkillScope::Global => "global".to_string(),
        SkillScope::Project(p) => format!("project-{p}"),
    };
    let cleaned_name = format!("maestro-{prefix}-{name}");
    let (front, body) = split_for_export(content);

    // Cursor: .cursor/rules/<n>.mdc
    {
        let cursor_dir = root.join(".cursor").join("rules");
        paths::ensure_dir(&cursor_dir)?;
        let path = cursor_dir.join(format!("{cleaned_name}.mdc"));
        let cursor_md = render_cursor_rule(name, &front, &body);
        std::fs::write(&path, cursor_md)
            .with_context(|| format!("write cursor rule {:?}", path))?;
    }

    // Claude Code: .claude/skills/<n>/SKILL.md
    {
        let claude_dir = root.join(".claude").join("skills").join(&cleaned_name);
        paths::ensure_dir(&claude_dir)?;
        let path = claude_dir.join("SKILL.md");
        let claude_md = render_claude_skill(name, &front, &body);
        std::fs::write(&path, claude_md)
            .with_context(|| format!("write claude skill {:?}", path))?;
    }

    Ok(())
}

pub fn remove_from_ide(scope: &SkillScope, name: &str) -> Result<()> {
    let root = paths::workspace_root()?;
    let prefix = match scope {
        SkillScope::Global => "global".to_string(),
        SkillScope::Project(p) => format!("project-{p}"),
    };
    let cleaned_name = format!("maestro-{prefix}-{name}");
    let cursor_rule = root
        .join(".cursor")
        .join("rules")
        .join(format!("{cleaned_name}.mdc"));
    if cursor_rule.exists() {
        let _ = std::fs::remove_file(cursor_rule);
    }
    let claude_skill_dir = root.join(".claude").join("skills").join(&cleaned_name);
    if claude_skill_dir.is_dir() {
        let _ = std::fs::remove_dir_all(claude_skill_dir);
    }
    Ok(())
}

/// Run `sync_to_ide` over every skill currently on disk. Returns (cursor_count, claude_count).
pub fn sync_all() -> Result<(usize, usize)> {
    let all = list_all()?;
    let mut cursor = 0;
    let mut claude = 0;
    for (_, skills) in all {
        for s in skills {
            // Re-read the raw file so the frontmatter survives the round-trip.
            let raw = match std::fs::read_to_string(&s.path) {
                Ok(r) => r,
                Err(e) => {
                    // An unreadable skill file used to be silently skipped via
                    // unwrap_or_default(); that turned every permission /
                    // disappeared-file error into a confusing "sync said it
                    // worked but my skill didn't show up". Log and move on.
                    tracing::warn!(
                        "skip skill {}/{} during sync: cannot read {}: {e}",
                        s.scope.dir_name(),
                        s.name,
                        s.path,
                    );
                    continue;
                }
            };
            if sync_to_ide(&s.scope, &s.name, &raw).is_ok() {
                cursor += 1;
                claude += 1;
            }
        }
    }
    Ok((cursor, claude))
}

fn split_for_export(content: &str) -> (Option<SkillFront>, String) {
    let stripped = content.trim_start_matches('\u{feff}');
    if !stripped.starts_with("---") {
        return (None, stripped.to_string());
    }
    let rest = &stripped[3..];
    let mut iter = rest.splitn(2, "\n---");
    let (fm, body) = match (iter.next(), iter.next()) {
        (Some(fm), Some(body)) => (fm, body),
        _ => return (None, stripped.to_string()),
    };
    let body_trimmed = body.trim_start_matches('\n').to_string();
    let front: Option<SkillFront> = serde_yaml::from_str(fm.trim()).ok();
    (front, body_trimmed)
}

fn render_cursor_rule(name: &str, front: &Option<SkillFront>, body: &str) -> String {
    let description = front
        .as_ref()
        .and_then(|f| f.description.clone())
        .unwrap_or_else(|| format!("{name} — synced from maestro"));
    // Cursor expects: description, globs (optional), alwaysApply
    format!(
        "---\ndescription: {description}\nglobs:\nalwaysApply: false\n---\n\n# {name}\n\n_Auto-synced from `.maestro/skills/`. Edit there or via the maestro dashboard; this file will be overwritten._\n\n{body}\n"
    )
}

fn render_claude_skill(name: &str, front: &Option<SkillFront>, body: &str) -> String {
    let description = front
        .as_ref()
        .and_then(|f| f.description.clone())
        .unwrap_or_else(|| format!("{name} — synced from maestro"));
    format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\n_Auto-synced from `.maestro/skills/`. Edit there or via the maestro dashboard; this file will be overwritten._\n\n{body}\n"
    )
}

fn summarize(s: Skill) -> SkillSummary {
    SkillSummary {
        name: s.name,
        scope: s.scope,
        description: s.description,
        trigger: s.trigger,
    }
}

/// Skills to inject when dispatching a *task* to an adapter:
/// global scope + the project's own scope.
pub fn index_for(project: Option<&str>) -> Vec<SkillSummary> {
    let mut out = vec![];
    if let Ok(globals) = list_scope(&SkillScope::Global) {
        out.extend(globals.into_iter().map(summarize));
    }
    if let Some(p) = project {
        if let Ok(scoped) = list_scope(&SkillScope::Project(p.to_string())) {
            out.extend(scoped.into_iter().map(summarize));
        }
    }
    out
}

/// Skills to advertise in the *chat* prelude: every scope, since the chat
/// orchestrator may need to call any of them as it coordinates across projects.
pub fn index_all() -> Vec<SkillSummary> {
    let mut out = vec![];
    if let Ok(all) = list_all() {
        for (_, skills) in all {
            out.extend(skills.into_iter().map(summarize));
        }
    }
    out
}

pub fn for_mode(mode: &crate::modes::Mode) -> Result<Vec<Skill>> {
    if mode.skills.is_empty() {
        let mut out = Vec::new();
        for (_, skills) in list_all()? {
            out.extend(skills);
        }
        return Ok(out);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for name in &mode.skills {
        let skill = load_visible(None, name)
            .with_context(|| format!("load mode skill {:?} for mode {}", name, mode.id))?;
        push_unique(&mut out, &mut seen, skill);
    }
    Ok(out)
}

/// Find every skill (global + the given project's, if any) whose `trigger`
/// phrase appears in `text` (case-insensitive, `|` separates alternative
/// phrases). Returns the matched skills with their full bodies loaded.
pub fn trigger_match(text: &str, project: Option<&str>) -> Result<Vec<Skill>> {
    let haystack = text.to_lowercase();
    let mut out = vec![];

    let mut scopes_to_scan = vec![SkillScope::Global];
    if let Some(p) = project {
        scopes_to_scan.push(SkillScope::Project(p.to_string()));
    }

    for scope in scopes_to_scan {
        let skills = list_scope(&scope).unwrap_or_default();
        for s in skills {
            let Some(trigger) = &s.trigger else { continue };
            let any_hit = trigger
                .split('|')
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .any(|p| haystack.contains(&p));
            if any_hit {
                out.push(s);
            }
        }
    }
    Ok(out)
}

/// Load skills explicitly requested by a task and append any trigger-matched
/// skills visible to that project. Explicit skills are fail-fast: a typo in
/// `tasks[*].skills` should stop dispatch instead of silently dropping the
/// playbook the workflow depends on.
pub fn resolve_for_task(
    text: &str,
    project: Option<&str>,
    explicit: &[String],
) -> Result<Vec<Skill>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for name in explicit.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let skill =
            load_visible(project, name).with_context(|| format!("load explicit skill {name:?}"))?;
        push_unique(&mut out, &mut seen, skill);
    }

    for skill in trigger_match(text, project)? {
        push_unique(&mut out, &mut seen, skill);
    }

    Ok(out)
}

/// Render full skill bodies into the adapter prompt. This is intentionally
/// separate from `MemorySlice`: skills are operating procedures, not facts.
pub fn render_task_section(skills: &[Skill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut out = String::from("# Maestro skills (required for this task)\n\n");
    out.push_str(
        "Follow these playbooks while doing the task. If a playbook conflicts with the explicit task prompt, obey the task prompt and mention the conflict in your final summary.\n\n",
    );
    for skill in skills {
        out.push_str(&format!(
            "## Skill: {} ({})\n\n",
            skill.name,
            skill.scope.dir_name()
        ));
        if let Some(description) = &skill.description {
            out.push_str(&format!("description: {description}\n\n"));
        }
        out.push_str(skill.content.trim());
        out.push_str("\n\n");
    }
    Some(out)
}

fn load_visible(project: Option<&str>, name: &str) -> Result<Skill> {
    if let Some((scope_name, skill_name)) = name.split_once('/') {
        let scope = SkillScope::from_dir(scope_name);
        return load(&scope, skill_name);
    }

    if let Some(project) = project {
        let scope = SkillScope::Project(project.to_string());
        if let Ok(skill) = load(&scope, name) {
            return Ok(skill);
        }
    }
    load(&SkillScope::Global, name)
}

/// Read-only existence of a skill at an exact scope. Computes the path directly
/// (never `ensure_dir`), so a validation caller doesn't create `.maestro/skills`.
/// A scope that fails path-component validation can't exist → `false`.
fn skill_file_exists(scope: &SkillScope, name: &str) -> bool {
    if let SkillScope::Project(p) = scope {
        if paths::validate_path_component("skill scope", p).is_err() {
            return false;
        }
    }
    let Ok(root) = paths::maestro_dir() else {
        return false;
    };
    root.join(SKILLS_DIR)
        .join(scope.dir_name())
        .join(format!("{}.md", sanitize(name)))
        .exists()
}

/// Does a skill reference resolve to an on-disk skill? Mirrors `load_visible`'s
/// scope resolution (explicit `scope/name`, else project scope, else global) so
/// validation and dispatch agree on what "exists" — but read-only (no
/// `ensure_dir`). Pass the project context when the profile is bound to one
/// (so an unscoped skill resolves project-first then global); pass `None` for
/// an unbound profile (explicit scopes + global only).
pub fn reference_exists(project: Option<&str>, name: &str) -> bool {
    if let Some((scope_name, skill_name)) = name.split_once('/') {
        return skill_file_exists(&SkillScope::from_dir(scope_name), skill_name);
    }
    if let Some(project) = project {
        if skill_file_exists(&SkillScope::Project(project.to_string()), name) {
            return true;
        }
    }
    skill_file_exists(&SkillScope::Global, name)
}

fn push_unique(out: &mut Vec<Skill>, seen: &mut HashSet<String>, skill: Skill) {
    let key = format!("{}/{}", skill.scope.dir_name(), skill.name);
    if seen.insert(key) {
        out.push(skill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scope_dir_rejects_project_scope_traversal() {
        // Validation runs before any filesystem/env access, so a traversal
        // payload is rejected without escaping the skills root.
        assert!(scope_dir(&SkillScope::Project("../../tmp".into())).is_err());
        assert!(scope_dir(&SkillScope::Project("..".into())).is_err());
        assert!(scope_dir(&SkillScope::Project("a/b".into())).is_err());
    }

    #[test]
    fn parse_skill_prefers_frontmatter_name_over_filename() {
        // an Anthropic-standard SKILL.md: name lives in frontmatter
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        std::fs::write(
            &path,
            "---\nname: pdf-extractor\ndescription: pull tables from PDFs\n---\n\n# body\n",
        )
        .unwrap();
        let skill = parse_skill(SkillScope::Global, &path).unwrap();
        assert_eq!(skill.name, "pdf-extractor");
        assert_eq!(skill.description.as_deref(), Some("pull tables from PDFs"));
    }

    #[test]
    fn parse_skill_falls_back_to_filename_without_frontmatter_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("my-playbook.md");
        std::fs::write(&path, "---\ndescription: x\n---\nbody").unwrap();
        let skill = parse_skill(SkillScope::Global, &path).unwrap();
        assert_eq!(skill.name, "my-playbook");
    }

    // ── F-107: the distributable Claude Code skill `skills/repo-maestro` ──
    // These guard the shipped artifact: it stays well-formed by maestro's
    // own rules, references only real CLI commands, and keeps the
    // load-bearing trigger + safety wording it was reviewed with.
    mod repo_maestro_skill {
        use super::super::{parse_skill, SkillScope};
        use std::path::PathBuf;

        fn skill() -> super::super::Skill {
            let path =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills/repo-maestro/SKILL.md");
            parse_skill(SkillScope::Global, &path).expect("repo-maestro skill parses")
        }

        #[test]
        fn parses_via_maestro_skill_loader() {
            let s = skill();
            assert_eq!(s.name, "repo-maestro", "frontmatter name");
            assert!(
                s.description
                    .as_deref()
                    .is_some_and(|d| !d.trim().is_empty()),
                "description must be non-empty"
            );
            assert!(!s.content.trim().is_empty(), "body must be non-empty");
        }

        /// Extract every `maestro <subcommand>` the body actually claims.
        /// Only scans inside backtick code spans so prose like "the
        /// `maestro` CLI" doesn't read "CLI" as a command; skips flag forms
        /// (`maestro --version`) since those aren't subcommands.
        fn referenced_subcommands(body: &str) -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            // Segments between backticks are code (odd indices after split).
            for (i, seg) in body.split('`').enumerate() {
                if i % 2 == 0 {
                    continue; // prose, not a command span
                }
                let toks: Vec<&str> = seg.split_whitespace().collect();
                for w in 0..toks.len() {
                    if toks[w] != "maestro" {
                        continue;
                    }
                    let Some(next) = toks.get(w + 1) else {
                        continue;
                    };
                    if next.starts_with('-') {
                        continue; // a flag like --version, not a subcommand
                    }
                    let cmd: String = next
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect();
                    if !cmd.is_empty() {
                        out.insert(cmd);
                    }
                }
            }
            out
        }

        #[test]
        fn body_references_only_real_cli_subcommands() {
            use clap::CommandFactory;
            let cmd = crate::cli::Cli::command();
            let subcommands: Vec<String> = cmd
                .get_subcommands()
                .map(|c| c.get_name().to_string())
                .collect();

            // Drift guard: extract the actual `maestro <cmd>` tokens the body
            // tells CC to run, and require EACH to be a real top-level
            // command. Unlike a hardcoded list, this catches a future
            // `maestro frobnicate` slipped into the body (dali N1).
            let referenced = referenced_subcommands(&skill().content);
            assert!(
                !referenced.is_empty(),
                "expected the body to reference at least one maestro subcommand"
            );
            // sanity: today's body should yield exactly the documented flow
            assert!(
                referenced.contains("init")
                    && referenced.contains("work")
                    && referenced.contains("open"),
                "extracted {referenced:?}, expected at least init/work/open"
            );
            for cmd in &referenced {
                assert!(
                    subcommands.iter().any(|s| s == cmd),
                    "skill body references `maestro {cmd}` which is not a real subcommand; \
                     known: {subcommands:?}"
                );
            }
        }

        #[test]
        fn trigger_text_keeps_multi_repo_scope_and_single_repo_exclusion() {
            let d = skill().description.unwrap_or_default();
            assert!(
                d.contains("MULTIPLE repos/projects"),
                "trigger must keep the multi-repo intent"
            );
            assert!(
                d.contains("Do NOT use for single-repo"),
                "trigger must keep the load-bearing single-repo exclusion"
            );
        }

        #[test]
        fn body_enforces_dry_before_run_with_explicit_approval() {
            let body = skill().content;
            let dry = body.find("--dry").expect("body mentions --dry");
            let run = body.find("--run").expect("body mentions --run");
            assert!(dry < run, "--dry must be introduced before --run");
            let lc = body.to_lowercase();
            assert!(
                lc.contains("approval") || lc.contains("explicit") || lc.contains("go before"),
                "body must require explicit user approval before running"
            );
        }

        #[test]
        fn body_keeps_no_direct_edit_guardrail() {
            let lc = skill().content.to_lowercase();
            assert!(
                lc.contains("does not edit repos directly"),
                "body must keep the drive-not-edit guardrail"
            );
        }
    }
}
