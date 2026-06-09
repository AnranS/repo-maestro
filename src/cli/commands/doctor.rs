//! Health diagnostics for the local maestro workspace.
//!
//! This is intentionally read-only. It checks the things that most often make
//! a run fail before any agent work begins: workspace state, projects.yaml,
//! external binaries, model cache, and the current run marker.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::process::Command;

use crate::cli::{util, DoctorArgs, DoctorCommand};
use crate::config::ProjectsConfig;
use crate::paths;
use crate::scheduler::worktree_policy::{WorktreePolicy, DENY_PATTERNS};
use crate::scheduler::{RunLiveness, RunStatus};
use crate::schema::runtime_health::{HealthStatus, RuntimeHealthReport};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const STALE_PROJECTS_TMP_THRESHOLD: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skip => "SKIP",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub category: &'static str,
    pub name: &'static str,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl DoctorCheck {
    fn pass(category: &'static str, name: &'static str, message: impl Into<String>) -> Self {
        Self {
            category,
            name,
            status: CheckStatus::Pass,
            message: message.into(),
            details: vec![],
            fix: None,
        }
    }

    fn warn(
        category: &'static str,
        name: &'static str,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name,
            status: CheckStatus::Warn,
            message: message.into(),
            details: vec![],
            fix: Some(fix.into()),
        }
    }

    fn fail(
        category: &'static str,
        name: &'static str,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name,
            status: CheckStatus::Fail,
            message: message.into(),
            details: vec![],
            fix: Some(fix.into()),
        }
    }

    fn skip(category: &'static str, name: &'static str, message: impl Into<String>) -> Self {
        Self {
            category,
            name,
            status: CheckStatus::Skip,
            message: message.into(),
            details: vec![],
            fix: None,
        }
    }

    fn detail(mut self, value: impl Into<String>) -> Self {
        self.details.push(value.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorSummary {
    pub total: usize,
    pub passed: usize,
    pub warnings: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub workspace_root: String,
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
}

pub async fn run(args: DoctorArgs) -> Result<()> {
    if let Some(DoctorCommand::Runtime(runtime)) = &args.command {
        // honour the parent `--json` too: `maestro doctor --json runtime` must emit
        // JSON, not the human report (a user who passed --json expects JSON on stdout).
        return run_runtime(args.json || runtime.json).await;
    }

    let report = build_report().await;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report, args.verbose);
    }

    if report.summary.failed > 0 {
        anyhow::bail!("doctor found {} failing check(s)", report.summary.failed);
    }
    Ok(())
}

/// `maestro doctor runtime [--json]` — the focused F-118 runtime-health view. The
/// report is validated BEFORE any output so a self-inconsistent report surfaces as
/// an internal error, never as half-structured JSON. Exit code follows the report:
/// any failing check is non-zero; warnings alone stay zero.
pub async fn run_runtime(json: bool) -> Result<()> {
    let generated_at = chrono::Utc::now().to_rfc3339();
    let report = crate::runtime_health::build_validated_report(generated_at)
        .await
        .context("internal error: runtime health report failed validation")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_runtime_report(&report));
    }

    if report.summary.failed > 0 {
        anyhow::bail!(
            "runtime health found {} failing check(s)",
            report.summary.failed
        );
    }
    Ok(())
}

/// Issue-first text rendering (fail, warn, pass, skip) preserving the stable v1
/// order within each status group, then a compact provider-probe summary.
fn render_runtime_report(report: &RuntimeHealthReport) -> String {
    let mut out = String::new();
    out.push_str("maestro runtime health\n\n");
    for status in [
        HealthStatus::Fail,
        HealthStatus::Warn,
        HealthStatus::Pass,
        HealthStatus::Skip,
    ] {
        for check in report.checks.iter().filter(|c| c.status == status) {
            out.push_str(&format!(
                "[{:<4}] {:<22} {}\n",
                check.status.as_str().to_uppercase(),
                check.label,
                check.message
            ));
            if let Some(fix) = &check.fix {
                out.push_str(&format!("       fix: {fix}\n"));
            }
        }
    }
    if !report.providers.is_empty() {
        out.push_str("\nproviders:\n");
        for p in &report.providers {
            let dur = p
                .probe
                .duration_ms
                .map(|ms| format!(" {ms}ms"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {:<8} {:<5}{} {}\n",
                p.id,
                p.probe.status.as_str(),
                dur,
                p.probe.message
            ));
        }
    }
    let s = &report.summary;
    out.push_str(&format!(
        "\nsummary: {} pass, {} warn, {} fail, {} skip ({} total)\n",
        s.passed, s.warnings, s.failed, s.skipped, s.total
    ));
    if let Some(top) = &s.top_issue {
        out.push_str(&format!("top issue: {top}\n"));
    }
    out
}

pub async fn build_report() -> DoctorReport {
    let workspace_root = paths::workspace_root().unwrap_or_else(|_| PathBuf::from("."));
    let mut checks = Vec::new();

    let projects = push_workspace_checks(&mut checks);
    push_project_checks(&mut checks, projects.as_ref());
    push_worktree_checks(&mut checks, projects.as_ref());
    push_plan_check(&mut checks);
    push_tool_checks(&mut checks, projects.as_ref()).await;
    push_model_checks(&mut checks, projects.as_ref());
    push_run_checks(&mut checks);

    let summary = summarize(&checks);
    DoctorReport {
        workspace_root: workspace_root.display().to_string(),
        checks,
        summary,
    }
}

fn push_workspace_checks(checks: &mut Vec<DoctorCheck>) -> Option<ProjectsConfig> {
    let root = match paths::workspace_root() {
        Ok(root) => root,
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "system",
                "workspace root",
                format!("could not resolve workspace root: {e:#}"),
                "run maestro from a real workspace directory",
            ));
            return None;
        }
    };
    checks.push(
        DoctorCheck::pass("system", "workspace root", root.display().to_string())
            .detail(format!("cwd source: {}", workspace_source())),
    );

    let dot = match paths::maestro_dir() {
        Ok(dot) => dot,
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "system",
                ".maestro directory",
                format!("could not resolve .maestro path: {e:#}"),
                "run `maestro init`",
            ));
            return None;
        }
    };
    if dot.exists() {
        checks.push(DoctorCheck::pass(
            "system",
            ".maestro directory",
            dot.display().to_string(),
        ));
    } else {
        checks.push(DoctorCheck::fail(
            "system",
            ".maestro directory",
            format!("missing {}", dot.display()),
            "run `maestro init`",
        ));
        return None;
    }

    let pfile = match paths::projects_file() {
        Ok(pfile) => pfile,
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "config",
                "projects.yaml",
                format!("could not resolve projects.yaml: {e:#}"),
                "run `maestro init`",
            ));
            return None;
        }
    };
    if !pfile.exists() {
        checks.push(DoctorCheck::fail(
            "config",
            "projects.yaml",
            format!("missing {}", pfile.display()),
            "run `maestro init`",
        ));
        return None;
    }
    match ProjectsConfig::load(&pfile) {
        Ok(cfg) => {
            checks.push(DoctorCheck::pass(
                "config",
                "projects.yaml",
                format!("parsed {} project(s)", cfg.projects.len()),
            ));
            if let Some(check) = stale_projects_tmp_check(&pfile, SystemTime::now()) {
                checks.push(check);
            }
            Some(cfg)
        }
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "config",
                "projects.yaml",
                format!("parse failed: {e:#}"),
                format!("edit {}", pfile.display()),
            ));
            None
        }
    }
}

fn stale_projects_tmp_check(pfile: &Path, now: SystemTime) -> Option<DoctorCheck> {
    let tmp_path = crate::config::projects::projects_tmp_path(pfile);
    if !tmp_path.exists() {
        return None;
    }
    let modified = std::fs::metadata(&tmp_path).ok()?.modified().ok()?;
    let age = now.duration_since(modified).ok()?;
    if age < STALE_PROJECTS_TMP_THRESHOLD {
        return None;
    }
    Some(DoctorCheck::warn(
        "config",
        "projects.yaml.tmp",
        format!(
            "stale temp file {} is older than {}s",
            tmp_path.display(),
            STALE_PROJECTS_TMP_THRESHOLD.as_secs()
        ),
        format!(
            "remove {} after confirming .maestro/projects.yaml is valid",
            tmp_path.display()
        ),
    ))
}

fn push_project_checks(checks: &mut Vec<DoctorCheck>, projects: Option<&ProjectsConfig>) {
    let Some(cfg) = projects else {
        checks.push(DoctorCheck::skip(
            "config",
            "project registry",
            "projects.yaml is unavailable",
        ));
        return;
    };
    if cfg.projects.is_empty() {
        checks.push(DoctorCheck::warn(
            "config",
            "project registry",
            "no projects registered",
            "run `maestro add <path>` or `maestro work --root <dir>`",
        ));
        return;
    }

    let mut missing_paths = Vec::new();
    let mut bad_deps = Vec::new();
    let names: BTreeSet<_> = cfg.projects.keys().cloned().collect();
    for (name, project) in &cfg.projects {
        match paths::expand(&project.path) {
            Ok(path) if path.exists() => {}
            Ok(path) => missing_paths.push(format!("{name}: {}", path.display())),
            Err(e) => missing_paths.push(format!("{name}: path expansion failed: {e:#}")),
        }
        for dep in &project.dependencies {
            if !names.contains(dep) {
                bad_deps.push(format!("{name} -> {dep}"));
            }
        }
    }

    if missing_paths.is_empty() {
        checks.push(DoctorCheck::pass(
            "config",
            "project paths",
            format!("{} path(s) exist", cfg.projects.len()),
        ));
    } else {
        let mut check = DoctorCheck::fail(
            "config",
            "project paths",
            format!("{} missing or invalid path(s)", missing_paths.len()),
            "fix paths in .maestro/projects.yaml",
        );
        check.details = missing_paths;
        checks.push(check);
    }

    if bad_deps.is_empty() {
        checks.push(DoctorCheck::pass(
            "config",
            "project dependencies",
            "all declared dependencies resolve",
        ));
    } else {
        let mut check = DoctorCheck::fail(
            "config",
            "project dependencies",
            format!(
                "{} dependency edge(s) reference unknown projects",
                bad_deps.len()
            ),
            "fix dependencies in .maestro/projects.yaml",
        );
        check.details = bad_deps;
        checks.push(check);
    }
}

fn push_worktree_checks(checks: &mut Vec<DoctorCheck>, projects: Option<&ProjectsConfig>) {
    let Some(projects) = projects else {
        checks.push(DoctorCheck::skip(
            "worktree",
            "policy",
            "projects.yaml unavailable",
        ));
        return;
    };
    checks.push(DoctorCheck::pass(
        "worktree",
        "deny list",
        format!("deny_list_patterns: {}", DENY_PATTERNS.len()),
    ));

    let mut copy_files =
        DoctorCheck::pass("worktree", "copy_files", "effective_copy_files_per_project");
    for (name, project) in &projects.projects {
        match WorktreePolicy::from_config(&projects.defaults, project) {
            Ok(policy) => {
                copy_files =
                    copy_files.detail(format!("{name}: {}", policy.effective_copy_files().len()));
            }
            Err(e) => {
                checks.push(DoctorCheck::fail(
                    "worktree",
                    "copy_files",
                    format!("{name}: {e}"),
                    "report this bug; built-in worktree policy failed to initialize",
                ));
            }
        }
    }
    checks.push(copy_files);
}

fn push_plan_check(checks: &mut Vec<DoctorCheck>) {
    let path = match paths::maestro_dir() {
        Ok(dir) => dir.join("PLAN.yaml"),
        Err(e) => {
            checks.push(DoctorCheck::skip(
                "plan",
                "PLAN.yaml",
                format!("could not resolve .maestro directory: {e:#}"),
            ));
            return;
        }
    };
    if !path.exists() {
        checks.push(DoctorCheck::skip(
            "plan",
            "PLAN.yaml",
            "no .maestro/PLAN.yaml yet",
        ));
        return;
    }
    match crate::config::Plan::load(&path) {
        Ok(plan) => checks.push(DoctorCheck::pass(
            "plan",
            "PLAN.yaml",
            format!("parsed {} task(s)", plan.tasks.len()),
        )),
        Err(e) => checks.push(DoctorCheck::fail(
            "plan",
            "PLAN.yaml",
            format!("parse failed: {e:#}"),
            format!("edit {} or regenerate with `maestro demo`", path.display()),
        )),
    }
}

async fn push_tool_checks(checks: &mut Vec<DoctorCheck>, projects: Option<&ProjectsConfig>) {
    checks.push(check_binary("tools", "bash", "bash", true).await);
    checks.push(check_binary("tools", "git", "git", true).await);
    checks.push(check_binary("tools", "gh", "gh", false).await);
    checks.push(check_git_repository().await);
    checks.push(check_git_worktree().await);

    let required_agents = required_agents(projects);
    for agent in ["codex", "cursor", "claude"] {
        let required = required_agents.contains(agent);
        let (env_name, default_binary) = match agent {
            "codex" => ("MAESTRO_CODEX", "codex"),
            "cursor" => ("MAESTRO_CURSOR_AGENT", "cursor-agent"),
            "claude" => ("MAESTRO_CLAUDE", "claude"),
            _ => unreachable!(),
        };
        let binary = std::env::var(env_name).unwrap_or_else(|_| default_binary.to_string());
        let mut check = check_binary("agents", agent, &binary, required).await;
        check
            .details
            .push(format!("{env_name}: {}", env_value(env_name)));
        checks.push(check);
    }

    checks.push(check_codegraph_engines());
}

/// Report the code-graph engines so `doctor` tells the user what powers the
/// Code Graph tab and how to upgrade it (codegraph is free; understand uses
/// tokens). The native engine always works, so this is informational.
fn check_codegraph_engines() -> DoctorCheck {
    let root =
        crate::codegraph::find_repo_root(&std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let engines = crate::codegraph::engine_status(&root);
    let active = engines
        .iter()
        .find(|e| e.active)
        .map(|e| e.engine.clone())
        .unwrap_or_else(|| "native".into());
    let mut check = DoctorCheck::pass(
        "tools",
        "code graph",
        format!("active engine: {active} (native always works, zero tokens)"),
    );
    for e in &engines {
        let state = if e.built {
            "built"
        } else if e.installed {
            "installed"
        } else {
            "not installed"
        };
        check.details.push(format!(
            "{} [{}] — {} · {}",
            e.engine, e.cost, state, e.hint
        ));
    }
    check
}

fn push_model_checks(checks: &mut Vec<DoctorCheck>, projects: Option<&ProjectsConfig>) {
    if crate::models::cache_missing_or_empty() {
        checks.push(DoctorCheck::warn(
            "models",
            "model cache",
            "cursor model cache is missing or empty",
            "run `maestro models --refresh`",
        ));
    } else {
        let count = crate::models::load_cached().map(|m| m.len()).unwrap_or(0);
        checks.push(DoctorCheck::pass(
            "models",
            "model cache",
            format!("{count} cached model(s)"),
        ));
    }

    let Some(cfg) = projects else {
        checks.push(DoctorCheck::skip(
            "models",
            "configured model ids",
            "projects.yaml is unavailable",
        ));
        return;
    };
    let cached = crate::models::load_cached().unwrap_or_default();
    let mut unknown = Vec::new();

    if let Some(model) = cfg
        .defaults
        .agent_model
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        push_unknown_model(&mut unknown, "defaults.agent_model", model, &cached);
    }
    if let Some(model) = cfg
        .defaults
        .cursor_model
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        push_unknown_model(&mut unknown, "defaults.cursor_model", model, &cached);
    }
    if let Some(model) = cfg
        .defaults
        .tagger_model
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        push_unknown_model(&mut unknown, "defaults.tagger_model", model, &cached);
    }
    for (profile_name, profile) in &cfg.defaults.model_profiles {
        for model in profile.candidates() {
            push_unknown_model(
                &mut unknown,
                &format!("defaults.model_profiles.{profile_name}"),
                &model,
                &cached,
            );
        }
    }
    for (name, project) in &cfg.projects {
        if let Some(model) = project.agent_model.as_deref().filter(|m| !m.is_empty()) {
            push_unknown_model(
                &mut unknown,
                &format!("projects.{name}.agent_model"),
                model,
                &cached,
            );
        }
        if let Some(model) = project.cursor_model.as_deref().filter(|m| !m.is_empty()) {
            push_unknown_model(
                &mut unknown,
                &format!("projects.{name}.cursor_model"),
                model,
                &cached,
            );
        }
    }

    let mut missing_profiles = Vec::new();
    if let Some(profile) = cfg
        .defaults
        .model_profile
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        push_missing_profile(
            &mut missing_profiles,
            "defaults.model_profile",
            profile,
            cfg,
        );
    }
    for (name, project) in &cfg.projects {
        if let Some(profile) = project
            .model_profile
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            push_missing_profile(
                &mut missing_profiles,
                &format!("projects.{name}.model_profile"),
                profile,
                cfg,
            );
        }
    }

    if missing_profiles.is_empty() {
        checks.push(DoctorCheck::pass(
            "models",
            "model profiles",
            "configured model profiles resolve",
        ));
    } else {
        let mut check = DoctorCheck::warn(
            "models",
            "model profiles",
            format!(
                "{} configured model profile reference(s) are missing",
                missing_profiles.len()
            ),
            "define the profile under defaults.model_profiles or fix the reference",
        );
        check.details = missing_profiles;
        checks.push(check);
    }

    if unknown.is_empty() {
        checks.push(DoctorCheck::pass(
            "models",
            "configured model ids",
            "configured model ids are known or unset",
        ));
    } else {
        let mut check = DoctorCheck::warn(
            "models",
            "configured model ids",
            format!("{} configured model id(s) are not cached", unknown.len()),
            "run `maestro models --refresh` or correct the model id",
        );
        check.details = unknown;
        checks.push(check);
    }
}

/// F-117: warning-only health for the current run's resume guard. Only flags an
/// ABANDONED run whose descriptor cannot cleanly seed a resume (truncated/rewritten
/// ledger, a settle since the descriptor, plan drift, missing/unsafe output
/// snapshot, task-set mismatch, corrupt descriptor). A live run is skipped (its
/// descriptor legitimately lags between settles), and a missing descriptor is not
/// a warning (pre-F-117 or a just-created run). Never fails doctor.
fn resume_descriptor_check(dir: &Path, state: &crate::scheduler::RunState) -> Option<DoctorCheck> {
    if matches!(
        crate::scheduler::liveness::classify_run(state),
        RunLiveness::Live
    ) {
        return None;
    }
    let report = crate::scheduler::resume::validate_resume_target(dir, true).ok()?;
    if report.has(crate::schema::resume::ResumeIssueCode::MissingDescriptor) {
        return None;
    }
    // force=true so liveness/info codes drop out; only real integrity issues stay.
    let problems: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.code.blocks(true))
        .map(|i| format!("[{}] {}", i.code.as_str(), i.detail))
        .collect();
    if problems.is_empty() {
        return None;
    }
    let mut check = DoctorCheck::warn(
        "runs",
        "resume guard",
        format!(
            "run {} cannot be cleanly resumed ({} issue(s))",
            report.run_id,
            problems.len()
        ),
        "use `maestro rerun` if the run is terminal, otherwise re-run the goal",
    );
    for p in problems {
        check = check.detail(p);
    }
    Some(check)
}

fn push_run_checks(checks: &mut Vec<DoctorCheck>) {
    let current = match paths::current_run_dir() {
        Ok(current) => current,
        Err(e) => {
            checks.push(DoctorCheck::warn(
                "runs",
                "current run",
                format!("current run link could not be resolved: {e:#}"),
                "remove or fix .maestro/runs/current",
            ));
            None
        }
    };
    if let Some(dir) = current {
        match crate::scheduler::RunState::load(&dir) {
            Ok(state) => {
                let status = format!("{:?}", state.status);
                let mut check = if state.approvals_pending.is_empty() {
                    DoctorCheck::pass(
                        "runs",
                        "current run",
                        format!("{} ({status})", state.run_id),
                    )
                } else {
                    DoctorCheck::warn(
                        "runs",
                        "current run",
                        format!(
                            "{} has {} pending approval(s)",
                            state.run_id,
                            state.approvals_pending.len()
                        ),
                        "approve pending tasks or cancel the run",
                    )
                };
                check.details.push(format!("spec: {}", state.spec));
                checks.push(check);
                checks.extend(schema_version_checks(&dir, false));
                if let Some(c) = resume_descriptor_check(&dir, &state) {
                    checks.push(c);
                }
            }
            Err(e) => checks.push(DoctorCheck::warn(
                "runs",
                "current run",
                format!("could not load run state from {}: {e:#}", dir.display()),
                "inspect or remove .maestro/runs/current",
            )),
        }
    } else {
        checks.push(DoctorCheck::pass("runs", "current run", "no current run"));
    }
    push_run_liveness_checks(checks);
}

fn push_run_liveness_checks(checks: &mut Vec<DoctorCheck>) {
    let Ok(runs_dir) = paths::runs_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() == std::ffi::OsStr::new(paths::CURRENT_LINK) {
            continue;
        }
        let run_dir = entry.path();
        if !run_dir.is_dir() {
            continue;
        }
        let Ok(state) = crate::scheduler::RunState::load(&run_dir) else {
            continue;
        };
        push_run_liveness_check(checks, &run_dir, &state);
    }
}

fn push_run_liveness_check(
    checks: &mut Vec<DoctorCheck>,
    run_dir: &Path,
    state: &crate::scheduler::RunState,
) {
    if state.status != RunStatus::Running {
        return;
    }
    match crate::scheduler::classify_run(state) {
        RunLiveness::Live => {}
        RunLiveness::UnknownLegacy => checks.push(
            DoctorCheck::skip(
                "runs",
                "run liveness",
                format!(
                    "{} is running but has no pid field (legacy state)",
                    state.run_id
                ),
            )
            .detail(format!("path: {}", run_dir.display())),
        ),
        RunLiveness::Abandoned => {
            let fix = format!(
                "restart with `maestro work --force-new {:?}` or drop the dir: rm -rf {}",
                state.spec,
                run_dir.display()
            );
            checks.push(
                DoctorCheck::warn(
                    "runs",
                    "abandoned run",
                    format!(
                        "{} status: running, pid {} is dead",
                        state.run_id, state.pid
                    ),
                    fix,
                )
                .detail(format!("path: {}", run_dir.display()))
                .detail(format!("spec: {}", state.spec)),
            );
        }
    }
}

fn schema_version_checks(run_dir: &Path, verbose: bool) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    check_jsonl_schema_versions(
        &mut checks,
        &run_dir.join(paths::RUN_EVENTS_FILE),
        "run events",
        &[crate::schema::RUN_EVENT_V1, crate::schema::RUN_EVENT_V2],
        verbose,
    );
    check_json_schema_version(
        &mut checks,
        &run_dir.join("evidence").join("summary.json"),
        "evidence summary",
        &[],
        verbose,
    );
    check_json_schema_version(
        &mut checks,
        &run_dir.join("evidence").join("artifacts.json"),
        "artifact manifest",
        &[crate::schema::ARTIFACT_MANIFEST_V1],
        verbose,
    );
    check_jsonl_schema_versions(
        &mut checks,
        &run_dir.join("channel_envelopes.ndjson"),
        "channel_envelope.v1",
        &[crate::schema::CHANNEL_ENVELOPE_V1],
        verbose,
    );
    let trajectories_dir = run_dir.join(paths::TRAJECTORIES_DIR);
    if let Ok(entries) = std::fs::read_dir(&trajectories_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("ndjson") {
                continue;
            }
            check_jsonl_schema_versions(
                &mut checks,
                &path,
                "trajectory events",
                &[crate::schema::TRAJECTORY_EVENT_V1],
                verbose,
            );
        }
    }
    checks
}

fn check_jsonl_schema_versions(
    checks: &mut Vec<DoctorCheck>,
    path: &Path,
    name: &'static str,
    supported: &[&str],
    verbose: bool,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        check_schema_value(checks, name, path, Some(idx + 1), &json, supported, verbose);
    }
}

fn check_json_schema_version(
    checks: &mut Vec<DoctorCheck>,
    path: &Path,
    name: &'static str,
    supported: &[&str],
    verbose: bool,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    check_schema_value(checks, name, path, None, &json, supported, verbose);
}

fn check_schema_value(
    checks: &mut Vec<DoctorCheck>,
    name: &'static str,
    path: &Path,
    line: Option<usize>,
    json: &serde_json::Value,
    supported: &[&str],
    verbose: bool,
) {
    let location = match line {
        Some(line) => format!("{} line {line}", path.display()),
        None => path.display().to_string(),
    };
    let Some(version) = json.get("schema_version").and_then(|v| v.as_str()) else {
        if verbose {
            checks.push(DoctorCheck::warn(
                "schemas",
                name,
                format!("{location} is missing schema_version and will be treated as legacy"),
                "regenerate the run evidence with the current Maestro version",
            ));
        }
        return;
    };
    if supported.contains(&version) || supported.is_empty() {
        return;
    }
    checks.push(DoctorCheck::warn(
        "schemas",
        name,
        format!("{location} uses unsupported schema version {version}"),
        "run the matching Maestro version or apply the documented schema migration",
    ));
}

async fn check_binary(
    category: &'static str,
    name: &'static str,
    binary: &str,
    required: bool,
) -> DoctorCheck {
    let Some(path) = crate::providers::find_binary(binary) else {
        let message = format!("`{binary}` not found on PATH");
        let fix = missing_binary_fix(name, binary, required);
        return if required {
            DoctorCheck::fail(category, name, message, fix)
        } else {
            DoctorCheck::warn(category, name, message, fix)
        };
    };

    let version = command_first_line(&path, &["--version"]).await;
    let mut check = DoctorCheck::pass(category, name, path.display().to_string());
    if let Some(version) = version {
        check.details.push(version);
    }
    check
}

fn missing_binary_fix(name: &str, binary: &str, required: bool) -> String {
    match name {
        "cursor" => "install Cursor CLI so `cursor-agent --version` works, or set MAESTRO_CURSOR_AGENT=/path/to/cursor-agent".to_string(),
        "codex" => "install OpenAI Codex CLI so `codex --version` works, or set MAESTRO_CODEX=/path/to/codex".to_string(),
        "claude" => "install Claude Code CLI so `claude --version` works, or set MAESTRO_CLAUDE=/path/to/claude".to_string(),
        "gh" => "install GitHub CLI from https://cli.github.com/ and run `gh auth login`".to_string(),
        "git" => "install git and ensure `git --version` works on PATH".to_string(),
        "bash" => {
            "install bash or run maestro from an environment with POSIX shell tools".to_string()
        }
        _ if required => format!("install `{binary}` or configure the related MAESTRO_* env var"),
        _ => format!("install `{binary}` to enable this optional integration"),
    }
}

async fn check_git_repository() -> DoctorCheck {
    if crate::providers::find_binary("git").is_none() {
        return DoctorCheck::skip("tools", "git repository", "git is unavailable");
    }
    match command_status("git", &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(true) => DoctorCheck::pass("tools", "git repository", "current workspace is inside git"),
        Ok(false) => DoctorCheck::warn(
            "tools",
            "git repository",
            "current workspace is not a git repository",
            "run `git init` if you want branch/worktree isolation; otherwise set max_parallel=1",
        ),
        Err(e) => DoctorCheck::warn(
            "tools",
            "git repository",
            format!("could not check git repository: {e:#}"),
            "verify `git rev-parse --is-inside-work-tree` manually",
        ),
    }
}

async fn check_git_worktree() -> DoctorCheck {
    if crate::providers::find_binary("git").is_none() {
        return DoctorCheck::skip("tools", "git worktree", "git is unavailable");
    }
    match command_status("git", &["worktree", "list"]).await {
        Ok(true) => DoctorCheck::pass("tools", "git worktree", "git worktree list succeeded"),
        Ok(false) => DoctorCheck::warn(
            "tools",
            "git worktree",
            "git worktree list exited non-zero",
            "upgrade git or disable worktree isolation for affected runs",
        ),
        Err(e) => DoctorCheck::warn(
            "tools",
            "git worktree",
            format!("could not check git worktree: {e:#}"),
            "verify `git worktree list` manually",
        ),
    }
}

fn required_agents(projects: Option<&ProjectsConfig>) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    let Some(cfg) = projects else {
        out.insert("cursor");
        out.insert("codex");
        return out;
    };
    push_agent_name(&mut out, &cfg.defaults.agent);
    for project in cfg.projects.values() {
        if let Some(agent) = project.agent.as_deref() {
            push_agent_name(&mut out, agent);
        }
    }
    out
}

fn push_agent_name(out: &mut BTreeSet<&'static str>, agent: &str) {
    match agent {
        "codex" => {
            out.insert("codex");
        }
        "cursor" => {
            out.insert("cursor");
        }
        "claude" => {
            out.insert("claude");
        }
        "mock" | "shell" => {}
        _ => {}
    }
}

fn push_unknown_model(
    unknown: &mut Vec<String>,
    source: &str,
    model: &str,
    cached: &[crate::models::ModelInfo],
) {
    if cached
        .iter()
        .any(|m| m.id == model || m.aliases.iter().any(|alias| alias == model))
    {
        return;
    }
    let suggestion = util::closest_model(model, cached)
        .map(|s| format!(" (did you mean `{s}`?)"))
        .unwrap_or_default();
    unknown.push(format!("{source}: `{model}`{suggestion}"));
}

fn push_missing_profile(
    missing: &mut Vec<String>,
    source: &str,
    profile: &str,
    cfg: &ProjectsConfig,
) {
    if cfg.defaults.model_profiles.contains_key(profile.trim()) {
        return;
    }
    missing.push(format!("{source}: `{}`", profile.trim()));
}

async fn command_first_line(binary: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new(binary)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn command_status(binary: &str, args: &[&str]) -> Result<bool> {
    let output = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new(binary)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .context("command timed out")??;
    Ok(output.status.success())
}

fn workspace_source() -> &'static str {
    if std::env::var("MAESTRO_WORKSPACE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        "MAESTRO_WORKSPACE_ROOT"
    } else {
        "cwd"
    }
}

fn env_value(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| "(unset)".into())
}

fn summarize(checks: &[DoctorCheck]) -> DoctorSummary {
    DoctorSummary {
        total: checks.len(),
        passed: checks
            .iter()
            .filter(|c| c.status == CheckStatus::Pass)
            .count(),
        warnings: checks
            .iter()
            .filter(|c| c.status == CheckStatus::Warn)
            .count(),
        failed: checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .count(),
        skipped: checks
            .iter()
            .filter(|c| c.status == CheckStatus::Skip)
            .count(),
    }
}

fn print_text_report(report: &DoctorReport, verbose: bool) {
    print!("{}", render_text_report(report, verbose));
}

fn render_text_report(report: &DoctorReport, verbose: bool) -> String {
    let mut out = String::new();
    out.push_str("maestro doctor\n");
    out.push_str(&format!("workspace: {}\n\n", report.workspace_root));
    for check in &report.checks {
        out.push_str(&format!(
            "[{}] {:<7} {:<22} {}",
            check.status.label(),
            check.category,
            check.name,
            check.message
        ));
        out.push('\n');
        if verbose || check.status != CheckStatus::Pass {
            for detail in &check.details {
                out.push_str(&format!("  - {detail}\n"));
            }
            if let Some(fix) = &check.fix {
                out.push_str(&format!("  fix: {fix}\n"));
            }
        }
    }
    out.push('\n');
    out.push_str(&format!(
        "summary: {} pass, {} warn, {} fail, {} skip ({} total)",
        report.summary.passed,
        report.summary.warnings,
        report.summary.failed,
        report.summary.skipped,
        report.summary.total
    ));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn mk_state(run_id: &str, pid: u32) -> crate::scheduler::RunState {
        serde_json::from_value(serde_json::json!({
            "run_id": run_id, "spec": "demo", "started_at": "2026-06-05T00:00:00Z",
            "ended_at": null, "status": "running", "max_parallel": 1, "pid": pid,
            "tasks": {}, "approvals_pending": [], "task_order": [],
        }))
        .unwrap()
    }

    #[test]
    fn resume_descriptor_check_skips_live_run() {
        // A live run's descriptor legitimately lags; never warn on it.
        let temp = tempfile::tempdir().unwrap();
        let state = mk_state("r1", std::process::id());
        assert!(resume_descriptor_check(temp.path(), &state).is_none());
    }

    #[test]
    fn resume_descriptor_check_silent_on_missing_descriptor() {
        // Abandoned run, no RESUME.json (pre-F-117 / just created) -> not a warning.
        let temp = tempfile::tempdir().unwrap();
        let state = mk_state("r1", 999_999);
        assert!(
            resume_descriptor_check(temp.path(), &state).is_none(),
            "missing descriptor is not a warning"
        );
    }

    #[test]
    fn resume_descriptor_check_warns_on_corrupt_descriptor() {
        // Abandoned run + an unusable descriptor -> a (non-blocking) doctor warning.
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            crate::scheduler::resume::descriptor_path(temp.path()),
            "{not json",
        )
        .unwrap();
        let state = mk_state("r1", 999_999);
        let check = resume_descriptor_check(temp.path(), &state).expect("warns");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(
            check
                .details
                .iter()
                .any(|d| d.contains("resume.schema_mismatch")),
            "details: {:?}",
            check.details
        );
    }

    #[test]
    fn summary_counts_statuses() {
        let checks = vec![
            DoctorCheck::pass("x", "a", "ok"),
            DoctorCheck::warn("x", "b", "warn", "fix"),
            DoctorCheck::fail("x", "c", "fail", "fix"),
            DoctorCheck::skip("x", "d", "skip"),
        ];
        let summary = summarize(&checks);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.warnings, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
    }

    #[test]
    fn unknown_model_suggests_close_cached_id() {
        let cached = vec![crate::models::ModelInfo {
            id: "gpt-5.2".into(),
            label: None,
            aliases: vec![],
            provider: Some("cursor".into()),
        }];
        let mut unknown = Vec::new();
        push_unknown_model(&mut unknown, "defaults.cursor_model", "gpt5.2", &cached);
        assert_eq!(unknown.len(), 1);
        assert!(unknown[0].contains("gpt-5.2"));
    }

    #[test]
    fn unknown_model_accepts_aliases() {
        let cached = vec![crate::models::ModelInfo {
            id: "gpt-5.2".into(),
            label: None,
            aliases: vec!["gpt5".into()],
            provider: Some("cursor".into()),
        }];
        let mut unknown = Vec::new();
        push_unknown_model(&mut unknown, "defaults.cursor_model", "gpt5", &cached);
        assert!(unknown.is_empty());
    }

    #[test]
    fn output_contains_fix_hints() {
        let checks = vec![DoctorCheck::fail(
            "agents",
            "codex",
            "`codex` not found on PATH",
            "install OpenAI Codex CLI",
        )];
        let report = DoctorReport {
            workspace_root: "/tmp/demo".into(),
            summary: summarize(&checks),
            checks,
        };
        let output = render_text_report(&report, false);
        assert!(output.contains("fix: install OpenAI Codex CLI"));
    }

    #[test]
    fn schema_version_checks_warn_on_future_versions() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(crate::paths::RUN_EVENTS_FILE),
            r#"{"schema_version":"maestro.run_event.v3","seq":1,"kind":"run.started"}"#,
        )
        .unwrap();
        let trajectories = temp.path().join(crate::paths::TRAJECTORIES_DIR);
        std::fs::create_dir_all(&trajectories).unwrap();
        std::fs::write(
            trajectories.join("T_trace.ndjson"),
            r#"{"schema_version":"maestro.trajectory_event.v2","seq":1,"kind":"command"}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("channel_envelopes.ndjson"),
            r#"{"schema_version":"maestro.channel_envelope.v2","channel":"feishu"}"#,
        )
        .unwrap();

        let checks = schema_version_checks(temp.path(), false);
        assert!(checks.iter().any(|check| {
            check.status == CheckStatus::Warn
                && check.message.contains("maestro.run_event.v3")
                && check
                    .fix
                    .as_deref()
                    .unwrap_or_default()
                    .contains("migration")
        }));
        assert!(checks.iter().any(|check| {
            check.status == CheckStatus::Warn
                && check.message.contains("maestro.trajectory_event.v2")
                && check
                    .fix
                    .as_deref()
                    .unwrap_or_default()
                    .contains("migration")
        }));
        assert!(checks.iter().any(|check| {
            check.status == CheckStatus::Warn
                && check.name == "channel_envelope.v1"
                && check.message.contains("maestro.channel_envelope.v2")
        }));
    }

    #[test]
    fn schema_version_checks_accept_current_run_event_v2() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(crate::paths::RUN_EVENTS_FILE),
            r#"{"schema_version":"maestro.run_event.v2","seq":1,"kind":"run.started"}"#,
        )
        .unwrap();
        let checks = schema_version_checks(temp.path(), false);
        assert!(
            !checks.iter().any(|check| {
                check.status == CheckStatus::Warn && check.message.contains("maestro.run_event.v2")
            }),
            "run_event.v2 is the current version and must not warn"
        );
    }

    #[test]
    fn schema_version_checks_accept_legacy_missing_versions() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(crate::paths::RUN_EVENTS_FILE),
            r#"{"seq":1,"kind":"run_created"}"#,
        )
        .unwrap();

        let checks = schema_version_checks(temp.path(), false);
        assert!(
            checks.iter().all(|check| check.status != CheckStatus::Fail),
            "legacy missing schema_version should not fail"
        );
        assert!(
            checks
                .iter()
                .all(|check| !check.message.contains("missing")),
            "non-verbose legacy files should not warn"
        );
    }

    #[test]
    fn stale_projects_tmp_check_warns_when_tmp_exceeds_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let pfile = temp.path().join(".maestro/projects.yaml");
        std::fs::create_dir_all(pfile.parent().unwrap()).unwrap();
        let tmp = crate::config::projects::projects_tmp_path(&pfile);
        std::fs::write(&tmp, "pending").unwrap();
        let modified = std::fs::metadata(&tmp).unwrap().modified().unwrap();
        let now = modified + STALE_PROJECTS_TMP_THRESHOLD + Duration::from_secs(1);

        let check = stale_projects_tmp_check(&pfile, now).unwrap();

        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(check.name, "projects.yaml.tmp");
        assert!(check.message.contains("projects.yaml.tmp"));
        assert!(check.fix.as_deref().unwrap_or_default().contains("remove"));
    }

    #[test]
    fn stale_projects_tmp_check_ignores_fresh_tmp() {
        let temp = tempfile::tempdir().unwrap();
        let pfile = temp.path().join(".maestro/projects.yaml");
        std::fs::create_dir_all(pfile.parent().unwrap()).unwrap();
        let tmp = crate::config::projects::projects_tmp_path(&pfile);
        std::fs::write(&tmp, "pending").unwrap();
        let modified = std::fs::metadata(&tmp).unwrap().modified().unwrap();

        assert!(stale_projects_tmp_check(&pfile, modified).is_none());
    }

    #[test]
    #[serial] // mutates the process-global MAESTRO_WORKSPACE_ROOT; must not race
              // with other env-mutating tests (bench/plan/work all use #[serial]).
    fn run_checks_warn_when_current_run_is_abandoned() {
        let temp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", temp.path());
        }
        let run_dir = temp.path().join(".maestro/runs/dead-run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(temp.path().join(".maestro/runs/current"), "dead-run").unwrap();
        std::fs::write(
            run_dir.join(crate::paths::RUN_STATE_FILE),
            r#"{
  "run_id": "dead-run",
  "spec": "same goal",
  "started_at": "2026-05-25T00:00:00Z",
  "ended_at": null,
  "status": "running",
  "max_parallel": 1,
  "tasks": {},
  "approvals_pending": [],
  "task_order": [],
  "pid": 999999,
  "verified": false
}"#,
        )
        .unwrap();

        let mut checks = Vec::new();
        push_run_checks(&mut checks);

        let abandoned = checks
            .iter()
            .find(|check| check.name == "abandoned run")
            .expect("abandoned run warning");
        assert_eq!(abandoned.status, CheckStatus::Warn);
        assert!(abandoned.message.contains("dead-run"));
        assert!(abandoned.message.contains("pid 999999 is dead"));
        let fix = abandoned.fix.as_deref().unwrap_or_default();
        assert!(fix.contains("maestro work --force-new"));
        assert!(fix.contains("rm -rf"));

        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[test]
    fn doctor_worktree_report_lists_deny_patterns_and_effective_copy_counts() {
        let projects: ProjectsConfig = serde_yaml::from_str(
            r#"
version: 1
defaults:
  copy_files: [.editorconfig]
projects:
  api:
    path: services/api
    copy_files: [.editorconfig, .vscode/settings.json]
"#,
        )
        .unwrap();
        let mut checks = Vec::new();
        push_worktree_checks(&mut checks, Some(&projects));
        let report = DoctorReport {
            workspace_root: "/tmp/demo".into(),
            summary: summarize(&checks),
            checks,
        };
        let output = render_text_report(&report, true);

        assert!(output.contains("deny_list_patterns: 10"));
        assert!(output.contains("api: 2"));
    }
}
