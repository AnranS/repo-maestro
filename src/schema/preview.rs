//! F-111 — the `PlanPreview` JSON contract + the generic `Issue` envelope.
//!
//! A stable, machine-readable preview of a synthesized (`work --dry`) or
//! validated (`plan validate`) plan, so a Claude-Code skill / MCP tool / WebUI
//! can dry-first read one shape instead of scraping stdout. Emitted by
//! `plan validate --json` and `work --dry --json`. Does NOT touch runtime.
//!
//! Design: `docs/experience/F-111-PLAN-PREVIEW-ISSUE-ENVELOPE-DESIGN.md`.

use serde::{Deserialize, Serialize};

/// The v1 `Issue.code` set — flat dotted strings. Producers should use these
/// constants so the codes a human sees match the codes a machine reads. Extend
/// deliberately as existing analyze/validate findings are folded in.
pub mod codes {
    pub const PARSE_FAILED: &str = "plan.parse_failed";
    pub const UNKNOWN_PROJECT: &str = "plan.unknown_project";
    pub const CYCLE: &str = "plan.cycle";
    pub const SELF_DEPENDENCY: &str = "plan.self_dependency";
    pub const TASK_ID_TRAVERSAL: &str = "plan.task_id_traversal";
    pub const DANGLING_CONTRACT: &str = "plan.dangling_contract";
    pub const SHELL_SYNTAX: &str = "plan.shell_syntax";
    pub const SIZE_WARNING: &str = "plan.size_warning";
    /// A task `agent_profile` / `review_profile` names a profile that is not
    /// defined in `defaults.agent_profiles`, or names a disabled one (F-114).
    pub const UNKNOWN_AGENT_PROFILE: &str = "plan.unknown_agent_profile";
    /// Generic fallback for a structural validate failure that has no more
    /// specific code yet (duplicate id, unknown dep, empty prompt, …).
    pub const PLAN_INVALID: &str = "plan.invalid";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
}

/// A structured problem — the same shape for warnings and errors, reusable by
/// later projections (F-112/F-113).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// Stable machine code (see [`codes`]).
    pub code: String,
    pub severity: IssueSeverity,
    /// Where it applies — a task id, project id, or `PLAN.yaml` pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// One-line, human-readable, neutral — no internal names.
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
    /// A docs-site anchor for the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

impl Issue {
    pub fn new(
        code: impl Into<String>,
        severity: IssueSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            path: None,
            message: message.into(),
            suggestions: Vec::new(),
            docs: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, IssueSeverity::Error, message)
    }

    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, IssueSeverity::Warning, message)
    }

    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn suggest(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    pub fn docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }
}

/// One plan-DAG edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

/// Contract + downstream impact of changing one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastEntry {
    pub task: String,
    pub project: String,
    pub downstream: Vec<String>,
}

/// `work --dry` goal-relevance narrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalMatched {
    pub matched: u32,
    pub total: u32,
}

/// The machine-readable preview of a plan. Identical shape whether synthesized
/// (`work --dry`) or read from `PLAN.yaml` (`plan validate`); only `goal` /
/// `goal_matched` differ in presence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanPreview {
    #[serde(default = "crate::schema::plan_preview_version")]
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub project_count: u32,
    pub task_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_matched: Option<GoalMatched>,
    #[serde(default)]
    pub dependency_edges: Vec<Edge>,
    #[serde(default)]
    pub blast_radius: Vec<BlastEntry>,
    #[serde(default)]
    pub warnings: Vec<Issue>,
    #[serde(default)]
    pub errors: Vec<Issue>,
}

impl Default for PlanPreview {
    fn default() -> Self {
        Self {
            schema_version: crate::schema::plan_preview_version(),
            goal: None,
            project_count: 0,
            task_count: 0,
            goal_matched: None,
            dependency_edges: Vec::new(),
            blast_radius: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }
}

impl PlanPreview {
    /// The `--json` error-path envelope: a well-formed preview with zero counts,
    /// empty edges/blast, and the given problem in `errors` — emitted on stdout
    /// even when the plan can't be parsed / loaded / validated.
    pub fn unparseable(error: Issue) -> Self {
        Self {
            errors: vec![error],
            ..Self::default()
        }
    }

    /// No blocking errors ⇒ the plan validates.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Serialize to a single-line JSON string for stdout. Infallible: the type
    /// is plain serde data, so this can't fail in practice; on the impossible
    /// error we still return valid JSON describing it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!("{{\"schema_version\":\"{}\",\"project_count\":0,\"task_count\":0,\"errors\":[{{\"code\":\"plan.parse_failed\",\"severity\":\"error\",\"message\":\"serialize preview failed: {e}\"}}]}}", crate::schema::PLAN_PREVIEW_V1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_preview_round_trips_full_and_bare() {
        let full = PlanPreview {
            goal: Some("update the shared contract and its consumers".into()),
            project_count: 3,
            task_count: 5,
            goal_matched: Some(GoalMatched {
                matched: 3,
                total: 8,
            }),
            dependency_edges: vec![Edge {
                from: "T_change_shared_contracts".into(),
                to: "T_change_billing_service".into(),
                kind: "contract".into(),
            }],
            blast_radius: vec![BlastEntry {
                task: "T_change_billing_service".into(),
                project: "billing-service".into(),
                downstream: vec!["T_verify_billing_service".into()],
            }],
            warnings: vec![Issue::warning(
                codes::DANGLING_CONTRACT,
                "no producer for shared-contracts",
            )
            .at("web-frontend")
            .suggest("declare a producer for shared-contracts")],
            errors: vec![],
            ..Default::default()
        };
        let back: PlanPreview = serde_json::from_str(&full.to_json()).unwrap();
        assert_eq!(back, full);
        assert_eq!(back.schema_version, crate::schema::PLAN_PREVIEW_V1);
        assert!(back.is_valid());

        // a bare preview (all optionals absent) still round-trips
        let bare = PlanPreview {
            project_count: 0,
            task_count: 0,
            ..Default::default()
        };
        let bare_back: PlanPreview = serde_json::from_str(&bare.to_json()).unwrap();
        assert_eq!(bare_back, bare);
        assert_eq!(bare_back.goal, None);
        assert_eq!(bare_back.goal_matched, None);
        assert!(bare_back.dependency_edges.is_empty());
    }

    #[test]
    fn issue_round_trips_each_severity() {
        for sev in [
            IssueSeverity::Info,
            IssueSeverity::Warning,
            IssueSeverity::Error,
        ] {
            let issue = Issue::new(codes::CYCLE, sev, "dependency cycle")
                .at("T_a")
                .docs("plan-validate#cycle");
            let back: Issue =
                serde_json::from_str(&serde_json::to_string(&issue).unwrap()).unwrap();
            assert_eq!(back, issue);
        }
        // optional fields omitted in JSON when empty
        let minimal = Issue::error(codes::PARSE_FAILED, "bad yaml");
        let json = serde_json::to_string(&minimal).unwrap();
        assert!(!json.contains("\"path\""));
        assert!(!json.contains("\"suggestions\""));
        assert!(!json.contains("\"docs\""));
    }

    #[test]
    fn unparseable_is_a_valid_zero_envelope_with_the_error() {
        let preview = PlanPreview::unparseable(Issue::error(
            codes::PARSE_FAILED,
            "could not parse PLAN.yaml",
        ));
        assert!(!preview.is_valid());
        assert_eq!(preview.project_count, 0);
        assert_eq!(preview.task_count, 0);
        assert!(preview.dependency_edges.is_empty());
        assert!(preview.blast_radius.is_empty());
        assert_eq!(preview.errors.len(), 1);
        assert_eq!(preview.errors[0].code, codes::PARSE_FAILED);
        // and it serializes to valid JSON
        let back: PlanPreview = serde_json::from_str(&preview.to_json()).unwrap();
        assert_eq!(back, preview);
    }

    #[test]
    fn schema_version_defaults_when_absent_on_read() {
        // a producer that omits schema_version still deserializes to v1
        let raw = r#"{"project_count":1,"task_count":2}"#;
        let p: PlanPreview = serde_json::from_str(raw).unwrap();
        assert_eq!(p.schema_version, crate::schema::PLAN_PREVIEW_V1);
        assert_eq!(p.task_count, 2);
    }
}
