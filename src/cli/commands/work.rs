//! High-level "one command to get moving" workflow.
//!
//! `maestro work` is the day-to-day path: scan, register, plan, validate, and
//! optionally run.

use anyhow::{Context, Result};

use crate::cli::{RunArgs, WorkArgs};
use crate::config::{self, Plan, ProjectsConfig};
use crate::memory::MemoryStore;
use crate::paths;

const SIGINT_EXIT_CODE: i32 = 130;

pub async fn run(args: WorkArgs) -> Result<()> {
    // --json is the dry-first preview: init + discovery run quietly INSIDE
    // run_json_preview so stdout stays a single PlanPreview JSON object.
    if args.json {
        return run_json_preview(args);
    }

    ensure_initialized(false)?;

    if let Some(root) = args.root.as_deref() {
        discover_and_apply(root, args.max_depth, &args.agent, false)?;
    }

    if args.run && !args.force_new {
        ensure_no_conflicting_work_run(&args.spec)?;
    }

    let plan_path = crate::cli::commands::plan::synthesize_file(
        &args.spec,
        args.out,
        args.projects,
        args.root.as_deref(),
    )?;
    validate_generated_plan(&plan_path)?;

    println!("→ workflow ready: {}", plan_path.display());
    if args.run || args.dry {
        let run_args = RunArgs {
            plan: plan_path.clone(),
            only: vec![],
            skip: vec![],
            max_parallel: args.max_parallel,
            continue_on_error: false,
            session_id: None,
            model: args.model,
            dry: args.dry,
            // Synthesis already orders by contract; the wiring pass is then a
            // harmless idempotent backstop.
            no_wire_contracts: false,
            max_tokens: args.max_tokens,
            gates: args.gates,
            // validate_generated_plan already printed the impact preview.
            quiet_impact: true,
        };
        if args.dry {
            return crate::cli::commands::run::run(run_args).await;
        }
        let run_id = crate::scheduler::generate_run_id();
        return run_with_sigint_bridge(run_args, run_id).await;
    }

    println!("next:");
    println!("  maestro run {}", plan_path.display());
    println!("  maestro open --no-browser   # optional: inspect the DAG in the dashboard");
    Ok(())
}

/// `work --dry --json` (F-111): synthesize the plan quietly, then emit ONE
/// machine-readable `PlanPreview` on stdout — the dry-first preview — and stop.
/// stdout is ALWAYS a valid `PlanPreview`, even on a synthesis failure (the
/// problem lands in `errors[]`, the full human error on stderr). Reuses the
/// shared `plan_preview` serializer; only `goal` / `goal_matched` are
/// work-dry-specific.
fn run_json_preview(args: WorkArgs) -> Result<()> {
    use crate::schema::preview::{codes, Issue, PlanPreview};
    match build_json_preview(&args) {
        Ok(preview) => {
            println!("{}", preview.to_json());
            Ok(())
        }
        Err(e) => {
            // Contract: `--json` stdout stays a valid PlanPreview envelope even
            // when synthesis fails. A neutral, path-free message in the issue;
            // the full human error (paths / "next: --root …" guidance) → stderr.
            let preview = PlanPreview::unparseable(Issue::error(
                codes::PLAN_INVALID,
                "could not synthesize plan preview",
            ));
            println!("{}", preview.to_json());
            eprintln!("work --dry --json: {e:#}");
            std::process::exit(2);
        }
    }
}

/// The fallible body of `work --dry --json`: quiet init + discovery + synthesis,
/// then the shared `plan_preview` + `goal` / `goal_matched`. Any error is
/// projected onto a `PlanPreview` envelope by [`run_json_preview`].
fn build_json_preview(args: &WorkArgs) -> Result<crate::schema::preview::PlanPreview> {
    use crate::schema::preview::GoalMatched;
    // Init + discovery run quietly: their progress lines go to stderr so stdout
    // stays a single PlanPreview JSON object (the F-111 contract).
    ensure_initialized(true)?;
    if let Some(root) = args.root.as_deref() {
        discover_and_apply(root, args.max_depth, &args.agent, true)?;
    }
    let plan_path = crate::cli::commands::plan::synthesize_file_quiet(
        &args.spec,
        args.out.clone(),
        args.projects.clone(),
        args.root.as_deref(),
    )?;
    let plan = Plan::load(&plan_path)?;
    let projects = paths::projects_file()
        .ok()
        .and_then(|f| if f.exists() { Some(f) } else { None })
        .and_then(|f| ProjectsConfig::load(&f).ok())
        .unwrap_or_else(|| ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        });
    let report = config::analyze(&plan, &projects);
    let mut preview = crate::cli::commands::plan::plan_preview(&plan, &report);
    preview.goal = Some(args.spec.clone());
    // matched = projects the synthesized plan narrowed to; total = registered
    // projects. Mirrors the human "goal matched X/Y" line.
    preview.goal_matched = Some(GoalMatched {
        matched: preview.project_count,
        total: projects.projects.len() as u32,
    });
    Ok(preview)
}

/// Print a human progress line to stdout, or to stderr when `quiet` — used by
/// `--json` paths that must keep stdout reserved for a single JSON object.
fn emit(quiet: bool, line: String) {
    if quiet {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

pub(crate) fn ensure_initialized(quiet: bool) -> Result<()> {
    let dir = paths::maestro_dir()?;
    paths::ensure_dir(&dir)?;
    paths::ensure_dir(&paths::runs_dir()?)?;
    paths::ensure_dir(&paths::approvals_dir()?)?;
    paths::ensure_dir(&paths::cancels_dir()?)?;

    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        }
        .save(&pfile)?;
        emit(quiet, format!("→ initialized {}", dir.display()));
    }

    let mut seeded = 0usize;
    for (scope, name, content) in crate::cli::samples::init_skill_samples() {
        let scope = crate::skills::SkillScope::from_dir(scope);
        let target = crate::skills::skill_path(&scope, name)?;
        if !target.exists() {
            crate::skills::save(&scope, name, content)?;
            seeded += 1;
        }
    }
    let store = MemoryStore::open()?;
    for (topic, name, content) in crate::cli::samples::init_memory_samples() {
        let target = store.l1_root().join(topic).join(name);
        if !target.exists() {
            store.add(topic, name, content)?;
            seeded += 1;
        }
    }
    if seeded > 0 {
        emit(
            quiet,
            format!("→ seeded {seeded} workflow skill/memory sample(s)"),
        );
    }
    Ok(())
}

pub(crate) fn discover_and_apply(
    root: &std::path::Path,
    max_depth: usize,
    agent: &str,
    quiet: bool,
) -> Result<()> {
    let mut report = config::discover(root, &config::DiscoverOptions { max_depth })?;
    let discovered = report.projects.len();

    // F-109: read declared sibling workspaces from the existing config (if any)
    // so contract promotion can index providers that live in another repo.
    // Relative sibling paths resolve against the WORKSPACE ROOT (where this
    // projects.yaml lives), NOT the scan `--root` — the two diverge under
    // `--root <subdir>`. Empty = single-root behavior, unchanged.
    let pfile = paths::projects_file()?;
    let mut cfg = if pfile.exists() {
        ProjectsConfig::load(&pfile)?
    } else {
        ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        }
    };
    let workspace_root = paths::workspace_root()?;
    let sibling_roots =
        config::resolve_sibling_roots(&workspace_root, &cfg.defaults.sibling_workspaces);

    // F-103: run contract promotion BEFORE printing the edge count so the
    // promoted generated-client edges show up in the headline + are visible
    // in the per-edge enumeration below.
    let (promoted_providers, promoted_consumers) =
        config::promote_contracts_with_siblings(&mut report, &sibling_roots);
    emit(
        quiet,
        format!(
            "→ discovered {} project(s), {} manifest-declared edge(s)",
            discovered,
            report.edges.len()
        ),
    );
    if promoted_providers > 0 || promoted_consumers > 0 {
        emit(
            quiet,
            format!(
                "→ codegraph: promoted {} project(s) to contract provider(s), {} to consumer(s).",
                promoted_providers, promoted_consumers,
            ),
        );
    }
    for edge in &report.edges {
        emit(
            quiet,
            format!(
                "  {} -> {} [{}%] {}",
                edge.from, edge.to, edge.confidence, edge.reason
            ),
        );
    }

    // `pfile` + `cfg` were loaded above (to read sibling_workspaces before
    // promotion); reuse them here for the merge/write.
    let mut added = 0usize;
    let mut updated = 0usize;
    for (name, project) in &report.projects {
        let incoming = config::project_from_discovery(project, Some(agent));
        // MERGE incoming discovery output into any existing entry rather
        // than overwriting wholesale. Discovery's job is to (re)derive
        // path / type / stack / commands / dependencies; the user's
        // hand-customizations (memory_scope, role, agent override, model
        // overrides, contracts, copy_files) must survive a re-init.
        // Before this merge, a fresh `mst init` on an existing workspace
        // silently wiped every hand-edit — caught on a multi-project
        // workspace where a library's memory_scope had been pinned.
        match cfg.projects.get_mut(name) {
            Some(existing) => {
                config::merge_project_from_discovery(existing, incoming);
                updated += 1;
            }
            None => {
                cfg.projects.insert(name.clone(), incoming);
                added += 1;
            }
        }
    }

    // Fold codegraph-derived module deps into projects.yaml's
    // dependencies arrays. Without this step, a combined workspace where
    // the cross-stack contract / import edges only become visible to the
    // tree-sitter scan (bam-idl, openapi-client, go imports across
    // monorepo modules) gets 0 inferred edges in projects.yaml, the DAG
    // runs everything in parallel, and `mst brief` is the only place the
    // user sees the real topology.
    //
    // We DON'T overwrite hand-authored dependencies — the discovery
    // report's project_from_discovery already seeded them; we merge into
    // that set so derived edges are additive.
    let module_paths: Vec<(String, String)> = cfg
        .projects
        .iter()
        .map(|(name, p)| (name.clone(), p.path.clone()))
        .collect();
    let derived = crate::codegraph::load_derived_module_deps(root, &module_paths, 1);
    let mut derived_added = 0usize;
    for (consumer, producers) in &derived {
        let Some(project) = cfg.projects.get_mut(consumer) else {
            continue;
        };
        let existing: std::collections::BTreeSet<String> =
            project.dependencies.iter().cloned().collect();
        for producer in producers {
            if producer != consumer && !existing.contains(producer) {
                project.dependencies.push(producer.clone());
                derived_added += 1;
            }
        }
        // Keep dependencies stable across runs.
        project.dependencies.sort();
        project.dependencies.dedup();
    }
    if derived_added > 0 {
        emit(
            quiet,
            format!(
                "→ codegraph: folded {derived_added} derived dependency edge(s) into projects.yaml"
            ),
        );
    }

    cfg.save(&pfile)?;

    // Persist the discovered edges (with provenance: reason/confidence) so the
    // dashboard can distinguish source-inferred edges from declared ones.
    // projects.yaml only keeps `dependencies: [names]`, which loses that.
    if let Ok(dir) = paths::maestro_dir() {
        let topo = dir.join("topology.json");
        if let Ok(json) = serde_json::to_string_pretty(&serde_json::json!({
            "edges": report.edges,
        })) {
            let _ = std::fs::write(&topo, json);
        }
    }

    emit(
        quiet,
        format!(
            "→ updated {} ({} added, {} replaced)",
            pfile.display(),
            added,
            updated
        ),
    );
    Ok(())
}

pub(crate) fn validate_generated_plan(plan_path: &std::path::Path) -> Result<()> {
    let plan = Plan::load(plan_path)?;
    let pfile = paths::projects_file()?;
    let projects = ProjectsConfig::load(&pfile)
        .with_context(|| format!("load projects for {}", plan_path.display()))?;
    plan.enforce_task_cap(projects.defaults.max_total_tasks)?;
    let report = config::analyze(&plan, &projects);
    if report.findings.is_empty() {
        println!("→ plan validate ok · {} task(s)", plan.tasks.len());
        print_plan_impact(&plan, &projects);
        print_plan_notice(&plan);
        return Ok(());
    }

    println!(
        "→ plan analysis: {} error(s), {} warning(s)",
        report.error_count(),
        report.warning_count()
    );
    for finding in &report.findings {
        match finding {
            config::Finding::Error { task, message, .. } => {
                println!("  ✗ {} {message}", task.as_deref().unwrap_or("(plan)"));
            }
            config::Finding::Warning { task, message, .. } => {
                println!("  ⚠ {} {message}", task.as_deref().unwrap_or("(plan)"));
            }
        }
    }
    if report.has_errors() {
        anyhow::bail!("generated plan has errors; fix them before running");
    }
    print_plan_impact(&plan, &projects);
    print_plan_notice(&plan);
    Ok(())
}

/// Pre-run impact preview: per task, the contract it touches and how many tasks
/// are downstream of it (blast radius), so the user can judge the change before
/// running. Only prints rows that carry a contract or downstream dependents.
pub(crate) fn print_plan_impact(plan: &Plan, projects: &ProjectsConfig) {
    let impacts = config::plan_impact(plan, projects);
    let interesting: Vec<&config::TaskImpact> = impacts
        .iter()
        .filter(|i| i.provides.is_some() || i.consumes.is_some() || !i.downstream.is_empty())
        .collect();
    if interesting.is_empty() {
        return;
    }
    println!("→ plan impact (contract + downstream blast radius):");
    for i in interesting {
        let mut tags = Vec::new();
        if let Some(p) = &i.provides {
            tags.push(format!("provides {p}"));
        }
        if let Some(c) = &i.consumes {
            tags.push(format!("consumes {c}"));
        }
        let contract = if tags.is_empty() {
            String::new()
        } else {
            format!("  ({})", tags.join(", "))
        };
        let downstream = if i.downstream.is_empty() {
            String::new()
        } else {
            format!(
                "  → {} downstream: {}",
                i.downstream.len(),
                i.downstream.join(", ")
            )
        };
        println!("  {} [{}]{contract}{downstream}", i.task, i.project);
    }
}

fn print_plan_notice(plan: &Plan) {
    if let Some(notice) = plan.notice.as_deref() {
        println!("→ notice: {notice}");
    }
}

async fn run_with_sigint_bridge(run_args: RunArgs, run_id: String) -> Result<()> {
    let mut run_future = Box::pin(crate::cli::commands::run::run_live(
        run_args,
        Some(run_id.clone()),
    ));

    let final_state = tokio::select! {
        result = &mut run_future => {
            let final_state = result?;
            crate::cli::commands::run::print_run_finished(&final_state);
            if let Some(exit_code) = crate::cli::commands::run::exit_code_for_final_state(&final_state) {
                std::process::exit(exit_code);
            }
            return Ok(());
        }
        signal = tokio::signal::ctrl_c() => {
            signal.context("listen for SIGINT")?;
            let marker = write_sigint_cancel_marker(&run_id)?;
            eprintln!(
                "→ SIGINT received; requested cancellation for run {run_id} via {}",
                marker.display()
            );
            tokio::select! {
                result = &mut run_future => result?,
                second = tokio::signal::ctrl_c() => {
                    let _ = second;
                    eprintln!("→ second SIGINT received; exiting immediately");
                    std::process::exit(SIGINT_EXIT_CODE);
                }
            }
        }
    };

    crate::cli::commands::run::print_run_finished(&final_state);
    std::process::exit(SIGINT_EXIT_CODE);
}

fn write_sigint_cancel_marker(run_id: &str) -> Result<std::path::PathBuf> {
    let cancels_dir = paths::cancels_dir()?;
    paths::ensure_dir(&cancels_dir)?;
    let marker = paths::control_marker_path(&cancels_dir, "run id", run_id)?;
    std::fs::write(&marker, b"sigint\n")
        .with_context(|| format!("write cancel marker {}", marker.display()))?;
    Ok(marker)
}

fn ensure_no_conflicting_work_run(spec: &str) -> Result<()> {
    let runs_dir = paths::runs_dir()?;
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Ok(());
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
        if state.status != crate::scheduler::RunStatus::Running || state.spec != spec {
            continue;
        }
        match crate::scheduler::classify_run(&state) {
            crate::scheduler::RunLiveness::Live => {
                anyhow::bail!(
                    "another maestro work --run is already active for this goal\n  run id: {} (pid {})\n  wait for it to finish, or use --force-new to start a parallel run anyway",
                    state.run_id,
                    state.pid
                );
            }
            crate::scheduler::RunLiveness::Abandoned => {
                anyhow::bail!(
                    "a prior maestro work run for this goal did not finish cleanly\n  run id: {} (pid {}, no longer running)\n  pick one:\n    maestro work --force-new {:?}\n    rm -rf {}\n  or run `maestro doctor` to inspect",
                    state.run_id,
                    state.pid,
                    spec,
                    run_dir.display()
                );
            }
            crate::scheduler::RunLiveness::UnknownLegacy => {
                eprintln!(
                    "→ warning: prior running run {} has no pid field; starting a new run",
                    state.run_id
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn sigint_cancel_marker_targets_run_id_not_global_current() {
        let temp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", temp.path());
        }
        paths::ensure_dir(&paths::cancels_dir().expect("cancels dir")).expect("create cancels");

        let marker = write_sigint_cancel_marker("run-abc123").expect("write marker");

        assert_eq!(
            marker,
            paths::cancels_dir().unwrap().join("run-abc123"),
            "SIGINT bridge should target the concrete work --run id"
        );
        assert!(marker.exists());
        assert!(
            !paths::cancels_dir().unwrap().join("current").exists(),
            "SIGINT bridge must not use the global current marker"
        );

        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[test]
    #[serial]
    fn prior_abandoned_same_goal_blocks_work_without_force_new() {
        let temp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", temp.path());
        }
        let run_dir = temp.path().join(".maestro/runs/dead-run");
        std::fs::create_dir_all(&run_dir).unwrap();
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

        let err = ensure_no_conflicting_work_run("same goal").expect_err("abandoned run blocks");

        let message = format!("{err:#}");
        assert!(message.contains("prior maestro work run"));
        assert!(message.contains("dead-run"));
        assert!(message.contains("--force-new"));
        assert!(message.contains("rm -rf"));

        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    mod project_merge {
        //! Pin the merge semantics: discovery refreshes the auto-derived
        //! fields (path / type / stack / commands / dependencies) but
        //! preserves the user's hand-customizations on every other field.
        //! Tests target the PRODUCTION helper directly so we can't drift
        //! into "test passes against a copy that doesn't match what
        //! actually runs" (the kind of subtle regression dali flagged
        //! when this file had a hand-copied `fn merge()` inline).
        use crate::config::{merge_project_from_discovery, Contracts, Project};

        fn existing_with_user_customizations() -> Project {
            Project {
                path: "old/path".into(),
                r#type: Some("backend".into()),
                stack: vec!["python".into()],
                commands: Default::default(),
                contracts: Contracts {
                    provides: Some("api/v1.yaml".into()),
                    consumes: None,
                },
                dependencies: vec!["other".into()],
                memory_scope: vec!["backend/decisions".into(), "backend/contracts".into()],
                agent: Some("claude".into()),
                agent_model: Some("claude-opus".into()),
                cursor_model: None,
                model_profile: Some("strong".into()),
                role: Some("backend_rust".into()),
                agent_profile: None,
                review_profile: None,
                copy_files: vec![".env.example".into()],
            }
        }

        fn fresh_from_discovery() -> Project {
            Project {
                path: "new/path".into(),
                r#type: Some("library".into()),
                stack: vec!["go".into(), "kitex".into()],
                commands: {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("check".into(), "bazel test //...".into());
                    m
                },
                contracts: Contracts {
                    provides: Some("idl/new.thrift".into()),
                    consumes: Some("upstream.proto".into()),
                },
                dependencies: vec!["shared-lib".into()],
                memory_scope: vec![], // discovery never derives this
                agent: Some("codex".into()),
                agent_model: None,
                cursor_model: None,
                model_profile: None,
                role: None,
                agent_profile: None,
                review_profile: None,
                copy_files: vec![],
            }
        }

        #[test]
        fn merge_refreshes_auto_fields() {
            let mut p = existing_with_user_customizations();
            merge_project_from_discovery(&mut p, fresh_from_discovery());
            assert_eq!(p.path, "new/path");
            assert_eq!(p.r#type.as_deref(), Some("library"));
            assert_eq!(p.stack, vec!["go", "kitex"]);
            assert!(!p.commands.is_empty());
            assert_eq!(p.dependencies, vec!["shared-lib"]);
        }

        #[test]
        fn merge_preserves_memory_scope_role_and_model_overrides() {
            let mut p = existing_with_user_customizations();
            merge_project_from_discovery(&mut p, fresh_from_discovery());
            // The customizations the user added must survive a re-init.
            assert_eq!(
                p.memory_scope,
                vec![
                    "backend/decisions".to_string(),
                    "backend/contracts".to_string()
                ],
            );
            assert_eq!(p.role.as_deref(), Some("backend_rust"));
            assert_eq!(p.agent.as_deref(), Some("claude"));
            assert_eq!(p.agent_model.as_deref(), Some("claude-opus"));
            assert_eq!(p.model_profile.as_deref(), Some("strong"));
            assert_eq!(p.copy_files, vec![".env.example"]);
        }

        #[test]
        fn merge_preserves_existing_contracts_when_user_set_them() {
            let mut p = existing_with_user_customizations();
            // Existing has contracts.provides = Some(...)
            merge_project_from_discovery(&mut p, fresh_from_discovery());
            assert_eq!(p.contracts.provides.as_deref(), Some("api/v1.yaml"));
            assert!(p.contracts.consumes.is_none());
        }

        #[test]
        fn merge_takes_fresh_contracts_when_existing_is_empty() {
            let mut p = existing_with_user_customizations();
            p.contracts = Contracts::default(); // unset both sides
            merge_project_from_discovery(&mut p, fresh_from_discovery());
            assert_eq!(p.contracts.provides.as_deref(), Some("idl/new.thrift"));
            assert_eq!(p.contracts.consumes.as_deref(), Some("upstream.proto"));
        }
    }
}
