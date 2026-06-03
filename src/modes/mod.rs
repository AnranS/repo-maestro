use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mode {
    pub id: String,
    pub display: String,
    pub role_prelude: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub allowed_tools: AllowedTools,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedTools {
    #[serde(default = "default_true")]
    pub shell: bool,
    #[serde(default = "default_true")]
    pub git_write: bool,
    #[serde(default = "default_true")]
    pub network: bool,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

impl Default for AllowedTools {
    fn default() -> Self {
        Self {
            shell: true,
            git_write: true,
            network: true,
            allowed_commands: Vec::new(),
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Self::permissive()
    }
}

fn default_true() -> bool {
    true
}

impl Mode {
    pub fn from_role(role: &crate::roles::Role) -> Self {
        Self {
            id: role.name.clone(),
            display: role.display.clone(),
            role_prelude: role.prelude.clone(),
            skills: role.skills.clone().unwrap_or_default(),
            allowed_tools: role.allowed_tools.clone().unwrap_or_default(),
        }
    }

    pub fn permissive() -> Self {
        Self {
            id: "permissive".to_string(),
            display: "Permissive".to_string(),
            role_prelude: String::new(),
            skills: Vec::new(),
            allowed_tools: AllowedTools::default(),
        }
    }
}

pub fn resolve_mode_for_task(
    task: &crate::config::PlanTask,
    projects: &crate::config::ProjectsConfig,
) -> Mode {
    let role_name = crate::roles::resolve_for_task(
        task.role.as_deref(),
        projects.resolved_role(&task.project).as_deref(),
    );
    role_name
        .as_deref()
        .and_then(crate::roles::try_load)
        .as_ref()
        .map(Mode::from_role)
        .unwrap_or_else(Mode::permissive)
}

pub fn render_prompt_constraints(mode: &Mode) -> String {
    let commands = if mode.allowed_tools.allowed_commands.is_empty() {
        "(unrestricted)".to_string()
    } else {
        mode.allowed_tools.allowed_commands.join(", ")
    };
    format!(
        "# Mode constraints\n\nMode: {} ({})\nTools allowed: shell={}, git_write={}, network={}, commands={}\nDo not run commands outside this allowlist.\n\n",
        mode.id,
        mode.display,
        mode.allowed_tools.shell,
        mode.allowed_tools.git_write,
        mode.allowed_tools.network,
        commands
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_with_allowed_tools_loads() {
        let role = crate::roles::load("qa").unwrap();
        let mode = Mode::from_role(&role);
        assert!(!mode.allowed_tools.git_write);
        assert!(mode
            .allowed_tools
            .allowed_commands
            .iter()
            .any(|cmd| cmd == "cargo test*"));
        assert!(mode.skills.iter().any(|skill| skill == "qa-web-flow"));
    }

    #[test]
    fn role_without_fields_is_permissive() {
        let role = crate::roles::Role {
            name: "x".into(),
            display: "X".into(),
            summary: String::new(),
            source: None,
            tags: vec![],
            prelude: String::new(),
            builtin: false,
            path: None,
            skills: None,
            allowed_tools: None,
        };
        let mode = Mode::from_role(&role);
        assert!(mode.allowed_tools.shell);
        assert!(mode.allowed_tools.git_write);
        assert!(mode.allowed_tools.network);
        assert!(mode.allowed_tools.allowed_commands.is_empty());
    }
}
