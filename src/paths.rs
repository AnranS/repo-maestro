use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const MAESTRO_DIR: &str = ".maestro";
pub const PROJECTS_FILE: &str = "projects.yaml";
pub const RUNS_DIR: &str = "runs";
pub const CONTROL_DIR: &str = "control";
pub const APPROVALS_DIR: &str = "approvals";
pub const CANCELS_DIR: &str = "cancels";
pub const CURRENT_LINK: &str = "current";
pub const RUN_STATE_FILE: &str = "RUN_STATE.json";
pub const RUN_EVENTS_FILE: &str = "events.ndjson";
pub const RUN_FINDINGS_FILE: &str = "findings.ndjson";
pub const PLAN_SNAPSHOT: &str = "PLAN.yaml";
/// F-122: the pinned plan-preview snapshot beside `PLAN.yaml` in a real run dir.
pub const PLAN_PREVIEW_SNAPSHOT: &str = "PLAN_PREVIEW.json";
/// F-127: the durable PM-to-delivery record in a delivery dir.
pub const DELIVERIES_DIR: &str = "deliveries";
pub const DELIVERY_FILE: &str = "DELIVERY.json";
pub const RUN_REPORT_FILE: &str = "REPORT.md";
pub const LOGS_DIR: &str = "logs";
pub const TRAJECTORIES_DIR: &str = "trajectories";
pub const BENCH_DIR: &str = "bench";
pub const BENCH_SCENARIOS_DIR: &str = "scenarios";
pub const BENCH_CACHE_DIR: &str = "cache";
pub const BENCH_RUNS_DIR: &str = "runs";

/// Resolve the workspace root. By default this is the current working
/// directory of the `maestro` process. When the `MAESTRO_WORKSPACE_ROOT` env var is
/// set, that path wins — this is what the integration tests use so they can
/// point maestro at a `tempfile::TempDir` without `cd`-ing the whole process
/// (process-wide cwd mutation makes parallel tests race).
pub fn workspace_root() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("MAESTRO_WORKSPACE_ROOT") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    std::env::current_dir().context("get cwd")
}

pub fn maestro_dir() -> Result<PathBuf> {
    Ok(workspace_root()?.join(MAESTRO_DIR))
}

pub fn projects_file() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(PROJECTS_FILE))
}

pub fn runs_dir() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(RUNS_DIR))
}

/// F-127: PM-to-delivery records live here, one dir per delivery (a delivery
/// precedes a run and may span multiple runs/retries, so it is NOT a run dir).
pub fn deliveries_dir() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(DELIVERIES_DIR))
}

/// `.maestro/deliveries/<delivery_id>/` — the id is validated as a safe path
/// component first (never a traversal).
pub fn delivery_dir(delivery_id: &str) -> Result<PathBuf> {
    validate_path_component("delivery id", delivery_id)?;
    Ok(deliveries_dir()?.join(delivery_id))
}

pub fn bench_scenarios_dir() -> Result<PathBuf> {
    Ok(workspace_root()?.join(BENCH_DIR).join(BENCH_SCENARIOS_DIR))
}

pub fn bench_cache_dir() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(BENCH_DIR).join(BENCH_CACHE_DIR))
}

pub fn bench_runs_dir() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(BENCH_DIR).join(BENCH_RUNS_DIR))
}

pub fn validate_path_component(kind: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{kind} cannot be empty");
    }
    let path = Path::new(value);
    let mut components = path.components();
    let Some(first) = components.next() else {
        anyhow::bail!("{kind} cannot be empty");
    };
    if components.next().is_some() || !matches!(first, std::path::Component::Normal(_)) {
        anyhow::bail!("{kind} must be a single path component: {value:?}");
    }
    if value == "." || value == ".." || value.contains(['/', '\\']) {
        anyhow::bail!("{kind} must not contain path separators: {value:?}");
    }
    Ok(())
}

pub fn run_dir_for_id(id: &str) -> Result<PathBuf> {
    validate_path_component("run id", id)?;
    Ok(runs_dir()?.join(id))
}

pub fn control_marker_path(dir: &Path, kind: &str, id: &str) -> Result<PathBuf> {
    validate_path_component(kind, id)?;
    Ok(dir.join(id))
}

pub fn control_dir() -> Result<PathBuf> {
    Ok(maestro_dir()?.join(CONTROL_DIR))
}

pub fn approvals_dir() -> Result<PathBuf> {
    Ok(control_dir()?.join(APPROVALS_DIR))
}

pub fn cancels_dir() -> Result<PathBuf> {
    Ok(control_dir()?.join(CANCELS_DIR))
}

pub fn current_run_link() -> Result<PathBuf> {
    Ok(runs_dir()?.join(CURRENT_LINK))
}

pub fn current_run_dir() -> Result<Option<PathBuf>> {
    let link = current_run_link()?;
    if link.exists() {
        let real = std::fs::read_link(&link)
            .or_else(|_| std::fs::canonicalize(&link))
            .context("resolve current run link")?;
        if real.is_absolute() {
            Ok(Some(real))
        } else {
            Ok(Some(runs_dir()?.join(real)))
        }
    } else {
        Ok(None)
    }
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    if !p.exists() {
        std::fs::create_dir_all(p).with_context(|| format!("create dir {:?}", p))?;
    }
    Ok(())
}

pub fn expand(p: &str) -> Result<PathBuf> {
    let s = shellexpand::full(p)
        .with_context(|| format!("expand path {p:?}"))?
        .into_owned();
    Ok(PathBuf::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_component_rejects_traversal() {
        for bad in ["", ".", "..", "../run", "nested/run", r"nested\run"] {
            assert!(
                validate_path_component("id", bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        validate_path_component("id", "20260523-120000_abcd1234").unwrap();
        validate_path_component("id", "T_lint__web").unwrap();
    }

    #[test]
    fn control_marker_path_stays_under_control_dir() {
        let dir = Path::new("/tmp/maestro-control");
        assert_eq!(
            control_marker_path(dir, "task id", "T_review").unwrap(),
            dir.join("T_review")
        );
        assert!(control_marker_path(dir, "task id", "../current").is_err());
    }
}
