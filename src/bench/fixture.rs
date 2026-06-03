use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub const FIXTURE_FILE: &str = "fixture.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fixture {
    pub id: String,
    pub kind: FixtureKind,
    pub goal: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Upstream>,

    #[serde(default)]
    pub expected: Expected,

    #[serde(default)]
    pub budget: Budget,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureKind {
    OssReplay,
    ContractBreak,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Upstream {
    pub url: String,
    pub parent_sha: String,
    pub pr_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Expected {
    #[serde(default)]
    pub touched_files: Vec<String>,
    #[serde(default)]
    pub required_projects: Vec<String>,
    #[serde(default)]
    pub required_contract_consumers: Vec<String>,
    #[serde(default)]
    pub forbidden_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Budget {
    #[serde(default = "default_max_runtime_secs")]
    pub max_runtime_secs: u64,
    #[serde(default = "default_max_tasks")]
    pub max_tasks: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_runtime_secs: default_max_runtime_secs(),
            max_tasks: default_max_tasks(),
        }
    }
}

fn default_max_runtime_secs() -> u64 {
    120
}

fn default_max_tasks() -> usize {
    8
}

impl Fixture {
    pub fn validate(&self) -> Result<()> {
        paths::validate_path_component("fixture id", &self.id)?;
        if self.goal.trim().is_empty() {
            anyhow::bail!("fixture {} goal cannot be empty", self.id);
        }
        if self.budget.max_runtime_secs == 0 {
            anyhow::bail!("fixture {} budget.max_runtime_secs must be > 0", self.id);
        }
        if self.budget.max_tasks == 0 {
            anyhow::bail!("fixture {} budget.max_tasks must be > 0", self.id);
        }
        if matches!(self.kind, FixtureKind::OssReplay) {
            let upstream = self
                .upstream
                .as_ref()
                .with_context(|| format!("fixture {} oss-replay requires upstream", self.id))?;
            if upstream.url.trim().is_empty()
                || upstream.parent_sha.trim().is_empty()
                || upstream.pr_url.trim().is_empty()
            {
                anyhow::bail!("fixture {} upstream fields cannot be empty", self.id);
            }
        }
        Ok(())
    }
}

pub fn load_all() -> Result<Vec<Fixture>> {
    let dir = paths::bench_scenarios_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut fixture_files = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read bench scenarios dir {dir:?}"))? {
        let entry = entry?;
        let path = entry.path().join(FIXTURE_FILE);
        if path.exists() {
            fixture_files.push(path);
        }
    }
    fixture_files.sort();

    let mut seen = BTreeSet::new();
    let mut fixtures = Vec::new();
    for path in fixture_files {
        let fixture = load_one(&path)?;
        fixture.validate()?;
        if !seen.insert(fixture.id.clone()) {
            anyhow::bail!("duplicate fixture id: {}", fixture.id);
        }
        fixtures.push(fixture);
    }
    Ok(fixtures)
}

fn load_one(path: &Path) -> Result<Fixture> {
    let text = fs::read_to_string(path).with_context(|| format!("read fixture {path:?}"))?;
    serde_yaml::from_str(&text).with_context(|| format!("parse fixture {path:?}"))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn with_workspace() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
        }
        dir
    }

    fn clear_workspace_env() {
        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[test]
    #[serial]
    fn loads_all_scenarios() {
        let workspace = with_workspace();
        let scenario = workspace.path().join("bench/scenarios/sample");
        fs::create_dir_all(&scenario).expect("scenario dir");
        fs::write(
            scenario.join(FIXTURE_FILE),
            r#"id: sample
kind: contract-break
goal: Catch an upstream contract break
expected:
  required_contract_consumers: [consumer]
"#,
        )
        .expect("fixture yaml");

        let fixtures = load_all().expect("fixtures");

        assert_eq!(fixtures.len(), 1);
        assert_eq!(fixtures[0].id, "sample");
        assert_eq!(fixtures[0].kind, FixtureKind::ContractBreak);
        assert_eq!(fixtures[0].budget.max_tasks, 8);
        clear_workspace_env();
    }
}
