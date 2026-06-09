//! F-114 read-only existence checks for agent-profile `role`s and `skill`s against
//! the on-disk registries (`roles::exists` / `skills::reference_exists`).
//!
//! This is the single source of truth shared by `maestro validate` and F-118
//! runtime health, so both apply the same visibility rules. It lives at the crate
//! root (not under `cli::commands`) so non-CLI callers — the server's runtime-health
//! endpoint included — reuse it without reaching into a CLI command module. Pure
//! and filesystem-read-only: never creates `.maestro/roles` or `.maestro/skills`.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::ProjectsConfig;

/// The split result of an existence pass. `role_issues` are role-existence
/// problems; `skill_issues` are the (scope-aware) skill resolution problems.
/// `checked_skill_refs` is the distinct, non-deferred refs that were actually
/// verified; `unresolved_skill_refs` is the subset that failed.
#[derive(Debug, Default)]
pub struct ProfileExistenceReport {
    pub role_issues: Vec<String>,
    pub skill_issues: Vec<String>,
    pub checked_skill_refs: BTreeSet<String>,
    pub unresolved_skill_refs: BTreeSet<String>,
}

/// Compute the F-114 role/skill existence report for a loaded config, applying the
/// shared visibility rules: explicit scope → exact; an unscoped skill → per
/// pinning-project scope then global; an unscoped skill on an unreferenced /
/// trigger-only profile → deferred to the resolver (NOT a hard failure).
pub fn profile_existence_report(cfg: &ProjectsConfig) -> ProfileExistenceReport {
    let mut out = ProfileExistenceReport::default();
    // Which projects pin each profile (via `agent_profile` / `review_profile`).
    let mut pinned_by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (pname, proj) in &cfg.projects {
        for reference in [&proj.agent_profile, &proj.review_profile]
            .into_iter()
            .flatten()
        {
            pinned_by.entry(reference).or_default().push(pname);
        }
    }
    for (name, profile) in &cfg.defaults.agent_profiles {
        // Blank role/skills are already reported by the pure `issues()` helpers;
        // here we only add on-disk existence problems to avoid duplicates.
        if !profile.role.trim().is_empty() && !crate::roles::exists(&profile.role) {
            out.role_issues.push(format!(
                "agent_profile '{name}': role '{}' is not defined",
                profile.role
            ));
        }
        for skill_ref in &profile.skills {
            if skill_ref.trim().is_empty() {
                continue;
            }
            if skill_ref.contains('/') {
                // explicit scope (`_global/foo` / `<project>/foo`) — exact check.
                out.checked_skill_refs.insert(skill_ref.clone());
                if !crate::skills::reference_exists(None, skill_ref) {
                    out.unresolved_skill_refs.insert(skill_ref.clone());
                    out.skill_issues.push(format!(
                        "agent_profile '{name}': skill '{skill_ref}' is not defined"
                    ));
                }
            } else if let Some(projects) = pinned_by.get(name.as_str()) {
                // unscoped: must resolve (project scope, then global) for every
                // project that pins this profile.
                out.checked_skill_refs.insert(skill_ref.clone());
                for proj in projects {
                    if !crate::skills::reference_exists(Some(proj), skill_ref) {
                        out.unresolved_skill_refs.insert(skill_ref.clone());
                        out.skill_issues.push(format!(
                            "agent_profile '{name}': skill '{skill_ref}' is not defined for project '{proj}'"
                        ));
                    }
                }
            }
            // else: unscoped skill on an unreferenced profile → defer to resolver.
        }
    }
    out
}
