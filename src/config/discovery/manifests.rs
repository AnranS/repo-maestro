use super::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) fn analyze_gradle_module(
    root: &Path,
    workspace_root: &Path,
    dir: &Path,
) -> Option<Candidate> {
    if !dir.is_dir() {
        return None;
    }
    // TODO(iOS): add Podfile / xcworkspace discovery in a separate iOS-specific pass.
    let mut stack = BTreeSet::new();
    stack.insert("gradle".to_string());
    stack.insert("gradle-multi".to_string());
    let mut evidence = BTreeSet::new();
    evidence.insert(format!(
        "gradle-include:{}",
        display_path(relative_to(workspace_root, dir))
    ));
    Some(Candidate {
        path: dir.to_path_buf(),
        markers: vec!["gradle-module".to_string()],
        package_name: dir.file_name().map(|s| s.to_string_lossy().to_string()),
        project_type: Some(infer_type_from_path(root, dir)),
        stack,
        commands: BTreeMap::new(),
        local_dep_paths: Vec::new(),
        package_deps: Vec::new(),
        workspace_package_deps: Vec::new(),
        source_dep_targets: Vec::new(),
        provides: detect_contract(dir),
        evidence,
    })
}

pub(super) fn analyze_dir(root: &Path, dir: &Path) -> Result<Option<Candidate>> {
    let mut markers = Vec::new();
    let mut analysis = DirAnalysis::default();
    let mut has_project_marker = false;

    let package_json = dir.join("package.json");
    if package_json.is_file() {
        if is_package_workspace_only(&package_json)? {
            analysis
                .evidence
                .insert("workspace:package.json".to_string());
        } else {
            has_project_marker = true;
            markers.push("package.json".to_string());
            analysis.evidence.insert("marker:package.json".to_string());
        }
        analyze_package_json(dir, &package_json, &mut analysis)?;
    }

    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let text = std::fs::read_to_string(&cargo_toml)
            .with_context(|| format!("read {}", cargo_toml.display()))?;
        if is_cargo_workspace_only(&text) {
            analysis.evidence.insert("workspace:Cargo.toml".to_string());
        } else {
            has_project_marker = true;
            markers.push("Cargo.toml".to_string());
            analysis.evidence.insert("marker:Cargo.toml".to_string());
        }
        analyze_cargo_toml_text(dir, &text, &mut analysis)?;
    }

    let pyproject = dir.join("pyproject.toml");
    let requirements = dir.join("requirements.txt");
    if pyproject.is_file() || requirements.is_file() {
        has_project_marker = true;
        if pyproject.is_file() {
            markers.push("pyproject.toml".to_string());
            analysis
                .evidence
                .insert("marker:pyproject.toml".to_string());
        }
        if requirements.is_file() {
            markers.push("requirements.txt".to_string());
            analysis
                .evidence
                .insert("marker:requirements.txt".to_string());
        }
        analyze_python(
            dir,
            &mut analysis.project_type,
            &mut analysis.stack,
            &mut analysis.commands,
            &mut analysis.evidence,
        )?;
    }

    let go_mod = dir.join("go.mod");
    if go_mod.is_file() {
        has_project_marker = true;
        markers.push("go.mod".to_string());
        analysis.evidence.insert("marker:go.mod".to_string());
        analyze_go_mod(
            &go_mod,
            &mut analysis.package_name,
            &mut analysis.project_type,
            &mut analysis.stack,
            &mut analysis.commands,
            &mut analysis.evidence,
        )?;
    }

    let provides = detect_contract(dir);
    if let Some(contract) = &provides {
        markers.push(format!("contract:{contract}"));
        analysis.evidence.insert(format!("contract:{contract}"));
    }

    if !has_project_marker && provides.is_some() && !is_contract_container_dir(dir) {
        markers.push("contract-only".to_string());
        analysis.evidence.insert("marker:contract-only".to_string());
        analysis.stack.insert("contract".to_string());
        analysis
            .project_type
            .get_or_insert_with(|| "library".to_string());
    }

    if markers.is_empty() || (!has_project_marker && is_contract_container_dir(dir)) {
        return Ok(None);
    }

    if analysis.stack.is_empty() {
        analysis.stack.insert("unknown".to_string());
    }
    if analysis.project_type.is_none() {
        analysis.project_type = Some(infer_type_from_path(root, dir));
    }

    // Infer cross-project edges from source imports (relative specifiers that
    // resolve outside this dir) — catches monorepo packages that import each
    // other without declaring it in a manifest.
    scan_source_imports(dir, &mut analysis.source_dep_targets);

    analysis.local_dep_paths.sort();
    analysis.local_dep_paths.dedup();
    analysis.package_deps.sort();
    analysis.package_deps.dedup();
    analysis.workspace_package_deps.sort();
    analysis.workspace_package_deps.dedup();
    analysis.source_dep_targets.sort();
    analysis.source_dep_targets.dedup();

    Ok(Some(Candidate {
        path: dir.to_path_buf(),
        markers,
        package_name: analysis.package_name,
        project_type: analysis.project_type,
        stack: analysis.stack,
        commands: analysis.commands,
        local_dep_paths: analysis.local_dep_paths,
        package_deps: analysis.package_deps,
        workspace_package_deps: analysis.workspace_package_deps,
        source_dep_targets: analysis.source_dep_targets,
        provides,
        evidence: analysis.evidence,
    }))
}

pub(super) fn analyze_package_json(
    dir: &Path,
    path: &Path,
    analysis: &mut DirAnalysis,
) -> Result<()> {
    let value: Value = read_json(path)?;

    analysis.stack.insert("node".to_string());
    if dir.join("pnpm-lock.yaml").is_file()
        || find_upwards(dir, "pnpm-lock.yaml").is_some()
        || find_upwards(dir, "pnpm-workspace.yaml").is_some()
    {
        analysis.stack.insert("pnpm".to_string());
    } else if dir.join("yarn.lock").is_file() || find_upwards(dir, "yarn.lock").is_some() {
        analysis.stack.insert("yarn".to_string());
    } else {
        analysis.stack.insert("npm".to_string());
    }

    if analysis.package_name.is_none() {
        analysis.package_name = value
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    if let Some(name) = analysis.package_name.as_deref() {
        analysis.evidence.insert(format!("package:name={name}"));
    }

    if let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) {
        let runner = if analysis.stack.contains("pnpm") {
            "pnpm"
        } else if analysis.stack.contains("yarn") {
            "yarn"
        } else {
            "npm run"
        };
        // `check` is included because the planner's verification step prefers a
        // `check` command (see plan::verification_command); omitting it here left
        // projects with only a `check` script falling back to a placeholder
        // acceptance that always fails.
        for key in ["test", "check", "lint", "build"] {
            if scripts.get(key).and_then(|v| v.as_str()).is_some() {
                analysis.evidence.insert(format!("script:{key}"));
                let command = if runner == "npm run" {
                    format!("npm run {key}")
                } else {
                    format!("{runner} {key}")
                };
                analysis.commands.entry(key.to_string()).or_insert(command);
            }
        }
    }

    let mut dep_names = BTreeSet::<String>::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = value.get(section).and_then(|v| v.as_object()) {
            for (name, version) in deps {
                dep_names.insert(name.clone());
                if let Some(spec) = version.as_str() {
                    analysis
                        .evidence
                        .insert(format!("dependency:{name}={spec}"));
                    if spec.starts_with("workspace:") {
                        analysis.workspace_package_deps.push(name.clone());
                        analysis
                            .evidence
                            .insert(format!("workspace-dependency:{name}={spec}"));
                    }
                    if let Some(path) = spec.strip_prefix("file:") {
                        let path = PathBuf::from(path);
                        analysis.evidence.insert(format!(
                            "local-dependency:{}",
                            display_path(Some(path.clone()))
                        ));
                        analysis.local_dep_paths.push(path);
                    }
                } else {
                    analysis.evidence.insert(format!("dependency:{name}"));
                }
            }
        }
    }
    analysis.package_deps.extend(dep_names);

    // Classify by dependency NAMES (exact match or known scoped prefix), not by
    // substring-scanning the whole serialized manifest — a description that
    // mentions "react to events", or a dep like `preact`/`unreactive`, must not
    // flip the type.
    let is_frontend = analysis.package_deps.iter().any(|d| {
        matches!(
            d.as_str(),
            "react" | "react-dom" | "next" | "vue" | "nuxt" | "svelte"
        ) || d.starts_with("@vue/")
            || d.starts_with("@angular/")
    });
    let is_backend = analysis
        .package_deps
        .iter()
        .any(|d| matches!(d.as_str(), "express" | "fastify" | "koa") || d.starts_with("@nestjs/"));
    if is_frontend {
        analysis.project_type = Some("frontend".to_string());
    } else if is_backend {
        analysis.project_type = Some("backend".to_string());
    } else if dir.join("bin").is_dir() || value.get("bin").is_some() {
        analysis.project_type = Some("tool".to_string());
    } else if dir
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("mobile")
    {
        analysis.project_type = Some("mobile".to_string());
    } else {
        // No framework signal in deps — fall back to the dir name (its own
        // basename + parent, so we don't match the absolute path prefix), e.g.
        // `services/api` → backend, `apps/web` → frontend, instead of a flat
        // "library".
        let seg = |p: &Path| {
            p.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        };
        let hay = format!("{}/{}", dir.parent().map(seg).unwrap_or_default(), seg(dir));
        let inferred = if hay.contains("web") || hay.contains("front") || hay.contains("ui") {
            "frontend"
        } else if hay.contains("api") || hay.contains("server") || hay.contains("service") {
            "backend"
        } else if hay.contains("cli") || hay.contains("tool") {
            "tool"
        } else {
            "library"
        };
        analysis
            .project_type
            .get_or_insert_with(|| inferred.to_string());
    }
    Ok(())
}

pub(super) fn analyze_cargo_toml_text(
    dir: &Path,
    text: &str,
    analysis: &mut DirAnalysis,
) -> Result<()> {
    analysis.stack.insert("rust".to_string());
    analysis.package_name.get_or_insert_with(|| {
        parse_toml_package_name(text)
            .or_else(|| dir.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "rust-project".to_string())
    });
    if let Some(name) = analysis.package_name.as_deref() {
        analysis.evidence.insert(format!("package:name={name}"));
    }
    analysis
        .commands
        .entry("test".to_string())
        .or_insert_with(|| "cargo test".to_string());
    analysis.evidence.insert("script:test".to_string());
    analysis
        .commands
        .entry("build".to_string())
        .or_insert_with(|| "cargo build".to_string());
    analysis.evidence.insert("script:build".to_string());
    for path_dep in parse_cargo_path_deps(text) {
        analysis.evidence.insert(format!(
            "local-dependency:{}",
            display_path(Some(path_dep.clone()))
        ));
        analysis.local_dep_paths.push(path_dep);
    }
    if dir.join("src").join("main.rs").is_file() {
        analysis.project_type = Some("tool".to_string());
    } else {
        analysis
            .project_type
            .get_or_insert_with(|| "library".to_string());
    }
    Ok(())
}

pub(super) fn analyze_python(
    dir: &Path,
    project_type: &mut Option<String>,
    stack: &mut BTreeSet<String>,
    commands: &mut BTreeMap<String, String>,
    evidence: &mut BTreeSet<String>,
) -> Result<()> {
    stack.insert("python".to_string());
    let mut text = String::new();
    for file in ["pyproject.toml", "requirements.txt"] {
        let p = dir.join(file);
        if p.is_file() {
            text.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("fastapi") || lower.contains("django") || lower.contains("flask") {
        *project_type = Some("backend".to_string());
    } else {
        project_type.get_or_insert_with(|| "library".to_string());
    }
    if dir.join("tests").is_dir() || lower.contains("pytest") {
        commands
            .entry("test".to_string())
            .or_insert_with(|| "pytest -q".to_string());
        evidence.insert("script:test".to_string());
    }
    Ok(())
}

pub(super) fn analyze_go_mod(
    path: &Path,
    package_name: &mut Option<String>,
    project_type: &mut Option<String>,
    stack: &mut BTreeSet<String>,
    commands: &mut BTreeMap<String, String>,
    evidence: &mut BTreeSet<String>,
) -> Result<()> {
    stack.insert("go".to_string());
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if package_name.is_none() {
        *package_name = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("module "))
            .map(str::trim)
            .map(str::to_string);
    }
    if let Some(name) = package_name.as_deref() {
        evidence.insert(format!("package:name={name}"));
    }
    commands
        .entry("test".to_string())
        .or_insert_with(|| "go test ./...".to_string());
    evidence.insert("script:test".to_string());
    project_type.get_or_insert_with(|| "backend".to_string());
    Ok(())
}

pub(super) fn parse_toml_package_name(text: &str) -> Option<String> {
    parse_toml_document(text)?
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

pub(super) fn parse_toml_workspace_string_array(text: &str, key: &str) -> Vec<String> {
    let Some(value) = parse_toml_document(text) else {
        return Vec::new();
    };
    value
        .get("workspace")
        .and_then(|workspace| workspace.get(key))
        .and_then(|items| items.as_array())
        .map(|items| toml_string_array(items))
        .unwrap_or_default()
}

pub(super) fn parse_toml_document(text: &str) -> Option<toml::Value> {
    toml::from_str(text).ok()
}

pub(super) fn toml_string_array(items: &[toml::Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item.as_str())
        .map(str::to_string)
        .collect()
}

pub(super) fn yaml_string_array(value: Option<&serde_yaml::Value>) -> Vec<String> {
    value
        .and_then(|items| items.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_gradle_settings_paths(dir: &Path, text: &str) -> Vec<PathBuf> {
    let project_dirs = parse_gradle_project_dirs(text);
    parse_gradle_includes_text(text)
        .into_iter()
        .map(|module| {
            project_dirs
                .get(&module)
                .map(|path| dir.join(path))
                .unwrap_or_else(|| dir.join(module.trim_start_matches(':').replace(':', "/")))
        })
        .collect()
}

pub(super) fn parse_gradle_includes_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(offset) = text[pos..].find("include") {
        let start = pos + offset + "include".len();
        let rest = &text[start..];
        let end = rest.find('\n').unwrap_or(rest.len());
        let mut statement = rest[..end].to_string();
        if statement.trim_start().starts_with('(') && !statement.contains(')') {
            if let Some(close) = rest.find(')') {
                statement = rest[..=close].to_string();
                pos = start + close + 1;
            } else {
                pos = start + end;
            }
        } else {
            pos = start + end;
        }
        out.extend(
            quoted_gradle_strings(&statement)
                .into_iter()
                .filter(|item| item.starts_with(':')),
        );
    }
    out
}

pub(super) fn quoted_gradle_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '"' && ch != '\'' {
            continue;
        }
        let quote = ch;
        let start = chars.peek().map(|(idx, _)| *idx).unwrap_or(text.len());
        let mut end = start;
        for (idx, next) in chars.by_ref() {
            if next == quote {
                break;
            }
            end = idx + next.len_utf8();
        }
        let item = text[start..end].trim();
        out.push(item.to_string());
    }
    out
}

pub(super) fn parse_gradle_project_dirs(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if !line.starts_with("project(") || !line.contains("projectDir") {
            continue;
        }
        let modules = quoted_gradle_strings(line)
            .into_iter()
            .filter(|item| item.starts_with(':'))
            .collect::<Vec<_>>();
        let Some(module) = modules.first() else {
            continue;
        };
        let Some(file_arg) = line.split("file(").nth(1) else {
            continue;
        };
        let dirs = quoted_gradle_strings(file_arg);
        if let Some(dir) = dirs.first() {
            out.insert(module.clone(), dir.clone());
        }
    }
    out
}

pub(super) fn strip_yaml_string(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'').trim()
}

pub(super) fn parse_cargo_path_deps(text: &str) -> Vec<PathBuf> {
    let mut deps = Vec::new();
    let Some(value) = parse_toml_document(text) else {
        return deps;
    };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = value.get(key).and_then(|v| v.as_table()) {
            collect_cargo_path_deps(table, &mut deps);
        }
    }
    if let Some(targets) = value.get("target").and_then(|v| v.as_table()) {
        for target in targets.values() {
            if let Some(target_table) = target.as_table() {
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = target_table.get(key).and_then(|v| v.as_table()) {
                        collect_cargo_path_deps(table, &mut deps);
                    }
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

pub(super) fn collect_cargo_path_deps(
    table: &toml::map::Map<String, toml::Value>,
    deps: &mut Vec<PathBuf>,
) {
    for dep in table.values() {
        if let Some(path) = dep.get("path").and_then(|v| v.as_str()) {
            deps.push(PathBuf::from(path));
        }
    }
}
