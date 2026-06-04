//! F-114 — specialist agent profiles.
//!
//! A named, reusable bundle that composes existing Maestro primitives
//! (`role` + `skills` + `model_profile`) with triggers and an output contract,
//! so the scheduler can dispatch a specialist when the work shape matches —
//! without the main controller carrying every domain rule in its context.
//!
//! This is **not** a new agent engine. v1 is config types here + a pure
//! resolver (next step) that lowers a matched profile into the existing
//! role/skills/model_profile fields. Stored under
//! `defaults.agent_profiles` in `.maestro/projects.yaml`.
//!
//! Design: `docs/experience/F-114-SPECIALIST-AGENT-PROFILES-DESIGN.md`.

use serde::{Deserialize, Serialize};

/// A named specialist profile (`defaults.agent_profiles.<name>`). Composes
/// existing primitives; it never introduces a new `allowed_tools` surface (the
/// role owns tool/mode affordances).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Existing role name (`src/roles` builtin or custom).
    pub role: String,
    /// Existing skill names; resolved like task skills (project scope, then
    /// `_global`, unless explicitly scoped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Existing `defaults.model_profiles.<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,
    /// Soft maximum for the specialist handoff bundle; the resolver trims
    /// optional context before exceeding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_bytes: Option<usize>,
    /// Tie-break among automatically matched profiles; higher wins.
    #[serde(default)]
    pub priority: i32,
    /// Closed v1 trigger list. Empty means "manual / project-bound only".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<ProfileTrigger>,
    /// What the specialist is expected to emit (a contract + label, not a new
    /// transport).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ProfileOutput>,
    /// Training can create disabled drafts; default enabled. A disabled profile
    /// is never auto-matched (but can be `agent-profile eval`'d).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl AgentProfile {
    /// Does this profile run as a read-only review/verdict specialist?
    pub fn is_review_capable(&self) -> bool {
        self.outputs.iter().any(|o| o.review_verdict)
    }
}

/// When a trigger can be evaluated. Pre-dispatch triggers can choose a *writer*
/// before an agent runs; post-task triggers only choose review/refute/doctor
/// specialists, after there is evidence (changed files, risk, findings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerStage {
    PreDispatch,
    PostTask,
}

/// A closed v1 trigger. The `on` field is the YAML discriminant (no boolean
/// expressions / DSL in v1). A profile matches if **any** trigger matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum ProfileTrigger {
    // ── pre-dispatch (data available before the agent runs) ──
    TaskKind { kind: String },
    ProjectType { types: Vec<String> },
    ProjectStack { stacks: Vec<String> },
    ProjectHasContract,
    IssueCode { codes: Vec<String> },
    // ── post-task (needs evidence: files_changed / risk / findings) ──
    ContractChanged,
    HighRisk,
    PathChanged { patterns: Vec<String> },
    FindingKind { kinds: Vec<String> },
}

impl ProfileTrigger {
    pub fn stage(&self) -> TriggerStage {
        match self {
            ProfileTrigger::TaskKind { .. }
            | ProfileTrigger::ProjectType { .. }
            | ProfileTrigger::ProjectStack { .. }
            | ProfileTrigger::ProjectHasContract
            | ProfileTrigger::IssueCode { .. } => TriggerStage::PreDispatch,
            ProfileTrigger::ContractChanged
            | ProfileTrigger::HighRisk
            | ProfileTrigger::PathChanged { .. }
            | ProfileTrigger::FindingKind { .. } => TriggerStage::PostTask,
        }
    }

    /// Specificity rank for conflict resolution (higher = more specific):
    /// `path_changed` / `issue_code` / `finding_kind` (3) >
    /// `contract_changed` / `high_risk` / `project_has_contract` (2) >
    /// project type/stack / task kind (1).
    pub fn specificity(&self) -> u8 {
        match self {
            ProfileTrigger::PathChanged { .. }
            | ProfileTrigger::IssueCode { .. }
            | ProfileTrigger::FindingKind { .. } => 3,
            ProfileTrigger::ContractChanged
            | ProfileTrigger::HighRisk
            | ProfileTrigger::ProjectHasContract => 2,
            ProfileTrigger::TaskKind { .. }
            | ProfileTrigger::ProjectType { .. }
            | ProfileTrigger::ProjectStack { .. } => 1,
        }
    }
}

/// Declares what a specialist emits — a contract + UI label, not a new
/// transport. Runtime outputs still flow through review_by / the F-110 finding
/// ledger / the F-111 Issue envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileOutput {
    /// Optional F-110 finding kind (`risk` / `refute` / `approval` / `learn` /
    /// `doctor` / `channel`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_kind: Option<String>,
    /// When true, the profile may run through the existing `review_by` path and
    /// must produce `VERDICT: pass|fail`.
    #[serde(default)]
    pub review_verdict: bool,
    /// Optional F-111 issue codes the profile may emit in previews/diagnostics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_codes: Vec<String>,
}

/// F-110 finding kinds a profile output / `finding_kind` trigger may name.
/// Mirrors `scheduler::findings::FindingKind` (kept local to avoid a config →
/// scheduler dependency); a `#[cfg(test)]` drift guard asserts they stay equal.
pub(crate) const KNOWN_FINDING_KINDS: &[&str] =
    &["risk", "refute", "approval", "learn", "doctor", "channel"];

/// A non-empty list of non-empty entries. Returns a problem message keyed by
/// `label` when the list is empty or carries a blank entry.
fn list_issue(label: &str, items: &[String]) -> Option<String> {
    if items.is_empty() {
        Some(format!("trigger {label} has an empty list"))
    } else if items.iter().any(|s| s.trim().is_empty()) {
        Some(format!("trigger {label} has a blank entry"))
    } else {
        None
    }
}

/// Structural problem in a single trigger, if any (empty lists, blank entries,
/// an unknown `task_kind`, or an unknown `finding_kind`). Pure.
pub(crate) fn trigger_issue(t: &ProfileTrigger) -> Option<String> {
    match t {
        ProfileTrigger::TaskKind { kind } if !matches!(kind.trim(), "agent" | "verify") => Some(
            format!("trigger task_kind has invalid kind '{kind}' (expected agent|verify)"),
        ),
        ProfileTrigger::ProjectType { types } => list_issue("project_type", types),
        ProfileTrigger::ProjectStack { stacks } => list_issue("project_stack", stacks),
        ProfileTrigger::IssueCode { codes } => list_issue("issue_code", codes),
        ProfileTrigger::PathChanged { patterns } => list_issue("path_changed", patterns),
        ProfileTrigger::FindingKind { kinds } => list_issue("finding_kind", kinds).or_else(|| {
            kinds
                .iter()
                .find(|k| !KNOWN_FINDING_KINDS.contains(&k.trim()))
                .map(|k| {
                    format!(
                        "trigger finding_kind has unknown kind '{k}' (expected {})",
                        KNOWN_FINDING_KINDS.join("/")
                    )
                })
        }),
        _ => None,
    }
}

impl AgentProfile {
    /// Self-contained structural problems with this profile (no cross-config
    /// resolution — that lives on `ProjectsConfig`). Pure; each string is one
    /// human-readable issue. Empty = structurally valid.
    pub fn issues(&self) -> Vec<String> {
        let mut out = vec![];
        if self.role.trim().is_empty() {
            out.push("role is empty".to_string());
        }
        for s in &self.skills {
            if s.trim().is_empty() {
                out.push("has a blank skill entry".to_string());
            }
        }
        if matches!(self.context_budget_bytes, Some(0)) {
            out.push("context_budget_bytes is 0".to_string());
        }
        for t in &self.triggers {
            if let Some(msg) = trigger_issue(t) {
                out.push(msg);
            }
        }
        for o in &self.outputs {
            if let Some(fk) = &o.finding_kind {
                if !KNOWN_FINDING_KINDS.contains(&fk.trim()) {
                    out.push(format!(
                        "output finding_kind '{fk}' is not a known kind (expected {})",
                        KNOWN_FINDING_KINDS.join("/")
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_finding_kinds_match_finding_kind_enum() {
        // drift guard: KNOWN_FINDING_KINDS must equal the canonical F-110 enum.
        let canonical: Vec<&str> = crate::scheduler::findings::FindingKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(KNOWN_FINDING_KINDS, canonical.as_slice());
    }

    #[test]
    fn issues_flags_empty_role_blank_skill_and_zero_budget() {
        let p = AgentProfile {
            role: "  ".into(),
            skills: vec!["".into()],
            model_profile: None,
            context_budget_bytes: Some(0),
            priority: 0,
            triggers: vec![],
            outputs: vec![],
            enabled: true,
        };
        let issues = p.issues();
        assert!(issues.iter().any(|m| m.contains("role is empty")));
        assert!(issues.iter().any(|m| m.contains("blank skill")));
        assert!(issues
            .iter()
            .any(|m| m.contains("context_budget_bytes is 0")));
    }

    #[test]
    fn issues_flags_malformed_triggers_and_outputs() {
        assert_eq!(
            trigger_issue(&ProfileTrigger::ProjectType { types: vec![] }).as_deref(),
            Some("trigger project_type has an empty list")
        );
        assert!(trigger_issue(&ProfileTrigger::PathChanged {
            patterns: vec!["ok".into(), " ".into()],
        })
        .unwrap()
        .contains("blank entry"));
        assert!(trigger_issue(&ProfileTrigger::TaskKind {
            kind: "build".into()
        })
        .unwrap()
        .contains("invalid kind"));
        assert!(trigger_issue(&ProfileTrigger::FindingKind {
            kinds: vec!["bogus".into()],
        })
        .unwrap()
        .contains("unknown kind"));
        // well-formed triggers are silent
        assert!(trigger_issue(&ProfileTrigger::ContractChanged).is_none());
        assert!(trigger_issue(&ProfileTrigger::TaskKind {
            kind: "agent".into()
        })
        .is_none());

        let bad_out = AgentProfile {
            role: "refuter".into(),
            skills: vec![],
            model_profile: None,
            context_budget_bytes: None,
            priority: 0,
            triggers: vec![],
            outputs: vec![ProfileOutput {
                finding_kind: Some("explosion".into()),
                review_verdict: false,
                issue_codes: vec![],
            }],
            enabled: true,
        };
        assert!(bad_out
            .issues()
            .iter()
            .any(|m| m.contains("not a known kind")));
    }

    #[test]
    fn well_formed_profile_has_no_issues() {
        let p: AgentProfile = serde_yaml::from_str(
            "role: refuter\nskills: [_global/contract-first]\ntriggers:\n  - on: contract_changed\noutputs:\n  - review_verdict: true\n",
        )
        .unwrap();
        assert!(p.issues().is_empty(), "{:?}", p.issues());
    }

    #[test]
    fn agent_profile_round_trips_full_and_minimal() {
        let full = AgentProfile {
            role: "refuter".into(),
            skills: vec!["_global/contract-first".into()],
            model_profile: Some("strong-review".into()),
            context_budget_bytes: Some(32_000),
            priority: 5,
            triggers: vec![
                ProfileTrigger::ContractChanged,
                ProfileTrigger::PathChanged {
                    patterns: vec!["idl/**".into(), "schemas/**".into()],
                },
            ],
            outputs: vec![ProfileOutput {
                finding_kind: Some("refute".into()),
                review_verdict: true,
                issue_codes: vec![],
            }],
            enabled: true,
        };
        let yaml = serde_yaml::to_string(&full).unwrap();
        let back: AgentProfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, full);

        // minimal profile: only role; optionals omitted from the wire.
        let minimal: AgentProfile = serde_yaml::from_str("role: backend\n").unwrap();
        assert_eq!(minimal.role, "backend");
        assert!(minimal.skills.is_empty());
        assert_eq!(minimal.model_profile, None);
        assert!(minimal.enabled, "enabled defaults to true");
        assert_eq!(minimal.priority, 0);
    }

    #[test]
    fn trigger_serializes_with_on_tag_and_classifies_stage() {
        // unit variant
        let t: ProfileTrigger = serde_yaml::from_str("on: contract_changed\n").unwrap();
        assert_eq!(t, ProfileTrigger::ContractChanged);
        assert_eq!(t.stage(), TriggerStage::PostTask);
        // struct variant
        let p: ProfileTrigger =
            serde_yaml::from_str("on: path_changed\npatterns: [\"idl/**\"]\n").unwrap();
        assert!(matches!(p, ProfileTrigger::PathChanged { .. }));
        assert_eq!(p.stage(), TriggerStage::PostTask);
        assert_eq!(p.specificity(), 3);
        // pre-dispatch
        let k: ProfileTrigger = serde_yaml::from_str("on: task_kind\nkind: agent\n").unwrap();
        assert_eq!(k.stage(), TriggerStage::PreDispatch);
        assert_eq!(k.specificity(), 1);
        // round-trip the tag
        assert_eq!(
            serde_yaml::from_str::<ProfileTrigger>(&serde_yaml::to_string(&p).unwrap()).unwrap(),
            p
        );
    }

    #[test]
    fn design_yaml_example_deserializes() {
        // the `defaults.agent_profiles.contract-reviewer` example from the design.
        let yaml = r#"
role: refuter
skills:
  - _global/contract-first
  - _global/verify-before-done
model_profile: strong-review
context_budget_bytes: 32000
triggers:
  - on: contract_changed
  - on: path_changed
    patterns: ["idl/**", "schemas/**", "contracts/**", "openapi/**"]
outputs:
  - finding_kind: refute
  - review_verdict: true
"#;
        let p: AgentProfile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.role, "refuter");
        assert_eq!(p.triggers.len(), 2);
        assert_eq!(p.outputs.len(), 2);
        assert!(p.is_review_capable());
        assert!(p.enabled);
    }

    #[test]
    fn disabled_draft_round_trips() {
        let draft: AgentProfile = serde_yaml::from_str("role: backend\nenabled: false\n").unwrap();
        assert!(!draft.enabled);
    }
}
