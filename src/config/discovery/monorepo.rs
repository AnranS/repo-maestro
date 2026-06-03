use super::*;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// A Bazel/Go monorepo's module registry (`.monorepo_config.yaml`). Each entry
/// is an independently-deployable module living under one root `go.mod`.
#[derive(serde::Deserialize)]
pub(super) struct MonorepoConfig {
    #[serde(default)]
    modules: BTreeMap<String, MonorepoModule>,
}

#[derive(serde::Deserialize)]
pub(super) struct MonorepoModule {
    #[serde(rename = "modulePath", default)]
    module_path: String,
    #[serde(rename = "moduleName", default)]
    module_name: String,
    #[serde(default)]
    language: String,
    #[serde(rename = "serviceType", default)]
    service_type: String,
    #[serde(rename = "isLibrary", default)]
    is_library: bool,
    #[serde(default)]
    psm: String,
    #[serde(rename = "directoriesWithTest", default)]
    directories_with_test: Vec<String>,
}

/// Strip `//` line and `/* */` block comments from JSONC (e.g. rush.json),
/// leaving string contents (and any `//` inside them) untouched.
pub(super) fn strip_jsonc(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let (mut in_str, mut esc) = (false, false);
    while i < b.len() {
        let c = b[i] as char;
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < b.len() {
            match b[i + 1] as char {
                '/' => {
                    i += 2;
                    while i < b.len() && b[i] as char != '\n' {
                        i += 1;
                    }
                    continue;
                }
                '*' => {
                    i += 2;
                    while i + 1 < b.len() && !(b[i] as char == '*' && b[i + 1] as char == '/') {
                        i += 1;
                    }
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[derive(serde::Deserialize)]
pub(super) struct RushConfig {
    #[serde(default)]
    projects: Vec<RushProject>,
}

#[derive(serde::Deserialize)]
pub(super) struct RushProject {
    #[serde(rename = "packageName", default)]
    package_name: String,
    #[serde(rename = "projectFolder", default)]
    project_folder: String,
    #[serde(rename = "subspaceName", default)]
    subspace_name: String,
}

/// If `root` holds a `rush.json`, build the project list from its registry.
/// On a parse error returns `None` so discovery falls back to the (still
/// useful) recursive package.json scan.
pub(super) fn discover_from_rush(root: &Path) -> Result<Option<DiscoveryReport>> {
    let cfg_path = root.join("rush.json");
    if !cfg_path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("read {}", cfg_path.display()))?;
    let cfg: RushConfig = match serde_json::from_str(&strip_jsonc(&raw)) {
        Ok(c) => c,
        Err(_) => return Ok(None), // fall back to the package.json scan
    };
    if cfg.projects.is_empty() {
        return Ok(None);
    }

    let mut projects = BTreeMap::new();
    for p in &cfg.projects {
        if p.project_folder.is_empty() {
            continue;
        }
        let base = if p.package_name.is_empty() {
            Path::new(&p.project_folder)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| p.project_folder.clone())
        } else {
            p.package_name.clone()
        };
        let name = unique_name(&projects, &sanitize_project_name(&base));

        // Refine type from the folder: libs/sdk → library, cli/tool → tool,
        // else a web monorepo project is frontend.
        let folder = p.project_folder.to_ascii_lowercase();
        let project_type = if folder.contains("/lib") || folder.contains("sdk") {
            "library"
        } else if folder.contains("cli") || folder.contains("/tool") {
            "tool"
        } else {
            "frontend"
        };
        let mut stack = vec!["node".to_string()];
        if !p.subspace_name.is_empty() {
            stack.push(format!("subspace:{}", p.subspace_name));
        }
        let mut evidence = vec!["monorepo:rush.json".to_string()];
        if !p.subspace_name.is_empty() {
            evidence.push(format!("subspace:{}", p.subspace_name));
        }

        // Self-check: build the package (and its deps) via Rush. Rush repos
        // don't install a global `rush` — they run the bundled bootstrap
        // (`common/scripts/install-run-rush.js`). Prefer it when present
        // (located via the git root so it works from any task CWD), so the
        // verify actually runs instead of failing with "rush: command not
        // found".
        let mut commands = BTreeMap::new();
        if !p.package_name.is_empty() {
            let check = if root.join("common/scripts/install-run-rush.js").is_file() {
                format!(
                    "node \"$(git rev-parse --show-toplevel)/common/scripts/install-run-rush.js\" build --to {}",
                    p.package_name
                )
            } else {
                format!("rush build --to {}", p.package_name)
            };
            commands.insert("check".to_string(), check);
        }

        projects.insert(
            name.clone(),
            DiscoveredProject {
                name,
                path: root.join(&p.project_folder),
                relative_path: p.project_folder.clone(),
                confidence: 95,
                project_type: Some(project_type.to_string()),
                stack,
                package_name: (!p.package_name.is_empty()).then(|| p.package_name.clone()),
                commands,
                provides: None,
                consumes: None,
                dependencies: Vec::new(),
                markers: vec!["monorepo-module".to_string()],
                evidence,
            },
        );
    }
    if projects.is_empty() {
        return Ok(None);
    }

    Ok(Some(DiscoveryReport {
        root: root.to_path_buf(),
        projects,
        edges: Vec::new(),
        warnings: Vec::new(),
    }))
}

/// If `root` holds a `.monorepo_config.yaml`, build the project list straight
/// from its module registry (authoritative) rather than the manifest heuristic.
/// Returns `None` when there's no registry, so discovery falls back normally.
pub(super) fn discover_from_monorepo_config(root: &Path) -> Result<Option<DiscoveryReport>> {
    let cfg_path = root.join(".monorepo_config.yaml");
    if !cfg_path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("read {}", cfg_path.display()))?;
    let cfg: MonorepoConfig = match serde_yaml::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            // Malformed registry — degrade gracefully: warn and fall back to
            // the normal manifest scan rather than dead-ending at 0 projects.
            tracing::warn!(
                ".monorepo_config.yaml present but unparseable, falling back to scan: {e}"
            );
            return Ok(None);
        }
    };
    // An empty/edge-case registry also falls back rather than yielding nothing.
    if cfg.modules.is_empty() {
        return Ok(None);
    }

    let mut projects = BTreeMap::new();
    for (key, m) in &cfg.modules {
        let rel = if m.module_path.is_empty() {
            key
        } else {
            &m.module_path
        };
        let base = if m.module_name.is_empty() {
            Path::new(rel)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| rel.clone())
        } else {
            m.module_name.clone()
        };
        let name = unique_name(&projects, &sanitize_project_name(&base));

        let project_type = if m.is_library || m.service_type == "library" {
            "library"
        } else {
            "backend"
        };
        let mut stack = Vec::new();
        if !m.language.is_empty() {
            stack.push(m.language.clone());
        }
        if !m.service_type.is_empty() {
            stack.push(m.service_type.clone());
        }
        let mut evidence = vec!["monorepo:.monorepo_config.yaml".to_string()];
        if !m.psm.is_empty() {
            evidence.push(format!("psm:{}", m.psm));
        }

        // Self-check command (so maestro can verify its own work). Bazel:
        // `test` when the registry lists test dirs, else `build` — driven by
        // the registry, not guessed.
        let mut commands = BTreeMap::new();
        let check = if m.directories_with_test.is_empty() {
            format!("bazel build //{rel}/...")
        } else {
            format!("bazel test //{rel}/...")
        };
        commands.insert("check".to_string(), check);

        projects.insert(
            name.clone(),
            DiscoveredProject {
                name,
                path: root.join(rel),
                relative_path: rel.clone(),
                confidence: 95,
                project_type: Some(project_type.to_string()),
                stack,
                package_name: None,
                commands,
                provides: None,
                consumes: None,
                dependencies: Vec::new(),
                markers: vec!["monorepo-module".to_string()],
                evidence,
            },
        );
    }

    Ok(Some(DiscoveryReport {
        root: root.to_path_buf(),
        projects,
        edges: Vec::new(),
        warnings: Vec::new(),
    }))
}

/// Combined multi-repo workspace: shallow subdirs of `root` are themselves
/// registered monorepos (have `.monorepo_config.yaml` or `rush.json`). Recurse
/// into each and merge, prefixing relative paths by the subdir name. Returns
/// `None` when no such subdirs are found.
pub(super) fn discover_subrepo_workspace(
    root: &Path,
    opts: &DiscoverOptions,
) -> Result<Option<DiscoveryReport>> {
    let mut subs: Vec<(String, DiscoveryReport)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "node_modules" | "vendor" | "target" | "dist" | "bazel-out"
            )
        {
            continue;
        }
        let has_monorepo =
            path.join(".monorepo_config.yaml").is_file() || path.join("rush.json").is_file();
        if !has_monorepo {
            continue;
        }
        let report = discover(&path, opts)?;
        if !report.projects.is_empty() {
            subs.push((name, report));
        }
    }
    if subs.is_empty() {
        return Ok(None);
    }
    let mut merged = DiscoveryReport {
        root: root.to_path_buf(),
        projects: BTreeMap::new(),
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    for (prefix, sub) in subs {
        merged.warnings.extend(sub.warnings);
        for (name, mut project) in sub.projects {
            project.relative_path = if project.relative_path.is_empty() {
                prefix.clone()
            } else {
                format!("{prefix}/{}", project.relative_path)
            };
            // Re-anchor the project's path inside the combined workspace via
            // the symlinked subdir (`<root>/backend/app/x`) instead of leaving
            // the canonicalized target (`/private/tmp/acme/app/x`) which would
            // serialize as `../acme/...` — ugly and abstraction-breaking.
            project.path = root.join(&project.relative_path);
            project
                .evidence
                .insert(0, format!("multi-repo subdir:{prefix}"));
            merged.projects.insert(name, project);
        }
    }
    Ok(Some(merged))
}
