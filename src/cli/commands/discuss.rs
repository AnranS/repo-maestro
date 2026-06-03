//! LLM discussion mode for `maestro deliberate --discuss <backend>`.
//!
//! Each in-scope project's agent (codex / claude / cursor) is run via its local
//! CLI to state a position on the requirement; the takes are posted live to the
//! A2A mailbox so the discussion is visible in the coordination panel, then a
//! "lead" synthesis pass reads everyone's take and calls out cross-project risk.
//! The DAG ordering still comes from the deterministic contract-graph planner —
//! the LLM supplies the *reasoning*, not the dependency edges.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::Command;

use crate::config::{DeliberationReport, Position, ProjectsConfig};
use crate::mailbox::{MailDraft, MailboxStore};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscussBackend {
    Codex,
    Claude,
    Cursor,
}

impl DiscussBackend {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            "cursor" | "cursor-agent" => Ok(Self::Cursor),
            other => anyhow::bail!("unknown --discuss backend `{other}` (use codex|claude|cursor)"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }
}

/// Run the chosen agent CLI non-interactively on `prompt` in `cwd`, returning
/// its plain-text reply. These are read-and-opine calls (the agent doesn't edit
/// code), so we use each CLI's print/exec mode and capture stdout.
async fn ask_agent(backend: DiscussBackend, cwd: &Path, prompt: &str) -> Result<String> {
    let mut cmd;
    match backend {
        DiscussBackend::Claude => {
            cmd = Command::new("claude");
            cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");
        }
        DiscussBackend::Cursor => {
            cmd = Command::new("cursor-agent");
            cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");
        }
        DiscussBackend::Codex => {
            // codex reads the prompt from stdin; --skip-git-repo-check lets it
            // run outside a "trusted" repo without prompting.
            cmd = Command::new("codex");
            cmd.arg("exec").arg("--skip-git-repo-check").arg(prompt);
        }
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let out = cmd
        .output()
        .await
        .with_context(|| format!("run {} for discussion", backend.label()))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        anyhow::bail!("{} returned no output", backend.label());
    }
    Ok(text)
}

/// Resolve a project's working directory (for grounding the agent in its repo);
/// falls back to the workspace root if the path can't be resolved.
fn project_cwd(projects: &ProjectsConfig, project: &str) -> PathBuf {
    let root = paths::workspace_root().unwrap_or_else(|_| PathBuf::from("."));
    let Some(p) = projects.projects.get(project) else {
        return root;
    };
    match paths::expand(&p.path) {
        Ok(expanded) if expanded.is_absolute() => expanded,
        Ok(expanded) => root.join(expanded),
        Err(_) => root,
    }
}

fn position_prompt(spec: &str, pos: &Position) -> String {
    format!(
        "You are the engineer responsible for the project \"{project}\" (role: {role}) in a \
multi-repo workspace. A requirement just arrived.\n\n\
REQUIREMENT:\n{spec}\n\n\
YOUR PROJECT'S CONTRACTS:\n- provides: {provides}\n- consumes: {consumes}\n- \
you must land after: {deps}\n\n\
State your POSITION in 4-6 concise lines:\n\
1) What you will change in {project}.\n\
2) Any contract change you make and who it affects.\n\
3) What you must wait for.\n\
4) Your top risk/concern.\n\
Be specific and brief. Do NOT write code. Plain text only, no preamble.",
        project = pos.project,
        role = pos.role,
        spec = spec.trim(),
        provides = pos.provides.as_deref().unwrap_or("none"),
        consumes = pos.consumes.as_deref().unwrap_or("none"),
        deps = if pos.depends_on.is_empty() {
            "nothing".to_string()
        } else {
            pos.depends_on.join(", ")
        },
    )
}

fn lead_prompt(spec: &str, takes: &[(String, String, String)]) -> String {
    let mut s = String::from(
        "You are the tech lead. Below are the position statements from each project's engineer \
for this requirement. Synthesize them.\n\n",
    );
    s.push_str(&format!(
        "REQUIREMENT: {}\n\n",
        spec.trim().lines().next().unwrap_or("")
    ));
    s.push_str("POSITIONS:\n");
    for (project, role, take) in takes {
        s.push_str(&format!("### {project} ({role})\n{take}\n\n"));
    }
    s.push_str(
        "In 4-6 lines: state the agreed execution order, and call out any cross-project contract \
risk that must be resolved before merging. Plain text, no preamble.",
    );
    s
}

#[allow(clippy::too_many_arguments)]
fn post(
    store: &MailboxStore,
    from: &str,
    to: &str,
    project: Option<String>,
    backend: DiscussBackend,
    subject: String,
    body: String,
) {
    let _ = store.send(MailDraft {
        from: from.to_string(),
        to: to.to_string(),
        project,
        // Carry the backend that produced this take so the UI can badge the
        // avatar (claude / codex / cursor).
        task: Some(backend.label().to_string()),
        subject,
        body,
        blocking: false,
        ask: None,
    });
}

/// Run the live LLM discussion: each project states a position (posted to the
/// mailbox as it lands), then a lead synthesis. Enriches `report.positions`
/// summaries in place with the agents' own words and returns the lead summary.
/// Sequential per project so the coordination panel fills in visibly in order.
pub async fn run_discussion(
    backend: DiscussBackend,
    projects: &ProjectsConfig,
    report: &mut DeliberationReport,
    spec: &str,
) -> Result<String> {
    let store = MailboxStore::open().ok();
    let mut takes: Vec<(String, String, String)> = Vec::new();

    // Round 1 — positions, in resolved order so the discussion reads top-down.
    let order = report.order.clone();
    for project in &order {
        let Some(idx) = report.positions.iter().position(|p| &p.project == project) else {
            continue;
        };
        let pos = report.positions[idx].clone();
        let cwd = project_cwd(projects, project);
        println!(
            "→ [{}] {} is forming its position…",
            backend.label(),
            project
        );
        match ask_agent(backend, &cwd, &position_prompt(spec, &pos)).await {
            Ok(take) => {
                println!("  ✓ {project}:\n{}\n", indent(&take));
                if let Some(store) = &store {
                    post(
                        store,
                        &format!("{project}-agent"),
                        "lead",
                        Some(project.clone()),
                        backend,
                        format!("[discuss] {project} position ({})", pos.role),
                        take.clone(),
                    );
                }
                report.positions[idx].summary = take.clone();
                takes.push((project.clone(), pos.role.clone(), take));
            }
            Err(e) => {
                eprintln!("  ⚠️ {project} agent failed: {e:#} — keeping deterministic position");
                takes.push((project.clone(), pos.role.clone(), pos.summary.clone()));
            }
        }
    }

    // Round 2 — lead synthesis over all positions.
    println!("→ [{}] lead is synthesizing…", backend.label());
    let lead = match ask_agent(
        backend,
        &paths::workspace_root().unwrap_or_else(|_| ".".into()),
        &lead_prompt(spec, &takes),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  ⚠️ lead synthesis failed: {e:#}");
            format!("resolved order: {}", report.order.join(" → "))
        }
    };
    println!("  ✓ lead:\n{}\n", indent(&lead));
    if let Some(store) = &store {
        post(
            store,
            "lead",
            "team",
            None,
            backend,
            "[discuss] lead synthesis".to_string(),
            lead.clone(),
        );
    }
    Ok(lead)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
