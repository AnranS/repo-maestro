//! Zero-config hello-world DAG for first-time users.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::cli::DemoArgs;
use crate::config::{Plan, ProjectsConfig};

pub async fn run(args: &DemoArgs) -> Result<()> {
    let root = resolve_demo_dir(args.dir.as_deref())?;
    create_demo_workspace(&root)?;

    println!("→ demo workspace: {}", root.display());
    println!("  projects: core -> cli");
    println!("  agent: shell (no LLM credentials required)");

    if args.run {
        run_demo_dag(&root).await?;
    } else {
        render_demo_dry_run(&root)?;
    }
    Ok(())
}

fn resolve_demo_dir(dir: Option<&Path>) -> Result<PathBuf> {
    let dir = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("maestro-demo"));
    if dir.is_absolute() {
        Ok(dir)
    } else {
        Ok(std::env::current_dir()?.join(dir))
    }
}

fn create_demo_workspace(root: &Path) -> Result<()> {
    if root.exists() && std::fs::read_dir(root)?.next().is_some() {
        anyhow::bail!(
            "demo directory already exists and is not empty: {}\n  fix: choose a new --dir or remove the old demo directory",
            root.display()
        );
    }
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;

    write_demo_file(
        root,
        "core/package.json",
        r#"{"name":"maestro-demo-core","private":true,"scripts":{"test":"echo core ok"}}"#,
    )?;
    write_demo_file(
        root,
        "cli/package.json",
        r#"{"name":"maestro-demo-cli","private":true,"dependencies":{"maestro-demo-core":"file:../core"},"scripts":{"test":"echo cli ok"}}"#,
    )?;
    write_demo_file(root, ".maestro/projects.yaml", PROJECTS_YAML)?;
    write_demo_file(root, ".maestro/PLAN.yaml", PLAN_YAML)?;
    Ok(())
}

fn write_demo_file(root: &Path, rel: &str, content: &str) -> Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

fn render_demo_dry_run(root: &Path) -> Result<()> {
    let projects = ProjectsConfig::load(&root.join(".maestro/projects.yaml"))?;
    let plan = Plan::load(&root.join(".maestro/PLAN.yaml"))?;
    let summary = crate::scheduler::dry_run_in_workspace(&plan, &projects, root)?;

    println!(
        "→ dry-run {} ({} task(s))",
        summary.run_id, summary.task_count
    );
    for (id, path) in &summary.files {
        println!("  • {id:<28}  {}", path.display());
    }
    println!();
    println!("next:");
    println!("  cd {}", root.display());
    println!("  maestro run .maestro/PLAN.yaml");
    println!("  # or recreate and execute directly: maestro demo --dir <new-dir> --run");
    Ok(())
}

async fn run_demo_dag(root: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("locate maestro executable")?;
    let status = Command::new(exe)
        .args(["run", ".maestro/PLAN.yaml"])
        .current_dir(root)
        .status()
        .await
        .context("run demo DAG")?;
    if !status.success() {
        anyhow::bail!("demo DAG exited with status {status}");
    }

    let report = current_report_path(root);
    if let Some(report) = report.filter(|p| p.exists()) {
        println!("→ report: {}", report.display());
    } else {
        println!("→ demo DAG finished; report path was not found");
    }
    print!("{}", real_workspace_next_steps(root));
    Ok(())
}

fn real_workspace_next_steps(root: &Path) -> String {
    format!(
        "\nnext:\n  cd {}\n  maestro init --analyze --root /path/to/your/projects --agent mock\n  maestro work \"<goal>\" --root /path/to/your/projects --agent mock --dry\n",
        root.display()
    )
}

fn current_report_path(root: &Path) -> Option<PathBuf> {
    let runs_dir = root.join(".maestro/runs");
    let current = runs_dir.join("current");
    let target = std::fs::read_link(&current)
        .or_else(|_| std::fs::canonicalize(&current))
        .ok()?;
    let run_dir = if target.is_absolute() {
        target
    } else {
        runs_dir.join(target)
    };
    Some(run_dir.join("REPORT.md"))
}

const PROJECTS_YAML: &str = r#"version: 1
defaults:
  agent: shell
  branch_prefix: feat/
  max_parallel: 2
projects:
  core:
    path: core
    type: library
    stack: [node, npm]
    commands:
      test: echo core ok
    agent: shell
  cli:
    path: cli
    type: tool
    stack: [node, npm]
    commands:
      test: echo cli ok
    dependencies: [core]
    agent: shell
"#;

const PLAN_YAML: &str = r#"spec: Maestro hello-world DAG
created_by: maestro-demo
tasks:
  - id: T_verify_core
    project: core
    kind: verify
    agent: shell
    command: echo core ok
  - id: T_verify_cli
    project: cli
    kind: verify
    agent: shell
    depends_on: [T_verify_core]
    command: echo cli ok
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn demo_creates_runnable_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let demo_dir = dir.path().join("d");
        run(&DemoArgs {
            dir: Some(demo_dir.clone()),
            run: false,
        })
        .await
        .unwrap();
        assert!(demo_dir.join(".maestro/PLAN.yaml").exists());
        assert!(demo_dir.join(".maestro/projects.yaml").exists());
        assert!(demo_dir.join("core/package.json").exists());
        assert!(demo_dir.join("cli/package.json").exists());
        assert!(demo_dir.join(".maestro/runs").exists());
    }

    #[test]
    fn run_demo_next_steps_point_to_real_workspace_analysis() {
        let text = real_workspace_next_steps(Path::new("/tmp/demo"));

        assert!(text.contains("next:"));
        assert!(text.contains("maestro init --analyze --root /path/to/your/projects --agent mock"));
        assert!(text.contains("maestro work \"<goal>\" --root /path/to/your/projects"));
    }
}
