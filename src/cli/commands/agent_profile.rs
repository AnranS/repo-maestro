//! F-114 Step 6 — `maestro agent-profile` CLI.
//!
//! Manage specialist profiles under `defaults.agent_profiles`: create disabled
//! drafts (`new`), inspect (`ls` / `show`), dry-run trigger matching against a
//! fixture (`eval`), distill a draft from a prior run (`train`), and enable
//! after validation (`promote`).
//!
//! Output is plain config — never run transcripts, secrets, absolute paths, or
//! real project names. Templates use the neutral design names.

use crate::cli::AgentProfileCmd;
use crate::config::{
    trigger_matches, AgentProfile, MatchFacts, ProfileOutput, ProfileTrigger, ProjectsConfig,
    TriggerStage,
};
use crate::paths;
use anyhow::{bail, Context, Result};

pub fn run(cmd: AgentProfileCmd) -> Result<()> {
    match cmd {
        AgentProfileCmd::New {
            name,
            template,
            role,
        } => new(&name, template.as_deref(), role.as_deref()),
        AgentProfileCmd::Ls => ls(),
        AgentProfileCmd::Show { name } => show(&name),
        AgentProfileCmd::Eval { name, fixture } => eval(&name, &fixture),
        AgentProfileCmd::Promote { name } => promote(&name),
        AgentProfileCmd::Train { name, from_run } => train(&name, &from_run),
    }
}

/// A profile name is a `defaults.agent_profiles` key and a reference target, so
/// keep it a simple slug (no whitespace or path separators).
fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        bail!("profile name is required");
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == '/' || c == '\\')
    {
        bail!("profile name '{name}' must not contain whitespace or path separators");
    }
    Ok(name)
}

fn load() -> Result<(std::path::PathBuf, ProjectsConfig)> {
    let pfile = paths::projects_file()?;
    let cfg = ProjectsConfig::load(&pfile)?;
    Ok((pfile, cfg))
}

fn base(role: &str, triggers: Vec<ProfileTrigger>, outputs: Vec<ProfileOutput>) -> AgentProfile {
    AgentProfile {
        role: role.to_string(),
        skills: vec![],
        model_profile: None,
        context_budget_bytes: None,
        priority: 0,
        triggers,
        outputs,
        enabled: false, // always a draft
    }
}

/// Neutral starting shapes. Each uses an existing builtin role (`refuter`); the
/// user edits skills/model_profile/triggers before `promote`.
fn template_profile(template: &str) -> Result<AgentProfile> {
    let review = || ProfileOutput {
        finding_kind: None,
        review_verdict: true,
        issue_codes: vec![],
    };
    Ok(match template {
        "blank" => base("refuter", vec![], vec![]),
        "contract-reviewer" => base(
            "refuter",
            vec![
                ProfileTrigger::ContractChanged,
                ProfileTrigger::PathChanged {
                    patterns: vec![
                        "idl/**".into(),
                        "schemas/**".into(),
                        "contracts/**".into(),
                        "openapi/**".into(),
                    ],
                },
            ],
            vec![
                ProfileOutput {
                    finding_kind: Some("refute".into()),
                    review_verdict: true,
                    issue_codes: vec![],
                },
                review(),
            ],
        ),
        "release-privacy-reviewer" => base(
            "refuter",
            vec![ProfileTrigger::PathChanged {
                patterns: vec!["**/secrets/**".into(), "**/*.env".into()],
            }],
            vec![review()],
        ),
        "recovery-doctor" => base(
            "refuter",
            vec![
                ProfileTrigger::HighRisk,
                ProfileTrigger::FindingKind {
                    kinds: vec!["doctor".into()],
                },
            ],
            vec![ProfileOutput {
                finding_kind: Some("doctor".into()),
                review_verdict: false,
                issue_codes: vec![],
            }],
        ),
        other => bail!(
            "unknown template '{other}' (try: blank, contract-reviewer, \
             release-privacy-reviewer, recovery-doctor)"
        ),
    })
}

fn new(name: &str, template: Option<&str>, role: Option<&str>) -> Result<()> {
    let name = validate_name(name)?;
    let (pfile, mut cfg) = load()?;
    if cfg.defaults.agent_profiles.contains_key(name) {
        bail!("agent_profile '{name}' already exists (see: maestro agent-profile show {name})");
    }
    let mut profile = template_profile(template.unwrap_or("blank"))?;
    if let Some(r) = role.map(str::trim).filter(|s| !s.is_empty()) {
        profile.role = r.to_string();
    }
    profile.enabled = false;
    cfg.defaults
        .agent_profiles
        .insert(name.to_string(), profile);
    cfg.save(&pfile)?;
    println!(
        "created disabled draft agent_profile '{name}' in {}",
        pfile.display()
    );
    println!("edit it under defaults.agent_profiles, then: maestro agent-profile promote {name}");
    Ok(())
}

fn ls() -> Result<()> {
    let (_, cfg) = load()?;
    if cfg.defaults.agent_profiles.is_empty() {
        println!("(no agent_profiles defined under defaults.agent_profiles)");
        return Ok(());
    }
    for (name, p) in &cfg.defaults.agent_profiles {
        println!(
            "{:<24} role={:<14} {}  triggers={}{}",
            name,
            p.role,
            if p.enabled { "enabled " } else { "disabled" },
            p.triggers.len(),
            if p.is_review_capable() {
                "  [review]"
            } else {
                ""
            },
        );
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let (_, cfg) = load()?;
    let p = cfg
        .defaults
        .agent_profiles
        .get(name.trim())
        .with_context(|| format!("agent_profile '{name}' not found"))?;
    print!("{}", serde_yaml::to_string(p)?);
    Ok(())
}

/// The `eval` verdict line. A post-task trigger only makes a profile a reviewer
/// when it is review-capable (an `outputs.review_verdict: true`); a profile with
/// only a `finding_kind` output must NOT be reported as a reviewer even if a
/// post-task trigger fires (N1). Pure so the matrix is unit-tested.
fn eval_verdict(pre: bool, post: bool, review_capable: bool) -> &'static str {
    match (pre, post, review_capable) {
        (true, true, true) => "MATCHES as writer (pre-dispatch) and reviewer (post-task)",
        (true, true, false) => {
            "MATCHES as writer (pre-dispatch); a post-task trigger matched but the profile \
             is NOT review-capable (add an output with review_verdict: true)"
        }
        (true, false, _) => "MATCHES as writer (pre-dispatch)",
        (false, true, true) => "MATCHES as reviewer (post-task)",
        (false, true, false) => {
            "post-task trigger matched but the profile is NOT review-capable \
             (add an output with review_verdict: true)"
        }
        (false, false, _) => "no trigger matches this fixture",
    }
}

fn eval(name: &str, fixture: &std::path::Path) -> Result<()> {
    let (_, cfg) = load()?;
    let p = cfg
        .defaults
        .agent_profiles
        .get(name.trim())
        .with_context(|| format!("agent_profile '{name}' not found"))?;
    let raw = std::fs::read_to_string(fixture)
        .with_context(|| format!("read fixture {}", fixture.display()))?;
    let facts: MatchFacts =
        serde_yaml::from_str(&raw).context("parse fixture as MatchFacts (see docs for fields)")?;

    println!(
        "profile: {name} ({})  role={}{}",
        if p.enabled { "enabled" } else { "disabled" },
        p.role,
        p.model_profile
            .as_deref()
            .map(|m| format!("  model_profile={m}"))
            .unwrap_or_default(),
    );
    let mut pre = false;
    let mut post = false;
    if p.triggers.is_empty() {
        println!("  (no triggers — only usable via explicit task/project binding)");
    }
    for t in &p.triggers {
        let hit = trigger_matches(t, &facts);
        let stage = match t.stage() {
            TriggerStage::PreDispatch => "pre-dispatch",
            TriggerStage::PostTask => "post-task",
        };
        if hit {
            match t.stage() {
                TriggerStage::PreDispatch => pre = true,
                TriggerStage::PostTask => post = true,
            }
        }
        println!("  {} {:?} ({stage})", if hit { "✓" } else { "✗" }, t);
    }

    let review_capable = p.is_review_capable();
    println!("result: {}", eval_verdict(pre, post, review_capable));
    if pre || (post && review_capable) {
        let skills = if p.skills.is_empty() {
            "(none)".to_string()
        } else {
            p.skills.join(", ")
        };
        println!(
            "handoff: role={}  skills=[{}]{}{}",
            p.role,
            skills,
            p.model_profile
                .as_deref()
                .map(|m| format!("  model_profile={m}"))
                .unwrap_or_default(),
            p.context_budget_bytes
                .map(|b| format!("  context_budget_bytes={b}"))
                .unwrap_or_default(),
        );
    }
    println!("(eval is read-only — it does not run the agent)");
    Ok(())
}

/// Projects that pin `name` via `agent_profile` / `review_profile` (normalized).
fn pinning_projects<'a>(cfg: &'a ProjectsConfig, name: &str) -> Vec<&'a str> {
    cfg.projects
        .iter()
        .filter(|(_, pr)| {
            let m = |r: &Option<String>| r.as_deref().map(str::trim) == Some(name);
            m(&pr.agent_profile) || m(&pr.review_profile)
        })
        .map(|(n, _)| n.as_str())
        .collect()
}

/// Everything that blocks promotion: structural issues, a role/skill/
/// model_profile that doesn't resolve, and the "would never be used" rule — a
/// promoted profile must have at least one trigger OR an explicit project
/// binding. Each string is one human-readable reason; empty = promotable.
fn promote_issues(cfg: &ProjectsConfig, name: &str) -> Vec<String> {
    let Some(p) = cfg.defaults.agent_profiles.get(name) else {
        return vec![format!("agent_profile '{name}' not found")];
    };
    let mut out = p.issues();
    if !p.role.trim().is_empty() && !crate::roles::exists(&p.role) {
        out.push(format!("role '{}' is not defined", p.role));
    }
    let pinned = pinning_projects(cfg, name);
    for skill in &p.skills {
        let skill = skill.trim();
        if skill.is_empty() {
            continue;
        }
        if skill.contains('/') || pinned.is_empty() {
            // explicit scope → exact; unbound profile → global.
            if !crate::skills::reference_exists(None, skill) {
                out.push(format!("skill '{skill}' is not defined"));
            }
        } else {
            // unscoped + bound: it must resolve (project scope, then global) for
            // EVERY pinning project — matching `maestro validate` / dispatch,
            // which fail-fast per project. Reported per project, so a skill that
            // exists only in one pinning project's scope can't slip through.
            for proj in &pinned {
                if !crate::skills::reference_exists(Some(proj), skill) {
                    out.push(format!(
                        "skill '{skill}' is not defined for project '{proj}'"
                    ));
                }
            }
        }
    }
    if let Some(mp) = &p.model_profile {
        if !cfg.defaults.model_profiles.contains_key(mp.trim()) {
            out.push(format!(
                "model_profile '{mp}' is not defined under defaults.model_profiles"
            ));
        }
    }
    if p.triggers.is_empty() && pinned.is_empty() {
        out.push(
            "would never be used: add at least one trigger, or bind it from a project \
             (agent_profile / review_profile)"
                .to_string(),
        );
    }
    out
}

fn promote(name: &str) -> Result<()> {
    let name = name.trim();
    let (pfile, mut cfg) = load()?;
    if !cfg.defaults.agent_profiles.contains_key(name) {
        bail!("agent_profile '{name}' not found");
    }
    if cfg.defaults.agent_profiles[name].enabled {
        println!("agent_profile '{name}' is already enabled");
        return Ok(());
    }
    let issues = promote_issues(&cfg, name);
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("  ✗ {i}");
        }
        bail!(
            "agent_profile '{name}' cannot be promoted ({} issue(s)); fix them, then retry",
            issues.len()
        );
    }
    cfg.defaults.agent_profiles.get_mut(name).unwrap().enabled = true;
    cfg.save(&pfile)?;
    println!("promoted agent_profile '{name}' — now enabled");
    Ok(())
}

/// Neutral, config-level signals distilled from a run. Deliberately carries NO
/// paths, project names, task prompts, or transcript text — only role/skill
/// names, finding kinds, and counts — so a trained draft is safe to commit.
#[derive(Debug, Default, PartialEq)]
struct RunSignals {
    /// Most common task role (lexicographic tie-break).
    role: Option<String>,
    /// Sorted union of triggered skills that are safe to carry forward —
    /// `_global/<name>` and bare unscoped names only.
    skills: Vec<String>,
    /// Sorted distinct finding kinds recorded in the run.
    finding_kinds: Vec<String>,
    /// The single task kind if the whole run used one, else `None`.
    sole_task_kind: Option<String>,
    task_count: usize,
    high_risk_count: usize,
    /// How many distinct `<project>/<skill>` references were dropped — they'd
    /// leak the project name and don't generalize. The NAMES are never kept.
    dropped_project_scoped_skills: usize,
}

/// The per-task slice `aggregate_signals` reads — extracted from a `TaskState`
/// so the aggregation is pure and testable without building a full run.
struct TaskFacts {
    role: Option<String>,
    skills: Vec<String>,
    kind: String,
    high_risk: bool,
}

fn task_facts(state: &crate::scheduler::RunState) -> Vec<TaskFacts> {
    state
        .tasks
        .values()
        .map(|t| TaskFacts {
            role: t.role.clone(),
            skills: t.skills_triggered.clone(),
            kind: t.kind.clone(),
            high_risk: t.risk_level.as_deref() == Some("high"),
        })
        .collect()
}

/// Aggregate per-task facts + finding kinds into neutral run signals. Pure.
fn aggregate_signals(tasks: &[TaskFacts], finding_kinds: &[String]) -> RunSignals {
    use std::collections::{BTreeMap, BTreeSet};

    let mut role_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut skills: BTreeSet<String> = BTreeSet::new();
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    let mut task_kinds: BTreeSet<&str> = BTreeSet::new();
    let mut high_risk_count = 0;
    for t in tasks {
        if let Some(r) = t.role.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            *role_counts.entry(r).or_default() += 1;
        }
        for s in &t.skills {
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            // A skill recorded as `<scope>/<name>` keeps the scope. Carry forward
            // only the portable scopes: `_global/<name>` and bare unscoped names.
            // A project-scoped `<project>/<name>` would leak the project name and
            // wouldn't generalize, so drop it (by distinct ref, name never kept).
            match s.split_once('/') {
                Some((scope, _)) if scope != crate::skills::GLOBAL_SCOPE_DIR => {
                    dropped.insert(s.to_string());
                }
                _ => {
                    skills.insert(s.to_string());
                }
            }
        }
        if !t.kind.trim().is_empty() {
            task_kinds.insert(t.kind.trim());
        }
        if t.high_risk {
            high_risk_count += 1;
        }
    }
    let kinds: BTreeSet<String> = finding_kinds
        .iter()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    // most common role; ties broken by the BTreeMap's lexicographic order.
    let role = role_counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(r, _)| r.to_string());

    RunSignals {
        role,
        skills: skills.into_iter().collect(),
        finding_kinds: kinds.into_iter().collect(),
        sole_task_kind: (task_kinds.len() == 1)
            .then(|| task_kinds.iter().next().unwrap().to_string()),
        task_count: tasks.len(),
        high_risk_count,
        dropped_project_scoped_skills: dropped.len(),
    }
}

/// Build a DISABLED draft profile from run signals. Pure + deterministic.
/// Triggers: `high_risk` (if any high-risk task), `finding_kind` (observed
/// kinds), `task_kind` (when the run used a single kind). Outputs: one per
/// observed finding kind, plus `review_verdict` when a review-style kind
/// (refute/approval) appeared. Role defaults to the builtin `refuter`.
fn draft_from_signals(sig: &RunSignals) -> AgentProfile {
    let mut triggers = vec![];
    if sig.high_risk_count > 0 {
        triggers.push(ProfileTrigger::HighRisk);
    }
    if !sig.finding_kinds.is_empty() {
        triggers.push(ProfileTrigger::FindingKind {
            kinds: sig.finding_kinds.clone(),
        });
    }
    if let Some(kind) = sig
        .sole_task_kind
        .as_deref()
        .filter(|k| matches!(*k, "agent" | "verify"))
    {
        triggers.push(ProfileTrigger::TaskKind {
            kind: kind.to_string(),
        });
    }

    let mut outputs: Vec<ProfileOutput> = sig
        .finding_kinds
        .iter()
        .map(|k| ProfileOutput {
            finding_kind: Some(k.clone()),
            review_verdict: false,
            issue_codes: vec![],
        })
        .collect();
    if sig
        .finding_kinds
        .iter()
        .any(|k| k == "refute" || k == "approval")
    {
        outputs.push(ProfileOutput {
            finding_kind: None,
            review_verdict: true,
            issue_codes: vec![],
        });
    }

    AgentProfile {
        role: sig.role.clone().unwrap_or_else(|| "refuter".to_string()),
        skills: sig.skills.clone(),
        model_profile: None,
        context_budget_bytes: None,
        priority: 0,
        triggers,
        outputs,
        enabled: false,
    }
}

fn train(name: &str, from_run: &str) -> Result<()> {
    let name = validate_name(name)?;
    let (pfile, mut cfg) = load()?;
    if cfg.defaults.agent_profiles.contains_key(name) {
        bail!("agent_profile '{name}' already exists; train into a new name or remove it first");
    }
    let run_dir = if from_run == "current" {
        paths::current_run_dir()?.context("no current run")?
    } else {
        paths::run_dir_for_id(from_run)?
    };
    let state = crate::scheduler::RunState::load(&run_dir)
        .with_context(|| format!("load run '{from_run}'"))?;
    let findings = crate::scheduler::findings::read_findings(&run_dir).unwrap_or_default();
    let finding_kinds: Vec<String> = findings
        .iter()
        .map(|f| f.kind.as_str().to_string())
        .collect();

    let sig = aggregate_signals(&task_facts(&state), &finding_kinds);
    let draft = draft_from_signals(&sig);
    let (triggers, outputs, skills) = (
        draft.triggers.len(),
        draft.outputs.len(),
        draft.skills.len(),
    );
    let role = draft.role.clone();
    cfg.defaults.agent_profiles.insert(name.to_string(), draft);
    cfg.save(&pfile)?;

    // Sanitized summary — counts + the derived shape only, never paths/names.
    println!(
        "trained disabled draft agent_profile '{name}' from a run \
         ({} task(s), {} high-risk, {} finding kind(s){})",
        sig.task_count,
        sig.high_risk_count,
        sig.finding_kinds.len(),
        if sig.finding_kinds.is_empty() {
            String::new()
        } else {
            format!(": {}", sig.finding_kinds.join(", "))
        },
    );
    println!("  → role={role}, {skills} skill(s), {triggers} trigger(s), {outputs} output(s)");
    if sig.dropped_project_scoped_skills > 0 {
        // count only — the project-scoped names are never printed or kept.
        println!(
            "  dropped {} project-scoped skill ref(s) for privacy/generalization; add manually if intended",
            sig.dropped_project_scoped_skills
        );
    }
    println!("review with: maestro agent-profile show {name}  (then edit + promote)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_rejects_blank_and_separators() {
        assert_eq!(validate_name("  reviewer  ").unwrap(), "reviewer");
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a b").is_err());
    }

    #[test]
    fn templates_are_well_formed_disabled_drafts() {
        for t in [
            "blank",
            "contract-reviewer",
            "release-privacy-reviewer",
            "recovery-doctor",
        ] {
            let p = template_profile(t).unwrap();
            assert!(!p.enabled, "{t} template must be a disabled draft");
            assert!(!p.role.trim().is_empty(), "{t} has a role");
            // a template must be structurally valid (the validate guards pass).
            assert!(p.issues().is_empty(), "{t} issues: {:?}", p.issues());
        }
        assert!(template_profile("nope").is_err());
    }

    fn facts(role: &str, skills: &[&str], kind: &str, high_risk: bool) -> TaskFacts {
        TaskFacts {
            role: Some(role.to_string()),
            skills: skills.iter().map(|s| s.to_string()).collect(),
            kind: kind.to_string(),
            high_risk,
        }
    }

    #[test]
    fn aggregate_signals_drops_project_scoped_skills_for_privacy() {
        // N1: `<project>/<skill>` leaks the project name + won't generalize, so
        // it's dropped (by count, never by name). `_global/*` + bare unscoped
        // skills are kept.
        let tasks = [facts(
            "refuter",
            &["api/house-style", "_global/contract-first", "plain-skill"],
            "agent",
            false,
        )];
        let sig = aggregate_signals(&tasks, &[]);
        assert_eq!(
            sig.skills,
            vec![
                "_global/contract-first".to_string(),
                "plain-skill".to_string()
            ]
        );
        assert!(!sig.skills.iter().any(|s| s.contains("api")));
        assert_eq!(sig.dropped_project_scoped_skills, 1);
    }

    #[test]
    fn aggregate_signals_is_neutral_and_deterministic() {
        let tasks = [
            facts("backend_rust", &["_global/contract-first"], "agent", true),
            facts(
                "backend_rust",
                &["_global/verify-before-done"],
                "agent",
                false,
            ),
        ];
        let sig = aggregate_signals(&tasks, &["refute".into(), "refute".into()]);
        assert_eq!(sig.role.as_deref(), Some("backend_rust")); // most common
        assert_eq!(
            sig.skills,
            vec![
                "_global/contract-first".to_string(),
                "_global/verify-before-done".to_string()
            ] // sorted union
        );
        assert_eq!(sig.finding_kinds, vec!["refute".to_string()]); // deduped
        assert_eq!(sig.sole_task_kind.as_deref(), Some("agent"));
        assert_eq!((sig.task_count, sig.high_risk_count), (2, 1));
        // mixed kinds → no sole kind
        let mixed = [
            facts("r", &[], "agent", false),
            facts("r", &[], "verify", false),
        ];
        assert_eq!(aggregate_signals(&mixed, &[]).sole_task_kind, None);
    }

    #[test]
    fn draft_from_signals_builds_a_valid_disabled_draft() {
        let sig = RunSignals {
            role: Some("backend_rust".into()),
            skills: vec!["_global/x".into()],
            finding_kinds: vec!["refute".into()],
            sole_task_kind: Some("agent".into()),
            task_count: 3,
            high_risk_count: 1,
            dropped_project_scoped_skills: 0,
        };
        let d = draft_from_signals(&sig);
        assert!(!d.enabled, "trained profile is a disabled draft");
        assert_eq!(d.role, "backend_rust");
        assert_eq!(d.skills, vec!["_global/x".to_string()]);
        // high_risk + finding_kind + task_kind triggers
        assert_eq!(d.triggers.len(), 3);
        // a review-style finding kind adds a review_verdict output
        assert!(d.is_review_capable());
        // structurally valid → promotable after review
        assert!(d.issues().is_empty(), "{:?}", d.issues());
    }

    #[test]
    fn draft_review_verdict_only_for_review_style_kinds() {
        let doctor = draft_from_signals(&RunSignals {
            role: None,
            finding_kinds: vec!["doctor".into()],
            ..Default::default()
        });
        assert_eq!(doctor.role, "refuter"); // default role
        assert!(
            !doctor.is_review_capable(),
            "doctor is not a review verdict"
        );
        let approval = draft_from_signals(&RunSignals {
            finding_kinds: vec!["approval".into()],
            ..Default::default()
        });
        assert!(approval.is_review_capable());
        // no signals → empty disabled draft
        let empty = draft_from_signals(&RunSignals::default());
        assert!(empty.triggers.is_empty() && empty.outputs.is_empty() && !empty.enabled);
    }

    #[test]
    fn promote_issues_flags_never_used_and_unresolved_role() {
        // `bare`: valid builtin role but no trigger + no binding → never used.
        // `bad`: has a trigger, so not never-used, but its role doesn't resolve.
        let cfg: ProjectsConfig = serde_yaml::from_str(
            "version: 1\ndefaults:\n  agent_profiles:\n    bare:\n      role: refuter\n    bad:\n      role: ghost_role\n      triggers:\n        - on: high_risk\nprojects:\n  api:\n    path: ./api\n",
        )
        .unwrap();
        let bare = promote_issues(&cfg, "bare");
        assert!(
            bare.iter().any(|m| m.contains("would never be used")),
            "{bare:?}"
        );
        let bad = promote_issues(&cfg, "bad");
        assert!(
            bad.iter()
                .any(|m| m.contains("role 'ghost_role' is not defined")),
            "{bad:?}"
        );
        assert!(!bad.iter().any(|m| m.contains("would never be used")));
        assert!(!promote_issues(&cfg, "nope")
            .iter()
            .all(|m| !m.contains("not found")));
    }

    #[test]
    fn promote_issues_clean_when_bound_by_a_project() {
        // bound by a project (no trigger) → promotable.
        let cfg: ProjectsConfig = serde_yaml::from_str(
            "version: 1\ndefaults:\n  agent_profiles:\n    rev:\n      role: refuter\nprojects:\n  api:\n    path: ./api\n    review_profile: rev\n",
        )
        .unwrap();
        assert!(
            promote_issues(&cfg, "rev").is_empty(),
            "{:?}",
            promote_issues(&cfg, "rev")
        );
    }

    #[test]
    fn eval_verdict_gates_reviewer_claim_on_review_capability() {
        // N1: pre + post triggers but NOT review-capable → writer only, never
        // "and reviewer".
        let v = eval_verdict(true, true, false);
        assert!(v.contains("writer") && !v.contains("and reviewer"));
        assert!(v.contains("NOT review-capable"));
        // pre + post + review-capable → both
        assert!(eval_verdict(true, true, true).contains("and reviewer"));
        // post-only, not review-capable → not a reviewer
        let v = eval_verdict(false, true, false);
        assert!(!v.contains("MATCHES as reviewer") && v.contains("NOT review-capable"));
        // post-only + review-capable → reviewer
        assert_eq!(
            eval_verdict(false, true, true),
            "MATCHES as reviewer (post-task)"
        );
        assert_eq!(
            eval_verdict(true, false, false),
            "MATCHES as writer (pre-dispatch)"
        );
        assert_eq!(
            eval_verdict(false, false, true),
            "no trigger matches this fixture"
        );
    }

    #[test]
    fn review_templates_are_review_capable_writers_are_not() {
        assert!(template_profile("contract-reviewer")
            .unwrap()
            .is_review_capable());
        assert!(template_profile("release-privacy-reviewer")
            .unwrap()
            .is_review_capable());
        // recovery-doctor emits a doctor finding, not a review verdict.
        assert!(!template_profile("recovery-doctor")
            .unwrap()
            .is_review_capable());
        assert!(!template_profile("blank").unwrap().is_review_capable());
    }
}
