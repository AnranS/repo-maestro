//! Hierarchical repository instructions loaded from `AGENTS.md`.
//!
//! This is separate from maestro memory: memory is topical knowledge, while
//! `AGENTS.md` is the repo-local operating manual agents should obey.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::config::ProjectsConfig;
use crate::paths;

const AGENTS_FILE: &str = "AGENTS.md";
const MAX_INSTRUCTION_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectInstruction {
    pub source: String,
    pub content: String,
}

pub fn load_for_task(projects: &ProjectsConfig, project_name: &str) -> Vec<ProjectInstruction> {
    match try_load_for_task(projects, project_name) {
        Ok(instructions) => instructions,
        Err(e) => {
            tracing::warn!("AGENTS.md lookup failed for project {project_name:?}: {e:#}");
            Vec::new()
        }
    }
}

pub fn try_load_for_task(
    projects: &ProjectsConfig,
    project_name: &str,
) -> Result<Vec<ProjectInstruction>> {
    let workspace_root = paths::workspace_root()?;
    let project_path = if project_name == "_global" {
        workspace_root.clone()
    } else {
        projects.resolved_path(project_name)?
    };

    let mut out = Vec::new();
    for path in instruction_paths(&workspace_root, &project_path) {
        if !path.is_file() {
            continue;
        }
        if let Some(instruction) = read_instruction(&workspace_root, &path)? {
            out.push(instruction);
        }
    }
    Ok(out)
}

pub fn render_section(instructions: &[ProjectInstruction]) -> Option<String> {
    if instructions.is_empty() {
        return None;
    }

    let mut out = String::from("# Repository instructions (from AGENTS.md)\n\n");
    for instruction in instructions {
        out.push_str("## ");
        out.push_str(&instruction.source);
        out.push_str("\n\n");
        out.push_str(instruction.content.trim_end());
        out.push_str("\n\n");
    }
    Some(out.trim_end().to_string())
}

fn instruction_paths(workspace_root: &Path, project_path: &Path) -> Vec<PathBuf> {
    let root = normalize_path(workspace_root);
    let project = normalize_path(project_path);
    let mut dirs = Vec::new();
    dirs.push(root.clone());

    if project.starts_with(&root) {
        let mut cursor = root.clone();
        if let Ok(relative) = project.strip_prefix(&root) {
            for component in relative.components() {
                if let Component::Normal(part) = component {
                    cursor.push(part);
                    dirs.push(cursor.clone());
                }
            }
        }
    } else if project != root {
        dirs.push(project);
    }

    let mut seen = BTreeSet::new();
    dirs.into_iter()
        .filter(|dir| seen.insert(dir.clone()))
        .map(|dir| dir.join(AGENTS_FILE))
        .collect()
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn read_instruction(workspace_root: &Path, path: &Path) -> Result<Option<ProjectInstruction>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {:?}", path))?;
    if bytes.is_empty() {
        return Ok(None);
    }

    let truncated = bytes.len() > MAX_INSTRUCTION_BYTES;
    let slice = if truncated {
        &bytes[..MAX_INSTRUCTION_BYTES]
    } else {
        &bytes
    };
    let mut content = String::from_utf8_lossy(slice).trim().to_string();
    if content.is_empty() {
        return Ok(None);
    }
    if truncated {
        content.push_str("\n\n[truncated by maestro: AGENTS.md exceeded 32 KiB]");
    }

    Ok(Some(ProjectInstruction {
        source: display_source(workspace_root, path),
        content,
    }))
}

fn display_source(workspace_root: &Path, path: &Path) -> String {
    let root = normalize_path(workspace_root);
    let path = normalize_path(path);
    if let Ok(relative) = path.strip_prefix(&root) {
        return relative.display().to_string();
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_paths_walk_from_root_to_project() {
        let root = PathBuf::from("/tmp/ws");
        let project = root.join("apps").join("api");
        let paths = instruction_paths(&root, &project);
        assert_eq!(
            paths,
            vec![
                root.join("AGENTS.md"),
                root.join("apps").join("AGENTS.md"),
                root.join("apps").join("api").join("AGENTS.md"),
            ]
        );
    }

    #[test]
    fn render_section_lists_sources_in_order() {
        let rendered = render_section(&[
            ProjectInstruction {
                source: "AGENTS.md".into(),
                content: "Root rule".into(),
            },
            ProjectInstruction {
                source: "api/AGENTS.md".into(),
                content: "API rule".into(),
            },
        ])
        .unwrap();

        assert!(rendered.contains("# Repository instructions"));
        assert!(rendered.find("Root rule").unwrap() < rendered.find("API rule").unwrap());
    }
}
