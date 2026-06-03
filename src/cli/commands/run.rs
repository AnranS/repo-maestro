//! `maestro run` and `maestro rerun`.

use anyhow::Result;
use std::collections::BTreeSet;

use crate::cli::util::warn_unknown_model;
use crate::cli::{RerunArgs, ResumeArgs, RunArgs};
use crate::config::{Plan, ProjectsConfig};
use crate::paths;
use crate::scheduler::{dry_run, run_plan, ExecConfig, RunState, TaskStatus};

pub async fn run(a: RunArgs) -> Result<()> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` first");
    }
    let projects = ProjectsConfig::load(&pfile)?;
    let plan = Plan::load(&a.plan)?;

    if let Some(m) = a.model.as_deref() {
        warn_unknown_model(m);
    }
    warn_plan_models(&plan);

    // Surface a runaway plan in dry mode too, so the cap is discovered
    // before the user drops --dry and tries to actually run it.
    plan.enforce_task_cap(projects.defaults.max_total_tasks)?;

    // `--dry`: render every prompt and dump to disk, no agent calls.
    // We still run plan-analyze so the user sees contract warnings before
    // they commit to the live run.
    if a.dry {
        let (plan, _wired) = maybe_wire_contracts(plan, &projects, a.no_wire_contracts);
        let report = crate::config::analyze(&plan, &projects);
        if report.has_errors() {
            for f in &report.findings {
                if let crate::config::Finding::Error { task, message, .. } = f {
                    eprintln!("  ✗ {} {}", task.as_deref().unwrap_or("(plan)"), message);
                }
            }
            anyhow::bail!("plan validation failed; fix the errors above");
        }
        if !a.quiet_impact {
            crate::cli::commands::work::print_plan_impact(&plan, &projects);
        }
        let summary = dry_run(&plan, &projects)?;
        println!(
            "→ dry-run {} ({} task(s))",
            summary.run_id, summary.task_count
        );
        for (id, path) in &summary.files {
            println!("  • {id:<28}  {}", path.display());
        }
        println!(
            "\n  edit / inspect the prompts, then drop --dry to actually run.\n  dir: {}",
            summary.run_dir.display()
        );
        return Ok(());
    }

    let auto_pr = projects.defaults.auto_pr;
    let final_state = run_live_with_inputs(a, plan, projects, None).await?;
    print_run_finished(&final_state);
    if auto_pr && final_state.verified {
        maybe_auto_pr().await;
    }
    if let Some(exit_code) = exit_code_for_final_state(&final_state) {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Close the PR loop on a verified run: push the integration branch and open a
/// draft PR (`maestro pr --push` for the current run). Best-effort — a failure
/// (no remote, no `gh`) warns rather than failing the finished run.
async fn maybe_auto_pr() {
    println!("→ auto_pr: opening a draft PR for the verified run…");
    let args = crate::cli::PrDraftArgs {
        run: None,
        project: None,
        repo: None,
        base: None,
        push: true,
    };
    if let Err(e) = crate::cli::commands::pr::draft(args).await {
        eprintln!("  ⚠ auto_pr failed (run is still complete): {e:#}");
    }
}

pub(crate) async fn run_live(a: RunArgs, run_id: Option<String>) -> Result<RunState> {
    if a.dry {
        anyhow::bail!("run_live does not support --dry");
    }
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` first");
    }
    let projects = ProjectsConfig::load(&pfile)?;
    let plan = Plan::load(&a.plan)?;

    // Plan-drift sanity check: every task.project must be either
    // `_global` or a key in projects.yaml. A typo'd / removed project
    // here used to slip through and surface as a runtime workspace-
    // resolution error mid-DAG; catching it pre-flight saves a
    // half-finished run state file.
    check_plan_project_drift(&plan, &projects)?;

    if let Some(m) = a.model.as_deref() {
        warn_unknown_model(m);
    }
    warn_plan_models(&plan);

    run_live_with_inputs(a, plan, projects, run_id).await
}

/// Returns Err with the offending task ids if any task's `project`
/// references something not registered in `projects.yaml`. `_global` is
/// always valid (verify / shell tasks don't need a project).
fn check_plan_project_drift(plan: &crate::config::Plan, projects: &ProjectsConfig) -> Result<()> {
    let mut bad: Vec<(String, String)> = Vec::new();
    for t in &plan.tasks {
        if t.project.is_empty() || t.project == "_global" {
            continue;
        }
        if !projects.projects.contains_key(&t.project) {
            bad.push((t.id.clone(), t.project.clone()));
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    let mut msg =
        String::from("plan references project(s) not in projects.yaml — refusing to run.\n");
    for (task_id, project) in &bad {
        msg.push_str(&format!(
            "  · task `{task_id}` → project `{project}` (missing)\n"
        ));
    }
    msg.push_str(
        "Fix the plan or run `maestro init` / `maestro add <path>` to register the project.",
    );
    anyhow::bail!("{msg}")
}

/// Auto-wire contract producer→consumer edges (unless `--no-wire-contracts`)
/// and tell the user exactly what was added. Returns the (possibly mutated)
/// plan together with the edges added (recorded into the run's audit ledger).
fn maybe_wire_contracts(
    mut plan: Plan,
    projects: &ProjectsConfig,
    disabled: bool,
) -> (Plan, Vec<crate::config::WiredEdge>) {
    if disabled {
        return (plan, vec![]);
    }
    let wired = crate::config::wire_contract_dependencies(&mut plan, projects);
    if !wired.is_empty() {
        eprintln!(
            "→ wired {} contract dependency edge(s) so consumers don't race ahead of producers \
             (use --no-wire-contracts to disable):",
            wired.len()
        );
        for e in &wired {
            eprintln!(
                "  + {} depends_on {}  (contract `{}`)",
                e.consumer, e.producer, e.contract
            );
        }
    }
    (plan, wired)
}

async fn run_live_with_inputs(
    a: RunArgs,
    plan: Plan,
    projects: ProjectsConfig,
    run_id: Option<String>,
) -> Result<RunState> {
    // Safety rail: refuse a runaway plan before we spin up any agents.
    // Fresh `run` and `work` flow through here; `rerun`/`resume` recover an
    // already-admitted plan and deliberately skip this so a lowered cap
    // can't block recovery of a legitimately large historical run.
    plan.enforce_task_cap(projects.defaults.max_total_tasks)?;

    let (plan, wired_contract_edges) = maybe_wire_contracts(plan, &projects, a.no_wire_contracts);
    let cfg = ExecConfig {
        max_parallel: a
            .max_parallel
            .unwrap_or(projects.defaults.max_parallel.max(1)),
        continue_on_error: a.continue_on_error,
        only: if a.only.is_empty() {
            None
        } else {
            Some(a.only)
        },
        skip: a.skip,
        session_id: a
            .session_id
            .clone()
            .or_else(|| std::env::var("MAESTRO_SESSION_ID").ok()),
        model_override: a.model.clone(),
        run_id,
        wired_contract_edges,
        max_tokens: a.max_tokens,
        plan_gate: a.gates.flags().0,
        outcome_gate: a.gates.flags().1,
        ..Default::default()
    };

    // Heads-up about contracts that might race
    let report = crate::config::analyze(&plan, &projects);
    if report.warning_count() > 0 || report.has_errors() {
        eprintln!(
            "→ plan analysis: {} error(s), {} warning(s)",
            report.error_count(),
            report.warning_count()
        );
        for f in &report.findings {
            match f {
                crate::config::Finding::Error { task, message, .. } => {
                    eprintln!("  ✗ {} {}", task.as_deref().unwrap_or("(plan)"), message);
                }
                crate::config::Finding::Warning { task, message, .. } => {
                    eprintln!("  ⚠ {} {}", task.as_deref().unwrap_or("(plan)"), message);
                }
            }
        }
        if report.has_errors() {
            anyhow::bail!("plan validation failed; fix the errors above");
        }
    }

    println!("→ starting run with max_parallel={}", cfg.max_parallel);
    run_plan(plan, projects, cfg).await
}

pub(crate) fn print_run_finished(final_state: &RunState) {
    let n_done = final_state
        .tasks
        .values()
        .filter(|t| t.status == crate::scheduler::TaskStatus::Done)
        .count();
    let n_failed = final_state
        .tasks
        .values()
        .filter(|t| t.status == crate::scheduler::TaskStatus::Failed)
        .count();
    let total = final_state.tasks.len();
    println!(
        "→ run {} finished: {}/{} done{}",
        final_state.run_id,
        n_done,
        total,
        if n_failed > 0 {
            format!(", {n_failed} failed")
        } else {
            String::new()
        }
    );

    // Acceptance verdict — the thing the user actually wants to know.
    if let Some((p, n)) = final_state.acceptance_summary() {
        let verdict = if final_state.verified {
            "✅ verified"
        } else if p == n {
            "✅ checks passed (DAG had failures)"
        } else {
            "❌ acceptance failed"
        };
        println!("  acceptance: {p}/{n} — {verdict}");
    }

    // What maestro did automatically (round-5 ledger), surfaced at the moment
    // the user is looking — not buried in REPORT.md.
    if let Some(line) = auto_actions_line(&final_state.auto_actions) {
        println!("  {line}");
    }

    // Where to look next.
    let rd = final_state.run_dir.display();
    println!("next:");
    println!(
        "  maestro runs show {}   # task transcript",
        final_state.run_id
    );
    println!("  {rd}/REPORT.md");
    if n_failed == 0 {
        println!("  {rd}/PR_BODY.md           # draft PR body");
    }
    println!("  maestro open --no-browser  # inspect in the dashboard");
}

/// One-line summary of the automatic decisions maestro made during a run,
/// grouped by kind. `None` when it made none. Pure for testing.
fn auto_actions_line(actions: &[crate::scheduler::AutoAction]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for a in actions {
        *counts.entry(a.kind.as_str()).or_insert(0) += 1;
    }
    let label = |kind: &str, n: usize| -> String {
        let noun = match kind {
            "contract_wired" => "contract edge(s) wired",
            "retry" => "retry(ies)",
            "circuit_break" => "circuit-breaker stop(s)",
            "integration_conflict" => "integration conflict(s)",
            other => other,
        };
        format!("{n} {noun}")
    };
    let parts: Vec<String> = counts.iter().map(|(k, n)| label(k, *n)).collect();
    Some(format!(
        "maestro handled {} automatic action(s): {}",
        actions.len(),
        parts.join(", ")
    ))
}

pub(crate) fn exit_code_for_final_state(final_state: &RunState) -> Option<i32> {
    let n_failed = final_state
        .tasks
        .values()
        .filter(|t| t.status == crate::scheduler::TaskStatus::Failed)
        .count();
    (n_failed > 0 || !final_state.verified_or_no_goal()).then_some(2)
}

fn warn_plan_models(plan: &Plan) {
    for t in &plan.tasks {
        if let Some(m) = t.model.as_deref() {
            warn_unknown_model(m);
        }
    }
}

pub async fn rerun(a: RerunArgs) -> Result<()> {
    let pfile = paths::projects_file()?;
    let projects = ProjectsConfig::load(&pfile)?;
    let plan = Plan::load(&a.plan)?;

    let only = if a.only.is_empty() {
        None
    } else {
        Some(a.only.clone())
    };
    let seed_skipped_from = paths::current_run_dir()?
        .map(|dir| RunState::load(&dir))
        .transpose()?;
    let skip = compute_rerun_skip(
        &plan,
        a.skip.clone(),
        a.from.as_deref(),
        only.as_deref(),
        seed_skipped_from.as_ref(),
    )?;
    let effective_skip_count = plan
        .tasks
        .iter()
        .filter(|task| {
            skip.contains(&task.id)
                || only
                    .as_ref()
                    .map(|selected| !selected.contains(&task.id))
                    .unwrap_or(false)
        })
        .count();

    let cfg = ExecConfig {
        max_parallel: a
            .max_parallel
            .unwrap_or(projects.defaults.max_parallel.max(1)),
        continue_on_error: false,
        only,
        skip,
        session_id: std::env::var("MAESTRO_SESSION_ID").ok(),
        model_override: None,
        seed_skipped_from,
        ..Default::default()
    };

    println!(
        "→ rerunning (max_parallel={}, skipping {} task(s))",
        cfg.max_parallel, effective_skip_count
    );

    let final_state = run_plan(plan, projects, cfg).await?;
    let n_failed = final_state
        .tasks
        .values()
        .filter(|t| t.status == crate::scheduler::TaskStatus::Failed)
        .count();
    if n_failed > 0 || !final_state.verified_or_no_goal() {
        std::process::exit(2);
    }
    Ok(())
}

/// Continue an interrupted run without redoing completed work: load the run's
/// plan snapshot + state, skip the tasks already done (their integrated outputs
/// are seeded), and run the rest. Refuses a run that still looks alive unless
/// `--force`.
pub async fn resume(a: ResumeArgs) -> Result<()> {
    use crate::scheduler::liveness::{classify_run, RunLiveness};

    let dir = match a.run.as_deref() {
        None | Some("current") => {
            paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run to resume"))?
        }
        Some(id) => {
            let d = paths::run_dir_for_id(id)?;
            if !d.exists() {
                anyhow::bail!("run not found: {id}");
            }
            d
        }
    };
    let state = RunState::load(&dir)?;

    if matches!(classify_run(&state), RunLiveness::Live) && !a.force {
        anyhow::bail!(
            "run {} still appears alive (pid {}); use --force to resume anyway",
            state.run_id,
            state.pid
        );
    }

    let total = state.tasks.len();
    let done: Vec<String> = state
        .tasks
        .iter()
        .filter(|(_, t)| t.status == TaskStatus::Done)
        .map(|(id, _)| id.clone())
        .collect();
    if done.len() == total {
        println!(
            "→ run {} already complete ({total}/{total} done); nothing to resume",
            state.run_id
        );
        return Ok(());
    }

    let pfile = paths::projects_file()?;
    let projects = ProjectsConfig::load(&pfile)?;
    let plan = Plan::load(&dir.join(paths::PLAN_SNAPSHOT))?;

    println!(
        "→ resuming run {} — reusing {} completed task(s), continuing {} remaining",
        state.run_id,
        done.len(),
        total - done.len()
    );

    let cfg = ExecConfig {
        max_parallel: a
            .max_parallel
            .unwrap_or(projects.defaults.max_parallel.max(1)),
        skip: done,
        seed_skipped_from: Some(state),
        session_id: std::env::var("MAESTRO_SESSION_ID").ok(),
        ..Default::default()
    };
    let final_state = run_plan(plan, projects, cfg).await?;
    print_run_finished(&final_state);
    if let Some(code) = exit_code_for_final_state(&final_state) {
        std::process::exit(code);
    }
    Ok(())
}

fn compute_rerun_skip(
    plan: &Plan,
    mut skip: Vec<String>,
    from: Option<&str>,
    only: Option<&[String]>,
    seed: Option<&RunState>,
) -> Result<Vec<String>> {
    let Some(from_id) = from else {
        return Ok(skip);
    };
    if !plan.tasks.iter().any(|task| task.id == from_id) {
        anyhow::bail!("--from task {from_id:?} is not in PLAN");
    }

    let graph = crate::scheduler::dag::TaskGraph::from_plan(plan)?;

    // Preserve the historical behavior: strict upstream dependencies of
    // `--from` are treated as already done, even when no previous run state is
    // available to seed their outputs.
    let mut upstream = BTreeSet::new();
    let mut stack = vec![from_id.to_string()];
    while let Some(id) = stack.pop() {
        for up in graph.upstream(&id) {
            if upstream.insert(up.clone()) {
                stack.push(up);
            }
        }
    }
    extend_unique(&mut skip, upstream);

    let Some(seed) = seed else {
        return Ok(skip);
    };

    let only_set: Option<BTreeSet<&str>> =
        only.map(|items| items.iter().map(String::as_str).collect());
    let mut rerun_scope = BTreeSet::from([from_id.to_string()]);
    let mut stack = vec![from_id.to_string()];
    while let Some(id) = stack.pop() {
        for down in graph.downstream(&id) {
            if rerun_scope.insert(down.clone()) {
                stack.push(down);
            }
        }
    }

    let completed_outside_scope = plan.tasks.iter().filter_map(|task| {
        if rerun_scope.contains(&task.id) {
            return None;
        }
        if only_set
            .as_ref()
            .map(|selected| selected.contains(task.id.as_str()))
            .unwrap_or(false)
        {
            return None;
        }
        let previous = seed.tasks.get(&task.id)?;
        (previous.status == TaskStatus::Done).then(|| task.id.clone())
    });
    extend_unique(&mut skip, completed_outside_scope);

    Ok(skip)
}

fn extend_unique(skip: &mut Vec<String>, ids: impl IntoIterator<Item = String>) {
    for id in ids {
        if !skip.contains(&id) {
            skip.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn auto_actions_line_groups_and_counts() {
        use crate::scheduler::AutoAction;
        let mk = |kind: &str| AutoAction {
            kind: kind.into(),
            task: None,
            detail: String::new(),
        };
        assert!(auto_actions_line(&[]).is_none());
        let line =
            auto_actions_line(&[mk("contract_wired"), mk("contract_wired"), mk("retry")]).unwrap();
        assert!(line.contains("3 automatic action(s)"));
        assert!(line.contains("2 contract edge(s) wired"));
        assert!(line.contains("1 retry(ies)"));
    }

    fn parse_plan(yaml: &str) -> Plan {
        let mut plan: Plan = serde_yaml::from_str(yaml).expect("parse plan");
        plan = plan.expand_for_each();
        plan.validate().expect("plan validates");
        plan
    }

    fn projects() -> ProjectsConfig {
        ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        }
    }

    fn seed_with_done(plan: &Plan, done: &[&str]) -> RunState {
        let mut state = RunState::new(
            "seed".into(),
            plan,
            &projects(),
            2,
            PathBuf::from("/tmp/maestro-seed"),
        );
        for id in done {
            state.tasks.get_mut(*id).unwrap().status = TaskStatus::Done;
        }
        state
    }

    #[test]
    fn rerun_from_skips_done_independent_tasks_outside_rerun_scope() {
        let plan = parse_plan(
            r#"
spec: rerun skip done independent
tasks:
  - id: A
    project: a
    kind: verify
    agent: shell
    command: "echo a"
  - id: B
    project: b
    kind: verify
    agent: shell
    command: "echo b"
  - id: G
    project: _global
    kind: verify
    agent: shell
    depends_on: [A, B]
    command: "echo g"
"#,
        );
        let seed = seed_with_done(&plan, &["B"]);

        let skip = compute_rerun_skip(&plan, vec![], Some("A"), None, Some(&seed)).unwrap();

        assert_eq!(skip, vec!["B"]);
    }

    #[test]
    fn rerun_from_keeps_selected_only_tasks_even_when_done() {
        let plan = parse_plan(
            r#"
spec: rerun only wins
tasks:
  - id: A
    project: a
    kind: verify
    agent: shell
    command: "echo a"
  - id: B
    project: b
    kind: verify
    agent: shell
    command: "echo b"
"#,
        );
        let seed = seed_with_done(&plan, &["B"]);
        let only = vec!["B".to_string()];

        let skip = compute_rerun_skip(&plan, vec![], Some("A"), Some(&only), Some(&seed)).unwrap();

        assert!(skip.is_empty());
    }

    #[test]
    fn rerun_from_rejects_unknown_task() {
        let plan = parse_plan(
            r#"
spec: rerun unknown from
tasks:
  - id: A
    project: a
    kind: verify
    agent: shell
    command: "echo a"
"#,
        );

        let err = compute_rerun_skip(&plan, vec![], Some("missing"), None, None)
            .expect_err("unknown --from should fail");

        assert!(err.to_string().contains("--from task"));
    }

    mod plan_drift {
        //! Lock the pre-flight that catches a plan referencing a
        //! project not in projects.yaml — the classic footgun is a
        //! hand-edited plan + a forgotten `mst add` step, which used
        //! to half-execute before failing mid-DAG.
        use super::super::check_plan_project_drift;
        use crate::config::{Plan, ProjectsConfig};

        fn projects(names: &[&str]) -> ProjectsConfig {
            let yaml = format!(
                "version: 1\ndefaults:\n  agent: cursor\nprojects:\n{}",
                names
                    .iter()
                    .map(|n| format!("  {n}:\n    path: .\n"))
                    .collect::<String>()
            );
            serde_yaml::from_str(&yaml).unwrap()
        }

        fn plan_with(tasks: &[(&str, &str)]) -> Plan {
            // YAML fixture is more robust than struct literals to Plan /
            // PlanTask schema growth — each new optional field defaults
            // via serde rather than requiring every call-site to update.
            let mut yaml = String::from("spec: drift-test\ntasks:\n");
            for (id, project) in tasks {
                yaml.push_str(&format!(
                    "  - id: {id}\n    project: {project}\n    kind: agent\n",
                ));
            }
            serde_yaml::from_str(&yaml).expect("test plan fixture must parse")
        }

        #[test]
        fn ok_when_every_task_project_is_registered() {
            let p = projects(&["alpha", "beta"]);
            let plan = plan_with(&[("T1", "alpha"), ("T2", "beta")]);
            assert!(check_plan_project_drift(&plan, &p).is_ok());
        }

        #[test]
        fn global_is_always_valid() {
            let p = projects(&["alpha"]);
            let plan = plan_with(&[("T_verify", "_global")]);
            assert!(check_plan_project_drift(&plan, &p).is_ok());
        }

        #[test]
        fn errors_when_a_task_references_an_unknown_project() {
            let p = projects(&["alpha"]);
            let plan = plan_with(&[("T1", "alpha"), ("T2", "ghost")]);
            let err = check_plan_project_drift(&plan, &p).expect_err("missing project must Err");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("ghost"),
                "error message must name the bad project"
            );
            assert!(
                msg.contains("T2"),
                "error message must name the offending task"
            );
        }

        #[test]
        fn lists_every_drift_not_just_the_first() {
            let p = projects(&["alpha"]);
            let plan = plan_with(&[("T1", "ghost-1"), ("T2", "ghost-2")]);
            let err = check_plan_project_drift(&plan, &p).expect_err("multi-drift Err");
            let msg = format!("{err:#}");
            assert!(msg.contains("ghost-1"));
            assert!(msg.contains("ghost-2"));
        }
    }
}
