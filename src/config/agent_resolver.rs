//! F-114 Step 3 — pure agent-profile resolver.
//!
//! Trigger matching + precedence + conflict resolution as standalone, pure
//! functions (config in, decision out). Nothing here touches the executor, the
//! filesystem, or run state — so the precedence matrix is unit-tested directly
//! instead of being buried in dispatch branches (design §"Resolution and
//! precedence"). Steps 4/5 *lower* these decisions into the existing
//! role/skills/model_profile + `review_by` paths.
//!
//! Design: `docs/experience/F-114-SPECIALIST-AGENT-PROFILES-DESIGN.md`.

use super::agent_profile::{AgentProfile, ProfileTrigger, TriggerStage};
use globset::{Glob, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Builtin role attached by the F-106 high-risk fallback. Kept identical to
/// `scheduler::executor::resolve_reviewer_role` so the reviewer path stays
/// byte-compatible when no profile participates.
const REFUTER_ROLE: &str = "refuter";

/// Every fact a v1 trigger can examine. Pre-dispatch fields are knowable before
/// an agent runs; post-task fields need evidence (changed files, risk level,
/// findings). A caller fills only what its stage has; unset fields never match.
/// `serde(default)` so an `agent-profile eval` fixture can omit any field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchFacts {
    // ── pre-dispatch ──
    pub task_kind: Option<String>,
    pub project_type: Option<String>,
    pub project_stacks: Vec<String>,
    pub project_has_contract: bool,
    pub issue_codes: Vec<String>,
    // ── post-task ──
    pub contract_changed: bool,
    pub high_risk: bool,
    /// Project-relative changed paths.
    pub changed_paths: Vec<String>,
    pub finding_kinds: Vec<String>,
}

/// Does a single trigger fire against these facts? Pure.
pub fn trigger_matches(t: &ProfileTrigger, f: &MatchFacts) -> bool {
    match t {
        ProfileTrigger::TaskKind { kind } => f.task_kind.as_deref() == Some(kind.as_str()),
        ProfileTrigger::ProjectType { types } => f
            .project_type
            .as_deref()
            .is_some_and(|ty| types.iter().any(|x| x == ty)),
        ProfileTrigger::ProjectStack { stacks } => {
            f.project_stacks.iter().any(|s| stacks.contains(s))
        }
        ProfileTrigger::ProjectHasContract => f.project_has_contract,
        ProfileTrigger::IssueCode { codes } => f.issue_codes.iter().any(|c| codes.contains(c)),
        ProfileTrigger::ContractChanged => f.contract_changed,
        ProfileTrigger::HighRisk => f.high_risk,
        ProfileTrigger::PathChanged { patterns } => {
            path_changed_matches(patterns, &f.changed_paths)
        }
        ProfileTrigger::FindingKind { kinds } => f.finding_kinds.iter().any(|k| kinds.contains(k)),
    }
}

/// Build a glob set from project-relative patterns — skipping absolute / `..`
/// traversal / uncompilable patterns (they must never match) — then test the
/// changed paths against it.
fn path_changed_matches(patterns: &[String], changed: &[String]) -> bool {
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for p in patterns {
        if p.starts_with('/') || p.split('/').any(|seg| seg == "..") {
            continue;
        }
        if let Ok(g) = Glob::new(p) {
            builder.add(g);
            any = true;
        }
    }
    if !any {
        return false;
    }
    match builder.build() {
        Ok(set) => changed.iter().any(|c| set.is_match(c)),
        Err(_) => false,
    }
}

/// Highest-specificity trigger of `stage` that fires for this profile, if any.
fn best_matching_trigger<'a>(
    p: &'a AgentProfile,
    f: &MatchFacts,
    stage: TriggerStage,
) -> Option<&'a ProfileTrigger> {
    p.triggers
        .iter()
        .filter(|t| t.stage() == stage && trigger_matches(t, f))
        .max_by_key(|t| t.specificity())
}

/// How a profile came to be selected (provenance for task state / UI / audit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    ExplicitTask,
    ProjectDefault,
    Trigger,
}

/// Pick the winning *trigger-matched* profile at `stage`. Disabled profiles are
/// never auto-matched; `require_review_verdict` further restricts to
/// review-capable profiles (the reviewer path). Conflict resolution, in order:
/// higher `priority`, then more specific trigger, then lexicographically
/// smaller profile name (deterministic).
fn select_trigger_profile<'a>(
    profiles: &'a BTreeMap<String, AgentProfile>,
    f: &MatchFacts,
    stage: TriggerStage,
    require_review_verdict: bool,
) -> Option<(&'a str, &'a AgentProfile)> {
    profiles
        .iter()
        .filter(|(_, p)| p.enabled)
        .filter(|(_, p)| !require_review_verdict || p.is_review_capable())
        .filter_map(|(name, p)| {
            best_matching_trigger(p, f, stage).map(|t| (name, p, t.specificity()))
        })
        .max_by(|a, b| {
            a.1.priority
                .cmp(&b.1.priority) // higher priority wins
                .then(a.2.cmp(&b.2)) // higher specificity wins
                .then(b.0.cmp(a.0)) // smaller name wins
        })
        .map(|(name, p, _)| (name.as_str(), p))
}

/// Look up an enabled profile by an explicit reference name. The single point
/// that normalizes a reference (trims surrounding whitespace) so validation,
/// dispatch fail-fast, and selection agree — a padded `agent_profile:
/// " writer "` resolves to `writer` instead of silently falling through. The
/// returned `&str` is the canonical stored key, not the padded input.
fn enabled<'a>(
    profiles: &'a BTreeMap<String, AgentProfile>,
    name: &str,
) -> Option<(&'a str, &'a AgentProfile)> {
    profiles
        .get_key_value(name.trim())
        .filter(|(_, p)| p.enabled)
        .map(|(k, p)| (k.as_str(), p))
}

// ── writer path ───────────────────────────────────────────────────────────

/// Explicit task fields + project defaults the writer resolver fills around. A
/// profile only ever fills a *gap*; it never overwrites an explicit task field.
#[derive(Debug, Clone, Default)]
pub struct WriterInputs<'a> {
    pub task_role: Option<&'a str>,
    pub task_skills: &'a [String],
    pub task_model_profile: Option<&'a str>,
    pub project_role: Option<&'a str>,
    pub project_model_profile: Option<&'a str>,
}

/// The lowered writer decision: final field values after precedence + the
/// profile (if any) that contributed, for task-state provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWriter {
    pub role: Option<String>,
    pub skills: Vec<String>,
    pub model_profile: Option<String>,
    pub context_budget_bytes: Option<usize>,
    pub profile: Option<String>,
    pub source: Option<ProfileSource>,
}

/// Which profile (if any) supplies the writer specialist, by selection
/// precedence: explicit `task.agent_profile` > project `agent_profile` >
/// pre-dispatch trigger match. An explicit reference to a missing/disabled
/// profile is skipped (validation reports it separately).
fn select_writer_profile<'a>(
    profiles: &'a BTreeMap<String, AgentProfile>,
    task_agent_profile: Option<&str>,
    project_agent_profile: Option<&str>,
    facts: &MatchFacts,
) -> Option<(&'a str, &'a AgentProfile, ProfileSource)> {
    if let Some(name) = task_agent_profile {
        if let Some((k, p)) = enabled(profiles, name) {
            return Some((k, p, ProfileSource::ExplicitTask));
        }
    }
    if let Some(name) = project_agent_profile {
        if let Some((k, p)) = enabled(profiles, name) {
            return Some((k, p, ProfileSource::ProjectDefault));
        }
    }
    select_trigger_profile(profiles, facts, TriggerStage::PreDispatch, false)
        .map(|(n, p)| (n, p, ProfileSource::Trigger))
}

/// Resolve the effective writer. Precedence per field (highest first):
/// `role` = task > profile > project; `skills` = explicit task (if any) >
/// profile; `model_profile` = task > profile > project. Post-task triggers are
/// never consulted here — that data doesn't exist before dispatch.
pub fn resolve_writer(
    profiles: &BTreeMap<String, AgentProfile>,
    inputs: &WriterInputs,
    task_agent_profile: Option<&str>,
    project_agent_profile: Option<&str>,
    facts: &MatchFacts,
) -> ResolvedWriter {
    let selected =
        select_writer_profile(profiles, task_agent_profile, project_agent_profile, facts);

    let role = inputs
        .task_role
        .map(str::to_string)
        .or_else(|| selected.as_ref().map(|(_, p, _)| p.role.clone()))
        .or_else(|| inputs.project_role.map(str::to_string));

    let skills = if !inputs.task_skills.is_empty() {
        inputs.task_skills.to_vec()
    } else {
        selected
            .as_ref()
            .map(|(_, p, _)| p.skills.clone())
            .unwrap_or_default()
    };

    let model_profile = inputs
        .task_model_profile
        .map(str::to_string)
        .or_else(|| {
            selected
                .as_ref()
                .and_then(|(_, p, _)| p.model_profile.clone())
        })
        .or_else(|| inputs.project_model_profile.map(str::to_string));

    ResolvedWriter {
        role,
        skills,
        model_profile,
        context_budget_bytes: selected
            .as_ref()
            .and_then(|(_, p, _)| p.context_budget_bytes),
        profile: selected.as_ref().map(|(n, _, _)| n.to_string()),
        source: selected.as_ref().map(|(_, _, s)| *s),
    }
}

// ── reviewer / refuter path ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerSource {
    ExplicitReviewBy,
    TaskProfile,
    ProjectProfile,
    Trigger,
    RefuteFallback,
}

/// The lowered reviewer decision: the role to run read-only, plus provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReviewer {
    pub role: String,
    pub source: ReviewerSource,
    pub profile: Option<String>,
}

/// Resolve the reviewer, reusing F-106's shape (no `refute_by`). Precedence,
/// highest first: explicit `task.review_by`, then `task.review_profile`, then
/// project `review_profile`, then a post-task trigger profile with
/// `review_verdict=true`, then the F-106 `refute_on_high_risk` builtin refuter.
/// Returns `None` when nothing applies.
///
/// Byte-compatible with `resolve_reviewer_role` when no review profile / trigger
/// participates: explicit wins, else high-risk+`refute_on_high_risk` → refuter,
/// else none. Explicitly-named review profiles are honored regardless of their
/// `review_verdict` flag (the user asked for them); only the trigger auto-match
/// requires `review_verdict=true`.
pub fn resolve_reviewer(
    profiles: &BTreeMap<String, AgentProfile>,
    task_review_by: Option<&str>,
    task_review_profile: Option<&str>,
    project_review_profile: Option<&str>,
    facts: &MatchFacts,
    refute_on_high_risk: bool,
    high_risk: bool,
) -> Option<ResolvedReviewer> {
    if let Some(role) = task_review_by.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(ResolvedReviewer {
            role: role.to_string(),
            source: ReviewerSource::ExplicitReviewBy,
            profile: None,
        });
    }
    for (name_opt, src) in [
        (task_review_profile, ReviewerSource::TaskProfile),
        (project_review_profile, ReviewerSource::ProjectProfile),
    ] {
        if let Some(name) = name_opt {
            if let Some((k, p)) = enabled(profiles, name) {
                return Some(ResolvedReviewer {
                    role: p.role.clone(),
                    source: src,
                    profile: Some(k.to_string()),
                });
            }
        }
    }
    if let Some((name, p)) = select_trigger_profile(profiles, facts, TriggerStage::PostTask, true) {
        return Some(ResolvedReviewer {
            role: p.role.clone(),
            source: ReviewerSource::Trigger,
            profile: Some(name.to_string()),
        });
    }
    if refute_on_high_risk && high_risk {
        return Some(ResolvedReviewer {
            role: REFUTER_ROLE.to_string(),
            source: ReviewerSource::RefuteFallback,
            profile: None,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::agent_profile::{ProfileOutput, ProfileTrigger};

    fn profile(role: &str, triggers: Vec<ProfileTrigger>) -> AgentProfile {
        AgentProfile {
            role: role.into(),
            skills: vec![],
            model_profile: None,
            context_budget_bytes: None,
            priority: 0,
            triggers,
            outputs: vec![],
            enabled: true,
        }
    }

    fn reviewer_profile(role: &str, triggers: Vec<ProfileTrigger>) -> AgentProfile {
        let mut p = profile(role, triggers);
        p.outputs = vec![ProfileOutput {
            finding_kind: None,
            review_verdict: true,
            issue_codes: vec![],
        }];
        p
    }

    fn map(entries: Vec<(&str, AgentProfile)>) -> BTreeMap<String, AgentProfile> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    // ── trigger matcher ──

    #[test]
    fn trigger_matcher_basic_forms() {
        let mut f = MatchFacts {
            task_kind: Some("agent".into()),
            project_type: Some("backend".into()),
            project_stacks: vec!["rust".into()],
            project_has_contract: true,
            issue_codes: vec!["plan.invalid".into()],
            ..Default::default()
        };
        assert!(trigger_matches(
            &ProfileTrigger::TaskKind {
                kind: "agent".into()
            },
            &f
        ));
        assert!(!trigger_matches(
            &ProfileTrigger::TaskKind {
                kind: "verify".into()
            },
            &f
        ));
        assert!(trigger_matches(
            &ProfileTrigger::ProjectType {
                types: vec!["backend".into()]
            },
            &f
        ));
        assert!(trigger_matches(
            &ProfileTrigger::ProjectStack {
                stacks: vec!["go".into(), "rust".into()]
            },
            &f
        ));
        assert!(trigger_matches(&ProfileTrigger::ProjectHasContract, &f));
        assert!(trigger_matches(
            &ProfileTrigger::IssueCode {
                codes: vec!["plan.invalid".into()]
            },
            &f
        ));
        // structured records only — a finding kind not present does not match
        assert!(!trigger_matches(
            &ProfileTrigger::FindingKind {
                kinds: vec!["refute".into()]
            },
            &f
        ));
        f.finding_kinds = vec!["refute".into()];
        assert!(trigger_matches(
            &ProfileTrigger::FindingKind {
                kinds: vec!["refute".into()]
            },
            &f
        ));
    }

    #[test]
    fn path_changed_is_project_relative_and_rejects_traversal() {
        let f = MatchFacts {
            changed_paths: vec!["idl/user.thrift".into(), "src/main.rs".into()],
            ..Default::default()
        };
        assert!(trigger_matches(
            &ProfileTrigger::PathChanged {
                patterns: vec!["idl/**".into()]
            },
            &f
        ));
        assert!(!trigger_matches(
            &ProfileTrigger::PathChanged {
                patterns: vec!["schemas/**".into()]
            },
            &f
        ));
        // absolute + traversal patterns never match (skipped before building)
        assert!(!trigger_matches(
            &ProfileTrigger::PathChanged {
                patterns: vec!["/idl/**".into()]
            },
            &f
        ));
        assert!(!trigger_matches(
            &ProfileTrigger::PathChanged {
                patterns: vec!["../idl/**".into()]
            },
            &f
        ));
        // a valid pattern alongside a rejected one still matches on the valid one
        assert!(trigger_matches(
            &ProfileTrigger::PathChanged {
                patterns: vec!["../x".into(), "src/*.rs".into()]
            },
            &f
        ));
    }

    #[test]
    fn contract_changed_and_high_risk_are_post_task_bools() {
        let f = MatchFacts {
            contract_changed: true,
            high_risk: false,
            ..Default::default()
        };
        assert!(trigger_matches(&ProfileTrigger::ContractChanged, &f));
        assert!(!trigger_matches(&ProfileTrigger::HighRisk, &f));
    }

    // ── writer precedence ──

    #[test]
    fn explicit_task_fields_win_over_profile() {
        let profiles = map(vec![(
            "writer",
            AgentProfile {
                skills: vec!["_global/from-profile".into()],
                model_profile: Some("profile-mp".into()),
                ..profile("from_profile_role", vec![])
            },
        )]);
        let task_skills = vec!["_global/from-task".into()];
        let inputs = WriterInputs {
            task_role: Some("task_role"),
            task_skills: &task_skills,
            task_model_profile: Some("task-mp"),
            project_role: Some("project_role"),
            project_model_profile: Some("project-mp"),
        };
        let r = resolve_writer(
            &profiles,
            &inputs,
            Some("writer"),
            None,
            &MatchFacts::default(),
        );
        assert_eq!(r.role.as_deref(), Some("task_role"));
        assert_eq!(r.skills, vec!["_global/from-task".to_string()]);
        assert_eq!(r.model_profile.as_deref(), Some("task-mp"));
        assert_eq!(r.profile.as_deref(), Some("writer"));
        assert_eq!(r.source, Some(ProfileSource::ExplicitTask));
    }

    #[test]
    fn profile_fills_only_gaps() {
        // no explicit task role/skills/model → the profile supplies all three.
        let profiles = map(vec![(
            "writer",
            AgentProfile {
                skills: vec!["_global/s".into()],
                model_profile: Some("profile-mp".into()),
                ..profile("profile_role", vec![])
            },
        )]);
        let inputs = WriterInputs {
            project_role: Some("project_role"),
            ..Default::default()
        };
        let r = resolve_writer(
            &profiles,
            &inputs,
            Some("writer"),
            None,
            &MatchFacts::default(),
        );
        assert_eq!(r.role.as_deref(), Some("profile_role"));
        assert_eq!(r.skills, vec!["_global/s".to_string()]);
        assert_eq!(r.model_profile.as_deref(), Some("profile-mp"));
    }

    #[test]
    fn task_profile_beats_project_profile_beats_trigger() {
        let profiles = map(vec![
            ("task_p", profile("task_role", vec![])),
            ("project_p", profile("project_role", vec![])),
            (
                "trigger_p",
                profile(
                    "trigger_role",
                    vec![ProfileTrigger::TaskKind {
                        kind: "agent".into(),
                    }],
                ),
            ),
        ]);
        let facts = MatchFacts {
            task_kind: Some("agent".into()),
            ..Default::default()
        };
        // all three available → task profile wins
        let r = resolve_writer(
            &profiles,
            &WriterInputs::default(),
            Some("task_p"),
            Some("project_p"),
            &facts,
        );
        assert_eq!(
            (r.role.as_deref(), r.source),
            (Some("task_role"), Some(ProfileSource::ExplicitTask))
        );
        // no task profile → project profile wins
        let r = resolve_writer(
            &profiles,
            &WriterInputs::default(),
            None,
            Some("project_p"),
            &facts,
        );
        assert_eq!(
            (r.role.as_deref(), r.source),
            (Some("project_role"), Some(ProfileSource::ProjectDefault))
        );
        // neither → pre-dispatch trigger wins
        let r = resolve_writer(&profiles, &WriterInputs::default(), None, None, &facts);
        assert_eq!(
            (r.role.as_deref(), r.source),
            (Some("trigger_role"), Some(ProfileSource::Trigger))
        );
    }

    #[test]
    fn post_task_triggers_never_drive_writer_dispatch() {
        let profiles = map(vec![(
            "reviewer",
            reviewer_profile("reviewer_role", vec![ProfileTrigger::HighRisk]),
        )]);
        let facts = MatchFacts {
            high_risk: true,
            ..Default::default()
        };
        let r = resolve_writer(&profiles, &WriterInputs::default(), None, None, &facts);
        assert_eq!(
            r.profile, None,
            "a post-task trigger must not select a writer"
        );
        assert_eq!(r.role, None);
    }

    #[test]
    fn disabled_profile_is_never_auto_matched() {
        let mut p = profile(
            "role",
            vec![ProfileTrigger::TaskKind {
                kind: "agent".into(),
            }],
        );
        p.enabled = false;
        let profiles = map(vec![("draft", p)]);
        let facts = MatchFacts {
            task_kind: Some("agent".into()),
            ..Default::default()
        };
        let r = resolve_writer(&profiles, &WriterInputs::default(), None, None, &facts);
        assert_eq!(r.profile, None);
    }

    #[test]
    fn trigger_tie_break_priority_then_specificity_then_name() {
        // priority wins first
        let high = AgentProfile {
            priority: 5,
            ..profile(
                "high",
                vec![ProfileTrigger::TaskKind {
                    kind: "agent".into(),
                }],
            )
        };
        let low = AgentProfile {
            priority: 1,
            ..profile(
                "low",
                vec![ProfileTrigger::TaskKind {
                    kind: "agent".into(),
                }],
            )
        };
        let facts = MatchFacts {
            task_kind: Some("agent".into()),
            project_has_contract: true,
            ..Default::default()
        };
        let r = resolve_writer(
            &map(vec![("low", low), ("high", high)]),
            &WriterInputs::default(),
            None,
            None,
            &facts,
        );
        assert_eq!(r.profile.as_deref(), Some("high"));

        // equal priority → more specific trigger wins (issue_code(3) > task_kind(1))
        let specific = profile(
            "specific",
            vec![ProfileTrigger::IssueCode {
                codes: vec!["plan.invalid".into()],
            }],
        );
        let general = profile(
            "general",
            vec![ProfileTrigger::TaskKind {
                kind: "agent".into(),
            }],
        );
        let facts = MatchFacts {
            task_kind: Some("agent".into()),
            issue_codes: vec!["plan.invalid".into()],
            ..Default::default()
        };
        let r = resolve_writer(
            &map(vec![("general", general), ("specific", specific)]),
            &WriterInputs::default(),
            None,
            None,
            &facts,
        );
        assert_eq!(r.profile.as_deref(), Some("specific"));

        // equal priority + specificity → lexicographically smaller name wins
        let a = profile(
            "a_role",
            vec![ProfileTrigger::TaskKind {
                kind: "agent".into(),
            }],
        );
        let b = profile(
            "b_role",
            vec![ProfileTrigger::TaskKind {
                kind: "agent".into(),
            }],
        );
        let facts = MatchFacts {
            task_kind: Some("agent".into()),
            ..Default::default()
        };
        let r = resolve_writer(
            &map(vec![("bbb", b), ("aaa", a)]),
            &WriterInputs::default(),
            None,
            None,
            &facts,
        );
        assert_eq!(r.profile.as_deref(), Some("aaa"));
    }

    // ── reviewer precedence ──

    #[test]
    fn explicit_review_by_wins_over_everything() {
        let profiles = map(vec![(
            "rp",
            reviewer_profile("profile_reviewer", vec![ProfileTrigger::HighRisk]),
        )]);
        let facts = MatchFacts {
            high_risk: true,
            ..Default::default()
        };
        let r = resolve_reviewer(
            &profiles,
            Some("explicit_role"),
            Some("rp"),
            Some("rp"),
            &facts,
            true,
            true,
        )
        .unwrap();
        assert_eq!(r.role, "explicit_role");
        assert_eq!(r.source, ReviewerSource::ExplicitReviewBy);
        assert_eq!(r.profile, None);
    }

    #[test]
    fn reviewer_precedence_task_then_project_then_trigger_then_f106() {
        let profiles = map(vec![
            ("task_rp", reviewer_profile("task_reviewer", vec![])),
            ("project_rp", reviewer_profile("project_reviewer", vec![])),
            (
                "trig_rp",
                reviewer_profile("trigger_reviewer", vec![ProfileTrigger::HighRisk]),
            ),
        ]);
        let facts = MatchFacts {
            high_risk: true,
            ..Default::default()
        };
        // task review_profile
        let r = resolve_reviewer(
            &profiles,
            None,
            Some("task_rp"),
            Some("project_rp"),
            &facts,
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            (r.role.as_str(), r.source),
            ("task_reviewer", ReviewerSource::TaskProfile)
        );
        // project review_profile
        let r = resolve_reviewer(
            &profiles,
            None,
            None,
            Some("project_rp"),
            &facts,
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            (r.role.as_str(), r.source),
            ("project_reviewer", ReviewerSource::ProjectProfile)
        );
        // post-task trigger (review-capable)
        let r = resolve_reviewer(&profiles, None, None, None, &facts, true, true).unwrap();
        assert_eq!(
            (r.role.as_str(), r.source),
            ("trigger_reviewer", ReviewerSource::Trigger)
        );
    }

    #[test]
    fn non_review_capable_trigger_profile_is_not_auto_attached_as_reviewer() {
        // a writer-only profile (no review_verdict) must not auto-attach as reviewer
        let profiles = map(vec![(
            "writer",
            profile("writer_role", vec![ProfileTrigger::HighRisk]),
        )]);
        let facts = MatchFacts {
            high_risk: true,
            ..Default::default()
        };
        let r = resolve_reviewer(&profiles, None, None, None, &facts, false, true);
        assert_eq!(
            r, None,
            "no review-capable trigger + refute off → no reviewer"
        );
    }

    #[test]
    fn padded_explicit_references_are_normalized() {
        let profiles = map(vec![
            ("writer", profile("writer_role", vec![])),
            ("rp", reviewer_profile("reviewer_role", vec![])),
        ]);
        // writer: a padded task `agent_profile` still selects `writer`.
        let w = resolve_writer(
            &profiles,
            &WriterInputs::default(),
            Some("  writer  "),
            None,
            &MatchFacts::default(),
        );
        assert_eq!(w.profile.as_deref(), Some("writer"));
        assert_eq!(w.role.as_deref(), Some("writer_role"));
        // reviewer: a padded `review_profile` still selects `rp` (protects Step 5).
        let r = resolve_reviewer(
            &profiles,
            None,
            Some(" rp "),
            None,
            &MatchFacts::default(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(r.profile.as_deref(), Some("rp"));
        assert_eq!(
            (r.role.as_str(), r.source),
            ("reviewer_role", ReviewerSource::TaskProfile)
        );
        // a padded explicit `review_by` yields a trimmed role name.
        let r = resolve_reviewer(
            &profiles,
            Some("  custom_reviewer  "),
            None,
            None,
            &MatchFacts::default(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(r.role, "custom_reviewer");
        assert_eq!(r.source, ReviewerSource::ExplicitReviewBy);
    }

    #[test]
    fn f106_fallback_is_byte_compatible_when_no_profile_matches() {
        let empty = BTreeMap::new();
        let high = MatchFacts {
            high_risk: true,
            ..Default::default()
        };
        // refute_on_high_risk + high risk → builtin refuter
        let r = resolve_reviewer(&empty, None, None, None, &high, true, true).unwrap();
        assert_eq!(
            (r.role.as_str(), r.source, r.profile),
            ("refuter", ReviewerSource::RefuteFallback, None)
        );
        // refute off → none (existing behavior unchanged)
        assert_eq!(
            resolve_reviewer(&empty, None, None, None, &high, false, true),
            None
        );
        // not high risk → none
        assert_eq!(
            resolve_reviewer(
                &empty,
                None,
                None,
                None,
                &MatchFacts::default(),
                true,
                false
            ),
            None
        );
    }
}
