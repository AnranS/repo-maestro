use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROJECTS_TMP_READ_RETRY_ATTEMPTS: usize = 2;
const PROJECTS_TMP_READ_RETRY_DELAY: Duration = Duration::from_millis(50);
const PROJECTS_TMP_WRITE_RETRY_ATTEMPTS: usize = 5;
const PROJECTS_TMP_WRITE_RETRY_DELAY: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectsConfig {
    #[serde(default = "default_version")]
    pub version: u32,

    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default = "default_agent")]
    pub agent: String,

    #[serde(default = "default_branch_prefix")]
    pub branch_prefix: String,

    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,

    /// Hard ceiling on the total number of tasks a single run may execute.
    /// A safety rail against a runaway synthesized/dynamic plan: if a plan
    /// expands past this, the run is refused rather than truncated (silent
    /// truncation would drop work and report a misleading success). `0`
    /// disables the cap. Borrowed from the fixed runtime cap pattern in
    /// Anthropic's Dynamic Workflows (16 concurrent / 1000 total).
    #[serde(default = "default_max_total_tasks")]
    pub max_total_tasks: usize,

    /// Adapter-neutral model passed to task agents. `None` means "let the
    /// selected agent decide" (account-level default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,

    /// Legacy alias for `agent_model`, kept so older projects.yaml files still
    /// load. When both are set, `agent_model` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_model: Option<String>,

    /// Optional cheaper model for the AI Tagger. Falls back to `agent_model`,
    /// then legacy `cursor_model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagger_model: Option<String>,

    /// Optional workspace-wide model profile. A profile is a named fallback
    /// chain under `defaults.model_profiles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,

    /// Named model fallback chains. Profiles keep task/role config stable even
    /// when the specific model available in a user's account changes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_profiles: BTreeMap<String, ModelProfile>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_files: Vec<String>,

    /// Risk-driven oversight: when true, a completed task whose change is
    /// classified high-risk (touches a contract / large blast radius) pauses
    /// for human approval before its patch integrates — even if the task
    /// didn't declare `requires_approval_after`. Default off, preserving the
    /// existing all-or-nothing gating.
    #[serde(default)]
    pub gate_on_high_risk: bool,

    /// Task routing policy: pick agent/model from a task's static traits
    /// (kind, whether its project touches a contract) before it runs. Empty =
    /// no routing, default per-project resolution applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing: Vec<crate::scheduler::routing::RouteRule>,

    /// Close the PR loop: when true, a verified run automatically pushes its
    /// integration branch and opens a draft PR (equivalent to `maestro pr
    /// --push`). Default off. Best-effort — a push/PR failure warns, it doesn't
    /// fail the run.
    #[serde(default)]
    pub auto_pr: bool,

    /// Adversarial refute pass (F-106): when true, a completed task that is
    /// classified high-risk and has no explicit `review_by` gets the builtin
    /// `refuter` role auto-attached as its review step — an adversarial agent
    /// that hunts for what the writer missed before the change integrates. A
    /// refutation feeds the normal retry/circuit-breaker recovery path.
    /// Default off; explicit `review_by` always wins. Cost is contained by
    /// the default-off + high-risk-only scoping (the `--max-tokens` budget
    /// does not yet account for reviewer/refuter usage).
    #[serde(default)]
    pub refute_on_high_risk: bool,

    /// F-109: sibling workspace roots to also index contract providers from,
    /// so a consumer here can link `consumes` to a producer that lives in
    /// another repo. Each path points at a workspace root with its own
    /// `.maestro/projects.yaml`; only its projects that already have
    /// `contracts.provides` set are indexed (no discovery is run into the
    /// sibling tree). Relative paths resolve against this workspace root —
    /// **prefer relative**; absolute paths are machine-specific and should not
    /// be committed. Empty (default) = single-root behavior, unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_workspaces: Vec<String>,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            agent: default_agent(),
            branch_prefix: default_branch_prefix(),
            max_parallel: default_max_parallel(),
            max_total_tasks: default_max_total_tasks(),
            agent_model: None,
            cursor_model: None,
            tagger_model: None,
            model_profile: None,
            model_profiles: BTreeMap::new(),
            copy_files: Vec::new(),
            gate_on_high_risk: false,
            routing: Vec::new(),
            auto_pr: false,
            refute_on_high_risk: false,
            sibling_workspaces: Vec::new(),
        }
    }
}

fn default_agent() -> String {
    "cursor".into()
}
fn default_branch_prefix() -> String {
    "feat/".into()
}
fn default_max_parallel() -> usize {
    4
}
fn default_max_total_tasks() -> usize {
    // Generous default: real maestro plans are tens-to-low-hundreds of
    // tasks, so 1000 never bites a legitimate run but stops a runaway
    // expansion cold. Matches Anthropic Dynamic Workflows' 1000-agent cap.
    1000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "Contracts::is_empty")]
    pub contracts: Contracts,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_scope: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,

    /// Per-project adapter-neutral model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,

    /// Legacy alias for `agent_model`, kept so older projects.yaml files still
    /// load. When both are set, `agent_model` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_model: Option<String>,

    /// Per-project model profile. Resolved before `agent_model` and after
    /// task/run-level overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,

    /// Default role for tasks running in this project. Looked up against
    /// the role registry (builtin + `.maestro/roles/`). A `PlanTask.role`
    /// can override this on a per-task basis. `None` means "no role
    /// prelude" — the task gets the bare prompt + memory only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_files: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelProfile {
    /// First model to try for this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred: Option<String>,

    /// Ordered fallback model ids. When a refreshed model cache exists,
    /// maestro picks the first candidate present in the cache. Otherwise it
    /// passes the first configured candidate through to the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
}

impl ModelProfile {
    pub fn candidates(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(preferred) = self.preferred.as_deref().and_then(non_empty) {
            out.push(preferred.to_string());
        }
        for fallback in &self.fallback {
            if let Some(model) = non_empty(fallback) {
                if !out.iter().any(|seen| seen == model) {
                    out.push(model.to_string());
                }
            }
        }
        out
    }
}

impl Defaults {
    pub fn effective_agent_model(&self) -> Option<&str> {
        self.agent_model
            .as_deref()
            .and_then(non_empty)
            .or_else(|| self.cursor_model.as_deref().and_then(non_empty))
    }
}

impl Project {
    pub fn effective_agent_model(&self) -> Option<&str> {
        self.agent_model
            .as_deref()
            .and_then(non_empty)
            .or_else(|| self.cursor_model.as_deref().and_then(non_empty))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Contracts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provides: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumes: Option<String>,
}

impl Contracts {
    pub fn is_empty(&self) -> bool {
        self.provides.is_none() && self.consumes.is_none()
    }
}

impl ProjectsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg = Self::load_once(path)?;
        for _ in 0..PROJECTS_TMP_READ_RETRY_ATTEMPTS {
            if !cfg.projects.is_empty() || !projects_tmp_path(path).exists() {
                return Ok(cfg);
            }
            std::thread::sleep(PROJECTS_TMP_READ_RETRY_DELAY);
            cfg = Self::load_once(path)?;
        }
        Ok(cfg)
    }

    fn load_once(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read projects file {:?}", path))?;
        let cfg: Self = serde_yaml::from_str(&text).context("parse projects.yaml")?;
        cfg.validate_worktree_policy()
            .context("validate projects.yaml worktree policy")?;
        Ok(cfg)
    }

    fn validate_worktree_policy(&self) -> Result<()> {
        for (name, project) in &self.projects {
            let policy = crate::scheduler::worktree_policy::WorktreePolicy::from_config(
                &self.defaults,
                project,
            )
            .with_context(|| format!("worktree policy for project `{name}`"))?;
            policy
                .validate_copy_file_patterns()
                .with_context(|| format!("worktree policy for project `{name}`"))?;
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let text = serde_yaml::to_string(self).context("serialize projects.yaml")?;
        let tmp_path = projects_tmp_path(path);
        for attempt in 0..=PROJECTS_TMP_WRITE_RETRY_ATTEMPTS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
            {
                Ok(mut tmp_file) => {
                    let mut guard = TmpGuard::new(tmp_path.clone());
                    tmp_file
                        .write_all(text.as_bytes())
                        .with_context(|| format!("write projects tmp file {:?}", tmp_path))?;
                    drop(tmp_file);
                    std::fs::rename(&tmp_path, path).with_context(|| {
                        format!("rename projects tmp file into place {:?}", path)
                    })?;
                    guard.disarm();
                    return Ok(());
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if attempt == PROJECTS_TMP_WRITE_RETRY_ATTEMPTS {
                        bail!(
                            "another maestro process is writing the registry \
                             (sibling {:?} holds the writer lock for > 50ms); \
                             retry the command in a moment, or run `maestro doctor` \
                             to inspect the workspace.",
                            tmp_path
                        );
                    }
                    std::thread::sleep(PROJECTS_TMP_WRITE_RETRY_DELAY);
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("create projects tmp file {:?}", tmp_path));
                }
            }
        }
        unreachable!("projects tmp writer retry loop should return or bail")
    }

    pub fn resolved_path(&self, project_name: &str) -> Result<PathBuf> {
        let p = self
            .projects
            .get(project_name)
            .with_context(|| format!("project {:?} not in projects.yaml", project_name))?;
        crate::paths::expand(&p.path)
    }

    pub fn resolved_agent(&self, project_name: &str) -> String {
        self.projects
            .get(project_name)
            .and_then(|p| p.agent.clone())
            .unwrap_or_else(|| self.defaults.agent.clone())
    }

    /// Default role for a project. `None` if the project doesn't declare
    /// one — callers should NOT fall back to a global default; "no role"
    /// is a meaningful state (e.g. _global tasks shouldn't randomly
    /// inherit a builder role).
    pub fn resolved_role(&self, project_name: &str) -> Option<String> {
        self.projects.get(project_name).and_then(|p| p.role.clone())
    }

    /// Resolve an adapter-neutral model name for a task, applying overrides
    /// task → project → defaults. `None` means the selected agent picks.
    pub fn resolved_agent_model(
        &self,
        project_name: &str,
        task_override: Option<&str>,
    ) -> Option<String> {
        if let Some(m) = task_override {
            if !m.trim().is_empty() {
                return Some(m.to_string());
            }
        }
        self.projects
            .get(project_name)
            .and_then(Project::effective_agent_model)
            .map(str::to_string)
            .or_else(|| self.defaults.effective_agent_model().map(str::to_string))
    }

    /// Legacy name retained for callers that still refer to cursor-specific
    /// model settings. Prefer `resolved_agent_model`.
    pub fn resolved_cursor_model(
        &self,
        project_name: &str,
        task_override: Option<&str>,
    ) -> Option<String> {
        self.resolved_agent_model(project_name, task_override)
    }

    /// Resolve the actual model for an agent task.
    ///
    /// Priority:
    ///
    /// 1. Task-level `model`
    /// 2. Run-level `--model`
    /// 3. Task-level `model_profile`
    /// 4. Project `model_profile`
    /// 5. Role-named profile, if `defaults.model_profiles.<role>` exists
    /// 6. Workspace `defaults.model_profile`
    /// 7. Project `agent_model` (or legacy `cursor_model`)
    /// 8. Workspace `defaults.agent_model` (or legacy `cursor_model`)
    pub fn resolved_task_model(
        &self,
        project_name: &str,
        task_override: Option<&str>,
        run_override: Option<&str>,
        task_profile: Option<&str>,
        role_name: Option<&str>,
    ) -> Option<String> {
        if let Some(model) = task_override.and_then(non_empty) {
            return Some(model.to_string());
        }
        if let Some(model) = run_override.and_then(non_empty) {
            return Some(model.to_string());
        }

        let project = self.projects.get(project_name);
        let mut profiles = Vec::new();
        if let Some(profile) = task_profile.and_then(non_empty) {
            profiles.push(profile);
        }
        if let Some(profile) = project
            .and_then(|p| p.model_profile.as_deref())
            .and_then(non_empty)
        {
            profiles.push(profile);
        }
        if let Some(role) = role_name
            .and_then(non_empty)
            .filter(|role| self.defaults.model_profiles.contains_key(*role))
        {
            profiles.push(role);
        }
        if let Some(profile) = self.defaults.model_profile.as_deref().and_then(non_empty) {
            profiles.push(profile);
        }

        for profile in profiles {
            if let Some(model) = self.resolve_model_profile(profile) {
                return Some(model);
            }
        }

        project
            .and_then(Project::effective_agent_model)
            .map(str::to_string)
            .or_else(|| self.defaults.effective_agent_model().map(str::to_string))
    }

    pub fn resolve_model_profile(&self, name: &str) -> Option<String> {
        let profile = self.defaults.model_profiles.get(name)?;
        let candidates = profile.candidates();
        if candidates.is_empty() {
            return None;
        }

        if !crate::models::cache_missing_or_empty() {
            let cached = crate::models::load_cached().unwrap_or_default();
            for candidate in &candidates {
                if let Some(model) = cached.iter().find(|m| {
                    m.id == *candidate || m.aliases.iter().any(|alias| alias == candidate)
                }) {
                    return Some(model.id.clone());
                }
            }
        }

        candidates.into_iter().next()
    }

    /// Tagger model: defaults.tagger_model → defaults.agent_model →
    /// defaults.cursor_model → None.
    pub fn resolved_tagger_model(&self) -> Option<String> {
        self.defaults
            .tagger_model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.defaults.effective_agent_model().map(str::to_string))
    }
}

struct TmpGuard {
    path: Option<PathBuf>,
}

impl TmpGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) fn projects_tmp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("projects.yaml"));
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    path.with_file_name(tmp_name)
}

fn non_empty(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_profile() -> ProjectsConfig {
        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Defaults::default(),
            projects: BTreeMap::new(),
        };
        cfg.defaults.model_profiles.insert(
            "deep".into(),
            ModelProfile {
                preferred: Some("gpt-5.2".into()),
                fallback: vec!["composer-2".into()],
            },
        );
        cfg.projects.insert(
            "api".into(),
            Project {
                path: "api".into(),
                r#type: None,
                stack: vec![],
                commands: BTreeMap::new(),
                contracts: Contracts::default(),
                dependencies: vec![],
                memory_scope: vec![],
                agent: None,
                agent_model: None,
                cursor_model: Some("project-model".into()),
                model_profile: Some("deep".into()),
                role: None,
                copy_files: Vec::new(),
            },
        );
        cfg
    }

    fn cfg_with_project(name: &str) -> ProjectsConfig {
        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Defaults::default(),
            projects: BTreeMap::new(),
        };
        cfg.projects.insert(
            name.into(),
            Project {
                path: name.into(),
                r#type: None,
                stack: vec![],
                commands: BTreeMap::new(),
                contracts: Contracts::default(),
                dependencies: vec![],
                memory_scope: vec![],
                agent: None,
                agent_model: None,
                cursor_model: None,
                model_profile: None,
                role: None,
                copy_files: Vec::new(),
            },
        );
        cfg
    }

    #[test]
    fn task_model_wins_over_profiles() {
        let cfg = cfg_with_profile();
        assert_eq!(
            cfg.resolved_task_model("api", Some("task-model"), Some("run-model"), None, None),
            Some("task-model".into())
        );
    }

    #[test]
    fn run_model_wins_over_profiles() {
        let cfg = cfg_with_profile();
        assert_eq!(
            cfg.resolved_task_model("api", None, Some("run-model"), None, None),
            Some("run-model".into())
        );
    }

    #[test]
    fn project_profile_wins_over_cursor_model() {
        let cfg = cfg_with_profile();
        assert_eq!(
            cfg.resolved_task_model("api", None, None, None, None),
            Some("gpt-5.2".into())
        );
    }

    #[test]
    fn agent_model_wins_over_legacy_cursor_model() {
        let mut cfg = cfg_with_profile();
        cfg.defaults.agent_model = Some("default-agent-model".into());
        cfg.defaults.cursor_model = Some("default-cursor-model".into());
        cfg.projects.get_mut("api").unwrap().model_profile = None;
        cfg.projects.get_mut("api").unwrap().agent_model = Some("project-agent-model".into());
        assert_eq!(
            cfg.resolved_task_model("api", None, None, None, None),
            Some("project-agent-model".into())
        );
    }

    #[test]
    fn legacy_cursor_model_still_deserializes() {
        let cfg: ProjectsConfig = serde_yaml::from_str(
            r#"
version: 1
defaults:
  agent: codex
  cursor_model: legacy-default
projects:
  api:
    path: ./api
    cursor_model: legacy-project
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.resolved_task_model("api", None, None, None, None),
            Some("legacy-project".into())
        );
        assert_eq!(cfg.resolved_tagger_model(), Some("legacy-default".into()));
    }

    #[test]
    fn load_rejects_denied_copy_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        std::fs::write(
            &path,
            r#"
version: 1
projects:
  api:
    path: api
    copy_files: [.env]
"#,
        )
        .unwrap();

        let err = ProjectsConfig::load(&path).unwrap_err();
        assert!(err.to_string().contains("worktree policy"));
    }

    #[test]
    fn save_replaces_sibling_tmp_and_leaves_only_canonical_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        let tmp_path = temp.path().join("projects.yaml.tmp");

        let cfg = cfg_with_project("api");
        cfg.save(&path).unwrap();

        assert!(path.exists());
        assert!(!tmp_path.exists());
        let loaded = ProjectsConfig::load(&path).unwrap();
        assert!(loaded.projects.contains_key("api"));
    }

    #[test]
    fn save_reports_named_concurrent_writer_when_tmp_persists() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        let tmp_path = temp.path().join("projects.yaml.tmp");
        std::fs::write(&tmp_path, "active writer").unwrap();

        let err = cfg_with_project("api").save(&path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("another maestro process is writing the registry"));
        assert!(message.contains("projects.yaml.tmp"));
        assert!(tmp_path.exists());
    }

    #[test]
    fn load_returns_canonical_content_even_with_lingering_tmp_sibling() {
        // Pre-deflake (commit a35077f, CI run 26572504911), this case
        // spawned a thread that slept 10ms then renamed TMP → canonical
        // while load() polled. The timing window was small (load's retry
        // budget is 2 × 50ms = ~100ms) and CI's loaded test runners
        // occasionally scheduled the rename thread past the budget,
        // failing the assertion in a non-deterministic way.
        //
        // The production invariant the test actually wants to pin: when
        // canonical has content, load returns that content even if a
        // sibling .tmp file is still lingering (a writer hasn't cleaned
        // up yet, a stale earlier write, etc.). That invariant is
        // deterministic and doesn't need thread scheduling — stage both
        // files up front and just read.
        //
        // The retry-budget-exhaustion path is covered separately by
        // load_returns_empty_cleanly_when_retry_exhausts_with_tmp_still_present.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        let tmp_path = temp.path().join("projects.yaml.tmp");
        std::fs::write(
            &path,
            serde_yaml::to_string(&cfg_with_project("api")).unwrap(),
        )
        .unwrap();
        std::fs::write(&tmp_path, "pending-writer-content").unwrap();

        let loaded = ProjectsConfig::load(&path).unwrap();
        assert!(loaded.projects.contains_key("api"));
        // TMP should not have been touched — the load path is read-only.
        assert!(tmp_path.exists());
    }

    #[test]
    fn load_does_not_retry_when_no_tmp_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        std::fs::write(&path, "").unwrap();

        let loaded = ProjectsConfig::load(&path).unwrap();
        assert!(loaded.projects.is_empty());
    }

    #[test]
    fn load_returns_empty_cleanly_when_retry_exhausts_with_tmp_still_present() {
        // Deterministic counterpart to `load_retries_when_canonical_is_empty_and_tmp_exists`.
        // No concurrent rename: load should walk through the retry budget and return
        // an empty cfg without spurious errors. Guards the retry-exhaustion path from
        // regressions, race-free.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        let tmp_path = temp.path().join("projects.yaml.tmp");
        std::fs::write(&path, "").unwrap();
        std::fs::write(&tmp_path, "pending writer content").unwrap();

        let loaded = ProjectsConfig::load(&path).unwrap();
        assert!(loaded.projects.is_empty());
        assert!(tmp_path.exists());
    }

    #[test]
    fn load_propagates_parse_error_without_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projects.yaml");
        let tmp_path = temp.path().join("projects.yaml.tmp");
        std::fs::write(&path, "projects:\n  - name:").unwrap();
        std::fs::write(
            &tmp_path,
            serde_yaml::to_string(&cfg_with_project("api")).unwrap(),
        )
        .unwrap();

        let err = ProjectsConfig::load(&path).unwrap_err();
        assert!(err.to_string().contains("parse projects.yaml"));
    }
}
