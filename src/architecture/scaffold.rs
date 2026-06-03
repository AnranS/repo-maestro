//! Actually create the workspace: mkdir, git init, write template files,
//! drop contract placeholders, and produce a populated `projects.yaml`.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{self, Contracts, Project, ProjectsConfig};
use crate::paths;

use super::templates::{contract_placeholder, scaffold_files};
use super::Architecture;

#[derive(Debug, Serialize)]
pub struct ScaffoldReport {
    pub root: String,
    pub modules: Vec<ScaffoldedModule>,
    pub contracts: usize,
    pub projects_yaml_path: String,
}

#[derive(Debug, Serialize)]
pub struct ScaffoldedModule {
    pub name: String,
    pub path: String,
    pub created_files: Vec<String>,
    /// `true` if we did `git init` (false if directory already had a `.git/`).
    pub git_initialized: bool,
    /// `true` if the directory pre-existed; we touch only files we created and
    /// never overwrite.
    pub pre_existing: bool,
}

pub struct ScaffoldOptions {
    /// Workspace root. Each module is created as a subdirectory.
    pub root: PathBuf,
    /// If true, skip running `git init` (useful for nested workspaces or tests).
    pub no_git: bool,
    /// If true, don't write or merge into projects.yaml (dry-run style).
    pub no_register: bool,
}

impl ScaffoldOptions {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            no_git: false,
            no_register: false,
        }
    }
}

pub fn scaffold(arch: &Architecture, opts: &ScaffoldOptions) -> Result<ScaffoldReport> {
    arch.validate()?;
    paths::ensure_dir(&opts.root)?;

    let mut modules_out = vec![];
    for m in &arch.modules {
        let module_path = opts.root.join(&m.name);
        let pre_existing = module_path.exists();
        paths::ensure_dir(&module_path)?;

        let files = scaffold_files(&m.stack, &m.name);
        let mut written = vec![];
        for f in files {
            let target = module_path.join(f.rel_path);
            if let Some(parent) = target.parent() {
                paths::ensure_dir(parent)?;
            }
            if !target.exists() {
                std::fs::write(&target, f.content)
                    .with_context(|| format!("write {:?}", target))?;
                written.push(f.rel_path.to_string());
            }
        }

        // Contract placeholder — for the file this module `provides`.
        if let Some(rel) = &m.provides {
            let target = module_path.join(rel);
            if !target.exists() {
                if let Some(parent) = target.parent() {
                    paths::ensure_dir(parent)?;
                }
                if let Some(content) = contract_placeholder(Path::new(rel)) {
                    std::fs::write(&target, content)
                        .with_context(|| format!("write contract {:?}", target))?;
                    written.push(rel.clone());
                }
            }
        }

        // git init (skip if .git already there)
        let mut git_initialized = false;
        if !opts.no_git && !module_path.join(".git").exists() {
            let status = std::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .current_dir(&module_path)
                .status();
            if matches!(status, Ok(s) if s.success()) {
                git_initialized = true;
            }
        }

        modules_out.push(ScaffoldedModule {
            name: m.name.clone(),
            path: module_path.to_string_lossy().to_string(),
            created_files: written,
            git_initialized,
            pre_existing,
        });
    }

    // Build / merge projects.yaml.
    let mut projects_yaml_path = String::new();
    if !opts.no_register {
        // We register relative to the workspace root, not opts.root, since
        // `maestro ui` and friends look there.
        let workspace_root = paths::workspace_root()?;
        let pfile = workspace_root
            .join(crate::paths::MAESTRO_DIR)
            .join(crate::paths::PROJECTS_FILE);
        // `pfile` is always `<workspace_root>/.maestro/projects.yaml`, so the
        // parent is guaranteed to exist as a path component — but unwrap()
        // here is a footgun for anyone changing PROJECTS_FILE later. Use the
        // `?` path: an honest error if PROJECTS_FILE is ever rooted.
        let parent = pfile
            .parent()
            .with_context(|| format!("projects file path has no parent: {}", pfile.display()))?;
        paths::ensure_dir(parent)?;

        let mut cfg = if pfile.exists() {
            ProjectsConfig::load(&pfile)?
        } else {
            ProjectsConfig {
                version: 1,
                defaults: Default::default(),
                projects: Default::default(),
            }
        };

        for m in &arch.modules {
            if cfg.projects.contains_key(&m.name) {
                continue;
            }
            let path = opts.root.join(&m.name).to_string_lossy().to_string();
            cfg.projects.insert(
                m.name.clone(),
                Project {
                    path,
                    r#type: m.r#type.clone(),
                    stack: m.stack.clone(),
                    commands: BTreeMap::new(),
                    contracts: contracts_for(arch, &m.name),
                    dependencies: Vec::new(),
                    memory_scope: m.memory_scope.clone(),
                    agent: None,
                    agent_model: None,
                    cursor_model: None,
                    model_profile: None,
                    role: default_role_for_type(m.r#type.as_deref()),
                    copy_files: Vec::new(),
                },
            );
        }

        cfg.save(&pfile)?;
        projects_yaml_path = pfile.to_string_lossy().to_string();

        // Make sure `.maestro/runs`, approvals, cancels etc. exist (mirrors `maestro init`).
        paths::ensure_dir(&crate::paths::runs_dir()?)?;
        paths::ensure_dir(&crate::paths::approvals_dir()?)?;
        paths::ensure_dir(&crate::paths::cancels_dir()?)?;
    }

    Ok(ScaffoldReport {
        root: opts.root.to_string_lossy().to_string(),
        contracts: arch.contracts.len(),
        modules: modules_out,
        projects_yaml_path,
    })
}

/// Map a project's declared `type` (backend/frontend/mobile/game/...) to a
/// sensible default builder role. Returns `None` when the type doesn't
/// have a single obvious owner — e.g. unknown types or `tool`/`library`.
/// The user can override per project in `projects.yaml`.
fn default_role_for_type(t: Option<&str>) -> Option<String> {
    match t.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("backend") => Some("backend_rust".into()),
        Some("frontend") => Some("frontend".into()),
        // type=mobile is ambiguous (iOS vs Android vs RN). Default to
        // iOS — user can override per-project with `role: mobile_android`
        // or `role: mobile_rn` in projects.yaml. Granular types like
        // `ios` / `android` resolve unambiguously.
        Some("mobile") | Some("mobile_ios") | Some("ios") => Some("mobile_ios".into()),
        Some("mobile_android") | Some("android") => Some("mobile_android".into()),
        Some("mobile_rn") | Some("rn") | Some("react-native") => Some("mobile_rn".into()),
        Some("game") => Some("game_cocos".into()),
        _ => None,
    }
}

fn contracts_for(arch: &Architecture, module_name: &str) -> Contracts {
    let mut provides = arch
        .modules
        .iter()
        .find(|m| m.name == module_name)
        .and_then(|m| m.provides.clone());
    let mut consumes = arch
        .modules
        .iter()
        .find(|m| m.name == module_name)
        .and_then(|m| m.consumes.clone());

    // Cross-reference the explicit contracts list as a fallback so the agent
    // can authority-declare contracts there even if the module entries don't.
    for c in &arch.contracts {
        if c.from == module_name && provides.is_none() {
            provides = Some(c.file.clone());
        }
        if c.to.iter().any(|t| t == module_name) && consumes.is_none() {
            consumes = Some(c.file.clone());
        }
    }

    Contracts { provides, consumes }
}

#[allow(dead_code)]
fn _types_inhabited() -> Option<config::ProjectsConfig> {
    None
}
