use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub chat: ChatSettings,
    #[serde(default)]
    pub learning: LearningSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningSettings {
    /// Distill a finished run's FAILURES into inert guardrail proposals under
    /// `.maestro/proposals/` (review with `maestro learn`). OFF by default —
    /// a proposal never changes a future run until it is explicitly promoted.
    #[serde(default)]
    pub propose_guardrails: bool,
    /// Draft a reusable skill PLAYBOOK proposal from a verified, complex run
    /// (multi-project / contract-wiring / many tasks). Same propose→review→
    /// promote gate as guardrails; OFF by default.
    #[serde(default)]
    pub synthesize_skills: bool,
}

impl Settings {
    pub fn load() -> Self {
        let path = match crate::paths::maestro_dir() {
            Ok(dir) => dir.join("settings.yaml"),
            Err(_) => return Self::default(),
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            // Missing file is the common case — return defaults silently.
            return Self::default();
        };
        // But a PRESENT-and-malformed settings.yaml is a config error the
        // user needs to see: returning Self::default() silently used to
        // mean "your setting just doesn't apply and you have no idea why".
        match serde_yaml::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "{} is malformed ({e}); falling back to defaults — fix the file or delete it to keep this message from repeating",
                    path.display(),
                );
                Self::default()
            }
        }
    }
}
