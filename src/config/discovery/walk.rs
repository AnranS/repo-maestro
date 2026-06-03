use super::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn collect_candidates(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<Candidate>,
    ignores: &DiscoveryIgnores,
) -> Result<()> {
    collect_candidates_inner(root, dir, depth, max_depth, out, &[], ignores)
}

pub(super) fn collect_candidates_inner(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<Candidate>,
    inherited_excludes: &[PathBuf],
    ignores: &DiscoveryIgnores,
) -> Result<()> {
    if depth > 0 && should_skip_path_under_root(root, dir, ignores) {
        return Ok(());
    }
    if inherited_excludes
        .iter()
        .any(|excluded| dir == excluded.as_path() || dir.starts_with(excluded))
    {
        return Ok(());
    }
    // Skip re-analyzing a dir already captured (e.g. as an explicit workspace
    // member by the parent call) — but still recurse into its children below so
    // nested sub-projects are found. Avoids running analyze_dir +
    // scan_source_imports twice on every workspace member. Safe by construction:
    // a path-form mismatch merely falls back to re-analyzing, never to pruning.
    if !out.iter().any(|c| c.path == dir) {
        if let Some(candidate) = analyze_dir(root, dir)? {
            out.push(candidate);
        }
    }
    let workspace_dirs = explicit_workspace_dirs(dir)?;
    for member in &workspace_dirs.members {
        if should_skip_path_under_root(root, member, ignores) {
            continue;
        }
        if let Some(candidate) = analyze_dir(root, member)? {
            out.push(candidate);
        } else if workspace_dirs.gradle_members.contains(member) {
            if let Some(candidate) = analyze_gradle_module(root, dir, member) {
                out.push(candidate);
            }
        }
    }
    if depth >= max_depth {
        return Ok(());
    }
    let mut excludes = inherited_excludes.to_vec();
    excludes.extend(workspace_dirs.excludes.iter().cloned());
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("skip unreadable discover dir {}: {e:#}", dir.display());
            return Ok(());
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if should_skip_path_under_root(root, &path, ignores) {
            continue;
        }
        if workspace_dirs
            .excludes
            .iter()
            .any(|excluded| path.as_path() == excluded.as_path() || path.starts_with(excluded))
        {
            continue;
        }
        if path.is_dir() {
            collect_candidates_inner(root, &path, depth + 1, max_depth, out, &excludes, ignores)?;
        }
    }
    Ok(())
}

pub(super) fn explicit_workspace_dirs(dir: &Path) -> Result<WorkspaceDirs> {
    let mut patterns = Vec::<String>::new();
    let package_json = dir.join("package.json");
    if package_json.is_file() {
        patterns.extend(package_json_workspace_patterns(&package_json)?);
    }
    let pnpm_workspace = dir.join("pnpm-workspace.yaml");
    if pnpm_workspace.is_file() {
        patterns.extend(pnpm_workspace_patterns(&pnpm_workspace)?);
    }
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let text = std::fs::read_to_string(&cargo_toml)
            .with_context(|| format!("read {}", cargo_toml.display()))?;
        patterns.extend(parse_cargo_workspace_members(&text));
        patterns.extend(
            parse_cargo_workspace_excludes(&text)
                .into_iter()
                .map(|pattern| format!("!{pattern}")),
        );
    }

    let (mut members, mut excludes) = expand_workspace_patterns_with_excludes(dir, &patterns);
    let mut gradle_members = parse_gradle_includes(dir)?;
    members.extend(gradle_members.iter().cloned());
    members.sort();
    members.dedup();
    gradle_members.sort();
    gradle_members.dedup();
    excludes.sort();
    excludes.dedup();
    Ok(WorkspaceDirs {
        members,
        gradle_members,
        excludes,
    })
}

pub(super) fn package_json_workspace_patterns(path: &Path) -> Result<Vec<String>> {
    let value: Value = read_json(path)?;
    Ok(workspace_patterns_from_json(&value))
}

pub(super) fn workspace_patterns_from_json(value: &Value) -> Vec<String> {
    let Some(workspaces) = value.get("workspaces") else {
        return Vec::new();
    };
    if let Some(items) = workspaces.as_array() {
        return items
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect();
    }
    workspaces
        .get("packages")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn is_package_workspace_only(path: &Path) -> Result<bool> {
    let value: Value = read_json(path)?;
    Ok(value.get("workspaces").is_some()
        && value.get("name").is_none()
        && value.get("scripts").is_none()
        && value.get("dependencies").is_none()
        && value.get("devDependencies").is_none())
}

pub(super) fn pnpm_workspace_patterns(path: &Path) -> Result<Vec<String>> {
    let value: serde_yaml::Value = read_yaml(path)?;
    Ok(yaml_string_array(value.get("packages")))
}

pub(super) fn parse_cargo_workspace_members(text: &str) -> Vec<String> {
    parse_toml_workspace_string_array(text, "members")
}

pub(super) fn parse_cargo_workspace_excludes(text: &str) -> Vec<String> {
    parse_toml_workspace_string_array(text, "exclude")
}

pub(super) fn is_cargo_workspace_only(text: &str) -> bool {
    let Some(value) = parse_toml_document(text) else {
        return false;
    };
    value.get("workspace").is_some() && value.get("package").is_none()
}

pub(super) fn parse_gradle_includes(dir: &Path) -> Result<Vec<PathBuf>> {
    let path = ["settings.gradle", "settings.gradle.kts"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file());
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(parse_gradle_settings_paths(dir, &text))
}

pub(super) fn expand_workspace_patterns_with_excludes(
    base: &Path,
    patterns: &[String],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut out = Vec::new();
    let mut excludes = Vec::new();
    for pattern in patterns {
        let pattern = strip_yaml_string(pattern.trim());
        if pattern.is_empty() {
            continue;
        }
        if let Some(exclude) = pattern.strip_prefix('!') {
            expand_workspace_pattern(base, strip_yaml_string(exclude), &mut excludes);
        } else {
            expand_workspace_pattern(base, pattern, &mut out);
        }
    }
    out.retain(|path| !excludes.iter().any(|excluded| path == excluded));
    out.sort();
    out.dedup();
    (out, excludes)
}

pub(super) fn expand_workspace_pattern(base: &Path, pattern: &str, out: &mut Vec<PathBuf>) {
    if !pattern.contains('*') {
        let path = base.join(pattern);
        if path.is_dir() {
            out.push(path);
        }
        return;
    }
    let parts = pattern
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    expand_workspace_parts(base, &parts, out, 0);
}

pub(super) fn expand_workspace_parts(
    current: &Path,
    parts: &[&str],
    out: &mut Vec<PathBuf>,
    depth: usize,
) {
    if parts.is_empty() {
        if current.is_dir() {
            out.push(current.to_path_buf());
        }
        return;
    }
    if depth > 12 {
        return;
    }
    let part = parts[0];
    if part == "**" {
        expand_workspace_parts(current, &parts[1..], out, depth + 1);
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !should_skip_dir(&path) {
                expand_workspace_parts(&path, parts, out, depth + 1);
            }
        }
        return;
    }
    if part.contains('*') {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || should_skip_dir(&path) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if wildcard_segment_matches(part, &name) {
                expand_workspace_parts(&path, &parts[1..], out, depth + 1);
            }
        }
    } else {
        expand_workspace_parts(&current.join(part), &parts[1..], out, depth + 1);
    }
}

pub(super) fn wildcard_segment_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let chunks = pattern.split('*').collect::<Vec<_>>();
    let mut rest = value;
    if let Some(first) = chunks.first().filter(|first| !first.is_empty()) {
        if !rest.starts_with(first) {
            return false;
        }
        rest = &rest[first.len()..];
    }
    for chunk in chunks
        .iter()
        .skip(1)
        .take(chunks.len().saturating_sub(2))
        .filter(|chunk| !chunk.is_empty())
    {
        let Some(idx) = rest.find(chunk) else {
            return false;
        };
        rest = &rest[idx + chunk.len()..];
    }
    if let Some(last) = chunks.last().filter(|last| !last.is_empty()) {
        return rest.ends_with(last);
    }
    true
}
