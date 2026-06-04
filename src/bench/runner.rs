use crate::bench::fixture::{Fixture, FixtureKind};
use crate::config::{Plan, PlanTask, Project, ProjectsConfig, TaskKind, TaskOutput};
use crate::paths;
use crate::scheduler::{RunState, RunStatus, TaskStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BENCH_RESULT_FILE: &str = "bench-result.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BenchOptions {
    #[serde(default)]
    pub offline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRun {
    pub id: String,
    pub fixture_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_state: Option<RunState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BenchRun {
    pub fn run_dir(&self) -> Result<PathBuf> {
        Ok(paths::bench_runs_dir()?.join(&self.id))
    }

    pub fn write_result(&self) -> Result<PathBuf> {
        let run_dir = self.run_dir()?;
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("create bench run dir {run_dir:?}"))?;
        let path = run_dir.join(BENCH_RESULT_FILE);
        let text = serde_json::to_string_pretty(self).context("serialize bench result")?;
        fs::write(&path, text).with_context(|| format!("write bench result {path:?}"))?;
        Ok(path)
    }
}

pub fn ensure_upstream_checkout(fixture: &Fixture, offline: bool) -> Result<PathBuf> {
    let dst = paths::bench_cache_dir()?.join(&fixture.id);
    if dst.exists() {
        return Ok(dst);
    }
    if offline {
        // Keep "not cached and --offline set" stable: bench::classify_result
        // uses that phrase to distinguish cache misses from real failures.
        anyhow::bail!(
            "fixture {} not cached and --offline set; run `maestro bench hydrate --fixture {}` first or manually clone {} to {:?}",
            fixture.id,
            fixture.id,
            fixture
                .upstream
                .as_ref()
                .map(|upstream| upstream.url.as_str())
                .unwrap_or("(missing upstream url)"),
            dst
        );
    }

    let upstream = fixture
        .upstream
        .as_ref()
        .with_context(|| format!("fixture {} requires upstream checkout metadata", fixture.id))?;
    let parent = dst
        .parent()
        .with_context(|| format!("bench cache path has no parent: {dst:?}"))?;
    fs::create_dir_all(parent).with_context(|| format!("create bench cache dir {parent:?}"))?;

    run_git(
        Path::new("."),
        &["clone", "--depth", "50", "--no-tags", &upstream.url],
        Some(&dst),
        &format!(
            "clone fixture {} from {}; manually clone to {:?}",
            fixture.id, upstream.url, dst
        ),
    )?;
    run_git(
        &dst,
        &["checkout", &upstream.parent_sha],
        None,
        &format!(
            "checkout fixture {} parent {}; remove {:?} and retry",
            fixture.id, upstream.parent_sha, dst
        ),
    )?;
    Ok(dst)
}

pub async fn run_fixture(fixture: &Fixture, opts: &BenchOptions) -> Result<BenchRun> {
    fixture.validate()?;
    if matches!(fixture.kind, FixtureKind::OssReplay) {
        ensure_upstream_checkout(fixture, opts.offline)?;
    }

    let started_at = Utc::now();
    let plan = synthesize_fixture_plan(fixture);
    let mut run = BenchRun {
        id: format!("{}-{}", fixture.id, started_at.format("%Y%m%d-%H%M%S")),
        fixture_id: fixture.id.clone(),
        started_at,
        plan: Some(plan.clone()),
        run_state: None,
        error: None,
    };
    let run_dir = run.run_dir()?;
    fs::create_dir_all(&run_dir).with_context(|| format!("create bench run dir {run_dir:?}"))?;
    fs::write(
        run_dir.join(crate::bench::fixture::FIXTURE_FILE),
        serde_yaml::to_string(fixture).context("serialize fixture snapshot")?,
    )
    .with_context(|| format!("write fixture snapshot for {}", fixture.id))?;
    fs::write(
        run_dir.join(crate::paths::PLAN_SNAPSHOT),
        serde_yaml::to_string(&plan).context("serialize bench PLAN.yaml")?,
    )
    .with_context(|| format!("write bench PLAN.yaml for {}", fixture.id))?;
    fs::write(
        run_dir.join("stdout.log"),
        format!(
            "maestro bench replay fixture={} agent=mock dry_run=true\n",
            fixture.id
        ),
    )
    .with_context(|| format!("write bench stdout.log for {}", fixture.id))?;

    let projects = projects_for_plan(&plan);
    let mut state = RunState::new(run.id.clone(), &plan, &projects, 1, run_dir.clone());
    state.status = RunStatus::Done;
    state.ended_at = Some(Utc::now());
    for task in state.tasks.values_mut() {
        task.status = TaskStatus::Done;
        task.started_at = Some(started_at);
        task.ended_at = state.ended_at;
        task.artifacts.files_changed = fixture.expected.touched_files.clone();
    }
    state.verified = true;
    state.write_atomic()?;
    run.run_state = Some(state);
    run.write_result()?;
    Ok(run)
}

fn synthesize_fixture_plan(fixture: &Fixture) -> Plan {
    let mut projects = fixture.expected.required_projects.clone();
    for consumer in &fixture.expected.required_contract_consumers {
        if !projects.contains(consumer) {
            projects.push(consumer.clone());
        }
    }
    if projects.is_empty() {
        projects.push("_global".to_string());
    }

    let files_per_project = split_files(&fixture.expected.touched_files, projects.len());
    let tasks = projects
        .iter()
        .enumerate()
        .map(|(idx, project)| {
            let mut outputs = BTreeMap::new();
            for (file_idx, file) in files_per_project[idx].iter().enumerate() {
                outputs.insert(
                    format!("touched_{file_idx}"),
                    TaskOutput {
                        path: Some(file.clone()),
                        description: "benchmark expected touched file".to_string(),
                        required: false,
                        max_bytes: 64 * 1024,
                    },
                );
            }
            PlanTask {
                id: format!("bench_{}", project.replace('-', "_")),
                project: project.clone(),
                project_each: Vec::new(),
                kind: TaskKind::Agent,
                prompt: format!(
                    "Replay benchmark fixture `{}` for project `{project}`",
                    fixture.id
                ),
                command: None,
                depends_on: Vec::new(),
                parallel_group: None,
                requires_approval_after: false,
                timeout_minutes: fixture.budget.max_runtime_secs.div_ceil(60).max(1),
                memory_inject: Vec::new(),
                skills: Vec::new(),
                inputs: BTreeMap::new(),
                outputs,
                agent: Some("mock".to_string()),
                model: None,
                model_profile: None,
                role: None,
                review_by: None,
                agent_profile: None,
                review_profile: None,
            }
        })
        .collect();

    Plan {
        spec: fixture.goal.clone(),
        created_by: Some("maestro bench".to_string()),
        confirmed_at: Some(Utc::now()),
        contracts_change: Vec::new(),
        tasks,
        verification: BTreeMap::new(),
        notice: None,
        goal: None,
    }
}

fn split_files(files: &[String], buckets: usize) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new(); buckets.max(1)];
    for (idx, file) in files.iter().enumerate() {
        let bucket_count = out.len();
        out[idx % bucket_count].push(file.clone());
    }
    out
}

fn projects_for_plan(plan: &Plan) -> ProjectsConfig {
    let mut projects = BTreeMap::new();
    for task in &plan.tasks {
        if task.project == "_global" || projects.contains_key(&task.project) {
            continue;
        }
        projects.insert(
            task.project.clone(),
            Project {
                path: ".".to_string(),
                r#type: None,
                stack: Vec::new(),
                commands: BTreeMap::new(),
                contracts: Default::default(),
                dependencies: Vec::new(),
                memory_scope: Vec::new(),
                agent: Some("mock".to_string()),
                agent_model: None,
                cursor_model: None,
                model_profile: None,
                role: None,
                agent_profile: None,
                review_profile: None,
                copy_files: Vec::new(),
            },
        );
    }
    ProjectsConfig {
        version: 1,
        defaults: Default::default(),
        projects,
    }
}

fn run_git(cwd: &Path, args: &[&str], trailing_path: Option<&Path>, context: &str) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args);
    if let Some(path) = trailing_path {
        command.arg(path);
    }
    let output = command
        .output()
        .with_context(|| format!("run git {args:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            context,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::fixture::{Budget, Expected, Fixture, FixtureKind, Upstream};
    use serial_test::serial;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn fixture(id: &str, upstream: Upstream) -> Fixture {
        Fixture {
            id: id.to_string(),
            kind: FixtureKind::OssReplay,
            goal: "Replay a real PR".to_string(),
            upstream: Some(upstream),
            expected: Expected::default(),
            budget: Budget::default(),
        }
    }

    fn contract_fixture(id: &str) -> Fixture {
        Fixture {
            id: id.to_string(),
            kind: FixtureKind::ContractBreak,
            goal: "Catch an upstream contract break".to_string(),
            upstream: None,
            expected: Expected::default(),
            budget: Budget::default(),
        }
    }

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

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn make_upstream_repo() -> (TempDir, String) {
        let repo = TempDir::new().expect("upstream repo");
        git(repo.path(), &["init", "-q"]);
        fs::write(repo.path().join("README.md"), "hello\n").expect("write readme");
        git(repo.path(), &["add", "README.md"]);
        git(
            repo.path(),
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-q",
                "-m",
                "initial",
            ],
        );
        let sha = git(repo.path(), &["rev-parse", "HEAD"]);
        (repo, sha)
    }

    #[test]
    #[serial]
    fn ensure_upstream_checkout_returns_existing_cache_when_offline() {
        let workspace = with_workspace();
        let cached = crate::paths::bench_cache_dir()
            .expect("cache dir")
            .join("cached-fixture");
        fs::create_dir_all(&cached).expect("cached fixture dir");
        let fixture = fixture(
            "cached-fixture",
            Upstream {
                url: "https://example.invalid/repo.git".to_string(),
                parent_sha: "abc123".to_string(),
                pr_url: "https://example.invalid/pr/1".to_string(),
            },
        );

        let checkout = ensure_upstream_checkout(&fixture, true).expect("existing cache");

        assert_eq!(checkout, cached);
        assert!(workspace
            .path()
            .join(".maestro/bench/cache/cached-fixture")
            .exists());
        clear_workspace_env();
    }

    #[test]
    #[serial]
    fn ensure_upstream_checkout_clones_and_checks_out_parent_sha() {
        let _workspace = with_workspace();
        let (repo, parent_sha) = make_upstream_repo();
        let fixture = fixture(
            "clone-fixture",
            Upstream {
                url: repo.path().to_string_lossy().to_string(),
                parent_sha: parent_sha.clone(),
                pr_url: "https://example.invalid/pr/2".to_string(),
            },
        );

        let checkout = ensure_upstream_checkout(&fixture, false).expect("clone checkout");

        assert!(checkout.join("README.md").exists());
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), parent_sha);
        clear_workspace_env();
    }

    #[test]
    #[serial]
    fn ensure_upstream_checkout_rejects_offline_cache_miss_with_manual_hint() {
        let _workspace = with_workspace();
        let fixture = fixture(
            "missing-fixture",
            Upstream {
                url: "https://example.invalid/repo.git".to_string(),
                parent_sha: "abc123".to_string(),
                pr_url: "https://example.invalid/pr/3".to_string(),
            },
        );

        let error = ensure_upstream_checkout(&fixture, true).expect_err("offline miss");

        let message = format!("{error:#}");
        assert!(message.contains("--offline"));
        assert!(message.contains("maestro bench hydrate --fixture missing-fixture"));
        assert!(message.contains(".maestro/bench/cache/missing-fixture"));
        clear_workspace_env();
    }

    #[tokio::test]
    #[serial]
    async fn run_fixture_writes_bench_result_and_fixture_snapshot() {
        let _workspace = with_workspace();
        let fixture = contract_fixture("contract-fixture");

        let run = run_fixture(&fixture, &BenchOptions { offline: true })
            .await
            .expect("bench run");
        let run_dir = run.run_dir().expect("run dir");

        assert!(run_dir.join(BENCH_RESULT_FILE).exists());
        assert!(run_dir.join(crate::bench::fixture::FIXTURE_FILE).exists());
        assert_eq!(run.fixture_id, "contract-fixture");
        assert!(run_dir.join(crate::paths::PLAN_SNAPSHOT).exists());
        assert!(run_dir.join(crate::paths::RUN_STATE_FILE).exists());
        assert!(run.plan.is_some());
        assert!(run.run_state.is_some());
        clear_workspace_env();
    }
}
