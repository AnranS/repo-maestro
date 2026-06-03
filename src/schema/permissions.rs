use serde::{Deserialize, Serialize};

use crate::modes::AllowedTools;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    Hard,
    Soft,
    Unsupported,
    #[default]
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionEvidence {
    #[serde(default = "crate::schema::permission_version")]
    pub schema_version: String,
    pub task_id: String,
    pub provider_id: String,
    pub mode_id: String,
    pub requested: PermissionRequest,
    pub resolved: ResolvedPermission,
}

impl PermissionEvidence {
    pub const SCHEMA_VERSION: &'static str = crate::schema::PERMISSION_V1;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRequest {
    pub shell: bool,
    pub git_write: bool,
    pub network: bool,
    pub fs_write: bool,
    pub external_dir: bool,
    pub mcp: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedPermission {
    pub shell: Enforcement,
    pub git_write: Enforcement,
    pub network: Enforcement,
    pub fs_write: Enforcement,
    pub external_dir: Enforcement,
    pub mcp: Enforcement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
}

impl ResolvedPermission {
    pub fn all(enforcement: Enforcement) -> Self {
        Self {
            shell: enforcement,
            git_write: enforcement,
            network: enforcement,
            fs_write: enforcement,
            external_dir: enforcement,
            mcp: enforcement,
            allowed_commands: Vec::new(),
        }
    }
}

pub fn resolve_permission_evidence(
    task_id: impl Into<String>,
    provider_id: &str,
    mode_id: impl Into<String>,
    allowed_tools: &AllowedTools,
) -> PermissionEvidence {
    let mut resolved = provider_permission_profile(provider_id);
    resolved.allowed_commands = allowed_tools.allowed_commands.clone();
    PermissionEvidence {
        schema_version: crate::schema::PERMISSION_V1.to_string(),
        task_id: task_id.into(),
        provider_id: provider_id.to_string(),
        mode_id: mode_id.into(),
        requested: PermissionRequest {
            shell: allowed_tools.shell,
            git_write: allowed_tools.git_write,
            network: allowed_tools.network,
            // v1 has no standalone role field for filesystem write. Treat
            // git_write as the closest concrete request and keep external_dir
            // and mcp false until modes grow those dimensions.
            fs_write: allowed_tools.git_write,
            external_dir: false,
            mcp: false,
            allowed_commands: allowed_tools.allowed_commands.clone(),
        },
        resolved,
    }
}

pub fn provider_permission_profile(provider_id: &str) -> ResolvedPermission {
    match provider_id {
        "shell" => ResolvedPermission {
            shell: Enforcement::Hard,
            git_write: Enforcement::Soft,
            network: Enforcement::Soft,
            fs_write: Enforcement::Soft,
            external_dir: Enforcement::Soft,
            mcp: Enforcement::NotApplicable,
            allowed_commands: Vec::new(),
        },
        "mock" => ResolvedPermission::all(Enforcement::NotApplicable),
        "codex" => ResolvedPermission {
            shell: Enforcement::Soft,
            git_write: Enforcement::Soft,
            network: Enforcement::Hard,
            fs_write: Enforcement::Hard,
            external_dir: Enforcement::Hard,
            mcp: Enforcement::NotApplicable,
            allowed_commands: Vec::new(),
        },
        "cursor" => ResolvedPermission {
            shell: Enforcement::Soft,
            git_write: Enforcement::Soft,
            network: Enforcement::Soft,
            fs_write: Enforcement::Hard,
            external_dir: Enforcement::Hard,
            mcp: Enforcement::NotApplicable,
            allowed_commands: Vec::new(),
        },
        _ => ResolvedPermission::all(Enforcement::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_schema_version_is_stable() {
        assert_eq!(PermissionEvidence::SCHEMA_VERSION, "maestro.permission.v1");
    }

    #[test]
    fn allowed_command_globs_are_case_sensitive_fnmatch_style() {
        let evidence = resolve_permission_evidence(
            "T",
            "shell",
            "qa",
            &AllowedTools {
                shell: true,
                git_write: false,
                network: false,
                allowed_commands: vec!["cargo test*".into()],
            },
        );
        assert_eq!(evidence.requested.allowed_commands, vec!["cargo test*"]);
        assert_eq!(evidence.resolved.allowed_commands, vec!["cargo test*"]);
    }
}
