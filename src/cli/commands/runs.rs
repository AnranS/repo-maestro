use anyhow::Result;

use crate::cli::{RunsArgs, RunsSubcmd};
use crate::paths;

pub async fn run(a: RunsArgs) -> Result<()> {
    match a.subcmd {
        None | Some(RunsSubcmd::Ls) => cmd_runs_ls(),
        Some(RunsSubcmd::Show { run_id }) => cmd_run_show(run_id.as_deref()),
        Some(RunsSubcmd::Events {
            run_id,
            tail,
            follow,
            json,
        }) => cmd_run_events(run_id.as_deref(), tail, follow, json).await,
        Some(RunsSubcmd::Evidence { run_id, json }) => cmd_run_evidence(run_id.as_deref(), json),
        Some(RunsSubcmd::Replay { run_id, json }) => cmd_run_replay(run_id.as_deref(), json),
        Some(RunsSubcmd::PrBody { run_id, write }) => cmd_run_pr_body(run_id.as_deref(), write),
    }
}

fn cmd_runs_ls() -> Result<()> {
    let dir = paths::runs_dir()?;
    if !dir.exists() {
        println!("(no runs yet)");
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| e.file_name().to_string_lossy() != paths::CURRENT_LINK)
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for e in entries {
        let p = e.path();
        let state_file = p.join(paths::RUN_STATE_FILE);
        if !state_file.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&state_file)?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        let spec = v.get("spec").and_then(|s| s.as_str()).unwrap_or("");
        println!(
            "{}  {:<10}  {}",
            e.file_name().to_string_lossy(),
            status,
            crate::cli::util::spec_title(spec, 80)
        );
    }
    Ok(())
}

fn cmd_run_show(run_id: Option<&str>) -> Result<()> {
    let (id, dir) = resolve_run_dir_arg(run_id)?;
    let state = crate::scheduler::RunState::load(&dir)?;
    let events = crate::scheduler::events::read_events(&dir).unwrap_or_default();

    println!("run      {id}");
    println!("spec     {}", state.spec);
    println!("status   {:?}", state.status);
    println!("started  {}", state.started_at.format("%Y-%m-%d %H:%M:%S"));
    if let Some(ended) = state.ended_at {
        println!("ended    {}", ended.format("%Y-%m-%d %H:%M:%S"));
    }
    println!("tasks    {}", state.tasks.len());
    println!("events   {}", events.len());
    if let Some((passed, total)) = state.acceptance_summary() {
        println!("accept   {passed}/{total} passed");
        println!("verified {}", state.verified);
    }

    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for task in state.tasks.values() {
        *counts.entry(format!("{:?}", task.status)).or_default() += 1;
    }
    if !counts.is_empty() {
        println!();
        for (status, count) in counts {
            println!("{status:<18} {count}");
        }
    }
    Ok(())
}

fn cmd_run_evidence(run_id: Option<&str>, json: bool) -> Result<()> {
    let (_, dir) = resolve_run_dir_arg(run_id)?;
    let state = crate::scheduler::RunState::load(&dir)?;
    let evidence = crate::scheduler::evidence::build_run_evidence(&state);
    if json {
        println!("{}", serde_json::to_string_pretty(&evidence)?);
        return Ok(());
    }

    println!("run          {}", evidence.run_id);
    println!("status       {}", evidence.status);
    println!("verified     {}", evidence.verified);
    println!("max parallel {}", evidence.max_parallel);
    println!("observed     {}", evidence.max_observed_parallelism);
    println!("tasks        {}", evidence.task_count);
    println!("overlaps     {}", evidence.parallel_windows.len());
    if !evidence.acceptance.is_empty() {
        let passed = evidence.acceptance.iter().filter(|a| a.passed).count();
        println!("acceptance   {passed}/{} passed", evidence.acceptance.len());
    }

    if !evidence.parallel_windows.is_empty() {
        println!();
        println!("Parallel windows");
        for window in &evidence.parallel_windows {
            println!(
                "- {}..{}  x{}  tasks={}  projects={}",
                window.started_at.format("%H:%M:%S%.3f"),
                window.ended_at.format("%H:%M:%S%.3f"),
                window.concurrency,
                window.task_ids.join(","),
                window.projects.join(",")
            );
        }
    }

    println!();
    println!("Task workspaces");
    for task in &evidence.tasks {
        let workspace = task.workspace_path.as_deref().unwrap_or("(unknown)");
        let isolation = task
            .worktree_path
            .as_deref()
            .map(|path| format!("worktree={path}"))
            .unwrap_or_else(|| "worktree=(none)".to_string());
        println!(
            "- {:<24} {:<10} {:<8} {}  {}",
            task.id, task.project, task.status, workspace, isolation
        );
    }

    Ok(())
}

fn cmd_run_replay(run_id: Option<&str>, json: bool) -> Result<()> {
    let (_, dir) = resolve_run_dir_arg(run_id)?;
    let replay = crate::scheduler::evidence::build_run_replay(&dir)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&replay)?);
        return Ok(());
    }

    println!("run       {}", replay.run_id);
    println!("status    {}", replay.status);
    println!("events    {}", replay.event_count);
    println!("observed  {}", replay.max_observed_parallelism);
    println!();
    println!("Timeline");
    for event in &replay.events {
        println!(
            "{:>5}  {}  {:<24} {:<24} {}",
            event.seq,
            event.timestamp.format("%H:%M:%S"),
            event.kind,
            event.task_id.as_deref().unwrap_or("-"),
            event.message.as_deref().unwrap_or("")
        );
    }
    if !replay.tasks.is_empty() {
        println!();
        println!("Tasks");
        for task in &replay.tasks {
            let dur = task
                .duration_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "- {:<24} {:<10} {:<10} {}",
                task.id, task.project, task.status, dur
            );
        }
    }
    Ok(())
}

fn cmd_run_pr_body(run_id: Option<&str>, write: bool) -> Result<()> {
    let (_, dir) = resolve_run_dir_arg(run_id)?;
    let state = crate::scheduler::RunState::load(&dir)?;
    let body = crate::scheduler::evidence::render_pr_body(&state);
    if write {
        let path = crate::scheduler::evidence::write_pr_body(&state)?;
        println!("wrote {}", path.display());
    } else {
        print!("{body}");
    }
    Ok(())
}

async fn cmd_run_events(run_id: Option<&str>, tail: usize, follow: bool, json: bool) -> Result<()> {
    let (_, dir) = resolve_run_dir_arg(run_id)?;
    let events = crate::scheduler::events::read_events(&dir)?;
    if events.is_empty() && !follow {
        println!("(no events yet)");
        return Ok(());
    }
    let start = events.len().saturating_sub(tail);
    let mut last_seq = 0_u64;
    for event in &events[start..] {
        print_run_event(event, json)?;
        last_seq = last_seq.max(event.seq);
    }
    if !follow {
        return Ok(());
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let events = crate::scheduler::events::read_events(&dir)?;
        for event in &events {
            if event.seq <= last_seq {
                continue;
            }
            print_run_event(event, json)?;
            last_seq = last_seq.max(event.seq);
        }
    }
}

fn print_run_event(event: &crate::scheduler::RunEvent, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(event)?);
        return Ok(());
    }
    let task = event.task_id.as_deref().unwrap_or("-");
    let msg = event.message.as_deref().unwrap_or("");
    println!(
        "{:>5}  {}  {:<24} {:<28} {}",
        event.seq,
        event.timestamp.format("%m-%d %H:%M:%S"),
        event.kind,
        task,
        msg
    );
    Ok(())
}

pub(crate) fn resolve_run_dir_arg(run_id: Option<&str>) -> Result<(String, std::path::PathBuf)> {
    match run_id {
        None | Some("current") => {
            let dir = paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run"))?;
            let id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("current")
                .to_string();
            Ok((id, dir))
        }
        Some(id) => {
            let dir = paths::run_dir_for_id(id)?;
            if !dir.exists() {
                anyhow::bail!("run not found: {id}");
            }
            Ok((id.to_string(), dir))
        }
    }
}
