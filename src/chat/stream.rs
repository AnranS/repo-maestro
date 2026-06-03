//! Stream a chat reply from `cursor-agent --output-format stream-json
//! --stream-partial-output`, parsing ndjson lines and forwarding delta chunks
//! as plain text events to the caller.

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::config::ProjectsConfig;
use crate::memory::MemoryStore;
use crate::paths;
use crate::scheduler::RunState;

use super::parse_actions;
use super::providers::{self, ChatRequest};
use super::sessions::{save, update_title_from_first_message, Message, Role, Session};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Sent once before any text deltas; carries the assistant message id.
    Meta {
        message_id: String,
        session_id: String,
    },
    /// Incremental visible text the assistant wants to render.
    Delta {
        text: String,
    },
    /// Thinking-mode content from reasoning models (e.g. Claude Opus
    /// `-thinking-*` variants). NOT rendered as the answer — kept in a
    /// separate channel so the UI can show "the model is thinking…"
    /// with an expandable trace, while the visible bubble stays empty
    /// until real `delta` events arrive.
    Thinking {
        text: String,
    },
    Done {
        message: Message,
    },
    Error {
        message: String,
    },
}

/// Run a turn against the given session. Persists the user message + the final
/// assistant message. Emits StreamEvents over the channel as they arrive.
///
/// Returns when the cursor-agent process exits.
pub async fn send_streaming(
    session: Session,
    user_text: String,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    send_streaming_with_model(session, user_text, None, tx).await
}

/// Like `send_streaming` but with an optional per-call model override that
/// takes priority over the session's pinned model and the global default.
pub async fn send_streaming_with_model(
    session: Session,
    user_text: String,
    model_override: Option<String>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    send_streaming_with_options(session, user_text, model_override, None, tx).await
}

pub async fn send_streaming_with_options(
    mut session: Session,
    user_text: String,
    model_override: Option<String>,
    provider_override: Option<String>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let is_first = session.messages.is_empty();
    let user_msg = Message::new(Role::User, user_text.clone());
    session.messages.push(user_msg);
    update_title_from_first_message(&mut session, &user_text);
    save(&session)?;

    // Build the prompt that actually goes to cursor-agent.
    let projects = load_projects_safe();
    let topics = list_memory_topics_safe();
    let skills = crate::skills::index_all();
    let status_snippet = render_status_snippet().unwrap_or_default();
    let triggered = trigger_inject(&user_text);
    let prompt = if is_first {
        format!(
            "{}\n\n{}\n{}\n# Your turn\n\n{}",
            render_system_prelude(&projects, &topics, &skills),
            status_snippet,
            triggered,
            user_text
        )
    } else {
        // For follow-up turns, still refresh the live status block AND a
        // compact workspace recap so the model doesn't hallucinate project
        // names. Providers that resume their own session (cursor-agent
        // --resume) effectively re-see the turn-1 prelude, but providers
        // that don't (claude --print) get a fresh process every turn and
        // need at least the project registry restated. The recap is much
        // smaller than the full prelude (~3KB for 65 projects vs ~12KB)
        // so prompt caching still wins.
        format!(
            "{}\n{}\n{}\n{}",
            status_snippet,
            render_workspace_recap(&projects),
            triggered,
            user_text
        )
    };

    let assistant_msg = Message::new(Role::Assistant, String::new());
    let assistant_msg_id = assistant_msg.id.clone();
    let session_id_for_meta = session.id.clone();
    tx.send(StreamEvent::Meta {
        message_id: assistant_msg_id.clone(),
        session_id: session_id_for_meta,
    })
    .await
    .ok();

    let provider_id = provider_override
        .filter(|p| !p.trim().is_empty())
        .or_else(|| {
            session
                .chat_provider
                .clone()
                .filter(|p| !p.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("MAESTRO_CHAT_PROVIDER")
                .ok()
                .filter(|p| !p.trim().is_empty())
        })
        .or_else(|| crate::config::Settings::load().chat.default_provider)
        .unwrap_or_else(|| providers::default().id().to_string());
    let provider = providers::resolve(&provider_id)
        .with_context(|| format!("unknown chat provider `{provider_id}`"))?;

    // Resolve effective model: override -> session-pinned -> defaults.agent_model.
    let effective_model: Option<String> = model_override
        .filter(|m| !m.trim().is_empty())
        .or_else(|| {
            session
                .cursor_model
                .clone()
                .filter(|m| !m.trim().is_empty())
        })
        .or_else(|| {
            load_projects_safe()
                .defaults
                .effective_agent_model()
                .map(str::to_string)
        });

    let response = provider
        .stream(
            ChatRequest {
                session_id: session.id.clone(),
                provider_session_id: if provider.id() == "cursor" {
                    session.cursor_chat_id.clone()
                } else {
                    None
                },
                model: effective_model,
                prompt,
                include_thinking: provider.supports_thinking(),
                workspace: paths::workspace_root()?,
            },
            tx.clone(),
        )
        .await?;

    if provider.id() == "cursor" && session.cursor_chat_id.is_none() {
        session.cursor_chat_id = response.provider_session_id;
    }

    session.chat_provider = Some(provider.id().to_string());

    let actions = parse_actions(&response.full_text);
    let mut assistant_msg = Message {
        id: assistant_msg_id.clone(),
        role: Role::Assistant,
        content: response.full_text,
        timestamp: chrono::Utc::now(),
        actions,
        thinking: (!response.thinking.is_empty()).then_some(response.thinking),
    };
    // Keep ids stable.
    assistant_msg.id = assistant_msg_id;
    session.messages.push(assistant_msg.clone());
    session.updated_at = chrono::Utc::now();
    save(&session)?;

    tx.send(StreamEvent::Done {
        message: assistant_msg,
    })
    .await
    .ok();

    Ok(())
}

fn load_projects_safe() -> ProjectsConfig {
    paths::projects_file()
        .ok()
        .and_then(|p| if p.exists() { Some(p) } else { None })
        .and_then(|p| ProjectsConfig::load(&p).ok())
        .unwrap_or_else(|| ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        })
}

fn list_memory_topics_safe() -> Vec<String> {
    MemoryStore::open()
        .and_then(|s| s.list_l1())
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
}

fn render_system_prelude(
    projects: &ProjectsConfig,
    memory_topics: &[String],
    skills: &[crate::skills::SkillSummary],
) -> String {
    let mut s = String::new();
    s.push_str("# System (maestro orchestrator)\n\n");
    s.push_str("You are the orchestrator for `maestro`, a multi-project DAG executor.\n\n");
    s.push_str("Hard rules:\n");
    s.push_str("1. Only use projects from the registry below. Never invent names.\n");
    s.push_str("2. API/contract changes go on the server task first, with `requires_approval_after: true`.\n");
    s.push_str(
        "3. Mock/SDK regeneration runs after the contract is locked, before client tasks.\n",
    );
    s.push_str("4. Tasks consuming the same contract version can share a `parallel_group`.\n");
    s.push_str(
        "5. Every plan ends with a single `kind: verify` task with a concrete `command:`.\n",
    );
    s.push_str("6. ≤ 20 tasks per plan.\n\n");
    s.push_str(
        "**Tool-use**: when the user should run a `maestro` command, embed an action block:\n\n",
    );
    s.push_str("```maestro-action\n");
    s.push_str("verb: work | run | approve | status | rerun | plan_validate\n");
    s.push_str("# fields depend on verb, e.g.:\n");
    s.push_str("# spec: Add JSON output     # for work\n");
    s.push_str("plan: plans/foo.yaml\n");
    s.push_str("# plan_hash: fnv1a64:... # optional stale-plan guard from `maestro plan hash`\n");
    s.push_str("# task: T1_xxx          # for approve\n");
    s.push_str("# from: T3_xxx          # optional for rerun\n");
    s.push_str("# root: ./apps          # optional work scan dir\n");
    s.push_str("```\n\n");
    s.push_str("The UI will offer a \"Run\" confirm button for each action block. Never claim you've executed something — only the user can approve.\n\n");

    s.push_str("## Project registry\n\n```yaml\n");
    if projects.projects.is_empty() {
        s.push_str("# (no projects - suggest `maestro work \"<goal>\" --root <dir>`)\n");
    } else {
        for (name, p) in &projects.projects {
            s.push_str(&format!("- name: {name}\n"));
            if let Some(t) = &p.r#type {
                s.push_str(&format!("  type: {t}\n"));
            }
            if !p.stack.is_empty() {
                s.push_str(&format!("  stack: [{}]\n", p.stack.join(", ")));
            }
            if !p.memory_scope.is_empty() {
                s.push_str(&format!(
                    "  memory_scope: [{}]\n",
                    p.memory_scope.join(", ")
                ));
            }
        }
    }
    s.push_str("```\n\n");

    s.push_str("## Memory topics\n");
    if memory_topics.is_empty() {
        s.push_str("_(none — files in `.maestro/memory/l1_facts/<topic>/`)_\n");
    } else {
        for t in memory_topics {
            s.push_str(&format!("- `{t}`\n"));
        }
    }

    s.push_str("\n## Skills available\n");
    if skills.is_empty() {
        s.push_str("_(none — add markdown files under `.maestro/skills/_global/` or `.maestro/skills/<project>/`)_\n");
    } else {
        for sk in skills {
            let scope_tag = match &sk.scope {
                crate::skills::SkillScope::Global => "global".to_string(),
                crate::skills::SkillScope::Project(p) => p.clone(),
            };
            let desc = sk.description.as_deref().unwrap_or("(no description)");
            s.push_str(&format!("- `{}` [{}] — {desc}\n", sk.name, scope_tag));
            if let Some(t) = &sk.trigger {
                s.push_str(&format!("    trigger: {t}\n"));
            }
        }
        s.push_str(
            "\nWhen you want to follow a skill, name it and the user can fetch its full body.\n",
        );
    }
    s
}

/// Compact mid-turn workspace summary: project names + type tag, one per
/// line. Designed to keep stateless providers (claude --print) from
/// hallucinating project names without re-paying for the full system
/// prelude every turn. Bounded by truncation to keep the prompt cheap.
///
/// Format is deliberately strict ("Only refer to projects listed below.
/// Do not invent names.") because the first-turn prelude carries the
/// same rule + the model has shown it'll forget without the reminder.
fn render_workspace_recap(projects: &ProjectsConfig) -> String {
    if projects.projects.is_empty() {
        return String::new();
    }
    let mut s = String::from("# Workspace recap (registered projects)\n\n");
    s.push_str("Only refer to projects listed below. Do not invent names.\n\n");
    s.push_str("```yaml\n");
    let cap = 80; // enough for the real ~65-project monorepo case
    for (i, (name, p)) in projects.projects.iter().enumerate() {
        if i >= cap {
            s.push_str(&format!("# … and {} more\n", projects.projects.len() - cap));
            break;
        }
        match &p.r#type {
            Some(t) => s.push_str(&format!("- {name}  # {t}\n")),
            None => s.push_str(&format!("- {name}\n")),
        }
    }
    s.push_str("```\n");
    s
}

/// Scan the user's message for any skill's `trigger` phrases and, if matched,
/// inline that skill's full body as additional context for this turn.
fn trigger_inject(user_text: &str) -> String {
    let matches = crate::skills::trigger_match(user_text, None).unwrap_or_default();
    if matches.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n# Triggered skills (full body inlined for this turn)\n\n");
    for s in matches {
        out.push_str(&format!(
            "## skill: {} ({})\n\n{}\n\n",
            s.name,
            match &s.scope {
                crate::skills::SkillScope::Global => "global".to_string(),
                crate::skills::SkillScope::Project(p) => p.clone(),
            },
            s.content.trim()
        ));
    }
    out
}

fn render_status_snippet() -> Result<String> {
    let Some(run_dir) = paths::current_run_dir()? else {
        return Ok(String::from("# Current run status\n_(no run yet)_"));
    };
    let Ok(state) = RunState::load(&run_dir) else {
        return Ok(String::from("# Current run status\n_(no run yet)_"));
    };

    let mut out = String::new();
    out.push_str("# Current run status\n\n");
    out.push_str(&format!("- run id: `{}`\n", state.run_id));
    out.push_str(&format!("- spec: {}\n", state.spec));
    out.push_str(&format!("- overall: {:?}\n", state.status));

    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for t in state.tasks.values() {
        *counts
            .entry(format!("{:?}", t.status).to_lowercase())
            .or_insert(0) += 1;
    }
    out.push_str("- task counts:");
    for (k, v) in &counts {
        out.push_str(&format!(" {k}={v}"));
    }
    out.push('\n');

    if !state.approvals_pending.is_empty() {
        out.push_str(&format!(
            "- **awaiting approval**: {}\n",
            state.approvals_pending.join(", ")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! Pin the shape of the system prelude that gets prepended to every
    //! chat turn. The prelude is a contract with the LLM: the orchestrator
    //! relies on the model seeing project names, memory topics, and
    //! available skills in a stable place. Drift here would silently break
    //! every chat run.
    use super::*;
    use crate::config::{Contracts, Project, ProjectsConfig};
    use crate::skills::{SkillScope, SkillSummary};
    use std::collections::BTreeMap;

    fn sample_projects(names: &[&str]) -> ProjectsConfig {
        let mut p = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        for n in names {
            p.projects.insert(
                (*n).to_string(),
                Project {
                    path: format!("./{n}"),
                    r#type: Some("backend".into()),
                    stack: vec!["python".into(), "fastapi".into()],
                    commands: BTreeMap::new(),
                    contracts: Contracts::default(),
                    dependencies: Vec::new(),
                    memory_scope: vec!["api".into()],
                    agent: None,
                    agent_model: None,
                    cursor_model: None,
                    model_profile: None,
                    role: None,
                    copy_files: Vec::new(),
                },
            );
        }
        p
    }

    fn skill_summary(name: &str, scope: SkillScope, desc: &str, trigger: &str) -> SkillSummary {
        SkillSummary {
            name: name.to_string(),
            scope,
            description: Some(desc.to_string()),
            trigger: Some(trigger.to_string()),
        }
    }

    // ─── shape: always-present sections ─────────────────────────────────

    #[test]
    fn prelude_always_states_orchestrator_role_and_hard_rules() {
        let out = render_system_prelude(&sample_projects(&[]), &[], &[]);
        // Role
        assert!(out.contains("multi-project DAG executor"));
        // The 6 hard rules
        assert!(out.contains("1. Only use projects from the registry"));
        assert!(out.contains("`requires_approval_after: true`"));
        assert!(out.contains("`parallel_group`"));
        assert!(out.contains("`kind: verify`"));
        assert!(out.contains("≤ 20 tasks per plan"));
        // Action protocol cheat sheet
        assert!(out.contains("```maestro-action"));
        assert!(out.contains("verb: work | run | approve"));
    }

    #[test]
    fn prelude_keeps_action_surface_small() {
        let out = render_system_prelude(&sample_projects(&[]), &[], &[]);
        assert!(out.contains("verb: work | run | approve | status | rerun | plan_validate"));
        assert!(!out.contains("verb: scaffold"));
        assert!(!out.contains("## Architect mode"));
    }

    // ─── projects registry ───────────────────────────────────────────────

    #[test]
    fn project_registry_lists_every_project_with_stack_and_memory_scope() {
        let out = render_system_prelude(&sample_projects(&["login-api", "login-web"]), &[], &[]);
        assert!(out.contains("## Project registry"));
        assert!(out.contains("- name: login-api"));
        assert!(out.contains("- name: login-web"));
        assert!(out.contains("stack: [python, fastapi]"));
        assert!(out.contains("memory_scope: [api]"));
    }

    #[test]
    fn empty_project_registry_hints_user_to_maestro_work() {
        let out = render_system_prelude(&sample_projects(&[]), &[], &[]);
        assert!(
            out.contains("no projects") && out.contains("`maestro work"),
            "empty registry should nudge toward `maestro work --root` but got:\n{out}"
        );
    }

    // ─── memory topics ───────────────────────────────────────────────────

    #[test]
    fn memory_topics_are_listed_when_present() {
        let topics = vec!["api".into(), "design".into(), "schema".into()];
        let out = render_system_prelude(&sample_projects(&[]), &topics, &[]);
        assert!(out.contains("## Memory topics"));
        assert!(out.contains("- `api`"));
        assert!(out.contains("- `design`"));
        assert!(out.contains("- `schema`"));
    }

    #[test]
    fn empty_memory_topics_hints_at_file_location() {
        let out = render_system_prelude(&sample_projects(&[]), &[], &[]);
        // We want the model to know where to suggest the user creates topics.
        assert!(
            out.contains("l1_facts") && out.contains("topic"),
            "empty memory hint should point at l1_facts/<topic>/ but got:\n{out}"
        );
    }

    // ─── skills ──────────────────────────────────────────────────────────

    #[test]
    fn skills_render_with_scope_description_and_trigger() {
        let skills = vec![
            skill_summary(
                "pytest-playbook",
                SkillScope::Project("api".into()),
                "FastAPI pytest cases",
                "pytest|add coverage",
            ),
            skill_summary(
                "design-tokens",
                SkillScope::Global,
                "shared design tokens",
                "color|spacing",
            ),
        ];
        let out = render_system_prelude(&sample_projects(&[]), &[], &skills);
        assert!(out.contains("## Skills available"));
        assert!(out.contains("`pytest-playbook` [api]"));
        assert!(out.contains("FastAPI pytest cases"));
        assert!(out.contains("`design-tokens` [global]"));
        // Trigger is surfaced so the model knows which skill to fetch
        assert!(out.contains("trigger: pytest|add coverage"));
    }

    #[test]
    fn skill_without_description_falls_back_to_placeholder() {
        let skills = vec![SkillSummary {
            name: "naked".into(),
            scope: SkillScope::Global,
            description: None,
            trigger: None,
        }];
        let out = render_system_prelude(&sample_projects(&[]), &[], &skills);
        assert!(out.contains("`naked` [global]"));
        assert!(out.contains("(no description)"));
    }

    #[test]
    fn empty_skills_renders_helpful_hint() {
        let out = render_system_prelude(&sample_projects(&[]), &[], &[]);
        assert!(out.contains("## Skills available"));
        assert!(
            out.contains(".maestro/skills/_global") || out.contains(".maestro/skills/<project>"),
            "empty skills hint should point at the on-disk locations"
        );
    }

    // ─── section ordering (stable contract for downstream parsing) ──────

    #[test]
    fn sections_appear_in_canonical_order() {
        let out = render_system_prelude(
            &sample_projects(&["api"]),
            &["api".into()],
            &[skill_summary("s", SkillScope::Global, "desc", "trig")],
        );
        let role = out.find("multi-project DAG executor").unwrap();
        let registry = out.find("## Project registry").unwrap();
        let memory = out.find("## Memory topics").unwrap();
        let skills = out.find("## Skills available").unwrap();
        assert!(
            role < registry && registry < memory && memory < skills,
            "section order drifted: role={role} registry={registry} memory={memory} skills={skills}"
        );
    }

    // ─── workspace recap (mid-turn) ──────────────────────────────────────
    // Why these exist: claude --print is stateless across turns. Without a
    // mid-turn recap, claude hallucinates project names on turn 3+ even
    // though turn 1 had the full prelude. Tests lock the recap's invariants
    // so a future refactor can't silently drop the safety net.

    #[test]
    fn workspace_recap_names_every_project_with_type_tag() {
        let p = sample_projects(&["alpha", "beta", "gamma"]);
        let out = render_workspace_recap(&p);
        assert!(out.contains("- alpha  # backend"));
        assert!(out.contains("- beta  # backend"));
        assert!(out.contains("- gamma  # backend"));
        assert!(
            out.contains("Do not invent names"),
            "recap must repeat the don't-invent rule on every turn",
        );
    }

    #[test]
    fn workspace_recap_is_empty_when_no_projects_registered() {
        let p = sample_projects(&[]);
        let out = render_workspace_recap(&p);
        assert!(out.is_empty(), "empty workspace should produce empty recap");
    }

    #[test]
    fn workspace_recap_truncates_past_cap_with_remainder_marker() {
        // 90 > the 80-name cap — verify we truncate AND tell the model
        // there are more, so it doesn't assume the list is exhaustive.
        let names: Vec<String> = (0..90).map(|i| format!("proj-{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let out = render_workspace_recap(&sample_projects(&refs));
        assert!(out.contains("proj-00"));
        // proj-79 is the 80th entry (i=79 means name #80) — should be in;
        // proj-80 is name #81 — should be out, replaced by the remainder.
        assert!(out.contains("proj-79"));
        assert!(!out.contains("proj-80  # backend"));
        assert!(out.contains("# … and 10 more"));
    }
}
