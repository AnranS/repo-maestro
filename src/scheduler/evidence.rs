//! Durable evidence bundle for a completed run.
//!
//! `RUN_STATE.json` is optimized for the live dashboard. This file is a compact,
//! review-friendly summary that answers "what actually ran, where, and did
//! anything overlap?" without requiring the reader to reconstruct timelines
//! from log files.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::state::RunState;
use crate::paths;
use crate::schema::artifacts::{ArtifactManifest, ArtifactRef, ArtifactSource};
use crate::schema::permissions::PermissionEvidence;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvidence {
    pub run_id: String,
    pub spec: String,
    pub status: String,
    pub verified: bool,
    pub max_parallel: usize,
    pub max_observed_parallelism: usize,
    pub task_count: usize,
    pub tasks: Vec<TaskEvidence>,
    pub parallel_windows: Vec<ParallelWindow>,
    pub acceptance: Vec<AcceptanceEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
    #[serde(default)]
    pub browser: BrowserEvidenceSummary,
    /// Decisions maestro made automatically (contract wiring, retries,
    /// circuit-breaker trips, integration conflicts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_actions: Vec<super::state::AutoAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvidence {
    pub id: String,
    pub project: String,
    pub agent: String,
    pub kind: String,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub workspace_path: Option<String>,
    pub worktree_path: Option<String>,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelWindow {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub concurrency: usize,
    pub task_ids: Vec<String>,
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceEvidence {
    pub describe: String,
    pub check: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_excerpt: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserEvidenceSummary {
    pub present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_path: Option<String>,
    pub artifact_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<BrowserCheckEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dom_snapshots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub videos: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_failures: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub console_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserCheckEvidence {
    pub describe: String,
    pub check: String,
    pub passed: bool,
    pub browser_related: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserArtifactEvidence {
    pub kind: String,
    pub source_path: String,
    pub evidence_path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReplay {
    pub run_id: String,
    pub status: String,
    pub event_count: usize,
    pub max_observed_parallelism: usize,
    pub events: Vec<ReplayEvent>,
    pub tasks: Vec<ReplayTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayEvent {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayTask {
    pub id: String,
    pub project: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
}

pub fn write_run_evidence(state: &RunState) -> Result<PathBuf> {
    let mut evidence = build_run_evidence(state);
    let dir = state.run_dir.join("evidence");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    evidence.browser = write_browser_evidence(state, &evidence)?;
    evidence
        .artifact_refs
        .extend(browser_artifact_refs(&evidence.browser));
    write_artifact_manifest(&dir, &state.run_id, &evidence.artifact_refs)?;
    let path = dir.join("summary.json");
    let body = serde_json::to_string_pretty(&evidence).context("serialize run evidence")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn build_run_evidence(state: &RunState) -> RunEvidence {
    let tasks = state
        .task_order
        .iter()
        .filter_map(|id| state.tasks.get(id))
        .map(|task| {
            let duration_ms = task
                .started_at
                .zip(task.ended_at)
                .map(|(start, end)| (end - start).num_milliseconds().max(0));
            let mut artifact_refs = task.artifacts.to_artifact_refs(&task.id);
            if let Some(path) = task.trajectory_path.as_ref() {
                let bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
                artifact_refs.push(ArtifactRef {
                    kind: "trajectory".to_string(),
                    source: ArtifactSource::AgentTask,
                    task_id: Some(task.id.clone()),
                    path: Some(path.clone()),
                    uri: None,
                    name: Some(format!("{}.ndjson", task.id)),
                    bytes,
                });
            }
            TaskEvidence {
                id: task.id.clone(),
                project: task.project.clone(),
                agent: task.agent.clone(),
                kind: task.kind.clone(),
                status: format!("{:?}", task.status).to_lowercase(),
                started_at: task.started_at,
                ended_at: task.ended_at,
                duration_ms,
                workspace_path: task.workspace_path.clone(),
                worktree_path: task.worktree_path.clone(),
                log_path: task.log_path.clone(),
                trajectory_path: task.trajectory_path.clone(),
                permission: task.permission.clone(),
                artifact_refs,
            }
        })
        .collect::<Vec<_>>();
    let artifact_refs = tasks
        .iter()
        .flat_map(|task| task.artifact_refs.clone())
        .collect::<Vec<_>>();
    let parallel_windows = compute_parallel_windows(&tasks);
    let max_observed_parallelism = parallel_windows
        .iter()
        .map(|window| window.concurrency)
        .max()
        .unwrap_or_else(|| usize::from(!tasks.is_empty()));
    let acceptance = state
        .acceptance_results
        .iter()
        .map(|result| AcceptanceEvidence {
            describe: result.describe.clone(),
            check: result.check.clone(),
            passed: result.passed,
            exit_code: result.exit_code,
            output_excerpt: tail_excerpt(&result.output, 1200),
            started_at: result.started_at,
            ended_at: result.ended_at,
        })
        .collect();

    RunEvidence {
        run_id: state.run_id.clone(),
        spec: state.spec.clone(),
        status: format!("{:?}", state.status).to_lowercase(),
        verified: state.verified,
        max_parallel: state.max_parallel,
        max_observed_parallelism,
        task_count: tasks.len(),
        tasks,
        parallel_windows,
        acceptance,
        artifact_refs,
        browser: read_browser_summary(&state.run_dir).unwrap_or_default(),
        auto_actions: state.auto_actions.clone(),
    }
}

fn write_artifact_manifest(
    evidence_dir: &Path,
    run_id: &str,
    artifact_refs: &[ArtifactRef],
) -> Result<PathBuf> {
    let manifest = ArtifactManifest::new(run_id, artifact_refs.to_vec());
    let path = evidence_dir.join("artifacts.json");
    let body = serde_json::to_string_pretty(&manifest).context("serialize artifact manifest")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn browser_artifact_refs(summary: &BrowserEvidenceSummary) -> Vec<ArtifactRef> {
    let mut refs = Vec::new();
    for (kind, paths) in [
        ("screenshot", &summary.screenshots),
        ("trace", &summary.traces),
        ("dom", &summary.dom_snapshots),
        ("video", &summary.videos),
    ] {
        for path in paths {
            refs.push(ArtifactRef {
                kind: kind.to_string(),
                source: ArtifactSource::Verification,
                task_id: None,
                path: Some(path.clone()),
                uri: None,
                name: None,
                bytes: None,
            });
        }
    }
    refs
}

pub fn build_run_replay(run_dir: &Path) -> Result<RunReplay> {
    let state = RunState::load(run_dir)?;
    let evidence = read_evidence_summary(run_dir).unwrap_or_else(|| build_run_evidence(&state));
    let events = super::events::read_events(run_dir)?
        .into_iter()
        .map(|event| ReplayEvent {
            seq: event.seq,
            timestamp: event.timestamp,
            kind: event.kind.as_str().to_string(),
            task_id: event.task_id,
            message: event.message,
        })
        .collect::<Vec<_>>();
    let tasks = state
        .task_order
        .iter()
        .filter_map(|id| state.tasks.get(id))
        .map(|task| {
            let duration_ms = task
                .started_at
                .zip(task.ended_at)
                .map(|(start, end)| (end - start).num_milliseconds().max(0));
            ReplayTask {
                id: task.id.clone(),
                project: task.project.clone(),
                status: format!("{:?}", task.status).to_lowercase(),
                depends_on: task.depends_on.clone(),
                started_at: task.started_at,
                ended_at: task.ended_at,
                duration_ms,
            }
        })
        .collect::<Vec<_>>();
    Ok(RunReplay {
        run_id: state.run_id,
        status: format!("{:?}", state.status).to_lowercase(),
        event_count: events.len(),
        max_observed_parallelism: evidence.max_observed_parallelism,
        events,
        tasks,
    })
}

pub fn read_evidence_summary(run_dir: &Path) -> Option<RunEvidence> {
    let path = run_dir.join("evidence").join("summary.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn render_pr_body(state: &RunState) -> String {
    let evidence =
        read_evidence_summary(&state.run_dir).unwrap_or_else(|| build_run_evidence(state));
    let mut s = String::new();
    s.push_str(&format!("## Summary\n\n{}\n\n", state.spec));
    s.push_str("## Run Evidence\n\n");
    s.push_str(&format!("- Run: `{}`\n", state.run_id));
    s.push_str(&format!("- Status: `{}`\n", evidence.status));
    s.push_str(&format!("- Verified: `{}`\n", evidence.verified));
    s.push_str(&format!(
        "- Observed parallelism: `{}/{}` with {} overlap window(s)\n",
        evidence.max_observed_parallelism,
        evidence.max_parallel,
        evidence.parallel_windows.len()
    ));
    s.push_str("- Report: `.maestro/runs/");
    s.push_str(&state.run_id);
    s.push_str("/REPORT.md`\n");
    s.push_str("- Evidence: `.maestro/runs/");
    s.push_str(&state.run_id);
    s.push_str("/evidence/summary.json`\n");
    if evidence.browser.present {
        s.push_str("- Browser QA evidence: `.maestro/runs/");
        s.push_str(&state.run_id);
        s.push_str("/evidence/browser/summary.md`\n");
    }

    if !evidence.acceptance.is_empty() {
        s.push_str("\n## Acceptance\n\n");
        for check in &evidence.acceptance {
            let mark = if check.passed { "PASS" } else { "FAIL" };
            let code = check
                .exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "spawn error".to_string());
            s.push_str(&format!("- [{mark}] {} ({code})\n", check.describe));
            s.push_str(&format!("  - `{}`\n", check.check));
        }
    }

    if !evidence.parallel_windows.is_empty() {
        s.push_str("\n## Parallelism\n\n");
        for window in evidence.parallel_windows.iter().take(8) {
            s.push_str(&format!(
                "- `{}`..`{}` x{}: {} ({})\n",
                window.started_at.format("%H:%M:%S%.3f"),
                window.ended_at.format("%H:%M:%S%.3f"),
                window.concurrency,
                window.task_ids.join(", "),
                window.projects.join(", ")
            ));
        }
    }

    let changed = changed_files(state);
    s.push_str("\n## Changed Files\n\n");
    if changed.is_empty() {
        s.push_str("- No adapter-reported file list was available for this run.\n");
    } else {
        for file in changed {
            s.push_str(&format!("- `{file}`\n"));
        }
    }

    s.push_str("\n## Test Plan\n\n");
    if evidence.acceptance.is_empty() {
        s.push_str(
            "- Inspect `REPORT.md` and `evidence/summary.json` for the executed task transcript.\n",
        );
    } else {
        for check in &evidence.acceptance {
            s.push_str(&format!("- `{}`\n", check.check));
        }
    }
    s
}

pub fn write_pr_body(state: &RunState) -> Result<PathBuf> {
    let path = state.run_dir.join("PR_BODY.md");
    std::fs::write(&path, render_pr_body(state))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn compute_parallel_windows(tasks: &[TaskEvidence]) -> Vec<ParallelWindow> {
    #[derive(Debug, Clone)]
    struct Point<'a> {
        at: DateTime<Utc>,
        starts: Vec<&'a TaskEvidence>,
        ends: Vec<&'a TaskEvidence>,
    }

    let mut points: BTreeMap<DateTime<Utc>, Point<'_>> = BTreeMap::new();
    for task in tasks {
        let (Some(start), Some(end)) = (task.started_at, task.ended_at) else {
            continue;
        };
        if end <= start {
            continue;
        }
        points
            .entry(start)
            .or_insert_with(|| Point {
                at: start,
                starts: Vec::new(),
                ends: Vec::new(),
            })
            .starts
            .push(task);
        points
            .entry(end)
            .or_insert_with(|| Point {
                at: end,
                starts: Vec::new(),
                ends: Vec::new(),
            })
            .ends
            .push(task);
    }

    let mut active: BTreeMap<&str, &TaskEvidence> = BTreeMap::new();
    let mut windows = Vec::new();
    let mut last_at: Option<DateTime<Utc>> = None;
    for point in points.values() {
        if let Some(started_at) = last_at {
            if point.at > started_at && active.len() > 1 {
                let task_ids = active
                    .keys()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<_>>();
                let mut projects = active
                    .values()
                    .map(|task| task.project.clone())
                    .collect::<Vec<_>>();
                projects.sort();
                projects.dedup();
                windows.push(ParallelWindow {
                    started_at,
                    ended_at: point.at,
                    concurrency: active.len(),
                    task_ids,
                    projects,
                });
            }
        }
        for task in &point.ends {
            active.remove(task.id.as_str());
        }
        for task in &point.starts {
            active.insert(task.id.as_str(), task);
        }
        last_at = Some(point.at);
    }

    windows
}

fn write_browser_evidence(
    state: &RunState,
    evidence: &RunEvidence,
) -> Result<BrowserEvidenceSummary> {
    let browser_checks = evidence
        .acceptance
        .iter()
        .enumerate()
        .filter_map(|(idx, check)| {
            let browser_related = is_browser_related(&check.check)
                || is_browser_related(&check.describe)
                || is_browser_related(&check.output_excerpt);
            browser_related.then_some((idx + 1, check))
        })
        .collect::<Vec<_>>();

    let artifact_sources = browser_artifact_sources(state);
    if browser_checks.is_empty() && artifact_sources.is_empty() {
        return Ok(BrowserEvidenceSummary::default());
    }

    let dir = state.run_dir.join("evidence").join("browser");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let mut summary = BrowserEvidenceSummary {
        present: true,
        summary_path: Some(relative_to_run(&state.run_dir, &dir.join("summary.md"))),
        ..BrowserEvidenceSummary::default()
    };

    for (idx, check) in browser_checks {
        let filename = format!("check-{idx:03}.md");
        let path = dir.join(&filename);
        let mut body = String::new();
        body.push_str(&format!("# Browser QA check {idx}\n\n"));
        body.push_str(&format!("- describe: {}\n", check.describe));
        body.push_str(&format!("- passed: {}\n", check.passed));
        body.push_str(&format!("- exit_code: {:?}\n", check.exit_code));
        body.push_str("\n```bash\n");
        body.push_str(check.check.trim());
        body.push_str("\n```\n");
        if !check.output_excerpt.trim().is_empty() {
            body.push_str("\n## Output excerpt\n\n```text\n");
            body.push_str(check.output_excerpt.trim());
            body.push_str("\n```\n");
        }
        std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
        collect_failure_lines(&check.output_excerpt, &mut summary);
        summary.checks.push(BrowserCheckEvidence {
            describe: check.describe.clone(),
            check: check.check.clone(),
            passed: check.passed,
            browser_related: true,
            evidence_path: Some(relative_to_run(&state.run_dir, &path)),
        });
    }

    for artifact in copy_browser_artifacts(&state.run_dir, &dir, artifact_sources)? {
        summary.artifact_count += 1;
        match artifact.kind.as_str() {
            "screenshot" => summary.screenshots.push(artifact.evidence_path),
            "trace" => summary.traces.push(artifact.evidence_path),
            "dom" => summary.dom_snapshots.push(artifact.evidence_path),
            "video" => summary.videos.push(artifact.evidence_path),
            _ => {}
        }
    }

    write_browser_markdown(&dir.join("summary.md"), &summary)?;
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&summary).context("serialize browser evidence")?,
    )
    .with_context(|| format!("write {}", dir.join("summary.json").display()))?;
    Ok(summary)
}

fn read_browser_summary(run_dir: &Path) -> Option<BrowserEvidenceSummary> {
    let path = run_dir
        .join("evidence")
        .join("browser")
        .join("summary.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_browser_markdown(path: &Path, summary: &BrowserEvidenceSummary) -> Result<()> {
    let mut body = String::new();
    body.push_str("# Browser QA Evidence\n\n");
    body.push_str(&format!("- artifacts: {}\n", summary.artifact_count));
    body.push_str(&format!("- checks: {}\n", summary.checks.len()));
    body.push_str(&format!("- screenshots: {}\n", summary.screenshots.len()));
    body.push_str(&format!("- traces: {}\n", summary.traces.len()));
    body.push_str(&format!(
        "- DOM/report snapshots: {}\n",
        summary.dom_snapshots.len()
    ));
    body.push_str(&format!("- videos: {}\n", summary.videos.len()));
    if !summary.checks.is_empty() {
        body.push_str("\n## Checks\n\n");
        for check in &summary.checks {
            let mark = if check.passed { "PASS" } else { "FAIL" };
            let path = check.evidence_path.as_deref().unwrap_or("");
            body.push_str(&format!("- [{mark}] {} — `{path}`\n", check.describe));
        }
    }
    if !summary.network_failures.is_empty() {
        body.push_str("\n## Network Failures\n\n");
        for line in &summary.network_failures {
            body.push_str(&format!("- `{line}`\n"));
        }
    }
    if !summary.console_errors.is_empty() {
        body.push_str("\n## Console Errors\n\n");
        for line in &summary.console_errors {
            body.push_str(&format!("- `{line}`\n"));
        }
    }
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))
}

fn browser_artifact_sources(state: &RunState) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if let Ok(root) = paths::workspace_root() {
        roots.insert(root);
    }
    if let Some(root) = workspace_root_from_run_dir(&state.run_dir) {
        roots.insert(root);
    }
    for task in state.tasks.values() {
        if let Some(path) = &task.workspace_path {
            roots.insert(PathBuf::from(path));
        }
    }

    let common = [
        "test-results",
        "playwright-report",
        "cypress/screenshots",
        "cypress/videos",
        "e2e-results",
        "qa-results",
    ];
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        for rel in common {
            let dir = root.join(rel);
            if dir.exists() {
                collect_browser_artifacts(&dir, &mut out, &mut seen, 0);
            }
        }
    }
    out
}

fn workspace_root_from_run_dir(run_dir: &Path) -> Option<PathBuf> {
    let runs_dir = run_dir.parent()?;
    let maestro_dir = runs_dir.parent()?;
    if maestro_dir.file_name()? != paths::MAESTRO_DIR {
        return None;
    }
    maestro_dir.parent().map(Path::to_path_buf)
}

fn collect_browser_artifacts(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<String>,
    depth: usize,
) {
    if out.len() >= 200 || depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_browser_artifacts(&path, out, seen, depth + 1);
            continue;
        }
        if classify_browser_artifact(&path).is_none() {
            continue;
        }
        let key = path.to_string_lossy().to_string();
        if seen.insert(key) {
            out.push(path);
        }
        if out.len() >= 200 {
            break;
        }
    }
}

fn copy_browser_artifacts(
    run_dir: &Path,
    browser_dir: &Path,
    sources: Vec<PathBuf>,
) -> Result<Vec<BrowserArtifactEvidence>> {
    let mut out = Vec::new();
    for source in sources {
        let Some(kind) = classify_browser_artifact(&source) else {
            continue;
        };
        let meta = match std::fs::metadata(&source) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > 20 * 1024 * 1024 {
            continue;
        }
        let subdir = browser_dir.join("artifacts").join(kind);
        std::fs::create_dir_all(&subdir).with_context(|| format!("create {}", subdir.display()))?;
        let name = evidence_artifact_name(&source);
        let target = subdir.join(name);
        std::fs::copy(&source, &target)
            .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
        out.push(BrowserArtifactEvidence {
            kind: kind.to_string(),
            source_path: source.to_string_lossy().to_string(),
            evidence_path: relative_to_run(run_dir, &target),
            bytes: meta.len(),
        });
    }
    Ok(out)
}

fn classify_browser_artifact(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => Some("screenshot"),
        "webm" | "mp4" => Some("video"),
        "zip" if name.contains("trace") || path.to_string_lossy().contains("test-results") => {
            Some("trace")
        }
        "html" | "json" | "txt" if is_browser_related(&name) => Some("dom"),
        _ => None,
    }
}

fn collect_failure_lines(output: &str, summary: &mut BrowserEvidenceSummary) {
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let clipped = clip(line, 240);
        if contains_any(
            &lower,
            &[
                "requestfailed",
                "network error",
                "net::err",
                "failed request",
                "response 500",
                "response 404",
            ],
        ) && summary.network_failures.len() < 50
        {
            summary.network_failures.push(clipped.clone());
        }
        if contains_any(
            &lower,
            &["console error", "pageerror", "uncaught", "runtime error"],
        ) && summary.console_errors.len() < 50
        {
            summary.console_errors.push(clipped);
        }
    }
}

fn is_browser_related(text: &str) -> bool {
    contains_any(
        &text.to_ascii_lowercase(),
        &[
            "playwright",
            "browser",
            "headless",
            "cypress",
            "screenshot",
            "trace",
            "dom",
            "console",
            "network",
            "login flow",
            "web flow",
            "e2e",
            "end-to-end",
        ],
    )
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn tail_excerpt(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let tail = s
        .chars()
        .rev()
        .take(max_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("... (output truncated) ...\n{tail}")
}

fn clip(s: &str, max_chars: usize) -> String {
    let mut out = s.chars().take(max_chars).collect::<String>();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn evidence_artifact_name(path: &Path) -> String {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}-{filename}")
}

fn relative_to_run(run_dir: &Path, path: &Path) -> String {
    path.strip_prefix(run_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn changed_files(state: &RunState) -> Vec<String> {
    let mut files = BTreeSet::new();
    for task in state.tasks.values() {
        for file in &task.artifacts.files_changed {
            files.insert(file.clone());
        }
    }
    files.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::state::{AcceptanceResult, TaskState, TaskStatus};

    #[test]
    fn computes_overlap_windows_from_task_intervals() {
        let plan = crate::config::Plan {
            spec: "parallel".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: BTreeMap::new(),
            notice: None,
            goal: None,
        };
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        let mut state = RunState::new("run".into(), &plan, &projects, 2, std::env::temp_dir());
        let t0 = Utc::now();
        let a = task("A", "api", t0, t0 + chrono::Duration::milliseconds(100));
        let b = task(
            "B",
            "web",
            t0 + chrono::Duration::milliseconds(20),
            t0 + chrono::Duration::milliseconds(80),
        );
        state.task_order = vec!["A".into(), "B".into()];
        state.tasks.insert("A".into(), a);
        state.tasks.insert("B".into(), b);

        let evidence = build_run_evidence(&state);
        assert_eq!(evidence.max_observed_parallelism, 2);
        assert_eq!(evidence.parallel_windows.len(), 1);
        assert_eq!(evidence.parallel_windows[0].projects, vec!["api", "web"]);
    }

    #[test]
    fn writes_browser_evidence_for_playwright_acceptance_output() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp
            .path()
            .join(".maestro")
            .join("runs")
            .join("run-browser");
        std::fs::create_dir_all(&run_dir).unwrap();
        let artifacts = temp.path().join("test-results").join("login");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("trace.zip"), b"trace").unwrap();
        std::fs::write(artifacts.join("screenshot.png"), b"png").unwrap();
        let plan = crate::config::Plan {
            spec: "browser".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: BTreeMap::new(),
            notice: None,
            goal: None,
        };
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        let mut state = RunState::new("run-browser".into(), &plan, &projects, 1, run_dir.clone());
        let temp_root = temp.path().to_string_lossy().to_string();
        let now = Utc::now();
        state.acceptance_results.push(AcceptanceResult {
            describe: "login flow works".into(),
            check: "npx playwright test login.spec.ts".into(),
            passed: false,
            exit_code: Some(1),
            output: "console error: boom\nrequestfailed http://127.0.0.1/api/login".into(),
            started_at: now,
            ended_at: now,
        });
        for task in state.tasks.values_mut() {
            task.workspace_path = Some(temp_root.clone());
        }

        let path = write_run_evidence(&state).unwrap();
        let evidence: RunEvidence =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(evidence.browser.present);
        assert_eq!(evidence.browser.artifact_count, 2);
        assert_eq!(evidence.browser.screenshots.len(), 1);
        assert_eq!(evidence.browser.traces.len(), 1);
        assert_eq!(evidence.browser.console_errors.len(), 1);
        assert_eq!(evidence.browser.network_failures.len(), 1);
    }

    #[test]
    fn replay_reads_events_and_task_timeline() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join(".maestro").join("runs").join("run-replay");
        std::fs::create_dir_all(&run_dir).unwrap();
        let plan = crate::config::Plan {
            spec: "replay".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: BTreeMap::new(),
            notice: None,
            goal: None,
        };
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        let mut state = RunState::new("run-replay".into(), &plan, &projects, 2, run_dir.clone());
        let t0 = Utc::now();
        state.task_order = vec!["A".into()];
        state.tasks.insert(
            "A".into(),
            task("A", "api", t0, t0 + chrono::Duration::milliseconds(25)),
        );
        state.write_atomic().unwrap();
        super::super::events::append_event(
            &run_dir,
            "run-replay",
            super::super::events::RunEventKind::RunCreated,
            None,
            Some("created".into()),
            serde_json::json!({}),
        )
        .unwrap();
        super::super::events::append_event(
            &run_dir,
            "run-replay",
            super::super::events::RunEventKind::TaskStarted,
            Some("A"),
            Some("task started".into()),
            serde_json::json!({}),
        )
        .unwrap();

        let replay = build_run_replay(&run_dir).unwrap();
        assert_eq!(replay.run_id, "run-replay");
        assert_eq!(replay.event_count, 2);
        assert_eq!(replay.tasks.len(), 1);
        assert_eq!(replay.tasks[0].duration_ms, Some(25));
    }

    fn task(
        id: &str,
        project: &str,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
    ) -> TaskState {
        TaskState {
            id: id.into(),
            project: project.into(),
            agent: "shell".into(),
            status: TaskStatus::Done,
            started_at: Some(started_at),
            ended_at: Some(ended_at),
            chat_id: None,
            error: None,
            attempts: 0,
            risk_level: None,
            artifacts: Default::default(),
            permission: None,
            workflow_outputs: BTreeMap::new(),
            log_path: format!("{id}.log"),
            trajectory_path: None,
            depends_on: vec![],
            parallel_group: None,
            requires_approval_after: false,
            kind: "verify".into(),
            memory_used: vec![],
            context_bytes: None,
            skills_triggered: vec![],
            usage: None,
            steps: None,
            role: None,
            resolved_agent_profile: None,
            resolved_review_profile: None,
            workspace_path: Some(format!("/tmp/{project}")),
            worktree_path: None,
        }
    }
}
