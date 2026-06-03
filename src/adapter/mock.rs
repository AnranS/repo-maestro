use super::{AgentAdapter, AgentResult, AgentTask, Artifacts, Capability};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Fake adapter that just sleeps and writes a stub log.
/// Useful for testing the scheduler end-to-end without burning Cursor credits.
pub struct MockAdapter {
    pub sleep: Duration,
    pub behavior: MockBehavior,
}

#[derive(Debug, Clone)]
pub enum MockBehavior {
    Ok,
    Fail(String),
    ReplayDiff { from: PathBuf },
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self {
            sleep: Duration::from_millis(800),
            behavior: MockBehavior::Ok,
        }
    }
}

#[async_trait]
impl AgentAdapter for MockAdapter {
    fn name(&self) -> &str {
        "mock"
    }

    async fn run(&self, task: AgentTask) -> Result<AgentResult> {
        let mut log = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&task.log_path)
            .await?;

        log.write_all(
            format!(
                "[mock] task={} workspace={:?}\n[mock] model: {}\n[mock] context slices: {}\n",
                task.task_id,
                task.workspace,
                task.model.as_deref().unwrap_or("(none)"),
                task.context.len()
            )
            .as_bytes(),
        )
        .await
        .ok();
        for slice in &task.context {
            log.write_all(format!("[mock]   - topic={}\n", slice.topic).as_bytes())
                .await
                .ok();
        }
        log.write_all(format!("[mock] prompt:\n{}\n", task.prompt).as_bytes())
            .await
            .ok();

        let steps = 4;
        for i in 1..=steps {
            tokio::time::sleep(self.sleep / steps).await;
            log.write_all(format!("[mock] step {i}/{steps}\n").as_bytes())
                .await
                .ok();
        }

        log.write_all(b"[mock] done\n").await.ok();

        match &self.behavior {
            MockBehavior::Ok => {}
            MockBehavior::Fail(message) => {
                anyhow::bail!("{message}");
            }
            MockBehavior::ReplayDiff { from } => {
                let files_changed = apply_replay_diff(&task.workspace, from)?;
                let changed_count = files_changed.len();
                log.write_all(format!("[mock] applied diff: {changed_count} files\n").as_bytes())
                    .await
                    .ok();
                return Ok(AgentResult {
                    chat_id: Some(format!("mock-{}", task.task_id)),
                    artifacts: Artifacts {
                        pr_url: None,
                        branch: None,
                        files_changed,
                    },
                    transcript_summary: format!("applied diff: {changed_count} files"),
                    usage: None,
                    steps: None,
                });
            }
        }

        Ok(AgentResult {
            chat_id: Some(format!("mock-{}", task.task_id)),
            artifacts: Artifacts {
                pr_url: Some(format!("https://example.invalid/pr/{}", task.task_id)),
                branch: Some(format!("maestro/mock/{}", task.task_id)),
                files_changed: vec![],
            },
            transcript_summary: format!("mock completed task {}", task.task_id),
            usage: None,
            steps: None,
        })
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(cap, Capability::StreamOutput)
    }
}

fn apply_replay_diff(workspace: &Path, from: &Path) -> Result<Vec<String>> {
    let patches = replay_diff_files(from)?;
    for patch in patches {
        run_git_apply(workspace, &patch, true)?;
        run_git_apply(workspace, &patch, false)?;
    }
    changed_files(workspace)
}

fn replay_diff_files(from: &Path) -> Result<Vec<PathBuf>> {
    if from.is_file() {
        return Ok(vec![from.to_path_buf()]);
    }
    let mut patches = Vec::new();
    for entry in fs::read_dir(from).with_context(|| format!("read replay diff dir {from:?}"))? {
        let path = entry?.path();
        let is_patch = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext, "diff" | "patch"));
        if is_patch {
            patches.push(path);
        }
    }
    patches.sort();
    if patches.is_empty() {
        anyhow::bail!("no .diff or .patch files found in {from:?}");
    }
    Ok(patches)
}

fn run_git_apply(workspace: &Path, patch: &Path, check: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(workspace).arg("apply");
    if check {
        command.arg("--check");
    }
    command.arg(patch);
    let output = command
        .output()
        .with_context(|| format!("run git apply for {patch:?}"))?;
    if !output.status.success() {
        let mode = if check { "--check" } else { "" };
        anyhow::bail!(
            "git apply {mode} failed for {patch:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn changed_files(workspace: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["diff", "--name-only"])
        .output()
        .context("run git diff --name-only")?;
    if !output.status.success() {
        anyhow::bail!(
            "git diff --name-only failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect())
}
