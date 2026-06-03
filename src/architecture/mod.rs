//! Greenfield scaffolding — go from a single ARCHITECTURE.yaml to a workspace
//! of N git repositories with the right starter files, the right contract
//! placeholders, and a fully wired `projects.yaml`. The first phase that turns
//! maestro from "coordinator of existing repos" into "builder of new ones".

pub mod scaffold;
pub mod templates;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single declared module — what mkdir + scaffold + register in projects.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchModule {
    pub name: String,

    /// `backend`, `frontend`, `mobile`, `tool`, `library`, …
    #[serde(default)]
    pub r#type: Option<String>,

    /// Tag list. The first one that maps to a known template is the stack we
    /// scaffold. E.g. `[python, fastapi]` → fastapi template.
    #[serde(default)]
    pub stack: Vec<String>,

    /// Path to the contract file this module *publishes*. The scaffold step
    /// drops a minimal placeholder there so consumers can start typing imports.
    #[serde(default)]
    pub provides: Option<String>,

    /// Path to a contract file this module *consumes*. Just metadata — we
    /// don't auto-generate consumer-side bindings (yet).
    #[serde(default)]
    pub consumes: Option<String>,

    /// Free-form rationale that the planner agent fills in. Surfaces in the
    /// UI as the module's tooltip.
    #[serde(default)]
    pub rationale: Option<String>,

    /// `memory_scope` to copy onto the project entry in projects.yaml.
    #[serde(default)]
    pub memory_scope: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchContract {
    /// Path relative to whichever module produces it.
    pub file: String,
    pub from: String,
    #[serde(default)]
    pub to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Architecture {
    pub spec: String,
    pub modules: Vec<ArchModule>,
    #[serde(default)]
    pub contracts: Vec<ArchContract>,
}

impl Architecture {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read architecture file {:?}", path))?;
        Self::parse(&text)
    }

    pub fn parse(yaml: &str) -> Result<Self> {
        let arch: Self = serde_yaml::from_str(yaml).context("parse ARCHITECTURE.yaml")?;
        arch.validate()?;
        Ok(arch)
    }

    pub fn validate(&self) -> Result<()> {
        if self.modules.is_empty() {
            anyhow::bail!("architecture has no modules");
        }
        let mut names = std::collections::HashSet::new();
        for m in &self.modules {
            if m.name.is_empty() {
                anyhow::bail!("module with empty name");
            }
            if !names.insert(m.name.as_str()) {
                anyhow::bail!("duplicate module name: {}", m.name);
            }
        }
        for c in &self.contracts {
            if !names.contains(c.from.as_str()) {
                anyhow::bail!("contract `{}` from unknown module `{}`", c.file, c.from);
            }
            for t in &c.to {
                if !names.contains(t.as_str()) {
                    anyhow::bail!("contract `{}` to unknown module `{}`", c.file, t);
                }
            }
        }
        Ok(())
    }
}
