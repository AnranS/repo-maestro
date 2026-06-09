//! F-118 local runtime capability + health report (`maestro.runtime_health.v1`).
//!
//! A stable, on-demand, read-only projection of whether the local runtime can
//! start useful work right now — the data source behind the operator-console
//! Dashboard's six readiness rows, `maestro doctor runtime`, and future MCP
//! consumers. It carries provenance + short symbolic refs + pass/warn/fail/skip
//! verdicts ONLY — never an absolute path, env value, raw prompt/skill/log body,
//! provider stdout/stderr, token, or secret. This module owns the types +
//! validation + pure summarizer; the provider projection / check gatherers /
//! timed probe live in `crate::runtime_health`.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Short single-line text (messages, labels, fixes, refs).
const MAX_TEXT_BYTES: usize = 256;

/// Per-check / per-probe verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl HealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthStatus::Pass => "pass",
            HealthStatus::Warn => "warn",
            HealthStatus::Fail => "fail",
            HealthStatus::Skip => "skip",
        }
    }

    /// Map to the F-UI-001 shared WebUI status token.
    pub fn status_token(self) -> &'static str {
        match self {
            HealthStatus::Pass => "done",
            HealthStatus::Warn => "blocked",
            HealthStatus::Fail => "failed",
            HealthStatus::Skip => "pending",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthSeverity {
    Info,
    Low,
    Medium,
    High,
}

/// Closed v1 set of readiness check ids, in stable display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthCheckId {
    #[serde(rename = "workspace.detected")]
    WorkspaceDetected,
    #[serde(rename = "provider.available")]
    ProviderAvailable,
    #[serde(rename = "profiles.valid")]
    ProfilesValid,
    #[serde(rename = "skills.visible")]
    SkillsVisible,
    #[serde(rename = "plan.preview_valid")]
    PlanPreviewValid,
    #[serde(rename = "resume.guard_ready")]
    ResumeGuardReady,
}

impl HealthCheckId {
    /// The six checks in stable v1 order. A valid report carries exactly these.
    pub const ALL: [HealthCheckId; 6] = [
        HealthCheckId::WorkspaceDetected,
        HealthCheckId::ProviderAvailable,
        HealthCheckId::ProfilesValid,
        HealthCheckId::SkillsVisible,
        HealthCheckId::PlanPreviewValid,
        HealthCheckId::ResumeGuardReady,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HealthCheckId::WorkspaceDetected => "workspace.detected",
            HealthCheckId::ProviderAvailable => "provider.available",
            HealthCheckId::ProfilesValid => "profiles.valid",
            HealthCheckId::SkillsVisible => "skills.visible",
            HealthCheckId::PlanPreviewValid => "plan.preview_valid",
            HealthCheckId::ResumeGuardReady => "resume.guard_ready",
        }
    }

    /// Dashboard label.
    pub fn label(self) -> &'static str {
        match self {
            HealthCheckId::WorkspaceDetected => "Workspace detected",
            HealthCheckId::ProviderAvailable => "Provider available",
            HealthCheckId::ProfilesValid => "Profiles valid",
            HealthCheckId::SkillsVisible => "Skills visible",
            HealthCheckId::PlanPreviewValid => "Plan preview valid",
            HealthCheckId::ResumeGuardReady => "Resume guard ready",
        }
    }
}

/// A symbolic, short reference — NEVER an absolute path / env / secret. The
/// `(kind, reference)` pair is a CLOSED grammar (`validate_ref`); the design's
/// symbolic shapes map to it as:
///   `workspace`        → `kind="workspace"`, `reference="workspace"`
///   `projects.yaml`    → `kind="config"`,    `reference="projects.yaml"`
///   `PLAN.yaml`        → `kind="plan"`,      `reference="PLAN.yaml"`
///   `provider:<id>`    → `kind="provider"`,  `reference=<symbol>`
///   `profile:<name>`   → `kind="profile"`,   `reference=<symbol>`
///   `skill:<scope>/<name>` → `kind="skill"`, `reference="<symbol>/<symbol>"`
///   `run:<id>`         → `kind="run"`,       `reference=<symbol>`
/// Any other `kind`, or a `reference` not matching its kind, is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRef {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

impl HealthRef {
    pub fn new(kind: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            reference: reference.into(),
        }
    }
}

/// Timed local provider probe result. Carries verdict + duration only — never
/// stdout/stderr body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub status: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub timed_out: bool,
    pub message: String,
}

impl ProbeResult {
    pub fn skipped(message: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Skip,
            duration_ms: None,
            timed_out: false,
            message: message.into(),
        }
    }
    pub fn timed(status: HealthStatus, duration_ms: u64, message: impl Into<String>) -> Self {
        Self {
            status,
            duration_ms: Some(duration_ms),
            timed_out: false,
            message: message.into(),
        }
    }
    pub fn timeout(duration_ms: u64) -> Self {
        Self {
            status: HealthStatus::Warn,
            duration_ms: Some(duration_ms),
            timed_out: true,
            message: "local probe timed out".into(),
        }
    }
}

/// Compact provider capability booleans — never paths/env/binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilitySummary {
    pub model_override: bool,
    pub non_interactive: bool,
    pub streaming: bool,
    pub resume: bool,
    pub worktree_isolation: bool,
    /// `full` / `partial` / `none`.
    pub tool_trace: String,
}

/// Focused, UI/MCP-safe provider health. NO `execution.path` / env / binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub id: String,
    pub display: String,
    /// `task_adapter` / `known_cli`.
    pub kind: String,
    pub adapter_available: bool,
    pub installed: bool,
    pub probe: ProbeResult,
    pub capabilities: ProviderCapabilitySummary,
    pub message: String,
}

/// One readiness check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealthCheck {
    pub id: HealthCheckId,
    pub label: String,
    pub status: HealthStatus,
    pub severity: HealthSeverity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub refs: Vec<HealthRef>,
}

impl RuntimeHealthCheck {
    /// Build a check with the canonical label for its id and a derived default
    /// severity (`high` for fail, `medium` for warn, else `info`).
    pub fn new(id: HealthCheckId, status: HealthStatus, message: impl Into<String>) -> Self {
        let severity = match status {
            HealthStatus::Fail => HealthSeverity::High,
            HealthStatus::Warn => HealthSeverity::Medium,
            _ => HealthSeverity::Info,
        };
        Self {
            id,
            label: id.label().to_string(),
            status,
            severity,
            message: message.into(),
            fix: None,
            duration_ms: None,
            refs: Vec::new(),
        }
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    pub fn with_ref(mut self, r: HealthRef) -> Self {
        self.refs.push(r);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealthSummary {
    pub total: u64,
    pub passed: u64,
    pub warnings: u64,
    pub failed: u64,
    pub skipped: u64,
    pub overall: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_issue: Option<String>,
}

/// `maestro.runtime_health.v1` — the runtime health report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealthReport {
    #[serde(default = "crate::schema::runtime_health_version")]
    pub schema_version: String,
    /// RFC3339, set by the CLI/handler wrapper (so the pure projector is testable).
    pub generated_at: String,
    pub summary: RuntimeHealthSummary,
    pub checks: Vec<RuntimeHealthCheck>,
    pub providers: Vec<ProviderHealth>,
}

/// Pure summarizer: per-status counts + `overall` + issue-first `top_issue`
/// (first fail message in stable order, else first warn message).
pub fn summarize(checks: &[RuntimeHealthCheck]) -> RuntimeHealthSummary {
    let count = |s: HealthStatus| checks.iter().filter(|c| c.status == s).count() as u64;
    let passed = count(HealthStatus::Pass);
    let warnings = count(HealthStatus::Warn);
    let failed = count(HealthStatus::Fail);
    let skipped = count(HealthStatus::Skip);
    let overall = if failed > 0 {
        HealthStatus::Fail
    } else if warnings > 0 {
        HealthStatus::Warn
    } else if passed > 0 {
        HealthStatus::Pass
    } else {
        HealthStatus::Skip
    };
    let top_issue = checks
        .iter()
        .find(|c| c.status == HealthStatus::Fail)
        .or_else(|| checks.iter().find(|c| c.status == HealthStatus::Warn))
        .map(|c| c.message.clone());
    RuntimeHealthSummary {
        total: checks.len() as u64,
        passed,
        warnings,
        failed,
        skipped,
        overall,
        top_issue,
    }
}

/// Validate the report's shape + self-consistency + privacy, run BEFORE a write
/// AND AFTER a read (a hand-edited report that smuggles an absolute path / env /
/// inconsistent summary is corrupt, not served). Mirrors the F-116/F-117 discipline.
pub fn validate_report(report: &RuntimeHealthReport) -> Result<()> {
    ensure!(
        report.schema_version == crate::schema::RUNTIME_HEALTH_V1,
        "runtime health schema_version {:?} is not {}",
        report.schema_version,
        crate::schema::RUNTIME_HEALTH_V1
    );
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&report.generated_at).is_ok(),
        "runtime health generated_at {:?} is not RFC3339",
        report.generated_at
    );

    // Exactly the six v1 checks, in stable order, each with its canonical label.
    ensure!(
        report.checks.len() == HealthCheckId::ALL.len(),
        "runtime health must carry exactly {} checks, got {}",
        HealthCheckId::ALL.len(),
        report.checks.len()
    );
    for (index, check) in report.checks.iter().enumerate() {
        ensure!(
            check.id == HealthCheckId::ALL[index],
            "runtime health check {} is {:?}, expected {:?} (stable order)",
            index,
            check.id,
            HealthCheckId::ALL[index]
        );
        ensure!(
            check.label == check.id.label(),
            "runtime health check {:?} label {:?} != canonical {:?}",
            check.id,
            check.label,
            check.id.label()
        );
        validate_text("check message", &check.message)?;
        if let Some(fix) = &check.fix {
            validate_text("check fix", fix)?;
        }
        for r in &check.refs {
            validate_ref(r)?;
        }
    }

    // Summary must agree with the checks — including issue-first `top_issue`,
    // which the Dashboard renders verbatim (a stale top_issue must not validate).
    let recomputed = summarize(&report.checks);
    ensure!(
        report.summary.total == recomputed.total
            && report.summary.passed == recomputed.passed
            && report.summary.warnings == recomputed.warnings
            && report.summary.failed == recomputed.failed
            && report.summary.skipped == recomputed.skipped
            && report.summary.overall == recomputed.overall
            && report.summary.top_issue == recomputed.top_issue,
        "runtime health summary disagrees with its checks"
    );

    // Providers: safe shape only; message/probe text is short + path-free, and
    // `kind` / `tool_trace` are the closed v1 enums (String wire, validated here).
    for p in &report.providers {
        validate_text("provider display", &p.display)?;
        validate_text("provider message", &p.message)?;
        validate_text("provider probe message", &p.probe.message)?;
        ensure!(
            !p.id.trim().is_empty() && !contains_path_like(&p.id),
            "provider id {:?} must be a symbolic id",
            p.id
        );
        ensure!(
            matches!(p.kind.as_str(), "task_adapter" | "known_cli"),
            "provider {:?} kind {:?} is not a closed v1 kind",
            p.id,
            p.kind
        );
        ensure!(
            matches!(
                p.capabilities.tool_trace.as_str(),
                "full" | "partial" | "none"
            ),
            "provider {:?} tool_trace {:?} is not a closed v1 value",
            p.id,
            p.capabilities.tool_trace
        );
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<()> {
    ensure!(!value.contains(['\n', '\r']), "{field} must be single-line");
    ensure!(
        value.len() <= MAX_TEXT_BYTES,
        "{field} exceeds {MAX_TEXT_BYTES} bytes"
    );
    ensure!(
        !contains_path_like(value),
        "{field} must not contain an absolute path / file: uri: {value:?}"
    );
    Ok(())
}

/// Validate a `HealthRef` against the CLOSED v1 grammar: `kind` is from the known
/// set and `reference` matches that kind's shape. Rejects unknown kinds (incl. the
/// old `kind="ref"`), empty / multi-line / over-long values, a reference that does
/// not match its kind, and — as a backstop — any absolute / `..` / `file:` / drive
/// value that slips through a grammar-shaped reference.
fn validate_ref(r: &HealthRef) -> Result<()> {
    ensure!(
        !r.kind.contains(['\n', '\r']) && !r.reference.contains(['\n', '\r']),
        "health ref must be single-line: {:?}/{:?}",
        r.kind,
        r.reference
    );
    ensure!(
        !r.reference.trim().is_empty() && r.reference.len() <= MAX_TEXT_BYTES,
        "health ref reference {:?} empty or too long",
        r.reference
    );
    let matches_grammar = match r.kind.as_str() {
        "workspace" => r.reference == "workspace",
        "config" => r.reference == "projects.yaml",
        "plan" => r.reference == "PLAN.yaml",
        "provider" | "profile" | "run" => is_symbol_token(&r.reference),
        "skill" => {
            let mut parts = r.reference.split('/');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some(scope), Some(name), None)
                    if is_symbol_token(scope) && is_symbol_token(name)
            )
        }
        _ => false,
    };
    ensure!(
        matches_grammar,
        "health ref {:?}/{:?} is not an allowed symbolic ref",
        r.kind,
        r.reference
    );
    ensure!(
        !ref_value_is_unsafe(&r.reference),
        "health ref {:?} must be symbolic (no absolute / .. / file: / drive)",
        r.reference
    );
    Ok(())
}

/// A bare symbol token: non-empty, bounded, `[A-Za-z0-9._-]` only, and not a bare
/// `.`/`..` (so it can never name a parent/this dir or carry a path separator).
/// `pub(crate)` so the report builder can pre-validate a symbolic ref segment
/// before emitting it (a ref that would fail `validate_report` is simply omitted).
pub(crate) fn is_symbol_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// True if a symbolic ref value is actually an absolute / escaping / drive path.
fn ref_value_is_unsafe(value: &str) -> bool {
    if value.trim_start().to_ascii_lowercase().starts_with("file:") {
        return true;
    }
    if Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    if value.split(['/', '\\']).any(|c| c == "..") {
        return true;
    }
    // any `^[A-Za-z]:` is a Windows drive prefix (incl. `C:`, `C:foo`, `C:\x`).
    let b = value.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Heuristic backstop: does a message/id embed an absolute-path / file: / drive
/// token? The projector is the real guard; this catches a leak that slipped through.
fn contains_path_like(value: &str) -> bool {
    value.split_whitespace().any(|tok| {
        let t = tok.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric()
                && c != '/'
                && c != '\\'
                && c != ':'
                && c != '.'
                && c != '_'
                && c != '-'
        });
        t.to_ascii_lowercase().starts_with("file:")
            || (t.starts_with('/') && t.len() > 1)
            || t.starts_with('\\')
            // any `^[A-Za-z]:` token is a drive prefix (`C:`, `C:foo`, `C:\x`, `C:/x`).
            || (t.len() >= 2 && t.as_bytes()[0].is_ascii_alphabetic() && t.as_bytes()[1] == b':')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: HealthCheckId, status: HealthStatus, msg: &str) -> RuntimeHealthCheck {
        RuntimeHealthCheck::new(id, status, msg)
    }

    /// The six checks in stable order with the given statuses.
    fn six(statuses: [HealthStatus; 6]) -> Vec<RuntimeHealthCheck> {
        HealthCheckId::ALL
            .iter()
            .zip(statuses)
            .map(|(id, s)| check(*id, s, "ok"))
            .collect()
    }

    fn report(checks: Vec<RuntimeHealthCheck>) -> RuntimeHealthReport {
        RuntimeHealthReport {
            schema_version: crate::schema::runtime_health_version(),
            generated_at: "2026-06-05T00:00:00Z".into(),
            summary: summarize(&checks),
            checks,
            providers: vec![],
        }
    }

    #[test]
    fn summary_counts_overall_and_top_issue() {
        use HealthStatus::*;
        let checks = vec![
            check(HealthCheckId::WorkspaceDetected, Pass, "workspace ok"),
            check(HealthCheckId::ProviderAvailable, Fail, "no provider"),
            check(HealthCheckId::ProfilesValid, Warn, "1 profile warn"),
            check(HealthCheckId::SkillsVisible, Pass, "skills ok"),
            check(HealthCheckId::PlanPreviewValid, Skip, "no plan"),
            check(HealthCheckId::ResumeGuardReady, Skip, "no run"),
        ];
        let s = summarize(&checks);
        assert_eq!(
            (s.total, s.passed, s.warnings, s.failed, s.skipped),
            (6, 2, 1, 1, 2)
        );
        assert_eq!(s.overall, Fail);
        // issue-first: the fail message wins over the warn.
        assert_eq!(s.top_issue.as_deref(), Some("no provider"));
    }

    #[test]
    fn overall_is_warn_then_pass_then_skip() {
        use HealthStatus::*;
        assert_eq!(
            summarize(&six([Pass, Warn, Pass, Pass, Pass, Pass])).overall,
            Warn
        );
        assert_eq!(
            summarize(&six([Pass, Pass, Pass, Pass, Pass, Pass])).overall,
            Pass
        );
        assert_eq!(
            summarize(&six([Skip, Skip, Skip, Skip, Skip, Skip])).overall,
            Skip
        );
        assert_eq!(
            summarize(&six([Pass, Pass, Pass, Pass, Pass, Pass])).top_issue,
            None
        );
    }

    #[test]
    fn status_token_mapping() {
        assert_eq!(HealthStatus::Pass.status_token(), "done");
        assert_eq!(HealthStatus::Warn.status_token(), "blocked");
        assert_eq!(HealthStatus::Fail.status_token(), "failed");
        assert_eq!(HealthStatus::Skip.status_token(), "pending");
    }

    #[test]
    fn full_report_round_trips_and_validates() {
        use HealthStatus::*;
        let r = report(six([Pass, Pass, Pass, Pass, Skip, Skip]));
        assert!(validate_report(&r).is_ok());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"schema_version\":\"maestro.runtime_health.v1\""));
        assert!(json.contains("\"workspace.detected\""));
        let back: RuntimeHealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn validate_rejects_wrong_check_set_or_order() {
        // missing a check
        let mut r = report(six([HealthStatus::Pass; 6]));
        r.checks.pop();
        r.summary = summarize(&r.checks);
        assert!(validate_report(&r).is_err());

        // reordered
        let mut r = report(six([HealthStatus::Pass; 6]));
        r.checks.swap(0, 1);
        r.summary = summarize(&r.checks);
        assert!(validate_report(&r).is_err());
    }

    #[test]
    fn validate_rejects_tampered_summary_and_wrong_schema() {
        let mut r = report(six([HealthStatus::Pass; 6]));
        r.summary.failed += 1;
        assert!(validate_report(&r).is_err());

        let mut r = report(six([HealthStatus::Pass; 6]));
        r.schema_version = "maestro.runtime_health.v2".into();
        assert!(validate_report(&r).is_err());
    }

    #[test]
    fn validate_rejects_absolute_path_or_env_leak() {
        for bad in [
            "/opt/x/.maestro found",
            "binary at /usr/local/bin/codex",
            r"C:\Users\x\codex.exe",
            "see file:///opt/x",
        ] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].message = bad.to_string();
            assert!(
                validate_report(&r).is_err(),
                "leak should be rejected: {bad}"
            );
        }
        // relative / symbolic stays fine
        for ok in [
            "projects.yaml ok",
            "run `maestro doctor`",
            "provider codex installed",
        ] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].message = ok.to_string();
            assert!(validate_report(&r).is_ok(), "should pass: {ok}");
        }
    }

    #[test]
    fn validate_rejects_unsafe_ref() {
        // unsafe reference values are rejected regardless of an otherwise-valid kind.
        for bad in ["/abs/path", "../escape", r"C:\x", "C:foo", "file:///x"] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].refs = vec![HealthRef::new("provider", bad)];
            assert!(validate_report(&r).is_err(), "ref should reject: {bad}");
        }
    }

    #[test]
    fn validate_closed_ref_grammar() {
        // every allowed (kind, reference) shape passes
        for (kind, reference) in [
            ("workspace", "workspace"),
            ("config", "projects.yaml"),
            ("plan", "PLAN.yaml"),
            ("provider", "codex"),
            ("profile", "backend_rust"),
            ("skill", "_global/verify-before-done"),
            ("run", "r-20260605-abc"),
        ] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].refs = vec![HealthRef::new(kind, reference)];
            assert!(
                validate_report(&r).is_ok(),
                "ref should pass: {kind}/{reference}"
            );
        }
        // unknown kind (incl. the old free-form `ref`) and reference-not-matching-kind
        for (kind, reference) in [
            ("ref", "backend_rust"),    // unknown kind
            ("", "x"),                  // empty kind
            ("workspace", "elsewhere"), // singleton must be the literal
            ("config", "secrets.yaml"), // only projects.yaml allowed
            ("plan", "OTHER.yaml"),
            ("provider", "co dex"),        // space is not a symbol token
            ("provider", "a/b"),           // slash not allowed for provider
            ("skill", "only-one-segment"), // skill needs scope/name
            ("skill", "a/b/c"),            // too many segments
            ("skill", "../x"),             // escaping segment
        ] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].refs = vec![HealthRef::new(kind, reference)];
            assert!(
                validate_report(&r).is_err(),
                "ref should reject: {kind}/{reference}"
            );
        }
    }

    #[test]
    fn validate_rejects_stale_top_issue() {
        use HealthStatus::*;
        // counts stay correct, but top_issue is hand-edited to a stale string.
        let mut r = report(vec![
            check(HealthCheckId::WorkspaceDetected, Pass, "workspace ok"),
            check(HealthCheckId::ProviderAvailable, Fail, "no provider"),
            check(HealthCheckId::ProfilesValid, Pass, "ok"),
            check(HealthCheckId::SkillsVisible, Pass, "ok"),
            check(HealthCheckId::PlanPreviewValid, Skip, "no plan"),
            check(HealthCheckId::ResumeGuardReady, Skip, "no run"),
        ]);
        assert!(validate_report(&r).is_ok());
        r.summary.top_issue = Some("stale reason".into());
        assert!(
            validate_report(&r).is_err(),
            "stale top_issue must not validate"
        );

        // fail message updated but top_issue left at the old value.
        let mut r2 = report(vec![
            check(HealthCheckId::WorkspaceDetected, Pass, "ok"),
            check(HealthCheckId::ProviderAvailable, Fail, "updated reason"),
            check(HealthCheckId::ProfilesValid, Pass, "ok"),
            check(HealthCheckId::SkillsVisible, Pass, "ok"),
            check(HealthCheckId::PlanPreviewValid, Pass, "ok"),
            check(HealthCheckId::ResumeGuardReady, Pass, "ok"),
        ]);
        r2.summary.top_issue = Some("old reason".into());
        assert!(
            validate_report(&r2).is_err(),
            "out-of-date top_issue must not validate"
        );
    }

    #[test]
    fn validate_rejects_drive_relative_paths() {
        // text message: bare drive prefixes (no slash) must be rejected too.
        for bad in ["C:foo", "C:", r"C:\x", "C:/x", "see D:data"] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].message = bad.to_string();
            assert!(
                validate_report(&r).is_err(),
                "text drive should reject: {bad}"
            );
        }
        // ref value: drive prefixes rejected as well.
        for bad in ["C:foo", "C:", r"C:\x", "C:/x"] {
            let mut r = report(six([HealthStatus::Pass; 6]));
            r.checks[0].refs = vec![HealthRef::new("provider", bad)];
            assert!(
                validate_report(&r).is_err(),
                "ref drive should reject: {bad}"
            );
        }
    }

    #[test]
    fn validate_locks_provider_kind_and_tool_trace() {
        let good = ProviderHealth {
            id: "codex".into(),
            display: "Codex CLI".into(),
            kind: "task_adapter".into(),
            adapter_available: true,
            installed: true,
            probe: ProbeResult::skipped("step1"),
            capabilities: ProviderCapabilitySummary {
                model_override: true,
                non_interactive: true,
                streaming: true,
                resume: false,
                worktree_isolation: true,
                tool_trace: "partial".into(),
            },
            message: "installed; adapter available".into(),
        };
        let mut ok = report(six([HealthStatus::Pass; 6]));
        ok.providers = vec![good.clone()];
        assert!(validate_report(&ok).is_ok());

        let mut bad_kind = good.clone();
        bad_kind.kind = "daemon".into();
        let mut r2 = report(six([HealthStatus::Pass; 6]));
        r2.providers = vec![bad_kind];
        assert!(
            validate_report(&r2).is_err(),
            "out-of-enum provider kind must reject"
        );

        let mut bad_tt = good;
        bad_tt.capabilities.tool_trace = "some".into();
        let mut r3 = report(six([HealthStatus::Pass; 6]));
        r3.providers = vec![bad_tt];
        assert!(
            validate_report(&r3).is_err(),
            "out-of-enum tool_trace must reject"
        );
    }

    #[test]
    fn check_severity_defaults_by_status() {
        assert_eq!(
            check(HealthCheckId::ProviderAvailable, HealthStatus::Fail, "x").severity,
            HealthSeverity::High
        );
        assert_eq!(
            check(HealthCheckId::ProfilesValid, HealthStatus::Warn, "x").severity,
            HealthSeverity::Medium
        );
        assert_eq!(
            check(HealthCheckId::SkillsVisible, HealthStatus::Pass, "x").severity,
            HealthSeverity::Info
        );
    }
}
