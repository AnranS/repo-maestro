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

/// The providers whose enforcement profile maestro knows (the
/// `provider_permission_profile` match arms). The F-136a2 Settings "Providers" block
/// projects this matrix read-only; kept next to the matrix so the two never drift.
pub const KNOWN_PROVIDERS: [&str; 4] = ["shell", "codex", "cursor", "mock"];

/// A read-only projection of one provider's per-capability `Enforcement` — the honest
/// hard/soft/advisory matrix the Settings UI shows. `soft` is advisory (NOT
/// hard-blocked); `unsupported` is an unknown provider (fail-closed).
#[derive(Debug, Clone, Serialize)]
pub struct ProviderEnforcementProfile {
    pub provider_id: String,
    pub shell: Enforcement,
    pub git_write: Enforcement,
    pub network: Enforcement,
    pub fs_write: Enforcement,
    pub external_dir: Enforcement,
    pub mcp: Enforcement,
}

/// The enforcement matrix for every known provider (F-136a2). Pure projection of
/// `provider_permission_profile` — no behavior, no new fact source.
pub fn provider_enforcement_profiles() -> Vec<ProviderEnforcementProfile> {
    KNOWN_PROVIDERS
        .iter()
        .map(|&id| {
            let r = provider_permission_profile(id);
            ProviderEnforcementProfile {
                provider_id: id.to_string(),
                shell: r.shell,
                git_write: r.git_write,
                network: r.network,
                fs_write: r.fs_write,
                external_dir: r.external_dir,
                mcp: r.mcp,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_schema_version_is_stable() {
        assert_eq!(PermissionEvidence::SCHEMA_VERSION, "maestro.permission.v1");
    }

    #[test]
    fn provider_enforcement_matrix_projects_known_providers() {
        let profiles = provider_enforcement_profiles();
        assert_eq!(profiles.len(), KNOWN_PROVIDERS.len());
        let codex = profiles.iter().find(|p| p.provider_id == "codex").unwrap();
        // codex hard-enforces network/fs/external; shell/git_write are advisory (soft).
        assert_eq!(codex.network, Enforcement::Hard);
        assert_eq!(codex.fs_write, Enforcement::Hard);
        assert_eq!(codex.git_write, Enforcement::Soft);
        let shell = profiles.iter().find(|p| p.provider_id == "shell").unwrap();
        assert_eq!(shell.shell, Enforcement::Hard);
        assert_eq!(shell.git_write, Enforcement::Soft); // advisory, not hard
    }

    #[test]
    fn unknown_provider_is_all_unsupported_fail_closed() {
        // Kept a pure Rust check (not exposed via the API per F-136a2 pin 1).
        let p = provider_permission_profile("totally-unknown");
        assert_eq!(p.shell, Enforcement::Unsupported);
        assert_eq!(p.network, Enforcement::Unsupported);
        assert_eq!(p.mcp, Enforcement::Unsupported);
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
