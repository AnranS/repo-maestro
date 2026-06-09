//! F-118 runtime health builder.
//!
//! Step 1 is the **safe provider projection** — turning a `ProviderStatus` into a
//! UI/MCP-safe `ProviderHealth` that never reveals an absolute binary path or the
//! env var that pointed at it.
//!
//! Step 2 adds the **timed local provider probe** (a no-network, no-prompt
//! `--version`-style liveness check with a 2s deadline) and the full read-only
//! report assembly: six readiness gatherers that REUSE the existing validators
//! (workspace/config, provider registry + probe, F-114 profiles/skills, F-111
//! plan preview, F-117 resume guard) without mutating any workspace state. Every
//! check message is a controlled short string — counts and neutral phrasing only,
//! never a raw validator message, path, env value, or provider stdout/stderr. The
//! schema validator is the backstop, not the primary redaction. `generated_at` is
//! caller-supplied so assembly stays deterministic in tests.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::config::ProjectsConfig;
use crate::paths;
use crate::profile_visibility::ProfileExistenceReport;
use crate::providers::{ProviderKind, ProviderStatus, ToolTraceSupport};
use crate::schema::runtime_health::{
    is_symbol_token, summarize, validate_report, HealthCheckId, HealthRef, HealthStatus,
    ProbeResult, ProviderCapabilitySummary, ProviderHealth, RuntimeHealthCheck,
    RuntimeHealthReport,
};

/// Per-provider local probe deadline (design: 2 seconds, no model/prompt/network).
pub const PROBE_DEADLINE: Duration = Duration::from_secs(2);

/// The task-adapter ids whose local liveness gates `provider.available`. Built-in
/// `mock`/`shell` and the real CLIs all live here; KnownCli providers are excluded.
const TASK_ADAPTER_IDS: [&str; 4] = ["codex", "cursor", "shell", "mock"];

/// Project a `ProviderStatus` into a UI/MCP-safe `ProviderHealth`. Drops the
/// ENTIRE `execution` sub-struct (binary / path / env_override) — the report
/// only says installed / adapter-available + capability booleans, never where a
/// provider was found or which env var pointed at it. The probe is supplied by
/// the caller (Step 2 passes the timed result; pass a skip placeholder here).
pub fn project_provider(status: &ProviderStatus, probe: ProbeResult) -> ProviderHealth {
    ProviderHealth {
        id: status.id.to_string(),
        display: status.display.to_string(),
        kind: match status.kind {
            ProviderKind::TaskAdapter => "task_adapter",
            ProviderKind::KnownCli => "known_cli",
        }
        .to_string(),
        adapter_available: status.adapter_available,
        installed: status.installed,
        probe,
        capabilities: ProviderCapabilitySummary {
            model_override: status.models.supports_override,
            non_interactive: status.execution.non_interactive,
            streaming: status.execution.streaming,
            resume: status.execution.resume,
            worktree_isolation: status.execution.worktree_isolation,
            tool_trace: match status.tool_trace {
                ToolTraceSupport::Full => "full",
                ToolTraceSupport::Partial => "partial",
                ToolTraceSupport::None => "none",
            }
            .to_string(),
        },
        message: provider_message(status),
    }
}

/// A neutral one-line provider message — installed / adapter state only, never a
/// path or env value.
fn provider_message(status: &ProviderStatus) -> String {
    if !status.installed {
        "not installed".into()
    } else if !status.adapter_available {
        "installed; task adapter not wired".into()
    } else {
        "installed; adapter available".into()
    }
}

// ---------------------------------------------------------------------------
// Step 2 — timed local probe + read-only report assembly.
// ---------------------------------------------------------------------------

/// Build the full read-only `RuntimeHealthReport`. Spawns the timed provider
/// probe but mutates nothing; every gatherer reuses an existing validator. The
/// six checks are emitted in the stable `HealthCheckId::ALL` order. `generated_at`
/// is caller-supplied (the CLI/handler passes the real clock).
pub async fn build_report(generated_at: String) -> RuntimeHealthReport {
    let projects = load_projects();
    // F-114 role/skill existence — computed ONCE and shared by profiles + skills so
    // the on-disk registry reads happen a single time and both rows agree.
    let existence = projects
        .as_ref()
        .map(crate::profile_visibility::profile_existence_report);
    // PLAN.yaml task-level adapters feed the active-provider set (the plan is what
    // will actually run), alongside defaults/project agents.
    let plan_agents = plan_task_agent_names();
    let providers = probe_task_adapters(PROBE_DEADLINE).await;

    let checks = vec![
        workspace_check(projects.as_ref()),
        provider_check(projects.as_ref(), &providers, &plan_agents),
        profiles_check(projects.as_ref(), existence.as_ref()),
        skills_check(projects.as_ref(), existence.as_ref()),
        plan_check(projects.as_ref()),
        resume_check(),
    ];

    let summary = summarize(&checks);
    RuntimeHealthReport {
        schema_version: crate::schema::runtime_health_version(),
        generated_at,
        summary,
        checks,
        providers,
    }
}

/// Task-adapter agent ids explicitly named by `PLAN.yaml` tasks (read-only, parse
/// errors ignored — `plan.preview_valid` reports plan validity separately). Empty
/// when there is no readable plan.
fn plan_task_agent_names() -> Vec<String> {
    let Ok(dir) = paths::maestro_dir() else {
        return Vec::new();
    };
    let path = dir.join("PLAN.yaml");
    if !path.exists() {
        return Vec::new();
    }
    match crate::config::Plan::read_only(&path) {
        Ok(plan) => plan.tasks.iter().filter_map(|t| t.agent.clone()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Convenience for callers that want a validated report or a hard error — the CLI
/// and the WebUI handler use this so a self-inconsistent report never escapes as
/// half-structured JSON (the report is an internal error instead).
pub async fn build_validated_report(generated_at: String) -> anyhow::Result<RuntimeHealthReport> {
    let report = build_report(generated_at).await;
    validate_report(&report)?;
    Ok(report)
}

fn load_projects() -> Option<ProjectsConfig> {
    let pfile = paths::projects_file().ok()?;
    if !pfile.exists() {
        return None;
    }
    ProjectsConfig::load(&pfile).ok()
}

// --- provider probe ---------------------------------------------------------

/// Probe every task adapter concurrently (small fixed set) and project each into a
/// safe `ProviderHealth`. Results are sorted by id for a stable report.
async fn probe_task_adapters(deadline: Duration) -> Vec<ProviderHealth> {
    let statuses = crate::providers::provider_statuses(true);
    let futures = statuses.into_iter().map(|status| async move {
        let probe = probe_provider(&status, deadline).await;
        project_provider(&status, probe)
    });
    let mut out = futures::future::join_all(futures).await;
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Timed local liveness for one provider. Never runs a model/prompt/network call,
/// and never captures stdout/stderr — only pass/fail/timeout + duration. The probe
/// message is a fixed controlled string.
async fn probe_provider(status: &ProviderStatus, deadline: Duration) -> ProbeResult {
    // A built-in adapter with no binary (mock) is always locally live.
    if status.execution.binary.is_none() {
        return ProbeResult::timed(HealthStatus::Pass, 0, "built-in adapter");
    }
    // Without a resolved binary there is nothing to spawn; the gatherer decides
    // whether a missing active provider is a failure.
    let Some(path) = status.execution.path.as_deref() else {
        return ProbeResult::skipped("not installed");
    };
    // `shell` has no portable `--version`; confirm it can execute a trivial local
    // command instead. Real CLIs answer `--version` without network or a prompt.
    let args: &[&str] = if status.id == "shell" {
        &["-c", "exit 0"]
    } else {
        &["--version"]
    };
    match probe_binary(Path::new(path), args, deadline).await {
        ProbeOutcome::TimedOut(ms) => ProbeResult::timeout(ms),
        ProbeOutcome::Ok(ms) => ProbeResult::timed(HealthStatus::Pass, ms, "local liveness ok"),
        ProbeOutcome::Failed(ms) => {
            ProbeResult::timed(HealthStatus::Fail, ms, "local liveness check failed")
        }
    }
}

enum ProbeOutcome {
    Ok(u64),
    Failed(u64),
    TimedOut(u64),
}

/// Spawn `path args` with a deadline, discarding all stdio. `kill_on_drop` ensures
/// a timed-out probe never leaks a lingering child process.
async fn probe_binary(path: &Path, args: &[&str], deadline: Duration) -> ProbeOutcome {
    let start = Instant::now();
    let child = tokio::process::Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(deadline, child).await {
        Err(_) => ProbeOutcome::TimedOut(start.elapsed().as_millis() as u64),
        Ok(Err(_)) => ProbeOutcome::Failed(start.elapsed().as_millis() as u64),
        Ok(Ok(status)) if status.success() => ProbeOutcome::Ok(start.elapsed().as_millis() as u64),
        Ok(Ok(_)) => ProbeOutcome::Failed(start.elapsed().as_millis() as u64),
    }
}

// --- check gatherers --------------------------------------------------------

fn workspace_check(projects: Option<&ProjectsConfig>) -> RuntimeHealthCheck {
    let id = HealthCheckId::WorkspaceDetected;
    let dot_exists = paths::maestro_dir().map(|d| d.exists()).unwrap_or(false);
    if !dot_exists {
        return RuntimeHealthCheck::new(id, HealthStatus::Fail, "no .maestro workspace here")
            .fix("run `maestro init`")
            .with_ref(HealthRef::new("workspace", "workspace"));
    }
    let pfile_exists = paths::projects_file().map(|p| p.exists()).unwrap_or(false);
    if !pfile_exists {
        return RuntimeHealthCheck::new(id, HealthStatus::Fail, "projects.yaml is missing")
            .fix("run `maestro init`")
            .with_ref(HealthRef::new("config", "projects.yaml"));
    }
    match projects {
        Some(cfg) => RuntimeHealthCheck::new(
            id,
            HealthStatus::Pass,
            format!("workspace detected; {} project(s)", cfg.projects.len()),
        )
        .with_ref(HealthRef::new("config", "projects.yaml")),
        None => {
            RuntimeHealthCheck::new(id, HealthStatus::Fail, "projects.yaml could not be parsed")
                .fix("fix .maestro/projects.yaml")
                .with_ref(HealthRef::new("config", "projects.yaml"))
        }
    }
}

/// The task adapters whose liveness gates readiness: those named by
/// `defaults.agent`, a project `agent`, or a `PLAN.yaml` task `agent` (the plan is
/// what will actually run), falling back to codex+cursor when none is configured.
/// KnownCli providers (e.g. a chat-only CLI) never enter this blocking set.
fn active_task_provider_ids(
    projects: Option<&ProjectsConfig>,
    plan_agents: &[String],
) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    let mut consider = |name: &str| {
        if let Some(id) = TASK_ADAPTER_IDS.iter().find(|a| **a == name) {
            out.insert(*id);
        }
    };
    if let Some(cfg) = projects {
        consider(&cfg.defaults.agent);
        for project in cfg.projects.values() {
            if let Some(agent) = project.agent.as_deref() {
                consider(agent);
            }
        }
    }
    for agent in plan_agents {
        consider(agent);
    }
    if out.is_empty() {
        out.insert("codex");
        out.insert("cursor");
    }
    out
}

fn provider_check(
    projects: Option<&ProjectsConfig>,
    providers: &[ProviderHealth],
    plan_agents: &[String],
) -> RuntimeHealthCheck {
    let id = HealthCheckId::ProviderAvailable;
    let active = active_task_provider_ids(projects, plan_agents);
    let pool: Vec<&ProviderHealth> = providers
        .iter()
        .filter(|p| active.contains(p.id.as_str()))
        .collect();

    let ready: Vec<&str> = pool
        .iter()
        .filter(|p| p.probe.status == HealthStatus::Pass)
        .map(|p| p.id.as_str())
        .collect();
    let timed_out = pool.iter().any(|p| p.probe.status == HealthStatus::Warn);

    if !ready.is_empty() {
        let mut check = RuntimeHealthCheck::new(
            id,
            HealthStatus::Pass,
            format!("{} configured provider(s) ready", ready.len()),
        );
        for pid in ready.iter().take(3) {
            check = check.with_ref(HealthRef::new("provider", *pid));
        }
        check
    } else if timed_out {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Warn,
            "a configured provider is installed but its probe timed out",
        )
        .fix("re-run, or confirm the provider CLI starts locally")
    } else {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Fail,
            "no configured provider is installed",
        )
        .fix("install a provider CLI (e.g. codex or cursor-agent)")
    }
}

fn profiles_check(
    projects: Option<&ProjectsConfig>,
    existence: Option<&ProfileExistenceReport>,
) -> RuntimeHealthCheck {
    let id = HealthCheckId::ProfilesValid;
    let Some(cfg) = projects else {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "workspace config unavailable");
    };
    // Structural + cross-config issues (pure) PLUS on-disk role existence (F-114) —
    // a profile pinned to a missing role must not pass.
    let structural = cfg.agent_profile_issues().len();
    let missing_roles = existence.map(|e| e.role_issues.len()).unwrap_or(0);
    let total = structural + missing_roles;
    if total == 0 {
        RuntimeHealthCheck::new(id, HealthStatus::Pass, "agent profiles valid")
            .with_ref(HealthRef::new("config", "projects.yaml"))
    } else {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Warn,
            format!("{total} agent profile issue(s)"),
        )
        .fix("fix agent_profiles in .maestro/projects.yaml")
        .with_ref(HealthRef::new("config", "projects.yaml"))
    }
}

fn skills_check(
    projects: Option<&ProjectsConfig>,
    existence: Option<&ProfileExistenceReport>,
) -> RuntimeHealthCheck {
    let id = HealthCheckId::SkillsVisible;
    if projects.is_none() {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "workspace config unavailable");
    }
    let Some(report) = existence else {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "workspace config unavailable");
    };
    // F-114 scope-aware resolution lives in the shared helper: explicit scope is
    // exact, an unscoped skill is checked per pinning-project scope then global, and
    // an unscoped skill on an unreferenced (trigger-only) profile is deferred — it
    // is not counted here, so it can never false-warn the readiness row.
    let checked = report.checked_skill_refs.len();
    if checked == 0 {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "no skill references to verify");
    }
    if report.unresolved_skill_refs.is_empty() {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Pass,
            format!("{checked} skill reference(s) visible"),
        )
    } else {
        let mut check = RuntimeHealthCheck::new(
            id,
            HealthStatus::Warn,
            format!(
                "{} of {} skill reference(s) unresolved",
                report.unresolved_skill_refs.len(),
                checked
            ),
        )
        .fix("add the missing skill or fix the profile reference");
        for r in report.unresolved_skill_refs.iter().take(3) {
            // only emit a symbolic skill ref when both segments are clean tokens.
            if let Some((scope, name)) = r.split_once('/') {
                if is_symbol_token(scope) && is_symbol_token(name) {
                    check = check.with_ref(HealthRef::new("skill", format!("{scope}/{name}")));
                }
            }
        }
        check
    }
}

fn plan_check(projects: Option<&ProjectsConfig>) -> RuntimeHealthCheck {
    let id = HealthCheckId::PlanPreviewValid;
    let Ok(dir) = paths::maestro_dir() else {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "workspace unavailable");
    };
    let path = dir.join("PLAN.yaml");
    if !path.exists() {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "no PLAN.yaml");
    }
    let Some(cfg) = projects else {
        return RuntimeHealthCheck::new(id, HealthStatus::Skip, "workspace config unavailable");
    };
    let plan = match crate::config::Plan::read_only(&path) {
        Ok(plan) => plan,
        Err(_) => {
            return RuntimeHealthCheck::new(
                id,
                HealthStatus::Fail,
                "PLAN.yaml could not be parsed",
            )
            .fix("fix .maestro/PLAN.yaml")
            .with_ref(HealthRef::new("plan", "PLAN.yaml"));
        }
    };
    let report = crate::config::analyze::analyze(&plan, cfg);
    let preview = crate::config::analyze::plan_preview(&plan, &report);
    let plan_ref = HealthRef::new("plan", "PLAN.yaml");
    if !preview.errors.is_empty() {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Fail,
            format!("{} plan error(s)", preview.errors.len()),
        )
        .fix("run `maestro plan validate`")
        .with_ref(plan_ref)
    } else if !preview.warnings.is_empty() {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Warn,
            format!("{} plan warning(s)", preview.warnings.len()),
        )
        .fix("run `maestro plan validate`")
        .with_ref(plan_ref)
    } else {
        RuntimeHealthCheck::new(
            id,
            HealthStatus::Pass,
            format!("plan preview valid; {} task(s)", preview.task_count),
        )
        .with_ref(plan_ref)
    }
}

fn resume_check() -> RuntimeHealthCheck {
    use crate::scheduler::liveness::classify_run;
    use crate::scheduler::{RunLiveness, RunState};
    let id = HealthCheckId::ResumeGuardReady;

    let dir = match paths::current_run_dir() {
        Ok(Some(dir)) => dir,
        _ => return RuntimeHealthCheck::new(id, HealthStatus::Skip, "no current run"),
    };
    let state = match RunState::load(&dir) {
        Ok(state) => state,
        Err(_) => return RuntimeHealthCheck::new(id, HealthStatus::Skip, "current run unreadable"),
    };
    match classify_run(&state) {
        RunLiveness::Live => RuntimeHealthCheck::new(id, HealthStatus::Skip, "current run is live"),
        RunLiveness::UnknownLegacy => {
            RuntimeHealthCheck::new(id, HealthStatus::Skip, "current run has no liveness info")
        }
        RunLiveness::Abandoned => {
            let report = match crate::scheduler::resume::validate_resume_target(&dir, false) {
                Ok(report) => report,
                Err(_) => {
                    return RuntimeHealthCheck::new(
                        id,
                        HealthStatus::Fail,
                        "resume guard could not validate the run",
                    )
                    .fix("use `maestro rerun`");
                }
            };
            if report.has(crate::schema::resume::ResumeIssueCode::MissingDescriptor) {
                return RuntimeHealthCheck::new(
                    id,
                    HealthStatus::Skip,
                    "no resume descriptor for this run",
                );
            }
            let hard = report.issues.iter().filter(|i| i.code.blocks(true)).count();
            let force_only = report
                .issues
                .iter()
                .filter(|i| i.code.blocks(false) && !i.code.blocks(true))
                .count();
            let mut check = if hard > 0 {
                RuntimeHealthCheck::new(
                    id,
                    HealthStatus::Fail,
                    format!("abandoned run cannot be resumed ({hard} blocking issue(s))"),
                )
                .fix("use `maestro rerun`")
            } else if force_only > 0 {
                RuntimeHealthCheck::new(
                    id,
                    HealthStatus::Warn,
                    "abandoned run is resumable only with --force",
                )
                .fix("use `maestro resume --force` or `maestro rerun`")
            } else {
                RuntimeHealthCheck::new(id, HealthStatus::Pass, "abandoned run can be resumed")
            };
            if is_symbol_token(&report.run_id) {
                check = check.with_ref(HealthRef::new("run", report.run_id.clone()));
            }
            check
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderExecution, ProviderModelCapability};
    use crate::schema::runtime_health::{
        summarize, validate_report, HealthCheckId, HealthStatus, RuntimeHealthCheck,
        RuntimeHealthReport,
    };

    /// A ProviderStatus carrying an absolute binary path + env var — exactly the
    /// values that must never reach the health report.
    fn leaky_status() -> ProviderStatus {
        ProviderStatus {
            schema_version: crate::schema::PROVIDER_CAPABILITY_V1.to_string(),
            id: "codex",
            display: "Codex CLI",
            kind: ProviderKind::TaskAdapter,
            adapter_available: true,
            installed: true,
            authenticated: true,
            execution: ProviderExecution {
                binary: Some("/usr/local/bin/codex".into()),
                // absolute, secret-bearing path under a non-home root — proves the
                // projector drops it without inlining a real `/Users/<name>/` path.
                path: Some("/opt/secret/.local/bin/codex".into()),
                env_override: Some("MAESTRO_CODEX"),
                non_interactive: true,
                streaming: true,
                resume: false,
                worktree_isolation: true,
            },
            permissions: crate::schema::permissions::provider_permission_profile("codex"),
            models: ProviderModelCapability {
                supports_override: true,
                default: None,
                context_tokens: None,
            },
            tool_trace: ToolTraceSupport::Partial,
            notes: "task adapter",
        }
    }

    #[test]
    fn projection_keeps_capabilities_and_drops_execution() {
        let h = project_provider(&leaky_status(), ProbeResult::skipped("step1"));
        assert_eq!(h.id, "codex");
        assert_eq!(h.kind, "task_adapter");
        assert!(h.installed && h.adapter_available);
        assert!(h.capabilities.model_override);
        assert!(h.capabilities.streaming);
        assert!(h.capabilities.worktree_isolation);
        assert!(!h.capabilities.resume);
        assert_eq!(h.capabilities.tool_trace, "partial");
        assert_eq!(h.message, "installed; adapter available");
    }

    #[test]
    fn whole_report_json_never_leaks_path_or_env() {
        // bar 1: serialize the WHOLE report and assert NONE of the binary / path /
        // env_override values (or their field names) survive the projection.
        let provider = project_provider(&leaky_status(), ProbeResult::skipped("step1"));
        let checks: Vec<RuntimeHealthCheck> = HealthCheckId::ALL
            .iter()
            .map(|id| RuntimeHealthCheck::new(*id, HealthStatus::Pass, "ok"))
            .collect();
        let report = RuntimeHealthReport {
            schema_version: crate::schema::runtime_health_version(),
            generated_at: "2026-06-05T00:00:00Z".into(),
            summary: summarize(&checks),
            checks,
            providers: vec![provider],
        };
        assert!(validate_report(&report).is_ok());
        let json = serde_json::to_string(&report).unwrap();
        for leak in [
            "/usr/local/bin/codex",
            "/opt/secret",
            ".local/bin/codex",
            "MAESTRO_CODEX",
            "env_override",
            "\"path\"",
            "\"binary\"",
        ] {
            assert!(!json.contains(leak), "report leaked {leak:?}:\n{json}");
        }
        // the safe, useful facts ARE present
        assert!(json.contains("\"id\":\"codex\""));
        assert!(json.contains("\"installed\":true"));
    }

    #[test]
    fn not_installed_message_has_no_path() {
        let mut s = leaky_status();
        s.installed = false;
        let h = project_provider(&s, ProbeResult::skipped("x"));
        assert_eq!(h.message, "not installed");
        assert!(!h.message.contains('/'));
    }

    #[test]
    fn known_cli_kind_projects_as_known_cli() {
        let mut s = leaky_status();
        s.kind = ProviderKind::KnownCli;
        s.adapter_available = false;
        let h = project_provider(&s, ProbeResult::skipped("x"));
        assert_eq!(h.kind, "known_cli");
        assert_eq!(h.message, "installed; task adapter not wired");
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;
    use serial_test::serial;

    fn cfg(yaml: &str) -> ProjectsConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn ph(id: &str, probe: HealthStatus) -> ProviderHealth {
        ProviderHealth {
            id: id.to_string(),
            display: id.to_string(),
            kind: "task_adapter".to_string(),
            adapter_available: true,
            installed: probe != HealthStatus::Skip,
            probe: ProbeResult {
                status: probe,
                duration_ms: Some(5),
                timed_out: probe == HealthStatus::Warn,
                message: "probe".into(),
            },
            capabilities: ProviderCapabilitySummary {
                model_override: true,
                non_interactive: true,
                streaming: true,
                resume: false,
                worktree_isolation: true,
                tool_trace: "partial".into(),
            },
            message: "installed; adapter available".into(),
        }
    }

    fn ids(set: BTreeSet<&'static str>) -> Vec<&'static str> {
        set.into_iter().collect()
    }

    #[test]
    fn active_ids_default_to_codex_cursor_when_unconfigured() {
        assert_eq!(
            ids(active_task_provider_ids(None, &[])),
            vec!["codex", "cursor"]
        );
        // a KnownCli agent (claude) is not a task adapter -> falls back to defaults.
        let c = cfg("version: 1\ndefaults:\n  agent: claude\nprojects: {}\n");
        assert_eq!(
            ids(active_task_provider_ids(Some(&c), &[])),
            vec!["codex", "cursor"]
        );
    }

    #[test]
    fn active_ids_pick_up_configured_adapters() {
        let c = cfg("version: 1\ndefaults:\n  agent: mock\nprojects: {}\n");
        assert_eq!(ids(active_task_provider_ids(Some(&c), &[])), vec!["mock"]);
    }

    #[test]
    fn active_ids_fold_in_plan_task_agents() {
        // N3: a PLAN task `agent: mock` joins the active set even when the default
        // config does not point at mock; only task-adapter ids are folded in.
        let c = cfg("version: 1\ndefaults:\n  agent: codex\nprojects: {}\n");
        let cases: [(&[&str], Vec<&str>); 4] = [
            // no plan agents -> existing config/default cadence
            (&[], vec!["codex"]),
            // PLAN task using mock -> mock joins
            (&["mock"], vec!["codex", "mock"]),
            // a KnownCli plan agent is ignored (never blocking)
            (&["claude"], vec!["codex"]),
            // multiple, deduped + only task adapters
            (&["shell", "mock", "claude"], vec!["codex", "mock", "shell"]),
        ];
        for (plan, expect) in cases {
            let plan_agents: Vec<String> = plan.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                ids(active_task_provider_ids(Some(&c), &plan_agents)),
                expect,
                "plan agents {plan:?}"
            );
        }
        // with no config at all, PLAN mock still joins the codex+cursor fallback.
        assert_eq!(
            ids(active_task_provider_ids(None, &["mock".to_string()])),
            vec!["mock"]
        );
    }

    #[test]
    fn provider_check_pass_warn_fail_by_active_pool() {
        // one active provider ready -> pass, with that provider's ref.
        let pass = provider_check(
            None,
            &[
                ph("codex", HealthStatus::Pass),
                ph("cursor", HealthStatus::Skip),
            ],
            &[],
        );
        assert_eq!(pass.status, HealthStatus::Pass);
        assert!(pass
            .refs
            .iter()
            .any(|r| r.kind == "provider" && r.reference == "codex"));

        // none ready but one timed out -> warn.
        let warn = provider_check(
            None,
            &[
                ph("codex", HealthStatus::Warn),
                ph("cursor", HealthStatus::Skip),
            ],
            &[],
        );
        assert_eq!(warn.status, HealthStatus::Warn);

        // none ready, none timed out -> fail.
        let fail = provider_check(
            None,
            &[
                ph("codex", HealthStatus::Skip),
                ph("cursor", HealthStatus::Skip),
            ],
            &[],
        );
        assert_eq!(fail.status, HealthStatus::Fail);

        // a passing provider OUTSIDE the active set does not rescue readiness.
        let outside = provider_check(
            None,
            &[
                ph("mock", HealthStatus::Pass),
                ph("codex", HealthStatus::Skip),
            ],
            &[],
        );
        assert_eq!(
            outside.status,
            HealthStatus::Fail,
            "mock is not in the default active set"
        );

        // ...but once a PLAN task pins mock, mock enters the active set and rescues it.
        let plan_mock = provider_check(
            None,
            &[
                ph("mock", HealthStatus::Pass),
                ph("codex", HealthStatus::Skip),
            ],
            &["mock".to_string()],
        );
        assert_eq!(
            plan_mock.status,
            HealthStatus::Pass,
            "PLAN mock is now active"
        );
    }

    #[test]
    fn provider_check_respects_configured_mock() {
        let c = cfg("version: 1\ndefaults:\n  agent: mock\nprojects: {}\n");
        let check = provider_check(
            Some(&c),
            &[
                ph("mock", HealthStatus::Pass),
                ph("codex", HealthStatus::Skip),
            ],
            &[],
        );
        assert_eq!(check.status, HealthStatus::Pass);
    }

    #[tokio::test]
    async fn probe_provider_mock_is_builtin_pass() {
        let mock = crate::providers::known_providers()
            .into_iter()
            .find(|p| p.id == "mock")
            .map(crate::providers::provider_status)
            .unwrap();
        let result = probe_provider(&mock, PROBE_DEADLINE).await;
        assert_eq!(result.status, HealthStatus::Pass);
        assert_eq!(result.message, "built-in adapter");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_binary_reports_ok_fail_and_timeout() {
        // exit 0 -> Ok
        if let Some(t) = crate::providers::find_binary("true") {
            assert!(matches!(
                probe_binary(&t, &["--version"], PROBE_DEADLINE).await,
                ProbeOutcome::Ok(_)
            ));
        }
        // non-zero exit -> Failed
        if let Some(f) = crate::providers::find_binary("false") {
            assert!(matches!(
                probe_binary(&f, &[], PROBE_DEADLINE).await,
                ProbeOutcome::Failed(_)
            ));
        }
        // exceeds the deadline -> TimedOut (and kill_on_drop reaps the child)
        if let Some(sh) = crate::providers::find_binary("sh") {
            let outcome = probe_binary(&sh, &["-c", "sleep 1"], Duration::from_millis(50)).await;
            assert!(matches!(outcome, ProbeOutcome::TimedOut(_)));
        }
    }

    #[tokio::test]
    #[serial] // mutates the process-global MAESTRO_WORKSPACE_ROOT
    async fn build_report_validates_and_leaks_no_path() {
        let temp = tempfile::tempdir().unwrap();
        let maestro = temp.path().join(".maestro");
        std::fs::create_dir_all(&maestro).unwrap();
        std::fs::write(
            maestro.join("projects.yaml"),
            "version: 1\ndefaults:\n  agent: codex\nprojects: {}\n",
        )
        .unwrap();
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", temp.path());
        }

        let report = build_validated_report("2026-06-05T00:00:00Z".into())
            .await
            .expect("assembled report must validate");
        // exactly the six stable checks; workspace row is a real pass.
        assert_eq!(report.checks.len(), 6);
        assert_eq!(report.checks[0].id, HealthCheckId::WorkspaceDetected);
        assert_eq!(report.checks[0].status, HealthStatus::Pass);

        // the whole assembled report carries no absolute workspace path / env name.
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("/Users/"), "home path leaked: {json}");
        assert!(!json.contains("MAESTRO_"), "env name leaked: {json}");
        assert!(
            !json.contains(&temp.path().display().to_string()),
            "workspace path leaked: {json}"
        );

        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }
}
