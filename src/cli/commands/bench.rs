use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::bench::fixture::{Fixture, FixtureKind};
use crate::bench::runner::{ensure_upstream_checkout, BenchOptions, BenchRun};
use crate::bench::score::{evaluate, BenchResult};
use crate::cli::BenchCmd;

pub async fn run(cmd: BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::List => list(),
        BenchCmd::Run { id, offline } => run_one(&id, offline).await,
        BenchCmd::All { json, offline } => run_all(json, offline).await,
        BenchCmd::Hydrate { fixture, kind } => hydrate(fixture, kind),
        BenchCmd::Report { run_id } => report(&run_id),
    }
}

fn list() -> Result<()> {
    let fixtures = list_fixtures()?;
    if fixtures.is_empty() {
        println!("No benchmark fixtures found in bench/scenarios.");
        return Ok(());
    }
    for fixture in fixtures {
        println!("{}\t{:?}\t{}", fixture.id, fixture.kind, fixture.goal);
    }
    Ok(())
}

async fn run_one(id: &str, offline: bool) -> Result<()> {
    let fixture = find_fixture(id)?;
    let run = crate::bench::runner::run_fixture(&fixture, &BenchOptions { offline }).await?;
    let result = classify_result(evaluate(&run, &fixture));
    if !result.passed {
        let _ = crate::bench::report::write_lesson_draft(&run, &fixture, &result)?;
    }
    print_result(&result);
    if !result.passed {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_all(json_output: bool, offline: bool) -> Result<()> {
    let started_at = Utc::now();
    let fixtures = list_fixtures()?;
    let mut results = Vec::new();
    for fixture in &fixtures {
        let run = match crate::bench::runner::run_fixture(fixture, &BenchOptions { offline }).await
        {
            Ok(run) => run,
            Err(err) => failed_run(fixture, err),
        };
        results.push(classify_result(evaluate(&run, fixture)));
    }
    let report = BenchAllReport::from_results(started_at, Utc::now(), results);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for result in &report.results {
            print_result(result);
        }
        println!(
            "summary: {}/{} passed{}",
            report.summary.passed,
            report
                .summary
                .total
                .saturating_sub(report.summary.cache_miss),
            cache_miss_hint(report.summary.cache_miss)
        );
    }
    let exit_code = bench_all_exit_code(&report.summary);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn classify_result(mut result: BenchResult) -> BenchResult {
    if result
        .error
        .as_deref()
        .is_some_and(is_offline_cache_miss_error)
    {
        result.cache_miss = true;
        result.passed = false;
    }
    result
}

fn is_offline_cache_miss_error(error: &str) -> bool {
    error.contains("not cached and --offline set")
}

fn cache_miss_hint(cache_miss: usize) -> String {
    if cache_miss == 0 {
        String::new()
    } else {
        format!(", {cache_miss} cache miss (run `maestro bench hydrate` first)")
    }
}

fn bench_all_exit_code(summary: &BenchSummary) -> i32 {
    if summary.failed > 0 {
        1
    } else {
        0
    }
}

fn failed_run(fixture: &Fixture, err: anyhow::Error) -> BenchRun {
    let started_at = Utc::now();
    BenchRun {
        id: format!(
            "{}-error-{}",
            fixture.id,
            started_at.format("%Y%m%d-%H%M%S")
        ),
        fixture_id: fixture.id.clone(),
        started_at,
        plan: None,
        run_state: None,
        error: Some(format!("{err:#}")),
    }
}

fn report(run_id: &str) -> Result<()> {
    let path = crate::paths::bench_runs_dir()?
        .join(run_id)
        .join(crate::bench::runner::BENCH_RESULT_FILE);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {path:?}"))?;
    let run: BenchRun = serde_json::from_str(&text).with_context(|| format!("parse {path:?}"))?;
    println!("{}", serde_json::to_string_pretty(&run)?);
    Ok(())
}

fn hydrate(fixture: Option<String>, kind: Option<String>) -> Result<()> {
    let kind = kind
        .as_deref()
        .map(parse_fixture_kind)
        .transpose()
        .with_context(|| format!("invalid --kind `{}`", kind.unwrap_or_default()))?;
    let selector = HydrateSelector { fixture, kind };
    let fixtures = list_fixtures()?;
    let report = hydrate_fixtures(&fixtures, &selector)?;
    for status in &report.statuses {
        println!("{}", format_hydrate_status_line(status));
    }
    println!(
        "summary: hydrated={}, cached={}, skipped={}, failed={}",
        report.hydrated, report.cached, report.skipped, report.failed
    );
    if report.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn parse_fixture_kind(kind: &str) -> Result<FixtureKind> {
    match kind {
        "oss-replay" => Ok(FixtureKind::OssReplay),
        "contract-break" => Ok(FixtureKind::ContractBreak),
        _ => anyhow::bail!("expected one of: oss-replay, contract-break"),
    }
}

#[derive(Debug, Default)]
struct HydrateSelector {
    fixture: Option<String>,
    kind: Option<FixtureKind>,
}

#[derive(Debug, PartialEq, Eq)]
enum HydrateStatusKind {
    Hydrated,
    AlreadyCached,
    Skipped,
    Failed,
}

#[derive(Debug)]
struct HydrateStatus {
    fixture_id: String,
    status: HydrateStatusKind,
    message: Option<String>,
}

#[derive(Debug, Default)]
struct HydrateReport {
    hydrated: usize,
    cached: usize,
    skipped: usize,
    failed: usize,
    statuses: Vec<HydrateStatus>,
}

fn hydrate_fixtures(fixtures: &[Fixture], selector: &HydrateSelector) -> Result<HydrateReport> {
    let mut report = HydrateReport::default();
    for fixture in fixtures {
        if selector
            .fixture
            .as_ref()
            .is_some_and(|id| id != &fixture.id)
            || selector.kind.is_some_and(|kind| kind != fixture.kind)
        {
            continue;
        }
        if !matches!(fixture.kind, FixtureKind::OssReplay) {
            report.skipped += 1;
            report.statuses.push(HydrateStatus {
                fixture_id: fixture.id.clone(),
                status: HydrateStatusKind::Skipped,
                message: Some(format!("kind {:?} does not require cache", fixture.kind)),
            });
            continue;
        }

        let cache_dir = crate::paths::bench_cache_dir()?.join(&fixture.id);
        let cached_before = cache_dir.exists();
        match ensure_upstream_checkout(fixture, false) {
            Ok(_) if cached_before => {
                report.cached += 1;
                report.statuses.push(HydrateStatus {
                    fixture_id: fixture.id.clone(),
                    status: HydrateStatusKind::AlreadyCached,
                    message: None,
                });
            }
            Ok(_) => {
                report.hydrated += 1;
                report.statuses.push(HydrateStatus {
                    fixture_id: fixture.id.clone(),
                    status: HydrateStatusKind::Hydrated,
                    message: None,
                });
            }
            Err(err) => {
                report.failed += 1;
                report.statuses.push(HydrateStatus {
                    fixture_id: fixture.id.clone(),
                    status: HydrateStatusKind::Failed,
                    message: Some(format!("{err:#}")),
                });
            }
        }
    }
    Ok(report)
}

fn hydrate_status_label(status: &HydrateStatusKind) -> &'static str {
    match status {
        HydrateStatusKind::Hydrated => "HYDRATED",
        HydrateStatusKind::AlreadyCached => "ALREADY-CACHED",
        HydrateStatusKind::Skipped => "SKIPPED",
        HydrateStatusKind::Failed => "FAILED",
    }
}

fn format_hydrate_status_line(status: &HydrateStatus) -> String {
    let label = hydrate_status_label(&status.status);
    match &status.message {
        Some(message) => format!("{label:<14}  {}  {message}", status.fixture_id),
        None => format!("{label:<14}  {}", status.fixture_id),
    }
}

pub fn list_fixtures() -> Result<Vec<Fixture>> {
    crate::bench::fixture::load_all()
}

fn find_fixture(id: &str) -> Result<Fixture> {
    list_fixtures()?
        .into_iter()
        .find(|fixture| fixture.id == id)
        .with_context(|| format!("unknown benchmark fixture `{id}`"))
}

fn print_result(result: &BenchResult) {
    let status = if result.passed { "PASS" } else { "FAIL" };
    println!(
        "{status}\t{}\t{:?}\tcoverage={:.2}\tfiles={:.2}\truntime={}ms",
        result.fixture_id,
        result.kind,
        result.plan_score.project_coverage,
        result.plan_score.file_jaccard,
        result.runtime_ms
    );
}

#[derive(Debug, Serialize)]
pub struct BenchAllReport {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub results: Vec<BenchResult>,
    pub summary: BenchSummary,
}

impl BenchAllReport {
    fn from_results(
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        results: Vec<BenchResult>,
    ) -> Self {
        let summary = BenchSummary::from_results(&results);
        Self {
            started_at,
            finished_at,
            results,
            summary,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BenchSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cache_miss: usize,
    pub oss_replay_pass_rate: f64,
    pub contract_break_pass_rate: f64,
}

impl BenchSummary {
    fn from_results(results: &[BenchResult]) -> Self {
        let total = results.len();
        let passed = results.iter().filter(|result| result.passed).count();
        let cache_miss = results.iter().filter(|result| result.cache_miss).count();
        let oss = pass_rate(results, FixtureKind::OssReplay);
        let contract = pass_rate(results, FixtureKind::ContractBreak);
        Self {
            total,
            passed,
            failed: total.saturating_sub(passed + cache_miss),
            cache_miss,
            oss_replay_pass_rate: oss,
            contract_break_pass_rate: contract,
        }
    }
}

fn pass_rate(results: &[BenchResult], kind: FixtureKind) -> f64 {
    let total = results
        .iter()
        .filter(|result| result.kind == kind && !result.cache_miss)
        .count();
    if total == 0 {
        return 1.0;
    }
    let passed = results
        .iter()
        .filter(|result| result.kind == kind && result.passed && !result.cache_miss)
        .count();
    passed as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::bench::fixture::{Budget, Expected, Fixture, FixtureKind, Upstream};
    use crate::bench::score::{BenchResult, PlanScore};
    use std::fs;

    fn with_workspace() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
        }
        dir
    }

    fn clear_workspace_env() {
        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[test]
    #[serial]
    fn bench_list_works_when_empty() {
        let _workspace = with_workspace();

        let fixtures = super::list_fixtures().expect("fixtures");

        assert!(fixtures.is_empty());
        clear_workspace_env();
    }

    #[test]
    fn bench_summary_separates_cache_miss_from_real_failure() {
        let results = vec![
            bench_result("pass", FixtureKind::OssReplay, true, false),
            bench_result("cache-miss", FixtureKind::OssReplay, false, true),
            bench_result("fail", FixtureKind::ContractBreak, false, false),
        ];

        let summary = super::BenchSummary::from_results(&results);

        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.cache_miss, 1);
        assert_eq!(summary.oss_replay_pass_rate, 1.0);
        assert_eq!(summary.contract_break_pass_rate, 0.0);
    }

    #[test]
    fn classify_result_marks_offline_cache_miss_errors() {
        let result = bench_result("missing", FixtureKind::OssReplay, false, false);
        let result = BenchResult {
            error: Some(
                "fixture missing not cached and --offline set; run `maestro bench hydrate` first"
                    .to_string(),
            ),
            ..result
        };

        let result = super::classify_result(result);

        assert!(result.cache_miss);
        assert!(!result.passed);
    }

    #[test]
    fn bench_all_exit_code_allows_cache_miss_only_reports() {
        let cache_miss_only = super::BenchSummary::from_results(&[bench_result(
            "cache-miss",
            FixtureKind::OssReplay,
            false,
            true,
        )]);
        let real_failure = super::BenchSummary::from_results(&[bench_result(
            "fail",
            FixtureKind::ContractBreak,
            false,
            false,
        )]);

        assert_eq!(super::bench_all_exit_code(&cache_miss_only), 0);
        assert_eq!(super::bench_all_exit_code(&real_failure), 1);
    }

    #[test]
    #[serial]
    fn hydrate_fixtures_reports_cached_and_skipped_fixtures() {
        let _workspace = with_workspace();
        fs::create_dir_all(
            crate::paths::bench_cache_dir()
                .expect("bench cache")
                .join("cached-oss"),
        )
        .expect("cached fixture");
        let fixtures = vec![oss_fixture("cached-oss"), contract_fixture("contract-only")];

        let report = super::hydrate_fixtures(&fixtures, &super::HydrateSelector::default())
            .expect("hydrate");

        assert_eq!(report.cached, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(
            report.statuses[0].status,
            super::HydrateStatusKind::AlreadyCached
        );
        assert_eq!(report.statuses[1].status, super::HydrateStatusKind::Skipped);
        clear_workspace_env();
    }

    #[test]
    fn format_hydrate_status_line_aligns_status_labels() {
        let hydrated = super::HydrateStatus {
            fixture_id: "oss-one".to_string(),
            status: super::HydrateStatusKind::Hydrated,
            message: None,
        };
        let skipped = super::HydrateStatus {
            fixture_id: "contract-only".to_string(),
            status: super::HydrateStatusKind::Skipped,
            message: Some("kind ContractBreak does not require cache".to_string()),
        };

        assert_eq!(
            super::format_hydrate_status_line(&hydrated),
            "HYDRATED        oss-one"
        );
        assert_eq!(
            super::format_hydrate_status_line(&skipped),
            "SKIPPED         contract-only  kind ContractBreak does not require cache"
        );
    }

    fn bench_result(id: &str, kind: FixtureKind, passed: bool, cache_miss: bool) -> BenchResult {
        BenchResult {
            fixture_id: id.to_string(),
            kind,
            plan_score: PlanScore {
                project_coverage: if passed { 1.0 } else { 0.0 },
                file_jaccard: if passed { 1.0 } else { 0.0 },
                forbidden_hit: false,
                task_budget_ok: passed,
            },
            contract_score: None,
            error: if cache_miss {
                Some("fixture missing not cached and --offline set".to_string())
            } else {
                None
            },
            cache_miss,
            passed,
            runtime_ms: 0,
        }
    }

    fn oss_fixture(id: &str) -> Fixture {
        Fixture {
            id: id.to_string(),
            kind: FixtureKind::OssReplay,
            goal: "Replay a real PR".to_string(),
            upstream: Some(Upstream {
                url: "https://example.invalid/repo.git".to_string(),
                parent_sha: "abc123".to_string(),
                pr_url: "https://example.invalid/pr/1".to_string(),
            }),
            expected: Expected::default(),
            budget: Budget::default(),
        }
    }

    fn contract_fixture(id: &str) -> Fixture {
        Fixture {
            id: id.to_string(),
            kind: FixtureKind::ContractBreak,
            goal: "Catch an upstream contract break".to_string(),
            upstream: None,
            expected: Expected::default(),
            budget: Budget::default(),
        }
    }
}
