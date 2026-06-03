use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::cli::PrDraftArgs;

pub async fn draft(args: PrDraftArgs) -> Result<()> {
    draft_with_gh(args, None).await
}

async fn draft_with_gh(args: PrDraftArgs, gh_override: Option<PathBuf>) -> Result<()> {
    let (_run_id, dir) = resolve_run_dir(args.run.as_deref())?;
    draft_with_gh_for_run_dir(args, gh_override, dir).await
}

async fn draft_with_gh_for_run_dir(
    args: PrDraftArgs,
    gh_override: Option<PathBuf>,
    dir: PathBuf,
) -> Result<()> {
    let mut state = crate::scheduler::RunState::load(&dir)?;
    let task_id = select_task(&state, args.project.as_deref())?;
    let task = state
        .tasks
        .get(&task_id)
        .cloned()
        .context("selected task disappeared")?;
    let branch = task
        .artifacts
        .branch
        .clone()
        .or_else(|| infer_branch(task.workspace_path.as_deref()))
        .unwrap_or_else(|| "HEAD".to_string());

    // Closing the PR loop: the head branch must exist on the remote before
    // `gh pr create`. maestro otherwise never pushes (integration branches are
    // local), so `--push` is what makes an auto-PR actually openable.
    if args.push && branch != "HEAD" {
        if let Some(ws) = task.workspace_path.as_deref() {
            let dir = Path::new(ws);
            if crate::gitops::has_remote(dir, "origin") {
                crate::gitops::push_branch(dir, "origin", &branch)
                    .with_context(|| format!("pushing {branch} to origin"))?;
                println!("pushed {branch} → origin");
            } else {
                eprintln!(
                    "  ⚠ --push: no `origin` remote in {ws}; skipping push (gh pr create may fail)"
                );
            }
        }
    }

    let body_path = crate::scheduler::evidence::write_pr_body(&state)?;
    let base = args.base.unwrap_or_else(|| "main".to_string());
    let title = format!("{}: {}", task.project, state.spec);

    let gh = gh_override
        .or_else(|| std::env::var_os("MAESTRO_GH").map(PathBuf::from))
        .or_else(|| crate::providers::find_binary("gh"));
    let Some(gh) = gh else {
        let body = std::fs::read_to_string(&body_path).unwrap_or_default();
        println!(
            "PR body saved to {}, paste manually:\n",
            body_path.display()
        );
        print!("{body}");
        return Ok(());
    };

    let args_vec = gh_args(&title, &body_path, &base, &branch, args.repo.as_deref());
    let output = Command::new(&gh)
        .args(&args_vec)
        .current_dir(task.workspace_path.as_deref().unwrap_or("."))
        .output()
        .await
        .with_context(|| format!("spawn {}", gh.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Some(task) = state.tasks.get_mut(&task_id) {
        task.artifacts.pr_url = (!url.is_empty()).then_some(url.clone());
    }
    state.write_atomic()?;
    if url.is_empty() {
        println!("draft PR created");
    } else {
        println!("draft PR: {url}");
    }
    Ok(())
}

fn gh_args(
    title: &str,
    body_path: &Path,
    base: &str,
    branch: &str,
    repo: Option<&str>,
) -> Vec<OsString> {
    let mut args = vec![
        "pr".into(),
        "create".into(),
        "--draft".into(),
        "--title".into(),
        title.into(),
        "--body-file".into(),
        body_path.as_os_str().to_os_string(),
        "--base".into(),
        base.into(),
        "--head".into(),
        branch.into(),
    ];
    if let Some(repo) = repo.filter(|r| !r.trim().is_empty()) {
        args.push("--repo".into());
        args.push(repo.into());
    }
    args
}

fn resolve_run_dir(run_id: Option<&str>) -> Result<(String, PathBuf)> {
    match run_id {
        None | Some("current") => {
            let dir = crate::paths::current_run_dir()?
                .ok_or_else(|| anyhow::anyhow!("no current run"))?;
            let id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("current")
                .to_string();
            Ok((id, dir))
        }
        Some(id) => {
            let dir = crate::paths::run_dir_for_id(id)?;
            if !dir.exists() {
                anyhow::bail!("run not found: {id}");
            }
            Ok((id.to_string(), dir))
        }
    }
}

fn select_task(state: &crate::scheduler::RunState, project: Option<&str>) -> Result<String> {
    let mut matches = state
        .task_order
        .iter()
        .filter_map(|id| state.tasks.get(id))
        .filter(|task| {
            task.status == crate::scheduler::TaskStatus::Done
                && project.is_none_or(|p| task.project == p)
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    matches.sort();
    match matches.len() {
        0 => anyhow::bail!("no completed task found for PR draft"),
        1 => Ok(matches.remove(0)),
        _ if project.is_none() => {
            anyhow::bail!("multiple completed tasks found; pass --project <name>")
        }
        _ => Ok(matches.remove(0)),
    }
}

fn infer_branch(workspace: Option<&str>) -> Option<String> {
    let workspace = workspace?;
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(workspace)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Plan, Project, ProjectsConfig};
    use crate::scheduler::{RunState, RunStatus, TaskStatus};
    use std::collections::BTreeMap;

    #[test]
    fn pr_draft_calls_gh_with_body_file() {
        let args = gh_args(
            "title",
            Path::new("/tmp/body.md"),
            "main",
            "feat/demo",
            Some("owner/repo"),
        );
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(rendered.contains(&"--draft".to_string()));
        assert!(rendered.contains(&"--body-file".to_string()));
        assert!(rendered.contains(&"/tmp/body.md".to_string()));
        assert!(rendered.contains(&"--repo".to_string()));
    }

    #[tokio::test]
    async fn pr_draft_invokes_gh_and_persists_url() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(".maestro").join("runs").join("run-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let workspace = tmp.path().join("api-worktree");
        std::fs::create_dir_all(&workspace).unwrap();

        let plan: Plan = serde_yaml::from_str(
            r#"
spec: Draft PR e2e
tasks:
  - id: T_api
    project: api
    kind: verify
    agent: shell
    command: echo ok
"#,
        )
        .unwrap();
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects.projects.insert(
            "api".into(),
            Project {
                path: workspace.to_string_lossy().to_string(),
                r#type: None,
                stack: vec![],
                commands: BTreeMap::new(),
                contracts: Default::default(),
                dependencies: vec![],
                memory_scope: vec![],
                agent: Some("shell".into()),
                agent_model: None,
                cursor_model: None,
                model_profile: None,
                role: None,
                copy_files: Vec::new(),
            },
        );
        let mut state = RunState::new("run-1".into(), &plan, &projects, 1, run_dir.clone());
        state.status = RunStatus::Done;
        if let Some(task) = state.tasks.get_mut("T_api") {
            task.status = TaskStatus::Done;
            task.workspace_path = Some(workspace.to_string_lossy().to_string());
            task.worktree_path = Some(workspace.to_string_lossy().to_string());
            task.artifacts.branch = Some("maestro/run-1/T_api".into());
        }
        state.write_atomic().unwrap();

        let gh = tmp.path().join("gh");
        let args_path = tmp.path().join("gh-args.txt");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho https://github.com/acme/api/pull/7\n",
                args_path.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&gh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&gh, perms).unwrap();
        }

        draft_with_gh_for_run_dir(
            PrDraftArgs {
                run: Some("run-1".into()),
                project: Some("api".into()),
                repo: Some("acme/api".into()),
                base: Some("main".into()),
                push: false,
            },
            Some(gh),
            run_dir.clone(),
        )
        .await
        .unwrap();

        let args = std::fs::read_to_string(args_path).unwrap();
        assert!(args.contains("--draft"));
        assert!(args.contains("--body-file"));
        assert!(args.contains("--repo"));
        assert!(args.contains("acme/api"));
        assert!(args.contains("--head"));
        assert!(args.contains("maestro/run-1/T_api"));

        let updated = RunState::load(&run_dir).unwrap();
        assert_eq!(
            updated.tasks["T_api"].artifacts.pr_url.as_deref(),
            Some("https://github.com/acme/api/pull/7")
        );
    }
}
