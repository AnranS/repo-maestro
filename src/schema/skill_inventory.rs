//! F-121 — skill / profile visibility inventory (schema).
//!
//! A read-only metadata projection that explains which skills are visible to a
//! project or specialist profile, and how a profile's declared skill references
//! resolve — **without ever loading or returning skill body text**. This is the
//! pre-run visibility counterpart to the F-116 `skills.section` context layer
//! (which records what was *actually* injected at dispatch).
//!
//! The builder lives in [`crate::skills::inventory`]; this module owns the wire
//! contract and its strict, body-free validation. Nothing here carries Markdown
//! body, absolute paths, or content hashes.
//!
//! Design: `docs/experience/F-121-SKILL-INVENTORY-DESIGN.md`.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::schema::SKILL_INVENTORY_V1;

/// Cap for descriptor / ref labels (names, scopes, refs).
pub const MAX_NAME_BYTES: usize = 120;
/// Cap for free-ish single-line text (descriptions, triggers, messages,
/// source refs, suggestions).
pub const MAX_TEXT_BYTES: usize = 256;

/// Conservative scan-budget defaults (see design "Scan budget").
pub const DEFAULT_MAX_SCOPES: u32 = 128;
pub const DEFAULT_MAX_FILES: u32 = 512;
pub const DEFAULT_MAX_TOTAL_FRONTMATTER_BYTES: u64 = 1 << 20; // 1 MiB
pub const DEFAULT_MAX_FRONTMATTER_BYTES_PER_FILE: u64 = 16 << 10; // 16 KiB

/// Stable issue codes (flat dotted, F-111 issue-envelope style).
pub const CODE_BUDGET_EXCEEDED: &str = "skill_inventory.budget_exceeded";
pub const CODE_FRONTMATTER_TRUNCATED: &str = "skill_inventory.frontmatter_truncated";
pub const CODE_FRONTMATTER_INVALID: &str = "skill_inventory.frontmatter_invalid";
pub const CODE_UNSAFE_SCOPE: &str = "skill_inventory.unsafe_scope";
pub const CODE_UNSAFE_NAME: &str = "skill_inventory.unsafe_name";
pub const CODE_SYMLINK_SKIPPED: &str = "skill_inventory.symlink_skipped";
pub const CODE_MISSING_PROFILE: &str = "skill_inventory.missing_profile";
pub const CODE_MISSING_SKILL_REF: &str = "skill_inventory.missing_skill_ref";
pub const CODE_SHADOWED_BY_PROJECT: &str = "skill_inventory.shadowed_by_project";

/// Severity of an [`InventoryIssue`]. Serializes to `info` / `warning` /
/// `error` so the wire field is a stable string but invalid values are
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
}

/// How a declared profile skill reference resolved under project-first rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefResolution {
    /// Found at a concrete scope.
    Resolved,
    /// A project context exists but the ref matched no scope.
    Missing,
    /// The ref string is path-unsafe or malformed.
    Invalid,
    /// Unscoped ref with no project context — cannot be decided in v1.
    Deferred,
}

/// One neutral, single-line, capped issue. Mirrors the F-111 issue envelope
/// shape without importing the full `PlanPreview` types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryIssue {
    pub code: String,
    pub severity: IssueSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

impl InventoryIssue {
    pub fn new(code: &str, severity: IssueSeverity, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity,
            scope: None,
            skill: None,
            message: message.into(),
            suggestions: Vec::new(),
        }
    }

    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    pub fn skill(mut self, skill: impl Into<String>) -> Self {
        self.skill = Some(skill.into());
        self
    }
}

/// Effective scan limits echoed back in the response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillInventoryBudget {
    pub max_scopes: u32,
    pub max_files: u32,
    pub max_total_frontmatter_bytes: u64,
    pub max_frontmatter_bytes_per_file: u64,
    pub max_name_bytes: u32,
}

impl Default for SkillInventoryBudget {
    fn default() -> Self {
        Self {
            max_scopes: DEFAULT_MAX_SCOPES,
            max_files: DEFAULT_MAX_FILES,
            max_total_frontmatter_bytes: DEFAULT_MAX_TOTAL_FRONTMATTER_BYTES,
            max_frontmatter_bytes_per_file: DEFAULT_MAX_FRONTMATTER_BYTES_PER_FILE,
            max_name_bytes: MAX_NAME_BYTES as u32,
        }
    }
}

/// Counts and warning tallies for a glance-level summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillInventorySummary {
    pub scope_count: u32,
    pub skill_count: u32,
    pub visible_count: u32,
    pub profile_ref_count: u32,
    pub missing_ref_count: u32,
    pub skipped_count: u32,
    pub truncated_count: u32,
}

/// Metadata-only descriptor for one visible skill file. **No body field.**
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    pub scope: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    pub source_ref: String,
    pub file_bytes: u64,
    pub frontmatter_bytes_read: u64,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<InventoryIssue>,
}

/// One declared profile skill ref plus how it resolves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSkillRef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub resolution: RefResolution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Resolution of a specialist profile's declared skill references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSkillResolution {
    pub profile: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,
    pub declared_skills: Vec<ProfileSkillRef>,
}

/// The top-level read-only inventory projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillInventory {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub summary: SkillInventorySummary,
    pub visible_skills: Vec<SkillDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_resolution: Option<ProfileSkillResolution>,
    pub budget: SkillInventoryBudget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<InventoryIssue>,
}

/// Strict, body-free validation. Run before serving and after parsing so a
/// corrupt or tampered inventory is never trusted. Rejects absolute paths,
/// multi-line / oversized text, and summary counts that disagree with the
/// payload.
pub fn validate_inventory(inv: &SkillInventory) -> Result<()> {
    ensure!(
        inv.schema_version == SKILL_INVENTORY_V1,
        "skill inventory schema_version {:?} != {SKILL_INVENTORY_V1}",
        inv.schema_version
    );

    if let Some(project) = &inv.project {
        validate_label("inventory project", project)?;
    }
    if let Some(profile) = &inv.profile {
        validate_label("inventory profile", profile)?;
    }

    ensure!(inv.budget.max_scopes > 0, "budget.max_scopes must be > 0");
    ensure!(inv.budget.max_files > 0, "budget.max_files must be > 0");
    ensure!(
        inv.budget.max_total_frontmatter_bytes > 0,
        "budget.max_total_frontmatter_bytes must be > 0"
    );
    ensure!(
        inv.budget.max_frontmatter_bytes_per_file > 0,
        "budget.max_frontmatter_bytes_per_file must be > 0"
    );
    ensure!(
        inv.budget.max_name_bytes > 0,
        "budget.max_name_bytes must be > 0"
    );

    let mut truncated = 0u32;
    for d in &inv.visible_skills {
        validate_label("descriptor scope", &d.scope)?;
        validate_label("descriptor name", &d.name)?;
        if let Some(declared) = &d.declared_name {
            validate_label("descriptor declared_name", declared)?;
        }
        if let Some(desc) = &d.description {
            validate_text("descriptor description", desc, MAX_TEXT_BYTES)?;
        }
        if let Some(trig) = &d.trigger {
            validate_text("descriptor trigger", trig, MAX_TEXT_BYTES)?;
        }
        validate_text("descriptor source_ref", &d.source_ref, MAX_TEXT_BYTES)?;
        ensure!(
            !ref_value_is_unsafe(&d.source_ref),
            "descriptor source_ref must be symbolic, not a path: {:?}",
            d.source_ref
        );
        ensure!(
            d.frontmatter_bytes_read <= d.file_bytes,
            "descriptor frontmatter_bytes_read {} exceeds file_bytes {}",
            d.frontmatter_bytes_read,
            d.file_bytes
        );
        for issue in &d.issues {
            validate_issue(issue)?;
        }
        if d.truncated {
            truncated += 1;
        }
    }

    let mut profile_ref_count = 0u32;
    let mut missing_ref_count = 0u32;
    if let Some(res) = &inv.profile_resolution {
        validate_label("profile_resolution profile", &res.profile)?;
        if let Some(role) = &res.role {
            validate_label("profile_resolution role", role)?;
        }
        if let Some(mp) = &res.model_profile {
            validate_label("profile_resolution model_profile", mp)?;
        }
        for r in &res.declared_skills {
            validate_text("profile skill ref", &r.reference, MAX_TEXT_BYTES)?;
            ensure!(
                !r.reference.trim().is_empty(),
                "profile skill ref must be non-empty"
            );
            if let Some(scope) = &r.resolved_scope {
                validate_label("resolved_scope", scope)?;
            }
            if let Some(name) = &r.resolved_name {
                validate_label("resolved_name", name)?;
            }
            if let Some(reason) = &r.reason {
                validate_text("ref reason", reason, MAX_TEXT_BYTES)?;
            }
            profile_ref_count += 1;
            if r.resolution == RefResolution::Missing {
                missing_ref_count += 1;
            }
        }
    }

    for issue in &inv.issues {
        validate_issue(issue)?;
    }

    let skill_count = inv.visible_skills.len() as u32;
    ensure!(
        inv.summary.skill_count == skill_count,
        "summary.skill_count {} != visible_skills {}",
        inv.summary.skill_count,
        skill_count
    );
    ensure!(
        inv.summary.visible_count == skill_count,
        "summary.visible_count {} != visible_skills {}",
        inv.summary.visible_count,
        skill_count
    );
    ensure!(
        inv.summary.truncated_count == truncated,
        "summary.truncated_count {} != truncated descriptors {}",
        inv.summary.truncated_count,
        truncated
    );
    ensure!(
        inv.summary.profile_ref_count == profile_ref_count,
        "summary.profile_ref_count {} != declared refs {}",
        inv.summary.profile_ref_count,
        profile_ref_count
    );
    ensure!(
        inv.summary.missing_ref_count == missing_ref_count,
        "summary.missing_ref_count {} != missing refs {}",
        inv.summary.missing_ref_count,
        missing_ref_count
    );
    Ok(())
}

fn validate_issue(issue: &InventoryIssue) -> Result<()> {
    ensure!(
        !issue.code.trim().is_empty(),
        "issue code must be non-empty"
    );
    validate_text("issue code", &issue.code, MAX_NAME_BYTES)?;
    ensure!(
        issue
            .code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'),
        "issue code must be a flat dotted code: {:?}",
        issue.code
    );
    if let Some(scope) = &issue.scope {
        validate_label("issue scope", scope)?;
    }
    if let Some(skill) = &issue.skill {
        validate_text("issue skill", skill, MAX_TEXT_BYTES)?;
    }
    validate_text("issue message", &issue.message, MAX_TEXT_BYTES)?;
    for s in &issue.suggestions {
        validate_text("issue suggestion", s, MAX_TEXT_BYTES)?;
    }
    Ok(())
}

/// A short symbolic label: non-empty, single-line, capped, and never an
/// absolute / traversal path.
fn validate_label(field: &str, value: &str) -> Result<()> {
    validate_text(field, value, MAX_NAME_BYTES)?;
    ensure!(!value.trim().is_empty(), "{field} must be non-empty");
    ensure!(
        !ref_value_is_unsafe(value),
        "{field} must be symbolic, not a path: {value:?}"
    );
    Ok(())
}

/// Single-line + byte-capped text.
fn validate_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    ensure!(
        !value.contains(['\n', '\r']),
        "{field} must be single-line (no newline characters)"
    );
    ensure!(
        value.len() <= max_bytes,
        "{field} exceeds {max_bytes} bytes"
    );
    Ok(())
}

/// Reject `file:` URIs, Unix/Windows/UNC absolutes, leading slash/backslash,
/// and `..` traversal. A `scope/name` ref keeps its single `/` (only `..`
/// components are unsafe).
fn ref_value_is_unsafe(value: &str) -> bool {
    if value.trim_start().to_ascii_lowercase().starts_with("file:") {
        return true;
    }
    if Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(scope: &str, name: &str) -> SkillDescriptor {
        SkillDescriptor {
            scope: scope.to_string(),
            name: name.to_string(),
            declared_name: None,
            description: Some("one line".to_string()),
            trigger: None,
            source_ref: format!("{scope}/{name}"),
            file_bytes: 400,
            frontmatter_bytes_read: 80,
            truncated: false,
            issues: vec![],
        }
    }

    fn inventory(descs: Vec<SkillDescriptor>) -> SkillInventory {
        let skill_count = descs.len() as u32;
        SkillInventory {
            schema_version: SKILL_INVENTORY_V1.to_string(),
            project: Some("billing-service".to_string()),
            profile: None,
            summary: SkillInventorySummary {
                scope_count: 2,
                skill_count,
                visible_count: skill_count,
                profile_ref_count: 0,
                missing_ref_count: 0,
                skipped_count: 0,
                truncated_count: descs.iter().filter(|d| d.truncated).count() as u32,
            },
            visible_skills: descs,
            profile_resolution: None,
            budget: SkillInventoryBudget::default(),
            issues: vec![],
        }
    }

    #[test]
    fn valid_inventory_passes() {
        let inv = inventory(vec![
            descriptor("_global", "contract-reviewer"),
            descriptor("billing-service", "release-check"),
        ]);
        validate_inventory(&inv).unwrap();
    }

    #[test]
    fn wrong_schema_version_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.schema_version = "maestro.skill_inventory.v2".to_string();
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn absolute_source_ref_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.visible_skills[0].source_ref = "/etc/passwd".to_string();
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn traversal_scope_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.visible_skills[0].scope = "../escape".to_string();
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn multiline_description_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.visible_skills[0].description = Some("line one\nline two".to_string());
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn frontmatter_read_beyond_file_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.visible_skills[0].frontmatter_bytes_read = 9_999;
        inv.visible_skills[0].file_bytes = 100;
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn summary_count_mismatch_rejected() {
        let mut inv = inventory(vec![descriptor("_global", "x")]);
        inv.summary.skill_count = 9;
        assert!(validate_inventory(&inv).is_err());
    }

    #[test]
    fn missing_ref_count_must_match() {
        let mut inv = inventory(vec![]);
        inv.profile = Some("reviewer".to_string());
        inv.profile_resolution = Some(ProfileSkillResolution {
            profile: "reviewer".to_string(),
            enabled: true,
            role: Some("backend_rust".to_string()),
            model_profile: None,
            declared_skills: vec![ProfileSkillRef {
                reference: "missing-one".to_string(),
                resolution: RefResolution::Missing,
                resolved_scope: None,
                resolved_name: None,
                reason: Some("not_found".to_string()),
            }],
        });
        inv.summary.profile_ref_count = 1;
        inv.summary.missing_ref_count = 0; // wrong on purpose
        assert!(validate_inventory(&inv).is_err());
        inv.summary.missing_ref_count = 1;
        validate_inventory(&inv).unwrap();
    }

    #[test]
    fn bad_issue_code_rejected() {
        let mut inv = inventory(vec![]);
        inv.issues.push(InventoryIssue::new(
            "Bad Code!",
            IssueSeverity::Warning,
            "x",
        ));
        assert!(validate_inventory(&inv).is_err());
    }
}
