pub mod codex;
pub mod cursor;
pub mod mock;
pub mod shell;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> &str;

    async fn run(&self, task: AgentTask) -> Result<AgentResult>;

    fn supports(&self, capability: Capability) -> bool;
}

#[derive(Debug, Clone)]
pub struct AgentTask {
    pub task_id: String,
    pub workspace: PathBuf,
    pub prompt: String,
    pub context: Vec<MemorySlice>,
    pub timeout: Duration,
    pub mode: ExecutionMode,
    pub resume_chat_id: Option<String>,
    pub log_path: PathBuf,
    pub trajectory: Option<TrajectoryContext>,

    /// Cursor model to pass via `--model`; `None` means the adapter omits the
    /// flag and lets cursor-agent fall back to its account default.
    pub model: Option<String>,

    /// Optional role prelude (rendered via `crate::roles::render_section`)
    /// that gets injected between the memory facts and the task prompt.
    /// `None` means "no role" — the prompt is built with memory + body only.
    pub role_prelude: Option<String>,

    pub allowed_tools: crate::modes::AllowedTools,
}

#[derive(Debug, Clone)]
pub struct TrajectoryContext {
    pub run_id: String,
    pub task_id: String,
    pub provider_id: String,
    pub path: PathBuf,
}

impl TrajectoryContext {
    pub fn writer(&self) -> Result<crate::scheduler::trajectory::TrajectoryWriter> {
        crate::scheduler::trajectory::TrajectoryWriter::from_path(
            self.path.clone(),
            self.run_id.clone(),
            self.task_id.clone(),
            self.provider_id.clone(),
        )
    }
}

pub(crate) fn trajectory_writer_for(
    task: &AgentTask,
) -> Option<crate::scheduler::trajectory::TrajectoryWriter> {
    task.trajectory
        .as_ref()
        .and_then(|context| match context.writer() {
            Ok(writer) => Some(writer),
            Err(e) => {
                tracing::warn!(
                    task_id = %task.task_id,
                    "trajectory writer disabled: {e:#}"
                );
                None
            }
        })
}

pub(crate) fn log_ref(task: &AgentTask) -> crate::schema::artifacts::ArtifactRef {
    crate::schema::artifacts::ArtifactRef {
        kind: "log".to_string(),
        source: crate::schema::artifacts::ArtifactSource::AgentTask,
        task_id: Some(task.task_id.clone()),
        path: Some(task.log_path.to_string_lossy().to_string()),
        uri: None,
        name: None,
        bytes: None,
    }
}

pub(crate) fn append_trajectory_event(
    writer: &mut crate::scheduler::trajectory::TrajectoryWriter,
    task_id: &str,
    draft: crate::scheduler::trajectory::TrajectoryEventDraft,
) {
    if let Err(error) = writer.append(draft) {
        tracing::warn!(
            task_id = %task_id,
            "failed to append trajectory event: {error:#}"
        );
    }
}

#[derive(Debug, Clone)]
pub struct MemorySlice {
    pub topic: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Plan,
    Apply,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub chat_id: Option<String>,
    pub artifacts: Artifacts,
    pub transcript_summary: String,
    /// Token / cost figures reported by the adapter, when available.
    /// `None` for adapters that don't surface usage (shell / mock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Number of agent "steps" the underlying CLI ran (≈ tool calls).
    /// `None` when the adapter can't observe this (e.g. blob-JSON mode
    /// or non-cursor agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

/// Token + cost rollup for a single agent invocation. We keep the field
/// list small and adapter-agnostic; cursor-agent's keys are different from
/// e.g. Claude's, so adapters do their own parsing and map into this.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Estimated cost in USD. May be `None` when the provider doesn't
    /// publish per-call dollar figures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Model identifier the adapter actually used. Helpful for forensics
    /// when a task ran with a different model than the plan asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Usage {
    /// Add another usage record into this one. Used to sum task-level
    /// usage into a run-level rollup on `RunState`.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        // Model name only meaningful per-task, leave as-is on the rollup.
    }

    /// Total tokens (input + output) — the figure a budget gate compares against.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// True iff no field has been set. Skip-serialize predicate for the
    /// run-level rollup so empty totals don't pollute RUN_STATE.json.
    pub fn is_zero(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cost_usd.is_none()
            && self.model.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artifacts {
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub files_changed: Vec<String>,
}

impl Artifacts {
    pub fn to_artifact_refs(&self, task_id: &str) -> Vec<crate::schema::artifacts::ArtifactRef> {
        use crate::schema::artifacts::{ArtifactRef, ArtifactSource};

        let mut refs = Vec::new();
        if let Some(url) = self.pr_url.as_ref().filter(|url| !url.trim().is_empty()) {
            refs.push(ArtifactRef {
                kind: "pr".to_string(),
                source: ArtifactSource::AgentTask,
                task_id: Some(task_id.to_string()),
                path: None,
                uri: Some(url.clone()),
                name: None,
                bytes: None,
            });
        }
        if let Some(branch) = self
            .branch
            .as_ref()
            .filter(|branch| !branch.trim().is_empty())
        {
            refs.push(ArtifactRef {
                kind: "branch".to_string(),
                source: ArtifactSource::AgentTask,
                task_id: Some(task_id.to_string()),
                path: None,
                uri: None,
                name: Some(branch.clone()),
                bytes: None,
            });
        }
        for file in &self.files_changed {
            refs.push(ArtifactRef {
                kind: "file_changed".to_string(),
                source: ArtifactSource::AgentTask,
                task_id: Some(task_id.to_string()),
                path: Some(file.clone()),
                uri: None,
                name: None,
                bytes: None,
            });
        }
        refs
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Capability {
    StreamOutput,
    Resume,
    WorktreeIsolation,
}

pub fn registry_default() -> Arc<dyn AgentAdapter> {
    Arc::new(cursor::CursorAdapter::new())
}

pub fn pick(name: &str) -> Arc<dyn AgentAdapter> {
    match name {
        "codex" => Arc::new(codex::CodexAdapter::new()),
        "cursor" => Arc::new(cursor::CursorAdapter::new()),
        "mock" => Arc::new(mock::MockAdapter::default()),
        "shell" => Arc::new(shell::ShellAdapter::new()),
        other => {
            tracing::warn!("unknown adapter {other:?}, falling back to mock");
            Arc::new(mock::MockAdapter::default())
        }
    }
}

/// Builds an LLM-style prompt that prepends memory slices and an optional
/// role prelude before the actual task instruction. The composed shape is:
///
/// ```text
/// # Project context (L1 facts)
/// ...
///
/// # Role · <name>
/// <prelude>
///
/// # Task
/// <user-authored prompt body>
/// ```
///
/// Any layer that contributes nothing is skipped — so a task with no
/// memory and no role gets back the raw prompt unchanged.
pub fn render_prompt_with_context(prompt: &str, context: &[MemorySlice]) -> String {
    render_prompt_full(prompt, context, None)
}

pub fn join_prompt_prelude(parts: &[Option<String>]) -> Option<String> {
    let sections = parts
        .iter()
        .filter_map(|part| part.as_deref())
        .map(str::trim_end)
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Full renderer. Public so the executor can pass a role prelude without
/// us churning the callers of [`render_prompt_with_context`].
pub fn render_prompt_full(
    prompt: &str,
    context: &[MemorySlice],
    role_prelude: Option<&str>,
) -> String {
    let has_role = role_prelude.map(|s| !s.trim().is_empty()).unwrap_or(false);
    if context.is_empty() && !has_role {
        return prompt.to_string();
    }

    let mut s = String::new();
    if !context.is_empty() {
        s.push_str("# Project context (from .maestro/memory/l1_facts)\n\n");
        for slice in context {
            s.push_str(&format!("## {}\n\n", slice.topic));
            s.push_str("```\n");
            s.push_str(slice.content.trim_end());
            s.push_str("\n```\n\n");
        }
    }
    if let Some(p) = role_prelude.filter(|s| !s.trim().is_empty()) {
        s.push_str(p.trim_end());
        s.push_str("\n\n");
    }
    s.push_str("# Task\n\n");
    s.push_str(prompt);
    s
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    fn slice(topic: &str, body: &str) -> MemorySlice {
        MemorySlice {
            topic: topic.into(),
            content: body.into(),
        }
    }

    #[test]
    fn no_context_no_role_returns_prompt_verbatim() {
        let p = render_prompt_full("do thing", &[], None);
        assert_eq!(p, "do thing");
    }

    #[test]
    fn role_only_skips_context_header() {
        let p = render_prompt_full("do thing", &[], Some("# Role · x\n\nbody"));
        assert!(!p.contains("Project context"));
        assert!(p.contains("# Role · x"));
        assert!(p.ends_with("do thing"));
    }

    #[test]
    fn context_only_skips_role_block() {
        let p = render_prompt_full("do", &[slice("api", "fact")], None);
        assert!(p.contains("Project context"));
        assert!(!p.contains("# Role"));
    }

    #[test]
    fn empty_role_prelude_is_treated_as_no_role() {
        // Avoids "# Role · …\n\n" with empty body slipping through and
        // confusing the agent.
        let p = render_prompt_full("do", &[], Some("   \n  "));
        assert_eq!(p, "do");
    }

    #[test]
    fn full_render_orders_context_then_role_then_task() {
        let p = render_prompt_full(
            "GO",
            &[slice("api", "uses sqlx")],
            Some("# Role · backend_rust\n\ncontract first"),
        );
        let ctx_idx = p.find("Project context").unwrap();
        let role_idx = p.find("# Role").unwrap();
        let task_idx = p.find("# Task").unwrap();
        assert!(ctx_idx < role_idx);
        assert!(role_idx < task_idx);
    }

    #[test]
    fn join_prompt_prelude_skips_empty_parts() {
        let p = join_prompt_prelude(&[
            Some("# Repository instructions\n\nUse rustfmt.".into()),
            None,
            Some("  \n".into()),
            Some("# Skill\n\nStay scoped.".into()),
        ])
        .unwrap();
        assert!(p.contains("Use rustfmt."));
        assert!(p.contains("Stay scoped."));
        assert!(!p.contains("\n\n\n"));
    }

    #[test]
    fn usage_total_tokens_sums_input_and_output() {
        let u = Usage {
            input_tokens: 1200,
            output_tokens: 800,
            cost_usd: None,
            model: None,
        };
        assert_eq!(u.total_tokens(), 2000);
    }
}
