//! `--dry` execution: walk the same task graph, but instead of dispatching
//! agents, render each task's fully-assembled prompt (with memory + skills
//! + contracts) and write it to `.maestro/runs/<id>/dry/<task>.prompt.md`.
//!
//! This is the "see what would be sent before paying for it" mode. It
//! intentionally lives next to `executor.rs` and shares as much context
//! assembly logic as possible.

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

use crate::adapter::{join_prompt_prelude, render_prompt_full};
use crate::config::{Plan, ProjectsConfig, TaskKind};
use crate::memory::MemoryStore;
use crate::paths;

/// Result summary returned to the CLI.
pub struct DryRunSummary {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub task_count: usize,
    /// (task_id, output_file) for every task we rendered.
    pub files: Vec<(String, PathBuf)>,
}

/// Render every task's prompt and dump it to disk. Returns the run dir + a
/// per-task listing the caller can print.
pub fn dry_run(plan: &Plan, projects: &ProjectsConfig) -> Result<DryRunSummary> {
    dry_run_with_roots(plan, projects, MemoryStore::open()?, paths::runs_dir()?)
}

pub fn dry_run_in_workspace(
    plan: &Plan,
    projects: &ProjectsConfig,
    workspace_root: &Path,
) -> Result<DryRunSummary> {
    let maestro_dir = workspace_root.join(paths::MAESTRO_DIR);
    let memory = MemoryStore::open_at(maestro_dir.join("memory"))?;
    dry_run_with_roots(plan, projects, memory, maestro_dir.join(paths::RUNS_DIR))
}

fn dry_run_with_roots(
    plan: &Plan,
    projects: &ProjectsConfig,
    memory: MemoryStore,
    runs_dir: PathBuf,
) -> Result<DryRunSummary> {
    let run_id = format!(
        "dry-{}_{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let run_dir = runs_dir.join(&run_id);
    let dry_dir = run_dir.join("dry");
    paths::ensure_dir(&dry_dir)?;

    // Snapshot the plan beside the prompts so the user can diff later.
    std::fs::write(
        run_dir.join(paths::PLAN_SNAPSHOT),
        serde_yaml::to_string(plan)?,
    )
    .context("write PLAN snapshot")?;

    let mut files = Vec::with_capacity(plan.tasks.len());

    for task in &plan.tasks {
        // Adapter selection: same priority order as the real executor.
        let adapter_name = task.agent.clone().unwrap_or_else(|| match task.kind {
            TaskKind::Verify => "shell".to_string(),
            TaskKind::Agent => {
                if task.project == "_global" {
                    projects.defaults.agent.clone()
                } else {
                    projects.resolved_agent(&task.project)
                }
            }
        });

        let resolved_role_name = crate::roles::resolve_for_task(
            task.role.as_deref(),
            projects.resolved_role(&task.project).as_deref(),
        );

        // Model resolution: task > model profile > project/default > account.
        let model = projects.resolved_task_model(
            &task.project,
            task.model.as_deref(),
            None,
            task.model_profile.as_deref(),
            resolved_role_name.as_deref(),
        );

        // Memory topics: same precedence rule as the executor.
        let topics: Vec<String> = if !task.memory_inject.is_empty() {
            task.memory_inject.iter().map(|m| m.topic.clone()).collect()
        } else {
            projects
                .projects
                .get(&task.project)
                .map(|p| p.memory_scope.clone())
                .unwrap_or_default()
        };
        let context = if matches!(task.kind, TaskKind::Agent) {
            memory.load_for_topics(&topics).unwrap_or_default()
        } else {
            vec![]
        };

        // The actual prompt text the adapter would receive.
        let raw_prompt = match task.kind {
            TaskKind::Agent => task.prompt.clone(),
            TaskKind::Verify => task.command.clone().unwrap_or_else(|| task.prompt.clone()),
        };
        // Surface explicit + trigger-matched skills too, so dry output
        // captures exactly what an agent would have seen.
        let injected_skills = if matches!(task.kind, TaskKind::Agent) {
            crate::skills::resolve_for_task(&raw_prompt, Some(&task.project), &task.skills)?
        } else {
            Vec::new()
        };
        let skill_section = crate::skills::render_task_section(&injected_skills);
        let role_prelude = resolved_role_name
            .as_deref()
            .and_then(crate::roles::try_load)
            .as_ref()
            .map(crate::roles::render_section)
            .filter(|s| !s.is_empty());
        let project_instructions = if matches!(task.kind, TaskKind::Agent) {
            crate::project_instructions::load_for_task(projects, &task.project)
        } else {
            Vec::new()
        };
        let instruction_section =
            crate::project_instructions::render_section(&project_instructions);
        // Mirror the executor: deliver mailbox messages addressed to this task
        // so `--dry` previews exactly what the agent would read (closing the
        // multi-agent loop). See docs/design/multi-agent-collaboration.md.
        let inbox_section = if matches!(task.kind, TaskKind::Agent) {
            let role_id = resolved_role_name.clone().unwrap_or_default();
            let identities: Vec<&str> = [task.project.as_str(), role_id.as_str(), task.id.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            let messages = crate::mailbox::MailboxStore::open()
                .ok()
                .and_then(|store| store.inbox_for(&identities).ok())
                .unwrap_or_default();
            let audience = match resolved_role_name.as_deref() {
                Some(role) => format!("project `{}`, role `{}`", task.project, role),
                None => format!("project `{}`", task.project),
            };
            crate::mailbox::render_inbox(&messages, &audience)
        } else {
            None
        };
        // codegraph: mirror the executor so `--dry` previews the injected code
        // context too.
        let code_context_section = if matches!(task.kind, TaskKind::Agent) {
            let keep: Vec<String> = projects
                .projects
                .get(&task.project)
                .map(|p| {
                    p.contracts
                        .provides
                        .iter()
                        .chain(p.contracts.consumes.iter())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            projects
                .resolved_path(&task.project)
                .ok()
                .and_then(|repo| super::executor::build_code_context(&repo, &raw_prompt, &keep))
        } else {
            None
        };
        let prompt_prelude = join_prompt_prelude(&[
            instruction_section,
            code_context_section,
            inbox_section,
            role_prelude,
            skill_section,
        ]);
        let prompt_with_ctx = render_prompt_full(&raw_prompt, &context, prompt_prelude.as_deref());

        // Build a human-readable artifact: yaml header + the full prompt.
        let mut body = String::new();
        body.push_str("---\n");
        body.push_str(&format!("task_id: {}\n", task.id));
        body.push_str(&format!("project: {}\n", task.project));
        body.push_str(&format!("kind: {:?}\n", task.kind));
        body.push_str(&format!("adapter: {adapter_name}\n"));
        body.push_str(&format!(
            "model: {}\n",
            model.as_deref().unwrap_or("(adapter default)")
        ));
        if !context.is_empty() {
            body.push_str("memory_injected:\n");
            for slice in &context {
                body.push_str(&format!("  - {}\n", slice.topic));
            }
        }
        if !injected_skills.is_empty() {
            body.push_str("skills_injected:\n");
            for s in &injected_skills {
                body.push_str(&format!("  - {}/{}\n", s.scope.dir_name(), s.name));
            }
        }
        if !project_instructions.is_empty() {
            body.push_str("agents_instructions:\n");
            for instruction in &project_instructions {
                body.push_str(&format!("  - {}\n", instruction.source));
            }
        }
        body.push_str("---\n\n");
        body.push_str("# Prompt (what the adapter would receive)\n\n");
        body.push_str("```\n");
        body.push_str(&prompt_with_ctx);
        body.push_str("\n```\n");

        let path = dry_dir.join(format!("{}.prompt.md", sanitize_id(&task.id)));
        std::fs::write(&path, body).with_context(|| format!("write dry-run prompt {path:?}"))?;
        files.push((task.id.clone(), path));
    }

    Ok(DryRunSummary {
        run_id,
        run_dir,
        task_count: plan.tasks.len(),
        files,
    })
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
