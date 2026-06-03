//! `maestro compare` — the Voting pattern. Run the same task on several agents
//! in isolated worktrees, run the project check on each, and pick the best
//! (passed the check + actually changed something). Every candidate is
//! reported so a human can override the auto-pick.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

use crate::adapter::{self, AgentTask, ExecutionMode};
use crate::cli::CompareArgs;
use crate::config::ProjectsConfig;
use crate::gitops;
use crate::paths;
use crate::scheduler::voting::{select_winner, VoteCandidate, VoteOutcome};

pub async fn compare(a: CompareArgs) -> Result<()> {
    if a.agents.len() < 2 {
        anyhow::bail!("compare needs at least two --agents (e.g. --agents codex,cursor)");
    }
    let projects = ProjectsConfig::load(&paths::projects_file()?)?;
    let project_dir = projects.resolved_path(&a.project)?;
    let proj = projects
        .projects
        .get(&a.project)
        .with_context(|| format!("unknown project {}", a.project))?;
    let check = proj
        .commands
        .get("check")
        .cloned()
        .context("project has no `check` command to vote on")?;
    let repo_root = gitops::worktree_root(&project_dir)
        .with_context(|| format!("{} is not inside a git repo", project_dir.display()))?;
    let rel = project_dir.strip_prefix(&repo_root).unwrap_or(&project_dir);

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let base = paths::maestro_dir()?.join("compare").join(&stamp);
    std::fs::create_dir_all(&base)?;

    println!(
        "→ compare: {} agent(s) on {} · check `{check}`\n",
        a.agents.len(),
        a.project
    );

    let mut candidates: Vec<VoteCandidate> = Vec::new();
    let mut worktrees: Vec<(String, PathBuf, PathBuf)> = Vec::new(); // agent, repo-worktree, proj-dir

    for agent in &a.agents {
        let wt = base.join(agent);
        let branch = format!("maestro/compare/{stamp}/{agent}");
        let proj_ws = match gitops::create_worktree(&repo_root, &wt, &branch) {
            Ok(_) => wt.join(rel),
            Err(e) => {
                println!("  · {agent:<10} worktree failed: {e:#}");
                candidates.push(VoteCandidate {
                    agent: agent.clone(),
                    verified: false,
                    files_changed: 0,
                    error: Some(format!("worktree: {e}")),
                });
                continue;
            }
        };

        let log_path = base.join(format!("{agent}.log"));
        let task = AgentTask {
            task_id: format!("compare-{agent}"),
            workspace: proj_ws.clone(),
            prompt: a.prompt.clone(),
            context: vec![],
            timeout: Duration::from_secs(a.timeout_minutes * 60),
            mode: ExecutionMode::Apply,
            resume_chat_id: None,
            log_path,
            trajectory: None,
            model: None,
            role_prelude: None,
            allowed_tools: Default::default(),
        };

        let mut error = None;
        if let Err(e) = adapter::pick(agent).run(task).await {
            error = Some(format!("{e:#}"));
            println!("  · {agent:<10} agent error: {e:#}");
        }
        let files_changed = gitops::changed_files(&proj_ws)
            .map(|f| f.len())
            .unwrap_or(0);
        let verified = error.is_none() && run_check(&check, &proj_ws).await;
        println!(
            "  · {agent:<10} check={}  files={files_changed}",
            if verified { "PASS" } else { "fail" }
        );
        candidates.push(VoteCandidate {
            agent: agent.clone(),
            verified,
            files_changed,
            error,
        });
        worktrees.push((agent.clone(), wt, proj_ws));
    }

    println!();
    let outcome = select_winner(&candidates);
    let winner_agent = match &outcome {
        VoteOutcome::Winner { index, reason } => {
            println!("🏆 winner — {reason}");
            Some(candidates[*index].agent.clone())
        }
        VoteOutcome::NoWinner { reason } => {
            println!("✗ no winner — {reason}");
            None
        }
    };

    // Keep the winner's worktree for inspection; clean up the rest unless --keep.
    for (agent, wt, proj_ws) in &worktrees {
        let is_winner = winner_agent.as_deref() == Some(agent);
        if is_winner {
            println!("   winner worktree: {}", proj_ws.display());
        } else if !a.keep {
            gitops::remove_worktree(&repo_root, wt);
        }
    }
    if a.keep {
        println!("   (--keep: all worktrees left under {})", base.display());
    }
    Ok(())
}

/// Run the project check from `cwd`; true on exit 0.
async fn run_check(cmd: &str, cwd: &std::path::Path) -> bool {
    tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(cmd)
        .current_dir(cwd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
