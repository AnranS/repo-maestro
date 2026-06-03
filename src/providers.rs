//! Provider registry for task adapters and nearby CLI agents.
//!
//! This keeps provider discovery visible to `doctor`, the CLI, and the UI
//! instead of scattering "is codex installed?" checks through the codebase.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::schema::permissions::{provider_permission_profile, ResolvedPermission};

#[derive(Debug, Clone, Copy)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display: &'static str,
    pub kind: ProviderKind,
    pub default_binary: Option<&'static str>,
    pub env_override: Option<&'static str>,
    pub supports_model: bool,
    pub supports_non_interactive: bool,
    pub notes: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    TaskAdapter,
    KnownCli,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    #[serde(default = "crate::schema::provider_capability_version")]
    pub schema_version: String,
    pub id: &'static str,
    pub display: &'static str,
    pub kind: ProviderKind,
    pub adapter_available: bool,
    pub installed: bool,
    pub authenticated: bool,
    pub execution: ProviderExecution,
    pub permissions: ResolvedPermission,
    pub models: ProviderModelCapability,
    pub tool_trace: ToolTraceSupport,
    pub notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderExecution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_override: Option<&'static str>,
    pub non_interactive: bool,
    pub streaming: bool,
    pub resume: bool,
    pub worktree_isolation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderModelCapability {
    pub supports_override: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolTraceSupport {
    Full,
    Partial,
    None,
}

pub fn known_providers() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            id: "codex",
            display: "Codex CLI",
            kind: ProviderKind::TaskAdapter,
            default_binary: Some("codex"),
            env_override: Some("MAESTRO_CODEX"),
            supports_model: true,
            supports_non_interactive: true,
            notes: "runs `codex exec --json` for agent tasks",
        },
        ProviderDescriptor {
            id: "cursor",
            display: "Cursor Agent",
            kind: ProviderKind::TaskAdapter,
            default_binary: Some("cursor-agent"),
            env_override: Some("MAESTRO_CURSOR_AGENT"),
            supports_model: true,
            supports_non_interactive: true,
            notes: "runs `cursor-agent --output-format stream-json`",
        },
        ProviderDescriptor {
            id: "shell",
            display: "Shell",
            kind: ProviderKind::TaskAdapter,
            default_binary: Some("sh"),
            env_override: None,
            supports_model: false,
            supports_non_interactive: true,
            notes: "runs deterministic verify commands",
        },
        ProviderDescriptor {
            id: "mock",
            display: "Mock",
            kind: ProviderKind::TaskAdapter,
            default_binary: None,
            env_override: None,
            supports_model: false,
            supports_non_interactive: true,
            notes: "built-in dry-run adapter",
        },
        ProviderDescriptor {
            id: "claude",
            display: "Claude Code",
            kind: ProviderKind::KnownCli,
            default_binary: Some("claude"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: false,
            // Chat adapter is wired (stream-json + --include-partial-messages
            // for thinking trace) and the WebUI / chat-tui both route through
            // it. Task-adapter (running `claude` for a DAG task) is still
            // open work — different from chat, requires headless mode + JSON
            // output for trajectory capture.
            notes: "chat adapter wired (stream-json + thinking); task adapter not wired yet",
        },
        ProviderDescriptor {
            id: "gemini",
            display: "Gemini CLI",
            kind: ProviderKind::KnownCli,
            default_binary: Some("gemini"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; adapter not wired yet",
        },
        ProviderDescriptor {
            id: "opencode",
            display: "OpenCode",
            kind: ProviderKind::KnownCli,
            default_binary: Some("opencode"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; adapter not wired yet",
        },
        ProviderDescriptor {
            id: "qwen",
            display: "Qwen Code",
            kind: ProviderKind::KnownCli,
            default_binary: Some("qwen"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; channel/session model not wired yet",
        },
        ProviderDescriptor {
            id: "cline",
            display: "Cline CLI",
            kind: ProviderKind::KnownCli,
            default_binary: Some("cline"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI/IDE agent; adapter not wired yet",
        },
        ProviderDescriptor {
            id: "crush",
            display: "Crush",
            kind: ProviderKind::KnownCli,
            default_binary: Some("crush"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; hook/provider schema not wired yet",
        },
        ProviderDescriptor {
            id: "goose",
            display: "Goose",
            kind: ProviderKind::KnownCli,
            default_binary: Some("goose"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: false,
            notes: "known external CLI; adapter not wired yet",
        },
        ProviderDescriptor {
            id: "aider",
            display: "Aider",
            kind: ProviderKind::KnownCli,
            default_binary: Some("aider"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; repo-map and git loop not wired yet",
        },
        ProviderDescriptor {
            id: "mini-swe-agent",
            display: "mini-SWE-agent",
            kind: ProviderKind::KnownCli,
            default_binary: Some("mini"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI; trajectory format not wired yet",
        },
        ProviderDescriptor {
            id: "continue",
            display: "Continue CLI",
            kind: ProviderKind::KnownCli,
            default_binary: Some("cn"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external CLI/check runner; adapter not wired yet",
        },
        ProviderDescriptor {
            id: "openhands",
            display: "OpenHands",
            kind: ProviderKind::KnownCli,
            default_binary: Some("openhands"),
            env_override: None,
            supports_model: true,
            supports_non_interactive: true,
            notes: "known external platform; sandbox/event model not wired yet",
        },
    ]
}

pub fn provider_statuses(adapters_only: bool) -> Vec<ProviderStatus> {
    known_providers()
        .into_iter()
        .filter(|p| !adapters_only || p.kind != ProviderKind::KnownCli)
        .map(provider_status)
        .collect()
}

pub fn provider_install_hint(
    id: &str,
    binary: Option<&str>,
    env_override: Option<&str>,
) -> Option<String> {
    match id {
        "codex" => Some(
            "install OpenAI Codex CLI so `codex --version` works, or set MAESTRO_CODEX=/path/to/codex"
                .to_string(),
        ),
        "cursor" => Some(
            "install Cursor CLI so `cursor-agent --version` works, or set MAESTRO_CURSOR_AGENT=/path/to/cursor-agent"
                .to_string(),
        ),
        "shell" => Some("install POSIX shell tools so `sh` is available on PATH".to_string()),
        _ => binary.map(|binary| match env_override {
            Some(env) => format!("install `{binary}` so `{binary} --version` works, or set {env}=/path/to/{binary}"),
            None => format!("install `{binary}` so `{binary} --version` works"),
        }),
    }
}

pub fn provider_status(provider: ProviderDescriptor) -> ProviderStatus {
    let binary = provider.default_binary.map(|default| {
        provider
            .env_override
            .and_then(|env| std::env::var(env).ok())
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| default.to_string())
    });
    let path = binary.as_deref().and_then(find_binary);
    let installed = provider.default_binary.is_none() || path.is_some();
    ProviderStatus {
        schema_version: crate::schema::PROVIDER_CAPABILITY_V1.to_string(),
        id: provider.id,
        display: provider.display,
        kind: provider.kind,
        adapter_available: provider.kind != ProviderKind::KnownCli,
        installed,
        authenticated: installed,
        execution: ProviderExecution {
            binary,
            path: path.map(|p| p.display().to_string()),
            env_override: provider.env_override,
            non_interactive: provider.supports_non_interactive,
            streaming: matches!(provider.id, "codex" | "cursor"),
            resume: provider.id == "cursor",
            worktree_isolation: provider.kind == ProviderKind::TaskAdapter,
        },
        permissions: provider_permission_profile(provider.id),
        models: ProviderModelCapability {
            supports_override: provider.supports_model,
            default: None,
            context_tokens: None,
        },
        tool_trace: match provider.id {
            "codex" | "cursor" => ToolTraceSupport::Partial,
            "shell" => ToolTraceSupport::Full,
            _ => ToolTraceSupport::None,
        },
        notes: provider.notes,
    }
}

pub fn find_binary(binary: &str) -> Option<PathBuf> {
    let path = Path::new(binary);
    if path.components().count() > 1 && is_executable_file(path) {
        return Some(path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(binary);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_current_task_adapters() {
        let statuses = provider_statuses(true);
        let ids: Vec<_> = statuses.iter().map(|p| p.id).collect();
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"cursor"));
        assert!(ids.contains(&"shell"));
        assert!(ids.contains(&"mock"));
        assert!(!ids.contains(&"gemini"));
    }

    #[test]
    fn mock_provider_does_not_need_a_binary() {
        let mock = known_providers()
            .into_iter()
            .find(|p| p.id == "mock")
            .map(provider_status)
            .unwrap();
        assert!(mock.installed);
        assert!(mock.execution.binary.is_none());
    }

    #[test]
    fn registry_includes_benchmarked_external_clis() {
        let ids: Vec<_> = known_providers().into_iter().map(|p| p.id).collect();
        for id in [
            "qwen",
            "cline",
            "crush",
            "aider",
            "mini-swe-agent",
            "continue",
        ] {
            assert!(ids.contains(&id));
        }
    }

    #[test]
    fn provider_capability_schema_v1_is_stable() {
        let statuses = provider_statuses(false);
        let raw = serde_json::to_string(&statuses).unwrap();
        assert!(!raw.contains("task_fit"));
        assert!(!raw.contains("cost_hint"));
        assert!(!raw.contains("verification"));
        assert!(!raw.contains("simulation"));

        for status in &statuses {
            let json = serde_json::to_value(status).unwrap();
            assert_eq!(
                json["schema_version"],
                crate::schema::PROVIDER_CAPABILITY_V1
            );
            assert!(
                matches!(json["kind"].as_str(), Some("task_adapter" | "known_cli")),
                "provider {} serialized invalid kind: {}",
                status.id,
                json["kind"]
            );
        }

        for id in ["codex", "cursor", "shell", "mock"] {
            let provider = statuses.iter().find(|p| p.id == id).expect(id);
            assert!(
                provider.adapter_available,
                "{id} should be wired as a task adapter"
            );
        }
    }

    #[test]
    fn provider_install_hint_names_binary_and_env_override() {
        let hint = provider_install_hint("codex", Some("codex"), Some("MAESTRO_CODEX"))
            .expect("codex hint");

        assert!(hint.contains("codex --version"));
        assert!(hint.contains("MAESTRO_CODEX=/path/to/codex"));
    }
}
