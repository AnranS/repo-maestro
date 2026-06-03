//! `maestro role` subcommand: discover and inspect builder/cross-cutting
//! role personas the orchestrator can adopt per task.
//!
//! - `role ls`     — list every role visible to this workspace (builtins +
//!   `.maestro/roles/`), grouped by tag, with which projects currently use them.
//! - `role show N` — print one role's prelude markdown to stdout.
//! - `role export N` — copy a builtin role into `.maestro/roles/<n>.md` so
//!   the user can edit it (workspace overrides win).
//!
//! Intentionally tiny — no add/save/delete CLI. Users edit markdown files
//! directly. The role registry is read-only at runtime; changes show up on
//! the next invocation.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::ProjectsConfig;
use crate::paths;
use crate::roles;

pub fn cmd_role_ls() -> Result<()> {
    let roles = roles::list_all()?;
    if roles.is_empty() {
        println!("(no roles loaded — this should not happen; builtins are embedded)");
        return Ok(());
    }

    let projects_by_role = match load_projects_silent() {
        Some(cfg) => roles::projects_by_role(&cfg),
        None => BTreeMap::new(),
    };

    // Group by the first tag (`backend`, `frontend`, `mobile`, `game`,
    // `cross-cutting`, or `other`). One role can appear in multiple tags,
    // but for the listing we want a single bucket.
    let mut by_group: BTreeMap<String, Vec<&roles::RoleSummary>> = BTreeMap::new();
    for r in &roles {
        let group = r.tags.first().cloned().unwrap_or_else(|| "other".into());
        by_group.entry(group).or_default().push(r);
    }

    println!("{:<18}  {:<40}  WHERE", "ROLE", "DISPLAY");
    for (group, group_roles) in &by_group {
        println!("\n# {group}");
        for r in group_roles {
            let where_used = projects_by_role
                .get(&r.name)
                .map(|v| v.join(", "))
                .unwrap_or_else(|| "—".into());
            let marker = if r.builtin { " " } else { "*" };
            println!(
                "{marker} {:<16}  {:<40}  {}",
                r.name,
                truncate(&r.display, 40),
                where_used
            );
        }
    }
    println!(
        "\n  ({} role(s) total; `*` = user-defined in {})",
        roles.len(),
        relative_to_cwd(&roles::roles_root()?)
    );
    Ok(())
}

pub fn cmd_role_show(name: &str) -> Result<()> {
    let role = roles::load(name)?;
    println!("# {} · {}", role.name, role.display);
    if !role.summary.is_empty() {
        println!("\n> {}", role.summary);
    }
    if let Some(src) = &role.source {
        println!("\n_source: {src}_");
    }
    if !role.tags.is_empty() {
        println!("_tags: {}_", role.tags.join(", "));
    }
    println!("\n{}", "-".repeat(60));
    println!("{}", role.prelude.trim_end());
    if let Some(p) = &role.path {
        println!("\n_loaded from {}_", relative_to_cwd(p));
    } else {
        println!(
            "\n_(builtin — use `maestro role export {}` to customize)_",
            role.name
        );
    }
    Ok(())
}

pub fn cmd_role_export(name: &str) -> Result<()> {
    let path = roles::export_to_workspace(name)?;
    println!("exported → {}", relative_to_cwd(&path));
    println!("edit it and the next `maestro run` will pick up your changes.");
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn load_projects_silent() -> Option<ProjectsConfig> {
    let pfile = paths::projects_file().ok()?;
    if !pfile.exists() {
        return None;
    }
    ProjectsConfig::load(&pfile).ok()
}

fn relative_to_cwd(p: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| p.strip_prefix(&cwd).ok().map(|p| p.to_path_buf()))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string_lossy().to_string())
}
