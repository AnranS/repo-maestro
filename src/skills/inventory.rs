//! F-121 — budgeted, metadata-only skill inventory scanner.
//!
//! Builds a [`SkillInventory`] for a project / specialist-profile context
//! **without loading skill bodies**. It deliberately does NOT call
//! [`super::parse_skill`] (which reads the whole file and returns `content`);
//! instead it reads at most a per-file frontmatter budget and extracts only the
//! YAML frontmatter block.
//!
//! Resolution mirrors [`super::load_visible`] / [`super::reference_exists`]
//! (explicit `scope/name` is exact; an unscoped ref resolves project scope
//! first, then `_global`) — but read-only and parametrized by an explicit root
//! so it is fully testable against a temp tree and never creates directories.
//!
//! Design: `docs/experience/F-121-SKILL-INVENTORY-DESIGN.md`.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::agent_profile::AgentProfile;
use crate::paths;
use crate::schema::skill_inventory::*;

use super::{sanitize, SkillFront, SkillScope, GLOBAL_SCOPE_DIR, SKILLS_DIR};

/// A resolved specialist-profile context for the inventory builder. The
/// endpoint layer (Step 2) constructs `Present` when the profile exists in
/// config, or `Missing` when an explicit `profile=` query names an unknown
/// profile (it may still 404, but the builder records a `missing_profile`
/// issue so a non-strict caller gets a useful inventory).
pub enum ProfileInput<'a> {
    Present(&'a str, &'a AgentProfile),
    Missing(&'a str),
}

/// The real skills root, *without* creating it. `skills::skills_root` ensures
/// the directory exists, which is wrong for a read-only inventory — a missing
/// root must read as an empty inventory, not be conjured into existence.
pub fn skills_inventory_root() -> Result<PathBuf> {
    Ok(paths::maestro_dir()?.join(SKILLS_DIR))
}

/// Build a metadata-only inventory for the given context. Infallible: a missing
/// scope directory reads as empty, budget overflow returns a partial result
/// plus warnings, and unsafe inputs are skipped with issues. The returned value
/// always passes [`validate_inventory`].
pub fn build_inventory(
    root: &Path,
    project: Option<&str>,
    profile: Option<ProfileInput<'_>>,
    budget: &SkillInventoryBudget,
) -> SkillInventory {
    let mut issues: Vec<InventoryIssue> = Vec::new();
    let mut skipped = 0u32;

    // Validate the project query value up front — this is the realistic
    // `unsafe_scope` trigger (a real on-disk dir name can't contain a path
    // separator, but a `project=` query value can).
    let project: Option<String> = match project {
        Some(p)
            if paths::validate_path_component("skill scope", p).is_ok()
                && p.len() <= budget.max_name_bytes as usize =>
        {
            Some(p.to_string())
        }
        Some(_) => {
            // Do NOT echo the rejected value: it is path-unsafe by definition,
            // so it can't go in a (path-safe) label field, and re-emitting raw
            // input is needless.
            issues.push(InventoryIssue::new(
                CODE_UNSAFE_SCOPE,
                IssueSeverity::Error,
                "project scope rejected by path policy",
            ));
            None
        }
        None => None,
    };

    let mut budget_hit = false;

    // Visible scopes: `_global` always, then the named project — capped by the
    // scope budget (`_global` is kept first as the baseline).
    let mut visible_scopes: Vec<SkillScope> = vec![SkillScope::Global];
    if let Some(p) = &project {
        visible_scopes.push(SkillScope::Project(p.clone()));
    }
    if visible_scopes.len() as u32 > budget.max_scopes {
        visible_scopes.truncate(budget.max_scopes.max(1) as usize);
        budget_hit = true;
    }
    let scope_count = visible_scopes.len() as u32;

    let mut files_seen = 0u32;
    let mut bytes_read = 0u64;
    let mut visible_skills: Vec<SkillDescriptor> = Vec::new();

    for scope in &visible_scopes {
        let dir = root.join(scope.dir_name());
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // missing scope dir → nothing visible here
        };
        // Deterministic order, independent of filesystem enumeration order.
        let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = Vec::new();
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            entries.push((
                entry.file_name().to_string_lossy().to_string(),
                entry.path(),
                ft,
            ));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (fname, path, ft) in entries {
            if !fname.to_ascii_lowercase().ends_with(".md") {
                continue;
            }
            // Never traverse symlinks (no escaping the skills root via a link).
            if ft.is_symlink() {
                issues.push(
                    InventoryIssue::new(
                        CODE_SYMLINK_SKIPPED,
                        IssueSeverity::Warning,
                        "symlinked skill file skipped",
                    )
                    .scope(scope.dir_name())
                    .skill(cap_label(&fname, budget)),
                );
                skipped += 1;
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            if files_seen >= budget.max_files {
                budget_hit = true;
                skipped += 1;
                continue;
            }
            files_seen += 1;
            let remaining_total = budget
                .max_total_frontmatter_bytes
                .saturating_sub(bytes_read);
            if remaining_total == 0 {
                budget_hit = true;
                skipped += 1;
                continue;
            }
            // File identity = the on-disk stem; it drives `source_ref`, so it
            // must be safe and bounded regardless of any frontmatter `name`.
            let stem = &fname[..fname.len().saturating_sub(3)];
            if stem.trim().is_empty()
                || stem.len() > budget.max_name_bytes as usize
                || paths::validate_path_component("skill name", stem).is_err()
            {
                issues.push(
                    InventoryIssue::new(
                        CODE_UNSAFE_NAME,
                        IssueSeverity::Error,
                        "skill file name rejected by path policy",
                    )
                    .scope(scope.dir_name())
                    .skill(cap_label(&fname, budget)),
                );
                skipped += 1;
                continue;
            }
            // Read cap is the smaller of the per-file cap and what remains of
            // the cumulative budget, so a single file can never read over total.
            let read_cap = budget.max_frontmatter_bytes_per_file.min(remaining_total);
            let desc = read_skill_descriptor_budgeted(scope, &path, stem, read_cap, budget);
            bytes_read = bytes_read.saturating_add(desc.frontmatter_bytes_read);
            // Truncation caused by the *total* budget (not the per-file cap) is a
            // partial scan → flag it.
            if desc.truncated && read_cap < budget.max_frontmatter_bytes_per_file {
                budget_hit = true;
            }
            visible_skills.push(desc);
        }
    }

    if budget_hit {
        issues.push(InventoryIssue::new(
            CODE_BUDGET_EXCEEDED,
            IssueSeverity::Warning,
            "scan budget reached; inventory is partial",
        ));
    }

    visible_skills.sort_by(|a, b| a.scope.cmp(&b.scope).then(a.name.cmp(&b.name)));

    // Profile resolution (optional).
    let (profile_resolution, profile_name) = match profile {
        Some(input) => resolve_profile(root, project.as_deref(), input, budget, &mut issues),
        None => (None, None),
    };

    let skill_count = visible_skills.len() as u32;
    let truncated_count = visible_skills.iter().filter(|d| d.truncated).count() as u32;
    let (profile_ref_count, missing_ref_count) = profile_resolution
        .as_ref()
        .map(|r| {
            (
                r.declared_skills.len() as u32,
                r.declared_skills
                    .iter()
                    .filter(|x| x.resolution == RefResolution::Missing)
                    .count() as u32,
            )
        })
        .unwrap_or((0, 0));

    SkillInventory {
        schema_version: crate::schema::SKILL_INVENTORY_V1.to_string(),
        project,
        profile: profile_name,
        summary: SkillInventorySummary {
            scope_count,
            skill_count,
            visible_count: skill_count,
            profile_ref_count,
            missing_ref_count,
            skipped_count: skipped,
            truncated_count,
        },
        visible_skills,
        profile_resolution,
        budget: budget.clone(),
        issues,
    }
}

/// Read at most `max_frontmatter_bytes_per_file`, extract only the first YAML
/// frontmatter block, and never retain body text. `stem` is the already
/// path-validated on-disk file identity.
fn read_skill_descriptor_budgeted(
    scope: &SkillScope,
    path: &Path,
    stem: &str,
    read_cap: u64,
    budget: &SkillInventoryBudget,
) -> SkillDescriptor {
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut descriptor_issues: Vec<InventoryIssue> = Vec::new();
    let (front, frontmatter_bytes_read, truncated) =
        read_frontmatter_budgeted(path, scope, stem, read_cap, budget, &mut descriptor_issues);

    // Prefer a *safe* frontmatter `name`; otherwise fall back to the filename
    // stem. An unsafe/empty frontmatter name is ignored (stem wins).
    let declared = front
        .as_ref()
        .and_then(|f| f.name.as_ref())
        .map(|n| cap_label(n, budget))
        .filter(|n| !n.trim().is_empty() && !label_is_unsafe(n));
    let name = declared.clone().unwrap_or_else(|| stem.to_string());
    let declared_name = declared.filter(|d| d != stem);

    let description = front
        .as_ref()
        .and_then(|f| f.description.as_ref())
        .map(|d| cap_single_line(d, MAX_TEXT_BYTES))
        .filter(|d| !d.is_empty());
    let trigger = front
        .as_ref()
        .and_then(|f| f.trigger.as_ref())
        .map(|t| cap_single_line(t, MAX_TEXT_BYTES))
        .filter(|t| !t.is_empty());

    SkillDescriptor {
        scope: scope.dir_name(),
        name,
        declared_name,
        description,
        trigger,
        source_ref: format!("{}/{}", scope.dir_name(), stem),
        file_bytes,
        frontmatter_bytes_read,
        truncated,
        issues: descriptor_issues,
    }
}

/// Returns `(frontmatter, bytes_read, truncated)`. Mirrors
/// [`super::split_frontmatter`]'s delimiter logic but reads **incrementally and
/// stops at the closing `\n---`**, so it never pulls a body into memory: a file
/// with a short frontmatter and a huge body reads only ~frontmatter bytes, and
/// a file with no frontmatter reads only a small opening probe. `read_cap` is
/// the effective per-file budget (already min'd against the cumulative budget).
fn read_frontmatter_budgeted(
    path: &Path,
    scope: &SkillScope,
    stem: &str,
    read_cap: u64,
    budget: &SkillInventoryBudget,
    issues: &mut Vec<InventoryIssue>,
) -> (Option<SkillFront>, u64, bool) {
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    let invalid = |issues: &mut Vec<InventoryIssue>, msg: &str| {
        issues.push(
            InventoryIssue::new(CODE_FRONTMATTER_INVALID, IssueSeverity::Warning, msg)
                .scope(scope.dir_name())
                .skill(cap_label(stem, budget)),
        );
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            invalid(issues, "skill metadata could not be read");
            return (None, 0, false);
        }
    };
    // Small buffered reader so a body never lands in memory: we consume one byte
    // at a time and stop the moment the closing `\n---` arrives.
    let mut reader = std::io::BufReader::with_capacity(512, file);
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    // `start` = offset of the opening `---` (after an optional BOM), once known.
    let mut start: Option<usize> = None;

    while (buf.len() as u64) < read_cap {
        match reader.read(&mut byte) {
            Ok(0) => break, // EOF
            Ok(_) => buf.push(byte[0]),
            Err(_) => break,
        }

        // Decide whether an opening fence is present.
        if start.is_none() {
            // While the buffer is still a proper prefix of a BOM, keep reading.
            if buf.len() < 3 && BOM.starts_with(&buf[..]) {
                continue;
            }
            let s = if buf.starts_with(&BOM) { 3 } else { 0 };
            if buf.len() >= s + 3 {
                if &buf[s..s + 3] == b"---" {
                    start = Some(s);
                } else {
                    return (None, buf.len() as u64, false); // no frontmatter
                }
            }
            continue;
        }

        // Opening fence present — stop at the first closing `\n---`.
        let s = start.unwrap();
        if buf.len() >= s + 3 + 4 && &buf[buf.len() - 4..] == b"\n---" {
            let fm = String::from_utf8_lossy(&buf[s + 3..buf.len() - 4]);
            let consumed = buf.len() as u64;
            return match serde_yaml::from_str::<SkillFront>(fm.trim()) {
                Ok(front) => (Some(front), consumed, false),
                Err(_) => {
                    invalid(issues, "skill frontmatter is not valid YAML");
                    (None, consumed, false)
                }
            };
        }
    }

    match start {
        // Opening fence present but it never closed within what we read.
        Some(_) if buf.len() as u64 >= read_cap => {
            issues.push(
                InventoryIssue::new(
                    CODE_FRONTMATTER_TRUNCATED,
                    IssueSeverity::Warning,
                    "frontmatter exceeded the read budget",
                )
                .scope(scope.dir_name())
                .skill(cap_label(stem, budget)),
            );
            (None, buf.len() as u64, true)
        }
        Some(_) => {
            invalid(issues, "skill frontmatter is unterminated");
            (None, buf.len() as u64, false)
        }
        // No opening fence (file shorter than the fence, or empty).
        None => (None, buf.len() as u64, false),
    }
}

fn resolve_profile(
    root: &Path,
    project: Option<&str>,
    input: ProfileInput<'_>,
    budget: &SkillInventoryBudget,
    issues: &mut Vec<InventoryIssue>,
) -> (Option<ProfileSkillResolution>, Option<String>) {
    match input {
        ProfileInput::Missing(name) => {
            issues.push(
                InventoryIssue::new(
                    CODE_MISSING_PROFILE,
                    IssueSeverity::Error,
                    "requested profile does not exist",
                )
                .skill(cap_label(name, budget)),
            );
            (None, Some(cap_label(name, budget)))
        }
        ProfileInput::Present(name, profile) => {
            let mut declared = Vec::new();
            for raw in &profile.skills {
                let (skill_ref, shadow) = resolve_ref(root, project, raw, budget);
                if skill_ref.resolution == RefResolution::Missing {
                    issues.push(
                        InventoryIssue::new(
                            CODE_MISSING_SKILL_REF,
                            IssueSeverity::Error,
                            "profile declares a skill that cannot resolve",
                        )
                        .skill(skill_ref.reference.clone()),
                    );
                }
                if let Some(shadow_issue) = shadow {
                    issues.push(shadow_issue);
                }
                declared.push(skill_ref);
            }
            let role = cap_label(&profile.role, budget);
            let resolution = ProfileSkillResolution {
                profile: cap_label(name, budget),
                enabled: profile.enabled,
                role: (!role.trim().is_empty()).then_some(role),
                model_profile: profile.model_profile.as_ref().map(|m| cap_label(m, budget)),
                declared_skills: declared,
            };
            (Some(resolution), Some(cap_label(name, budget)))
        }
    }
}

/// Resolve one declared skill ref under project-first rules, body-free. The
/// optional second value is a `shadowed_by_project` info issue.
fn resolve_ref(
    root: &Path,
    project: Option<&str>,
    raw: &str,
    budget: &SkillInventoryBudget,
) -> (ProfileSkillRef, Option<InventoryIssue>) {
    let trimmed = raw.trim();
    let reference = cap_single_line(trimmed, MAX_TEXT_BYTES);

    if trimmed.is_empty() || label_is_unsafe(trimmed) {
        return (invalid_ref(reference, "path_unsafe"), None);
    }

    if let Some((scope_name, skill_name)) = trimmed.split_once('/') {
        if !valid_component(scope_name, budget) || !valid_component(skill_name, budget) {
            return (invalid_ref(reference, "invalid_ref"), None);
        }
        if skill_file_exists_under(root, scope_name, skill_name) {
            return (
                ProfileSkillRef {
                    reference,
                    resolution: RefResolution::Resolved,
                    resolved_scope: Some(cap_label(scope_name, budget)),
                    resolved_name: Some(cap_label(skill_name, budget)),
                    reason: Some("exact".to_string()),
                },
                None,
            );
        }
        return (
            ProfileSkillRef {
                reference,
                resolution: RefResolution::Missing,
                resolved_scope: None,
                resolved_name: None,
                reason: Some("not_found".to_string()),
            },
            None,
        );
    }

    // Unscoped reference.
    if let Some(p) = project {
        let in_project = skill_file_exists_under(root, p, trimmed);
        let in_global = skill_file_exists_under(root, GLOBAL_SCOPE_DIR, trimmed);
        if in_project {
            let shadow = in_global.then(|| {
                InventoryIssue::new(
                    CODE_SHADOWED_BY_PROJECT,
                    IssueSeverity::Info,
                    "project-local skill shadows _global for this ref",
                )
                .scope(cap_label(p, budget))
                .skill(reference.clone())
            });
            return (
                ProfileSkillRef {
                    reference,
                    resolution: RefResolution::Resolved,
                    resolved_scope: Some(cap_label(p, budget)),
                    resolved_name: Some(cap_label(trimmed, budget)),
                    reason: Some(
                        if in_global {
                            "project_shadow"
                        } else {
                            "project"
                        }
                        .to_string(),
                    ),
                },
                shadow,
            );
        }
        if in_global {
            return (
                ProfileSkillRef {
                    reference,
                    resolution: RefResolution::Resolved,
                    resolved_scope: Some(GLOBAL_SCOPE_DIR.to_string()),
                    resolved_name: Some(cap_label(trimmed, budget)),
                    reason: Some("global_fallback".to_string()),
                },
                None,
            );
        }
        return (
            ProfileSkillRef {
                reference,
                resolution: RefResolution::Missing,
                resolved_scope: None,
                resolved_name: None,
                reason: Some("not_found".to_string()),
            },
            None,
        );
    }

    // No project context: only `_global` resolves; otherwise deferred (cannot
    // decide without a project to check the project scope).
    if skill_file_exists_under(root, GLOBAL_SCOPE_DIR, trimmed) {
        return (
            ProfileSkillRef {
                reference,
                resolution: RefResolution::Resolved,
                resolved_scope: Some(GLOBAL_SCOPE_DIR.to_string()),
                resolved_name: Some(cap_label(trimmed, budget)),
                reason: Some("global".to_string()),
            },
            None,
        );
    }
    (
        ProfileSkillRef {
            reference,
            resolution: RefResolution::Deferred,
            resolved_scope: None,
            resolved_name: None,
            reason: Some("no_project_context".to_string()),
        },
        None,
    )
}

fn invalid_ref(reference: String, reason: &str) -> ProfileSkillRef {
    ProfileSkillRef {
        reference,
        resolution: RefResolution::Invalid,
        resolved_scope: None,
        resolved_name: None,
        reason: Some(reason.to_string()),
    }
}

/// Read-only existence of `<root>/<scope_dir>/<sanitize(name)>.md`, mirroring
/// dispatch's filename derivation. A scope failing component validation can't
/// exist.
fn skill_file_exists_under(root: &Path, scope_dir: &str, name: &str) -> bool {
    if scope_dir != GLOBAL_SCOPE_DIR
        && paths::validate_path_component("skill scope", scope_dir).is_err()
    {
        return false;
    }
    let file = root.join(scope_dir).join(format!("{}.md", sanitize(name)));
    // `symlink_metadata` does NOT follow the final component, so a symlinked
    // skill (which the scanner skips) does not count as a resolvable file —
    // resolution and visible_skills stay consistent.
    std::fs::symlink_metadata(&file)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

fn valid_component(value: &str, budget: &SkillInventoryBudget) -> bool {
    value.len() <= budget.max_name_bytes as usize
        && (value == GLOBAL_SCOPE_DIR || paths::validate_path_component("skill ref", value).is_ok())
}

fn label_is_unsafe(value: &str) -> bool {
    if value.trim_start().to_ascii_lowercase().starts_with("file:") {
        return true;
    }
    if Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    if value.split(['/', '\\']).any(|c| c == "..") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

/// Single-line, trimmed, byte-capped to the budget's name cap.
fn cap_label(value: &str, budget: &SkillInventoryBudget) -> String {
    cap_single_line(value, budget.max_name_bytes as usize)
}

fn cap_single_line(value: &str, max_bytes: usize) -> String {
    let one_line = value.split(['\n', '\r']).next().unwrap_or("").trim();
    truncate_bytes(one_line, max_bytes)
}

fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::agent_profile::AgentProfile;
    use std::fs;
    use std::path::Path;

    fn write_skill(root: &Path, scope: &str, file: &str, body: &str) {
        let dir = root.join(scope);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), body).unwrap();
    }

    fn profile(skills: &[&str]) -> AgentProfile {
        AgentProfile {
            role: "backend_rust".to_string(),
            skills: skills.iter().map(|s| s.to_string()).collect(),
            model_profile: None,
            context_budget_bytes: None,
            priority: 0,
            triggers: vec![],
            outputs: vec![],
            enabled: true,
        }
    }

    #[test]
    fn metadata_only_never_returns_body() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "contract-reviewer.md",
            "---\ndescription: review contracts\ntrigger: review\n---\nSECRET_BODY_MARKER body text here\n",
        );
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        let json = serde_json::to_string(&inv).unwrap();
        assert!(
            !json.contains("SECRET_BODY_MARKER"),
            "body leaked into inventory"
        );
        assert_eq!(inv.visible_skills.len(), 1);
        let d = &inv.visible_skills[0];
        assert_eq!(d.name, "contract-reviewer");
        assert_eq!(d.description.as_deref(), Some("review contracts"));
        assert_eq!(d.trigger.as_deref(), Some("review"));
    }

    #[test]
    fn huge_body_with_marker_never_appears() {
        let tmp = tempfile::tempdir().unwrap();
        let body = format!(
            "---\ndescription: ok\n---\n{}",
            "HUGE_MARKER ".repeat(50_000) // ~600 KiB body
        );
        write_skill(tmp.path(), "_global", "big.md", &body);
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        let json = serde_json::to_string(&inv).unwrap();
        assert!(!json.contains("HUGE_MARKER"));
        assert!(
            inv.visible_skills[0].frontmatter_bytes_read <= DEFAULT_MAX_FRONTMATTER_BYTES_PER_FILE
        );
    }

    #[test]
    fn complete_frontmatter_returns_fields() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "release-check.md",
            "---\nname: release-check\ndescription: run release checks\ntrigger: release|ship\n---\nbody\n",
        );
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        let d = &inv.visible_skills[0];
        assert_eq!(d.name, "release-check");
        assert_eq!(d.declared_name, None); // matches filename → not repeated
        assert_eq!(d.trigger.as_deref(), Some("release|ship"));
        assert_eq!(d.source_ref, "_global/release-check");
    }

    #[test]
    fn frontmatter_name_override_is_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "cr.md",
            "---\nname: contract-reviewer\ndescription: x\n---\nbody\n",
        );
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        let d = &inv.visible_skills[0];
        assert_eq!(d.name, "contract-reviewer");
        assert_eq!(d.declared_name.as_deref(), Some("contract-reviewer"));
        assert_eq!(d.source_ref, "_global/cr"); // source_ref tracks the file stem
    }

    #[test]
    fn incomplete_frontmatter_at_cap_is_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        // Frontmatter opens but never closes within the tiny per-file cap.
        let body = format!("---\ndescription: {}\n", "a".repeat(2000));
        write_skill(tmp.path(), "_global", "long-fm.md", &body);
        let budget = SkillInventoryBudget {
            max_frontmatter_bytes_per_file: 64,
            ..SkillInventoryBudget::default()
        };
        let inv = build_inventory(tmp.path(), None, None, &budget);
        validate_inventory(&inv).unwrap();
        let d = &inv.visible_skills[0];
        assert!(d.truncated, "should be truncated by per-file budget");
        assert_eq!(inv.summary.truncated_count, 1);
        assert!(inv.visible_skills.iter().any(|d| d
            .issues
            .iter()
            .any(|i| i.code == CODE_FRONTMATTER_TRUNCATED)));
    }

    #[test]
    fn symlinked_skill_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "real.md",
            "---\ndescription: real\n---\nbody\n",
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            tmp.path().join("_global").join("real.md"),
            tmp.path().join("_global").join("link.md"),
        )
        .unwrap();
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        #[cfg(unix)]
        {
            assert_eq!(
                inv.visible_skills.len(),
                1,
                "symlink must not become a descriptor"
            );
            assert!(inv.issues.iter().any(|i| i.code == CODE_SYMLINK_SKIPPED));
            assert!(inv.summary.skipped_count >= 1);
        }
    }

    #[test]
    fn over_long_filename_is_unsafe_name_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let long = format!("{}.md", "x".repeat(200));
        write_skill(
            tmp.path(),
            "_global",
            &long,
            "---\ndescription: x\n---\nbody\n",
        );
        write_skill(
            tmp.path(),
            "_global",
            "ok.md",
            "---\ndescription: x\n---\nbody\n",
        );
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        assert_eq!(
            inv.visible_skills.len(),
            1,
            "only the safe file is a descriptor"
        );
        assert_eq!(inv.visible_skills[0].name, "ok");
        assert!(inv.issues.iter().any(|i| i.code == CODE_UNSAFE_NAME));
    }

    #[test]
    fn unsafe_project_scope_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "ok.md",
            "---\ndescription: x\n---\nbody\n",
        );
        let inv = build_inventory(
            tmp.path(),
            Some("../evil"),
            None,
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        assert!(inv.project.is_none(), "unsafe project not echoed");
        assert!(inv.issues.iter().any(|i| i.code == CODE_UNSAFE_SCOPE));
    }

    #[test]
    fn project_local_skill_shadows_global_for_unscoped_ref() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "contract-reviewer.md",
            "---\ndescription: g\n---\nb\n",
        );
        write_skill(
            tmp.path(),
            "billing-service",
            "contract-reviewer.md",
            "---\ndescription: p\n---\nb\n",
        );
        let p = profile(&["contract-reviewer"]);
        let inv = build_inventory(
            tmp.path(),
            Some("billing-service"),
            Some(ProfileInput::Present("reviewer", &p)),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        let res = inv.profile_resolution.as_ref().unwrap();
        let r = &res.declared_skills[0];
        assert_eq!(r.resolution, RefResolution::Resolved);
        assert_eq!(r.resolved_scope.as_deref(), Some("billing-service"));
        assert!(inv
            .issues
            .iter()
            .any(|i| i.code == CODE_SHADOWED_BY_PROJECT));
    }

    #[test]
    fn explicit_global_ref_ignores_project_shadow() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "contract-reviewer.md",
            "---\ndescription: g\n---\nb\n",
        );
        write_skill(
            tmp.path(),
            "billing-service",
            "contract-reviewer.md",
            "---\ndescription: p\n---\nb\n",
        );
        let p = profile(&["_global/contract-reviewer"]);
        let inv = build_inventory(
            tmp.path(),
            Some("billing-service"),
            Some(ProfileInput::Present("reviewer", &p)),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        let r = &inv.profile_resolution.as_ref().unwrap().declared_skills[0];
        assert_eq!(r.resolution, RefResolution::Resolved);
        assert_eq!(r.resolved_scope.as_deref(), Some("_global"));
        assert!(!inv
            .issues
            .iter()
            .any(|i| i.code == CODE_SHADOWED_BY_PROJECT));
    }

    #[test]
    fn missing_profile_ref_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let p = profile(&["does-not-exist"]);
        let inv = build_inventory(
            tmp.path(),
            Some("billing-service"),
            Some(ProfileInput::Present("reviewer", &p)),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        let r = &inv.profile_resolution.as_ref().unwrap().declared_skills[0];
        assert_eq!(r.resolution, RefResolution::Missing);
        assert_eq!(inv.summary.missing_ref_count, 1);
        assert!(inv.issues.iter().any(|i| i.code == CODE_MISSING_SKILL_REF));
    }

    #[test]
    fn missing_profile_yields_issue_not_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = build_inventory(
            tmp.path(),
            None,
            Some(ProfileInput::Missing("ghost")),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        assert!(inv.profile_resolution.is_none());
        assert_eq!(inv.profile.as_deref(), Some("ghost"));
        assert!(inv.issues.iter().any(|i| i.code == CODE_MISSING_PROFILE));
    }

    #[test]
    fn unscoped_ref_without_project_is_deferred() {
        let tmp = tempfile::tempdir().unwrap();
        let p = profile(&["floating"]);
        let inv = build_inventory(
            tmp.path(),
            None,
            Some(ProfileInput::Present("reviewer", &p)),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        let r = &inv.profile_resolution.as_ref().unwrap().declared_skills[0];
        assert_eq!(r.resolution, RefResolution::Deferred);
    }

    #[test]
    fn budget_overflow_returns_partial_plus_warning() {
        let tmp = tempfile::tempdir().unwrap();
        for n in ["a", "b", "c"] {
            write_skill(
                tmp.path(),
                "_global",
                &format!("{n}.md"),
                "---\ndescription: x\n---\nb\n",
            );
        }
        let budget = SkillInventoryBudget {
            max_files: 1,
            ..SkillInventoryBudget::default()
        };
        let inv = build_inventory(tmp.path(), None, None, &budget);
        validate_inventory(&inv).unwrap();
        assert_eq!(inv.visible_skills.len(), 1, "partial");
        assert!(inv.summary.skipped_count >= 2);
        assert!(inv.issues.iter().any(|i| i.code == CODE_BUDGET_EXCEEDED));
    }

    #[test]
    fn missing_skills_root_is_empty_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = build_inventory(
            &tmp.path().join("nonexistent"),
            Some("billing-service"),
            None,
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        assert!(inv.visible_skills.is_empty());
        assert_eq!(inv.summary.skill_count, 0);
    }

    // N1 — incremental read stops at the closing fence, never slurping a body.
    #[test]
    fn short_frontmatter_does_not_read_huge_body() {
        let tmp = tempfile::tempdir().unwrap();
        let body = format!(
            "---\ndescription: short\n---\n{}",
            "BODY_MARKER ".repeat(100_000) // ~1.2 MiB body
        );
        write_skill(tmp.path(), "_global", "tight.md", &body);
        let inv = build_inventory(tmp.path(), None, None, &SkillInventoryBudget::default());
        validate_inventory(&inv).unwrap();
        let d = &inv.visible_skills[0];
        assert_eq!(d.description.as_deref(), Some("short"));
        assert!(!d.truncated);
        assert!(
            d.frontmatter_bytes_read < 128,
            "read {} bytes; should be ~frontmatter length, not the 16 KiB cap",
            d.frontmatter_bytes_read
        );
        let json = serde_json::to_string(&inv).unwrap();
        assert!(!json.contains("BODY_MARKER"));
    }

    // N2 — the cumulative byte budget is a hard cap even for a single file.
    #[test]
    fn total_byte_budget_caps_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let body = format!("---\ndescription: {}\n---\nbody\n", "a".repeat(500));
        write_skill(tmp.path(), "_global", "fat.md", &body);
        let budget = SkillInventoryBudget {
            max_total_frontmatter_bytes: 16,
            ..SkillInventoryBudget::default()
        };
        let inv = build_inventory(tmp.path(), None, None, &budget);
        validate_inventory(&inv).unwrap();
        let d = &inv.visible_skills[0];
        assert!(
            d.frontmatter_bytes_read <= 16,
            "must respect the total budget"
        );
        assert!(d.truncated);
        assert!(inv.issues.iter().any(|i| i.code == CODE_BUDGET_EXCEEDED));
    }

    // N3 — a ref to a symlinked skill must not resolve (the scanner skips it).
    #[test]
    #[cfg(unix)]
    fn profile_ref_to_symlink_is_not_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "real.md",
            "---\ndescription: r\n---\nb\n",
        );
        std::os::unix::fs::symlink(
            tmp.path().join("_global").join("real.md"),
            tmp.path().join("_global").join("linked.md"),
        )
        .unwrap();
        let p = profile(&["linked"]);
        let inv = build_inventory(
            tmp.path(),
            Some("billing-service"),
            Some(ProfileInput::Present("reviewer", &p)),
            &SkillInventoryBudget::default(),
        );
        validate_inventory(&inv).unwrap();
        let r = &inv.profile_resolution.as_ref().unwrap().declared_skills[0];
        assert_eq!(
            r.resolution,
            RefResolution::Missing,
            "symlink must not resolve"
        );
    }

    // N4 — max_scopes gates the scan and warns.
    #[test]
    fn max_scopes_caps_visible_scopes_with_warning() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "_global",
            "g.md",
            "---\ndescription: g\n---\nb\n",
        );
        write_skill(
            tmp.path(),
            "billing-service",
            "p.md",
            "---\ndescription: p\n---\nb\n",
        );
        let budget = SkillInventoryBudget {
            max_scopes: 1,
            ..SkillInventoryBudget::default()
        };
        let inv = build_inventory(tmp.path(), Some("billing-service"), None, &budget);
        validate_inventory(&inv).unwrap();
        assert_eq!(inv.summary.scope_count, 1);
        assert!(inv.visible_skills.iter().all(|d| d.scope == "_global"));
        assert!(inv.issues.iter().any(|i| i.code == CODE_BUDGET_EXCEEDED));
    }
}
