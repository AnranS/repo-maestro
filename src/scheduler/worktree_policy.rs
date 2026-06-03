use std::collections::HashSet;
use std::path::{Component, Path};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::{Defaults, Project};

pub const DENY_PATTERNS: &[&str] = &[
    "**/.env*",
    "*.key",
    "*.pem",
    "id_rsa*",
    "*.p12",
    "*.pfx",
    ".aws/credentials",
    ".ssh/*",
    "*.kdbx",
    ".git-credentials",
];

#[derive(Debug)]
pub struct WorktreePolicy {
    deny_set: GlobSet,
    effective_copy_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFileDenial {
    pub path: String,
    pub rule: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreePolicyError {
    #[error("copy_files entry `{path}` is denied by built-in pattern `{pattern}`")]
    DeniedPath { path: String, pattern: String },
    #[error("{message}")]
    CopyFilesDenied {
        message: String,
        denials: Vec<CopyFileDenial>,
    },
    #[error("copy_files entry `{path}` failed: {source}")]
    CopyFailed {
        path: String,
        source: std::io::Error,
    },
    #[error("copy_files entry `{path}` contains npm auth")]
    NpmrcAuthFound { path: String },
    #[error("invalid worktree deny pattern `{pattern}`: {source}")]
    InvalidPattern {
        pattern: String,
        source: globset::Error,
    },
}

impl WorktreePolicy {
    pub fn from_config(
        defaults: &Defaults,
        project: &Project,
    ) -> Result<Self, WorktreePolicyError> {
        let mut builder = GlobSetBuilder::new();
        for pattern in DENY_PATTERNS {
            builder.add(Glob::new(pattern).map_err(|source| {
                WorktreePolicyError::InvalidPattern {
                    pattern: (*pattern).to_string(),
                    source,
                }
            })?);
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut effective_copy_files = Vec::new();
        for entry in defaults.copy_files.iter().chain(project.copy_files.iter()) {
            if seen.insert(entry.clone()) {
                effective_copy_files.push(entry.clone());
            }
        }

        Ok(Self {
            deny_set: builder
                .build()
                .map_err(|source| WorktreePolicyError::InvalidPattern {
                    pattern: "<globset>".to_string(),
                    source,
                })?,
            effective_copy_files,
        })
    }

    pub fn effective_copy_files(&self) -> &[String] {
        &self.effective_copy_files
    }

    pub fn deny_pattern_count(&self) -> usize {
        self.deny_set.len()
    }

    pub fn validate_copy_files(
        &self,
        source_root: &std::path::Path,
    ) -> Result<(), WorktreePolicyError> {
        let mut denials = self.copy_file_pattern_denials();

        for entry in &self.effective_copy_files {
            if denials.iter().any(|denial| denial.path == *entry) {
                continue;
            }
            if entry.ends_with(".npmrc") {
                let contents =
                    std::fs::read_to_string(source_root.join(entry)).map_err(|source| {
                        WorktreePolicyError::CopyFailed {
                            path: entry.clone(),
                            source,
                        }
                    })?;
                if ["_authToken", "_password", "_auth"]
                    .iter()
                    .any(|marker| contents.contains(marker))
                {
                    denials.push(CopyFileDenial {
                        path: entry.clone(),
                        rule: "contains npm auth".to_string(),
                    });
                }
            }
        }
        if !denials.is_empty() {
            return Err(self.denials_error(denials));
        }
        Ok(())
    }

    pub fn validate_copy_file_patterns(&self) -> Result<(), WorktreePolicyError> {
        let denials = self.copy_file_pattern_denials();
        if !denials.is_empty() {
            return Err(self.denials_error(denials));
        }
        Ok(())
    }

    pub fn copy_files_into(
        &self,
        source_root: &std::path::Path,
        worktree: &std::path::Path,
    ) -> Result<usize, WorktreePolicyError> {
        self.validate_copy_files(source_root)?;
        let mut copied = 0;
        for entry in &self.effective_copy_files {
            let source = source_root.join(entry);
            if !source.exists() {
                tracing::warn!("copy_files entry `{entry}` does not exist; skipping");
                continue;
            }
            let destination = worktree.join(entry);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|source| {
                    WorktreePolicyError::CopyFailed {
                        path: entry.clone(),
                        source,
                    }
                })?;
            }
            std::fs::copy(&source, &destination).map_err(|source| {
                WorktreePolicyError::CopyFailed {
                    path: entry.clone(),
                    source,
                }
            })?;
            copied += 1;
        }
        Ok(copied)
    }

    fn matching_deny_pattern(&self, entry: &str) -> Option<&'static str> {
        self.deny_set
            .matches(entry)
            .first()
            .and_then(|index| DENY_PATTERNS.get(*index).copied())
    }

    fn copy_file_pattern_denials(&self) -> Vec<CopyFileDenial> {
        self.effective_copy_files
            .iter()
            .filter_map(|entry| self.copy_file_pattern_denial(entry))
            .collect()
    }

    fn copy_file_pattern_denial(&self, entry: &str) -> Option<CopyFileDenial> {
        let path = Path::new(entry);
        let rule = if path.is_absolute() {
            Some("absolute path".to_string())
        } else if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            Some("path traversal".to_string())
        } else {
            self.matching_deny_pattern(entry)
                .map(|pattern| format!("built-in pattern `{pattern}`"))
        }?;
        Some(CopyFileDenial {
            path: entry.to_string(),
            rule,
        })
    }

    fn denials_error(&self, denials: Vec<CopyFileDenial>) -> WorktreePolicyError {
        if denials.len() == 1 {
            let denial = &denials[0];
            if let Some(pattern) = denial
                .rule
                .strip_prefix("built-in pattern `")
                .and_then(|rule| rule.strip_suffix('`'))
            {
                return WorktreePolicyError::DeniedPath {
                    path: denial.path.clone(),
                    pattern: pattern.to_string(),
                };
            }
            if denial.rule == "contains npm auth" {
                return WorktreePolicyError::NpmrcAuthFound {
                    path: denial.path.clone(),
                };
            }
        }
        let message = denials
            .iter()
            .map(|denial| format!("copy_files entry `{}` denied: {}", denial.path, denial.rule))
            .collect::<Vec<_>>()
            .join("\n");
        WorktreePolicyError::CopyFilesDenied { message, denials }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::config::{Project, ProjectsConfig};

    use super::{WorktreePolicy, WorktreePolicyError, DENY_PATTERNS};

    fn config_with_copy_files() -> ProjectsConfig {
        serde_yaml::from_str(
            r#"
version: 1
defaults:
  copy_files:
    - .editorconfig
    - tsconfig.base.json
projects:
  api:
    path: services/api
    copy_files:
      - tsconfig.base.json
      - .vscode/settings.json
"#,
        )
        .unwrap()
    }

    fn project_with_copy_files(copy_files: &[&str]) -> Project {
        Project {
            path: "services/api".to_string(),
            r#type: None,
            stack: Vec::new(),
            commands: BTreeMap::new(),
            contracts: Default::default(),
            dependencies: Vec::new(),
            memory_scope: Vec::new(),
            agent: None,
            agent_model: None,
            cursor_model: None,
            model_profile: None,
            role: None,
            copy_files: copy_files
                .iter()
                .map(|entry| (*entry).to_string())
                .collect(),
        }
    }

    fn policy_for(copy_files: &[&str]) -> WorktreePolicy {
        WorktreePolicy::from_config(&Default::default(), &project_with_copy_files(copy_files))
            .unwrap()
    }

    #[test]
    fn from_config_dedupes_defaults_and_project() {
        let cfg = config_with_copy_files();
        let project = cfg.projects.get("api").unwrap();
        let policy = WorktreePolicy::from_config(&cfg.defaults, project).unwrap();

        assert_eq!(
            policy.effective_copy_files(),
            vec![
                ".editorconfig",
                "tsconfig.base.json",
                ".vscode/settings.json"
            ]
        );
    }

    #[test]
    fn from_config_builds_all_ten_deny_patterns() {
        let defaults = Default::default();
        let project = project_with_copy_files(&[]);

        let policy = WorktreePolicy::from_config(&defaults, &project).unwrap();
        assert_eq!(DENY_PATTERNS.len(), 10);
        assert_eq!(policy.deny_pattern_count(), 10);
    }

    #[test]
    fn validate_accepts_legal_editor_config() {
        policy_for(&[".editorconfig"])
            .validate_copy_files(std::path::Path::new("."))
            .unwrap();
    }

    #[test]
    fn validate_accepts_empty_list() {
        policy_for(&[])
            .validate_copy_files(std::path::Path::new("."))
            .unwrap();
    }

    #[test]
    fn deny_patterns_reject_secret_copy_files() {
        for (entry, expected) in [
            (".env", "**/.env*"),
            (".env.local", "**/.env*"),
            ("services/api/.env.local", "**/.env*"),
            ("secret.key", "*.key"),
            ("cert.pem", "*.pem"),
            ("id_rsa", "id_rsa*"),
            ("bundle.p12", "*.p12"),
            ("bundle.pfx", "*.pfx"),
            (".aws/credentials", ".aws/credentials"),
            (".ssh/id_rsa", ".ssh/*"),
            ("vault.kdbx", "*.kdbx"),
            (".git-credentials", ".git-credentials"),
        ] {
            let err = policy_for(&[entry])
                .validate_copy_files(std::path::Path::new("."))
                .unwrap_err();
            assert!(matches!(
                err,
                WorktreePolicyError::DeniedPath { ref pattern, .. } if pattern == expected
            ));
        }
    }

    #[test]
    fn validate_reports_all_denied_copy_files_in_one_error() {
        let err = policy_for(&[
            "../../../../../etc/passwd",
            "/etc/hosts",
            ".env",
            "../outside-secret.txt",
        ])
        .validate_copy_files(std::path::Path::new("."))
        .unwrap_err();
        let message = err.to_string();

        assert!(message.contains("../../../../../etc/passwd"));
        assert!(message.contains("path traversal"));
        assert!(message.contains("/etc/hosts"));
        assert!(message.contains("absolute path"));
        assert!(message.contains(".env"));
        assert!(message.contains("**/.env*"));
        assert!(message.contains("../outside-secret.txt"));
        assert_eq!(
            message
                .lines()
                .filter(|line| line.contains("copy_files entry"))
                .count(),
            4
        );
    }

    #[test]
    fn npmrc_with_authtoken_rejected() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".npmrc"),
            "//registry.npmjs.org/:_authToken=abc",
        )
        .unwrap();

        let err = policy_for(&[".npmrc"])
            .validate_copy_files(root.path())
            .unwrap_err();
        assert!(matches!(err, WorktreePolicyError::NpmrcAuthFound { .. }));
    }

    #[test]
    fn npmrc_without_auth_accepted() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".npmrc"),
            "registry=https://registry.npmjs.org/",
        )
        .unwrap();

        policy_for(&[".npmrc"])
            .validate_copy_files(root.path())
            .unwrap();
    }

    #[test]
    fn npmrc_missing_file_returns_io_error() {
        let root = tempfile::tempdir().unwrap();
        let err = policy_for(&[".npmrc"])
            .validate_copy_files(root.path())
            .unwrap_err();
        match err {
            WorktreePolicyError::CopyFailed { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected CopyFailed, got {other:?}"),
        }
    }

    #[test]
    fn copy_files_into_copies_single_file() {
        let source = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join(".editorconfig"), "root = true").unwrap();

        let copied = policy_for(&[".editorconfig"])
            .copy_files_into(source.path(), worktree.path())
            .unwrap();

        assert_eq!(copied, 1);
        assert_eq!(
            std::fs::read_to_string(worktree.path().join(".editorconfig")).unwrap(),
            "root = true"
        );
    }

    #[test]
    fn copy_files_into_creates_parent_dirs() {
        let source = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join(".vscode")).unwrap();
        std::fs::write(source.path().join(".vscode/settings.json"), "{}").unwrap();

        policy_for(&[".vscode/settings.json"])
            .copy_files_into(source.path(), worktree.path())
            .unwrap();

        assert!(worktree.path().join(".vscode/settings.json").exists());
    }

    #[test]
    fn copy_files_into_skips_missing_source() {
        let source = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        let copied = policy_for(&["missing.file"])
            .copy_files_into(source.path(), worktree.path())
            .unwrap();

        assert_eq!(copied, 0);
    }
}
