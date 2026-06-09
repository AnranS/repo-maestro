//! Built-in command implementations split out of `cli/mod.rs`, which keeps the
//! clap surface (the `Cmd` enum + `*Args` structs + dispatch). These are the
//! command bodies and their private helpers; the dispatch in `cli::run` calls
//! them via a glob import. Pure move — no behavior change.

use anyhow::{Context, Result};
use std::io::{IsTerminal, Write as _};

use crate::cli::*;
use crate::config::{Project, ProjectsConfig};
use crate::paths;

pub(crate) fn cmd_scaffold(a: ScaffoldArgs) -> Result<()> {
    let arch = crate::architecture::Architecture::load(&a.architecture)?;
    // Propagate a workspace-root resolution failure instead of scaffolding
    // (git-init + dir creation) into an empty/cwd-relative root.
    let root = match a.root {
        Some(r) => r,
        None => paths::workspace_root()?,
    };
    let opts = crate::architecture::scaffold::ScaffoldOptions {
        root: root.clone(),
        no_git: a.no_git,
        no_register: a.no_register,
    };

    println!("→ scaffolding into {:?}", root);
    println!("  spec: {}", arch.spec);
    println!();

    let report = crate::architecture::scaffold::scaffold(&arch, &opts)?;

    for m in &report.modules {
        let marker = if m.pre_existing { "·" } else { "+" };
        println!(
            "  {marker} {:<22} {} files{}",
            m.name,
            m.created_files.len(),
            if m.git_initialized {
                " · git init"
            } else {
                ""
            }
        );
        if !m.created_files.is_empty() {
            for f in &m.created_files {
                println!("      {f}");
            }
        }
    }
    println!();
    if report.projects_yaml_path.is_empty() {
        println!("→ skipped projects.yaml (--no-register)");
    } else {
        println!("→ updated {}", report.projects_yaml_path);
    }
    println!(
        "\nnext: open `{}` in your editor / run `maestro ls` to confirm",
        report.root
    );
    Ok(())
}

pub(crate) async fn cmd_setup(a: SetupArgs) -> Result<()> {
    println!("→ workspace");
    cmd_init(InitArgs {
        bare: a.bare,
        analyze: false,
        no_analyze: true,
        agent: "codex".into(),
        root: None,
        max_depth: 3,
        out: None,
        projects: vec![],
    })?;

    println!("\n→ bundled skills");
    let skill_results = samples::update_bundled_skills(None, a.force_skills)?;
    commands::skills::print_update_results(&skill_results);
    if !a.force_skills {
        let skipped = skill_results
            .iter()
            .filter(|r| r.status == samples::SkillUpdateStatus::Skipped)
            .count();
        if skipped > 0 {
            println!(
                "  hint: {skipped} local edit(s) were preserved; pass --force-skills to overwrite."
            );
        }
    }

    println!("\n→ providers");
    let providers = crate::providers::provider_statuses(false);
    let ready_adapters = providers
        .iter()
        .filter(|p| p.adapter_available && p.installed)
        .count();
    let missing_adapters: Vec<_> = providers
        .iter()
        .filter(|p| p.adapter_available && !p.installed)
        .map(|p| p.id)
        .collect();
    println!("  {ready_adapters} runnable adapter(s) detected");
    if missing_adapters.is_empty() {
        println!("  all runnable adapters are available");
    } else {
        println!(
            "  missing runnable adapter binaries: {}",
            missing_adapters.join(", ")
        );
        for provider in providers
            .iter()
            .filter(|p| p.adapter_available && !p.installed)
        {
            if let Some(hint) = crate::providers::provider_install_hint(
                provider.id,
                provider.execution.binary.as_deref(),
                provider.execution.env_override,
            ) {
                println!("  hint for {}: {hint}", provider.id);
            }
        }
    }
    for provider in &providers {
        println!(
            "  {:<12} {:<9} {:<10} {}",
            provider.id,
            commands::providers::provider_kind_label(provider.kind),
            if provider.installed {
                "installed"
            } else {
                "missing"
            },
            provider.notes
        );
    }
    println!("  details: maestro providers");

    if a.refresh_models {
        println!("\n→ models");
        if let Err(e) = commands::models::run(ModelsArgs { refresh: true }).await {
            eprintln!("  model refresh/list failed: {e:#}");
        }
    } else {
        println!("\n→ models");
        println!("  skipped live refresh; pass --refresh-models to query cursor-agent now");
    }

    let mut doctor_had_issues = false;
    if !a.skip_doctor {
        println!("\n→ doctor");
        let doctor = commands::doctor::run(DoctorArgs {
            command: None,
            json: false,
            verbose: false,
        })
        .await;
        if let Err(e) = doctor {
            doctor_had_issues = true;
            eprintln!("  doctor reported issues: {e:#}");
            if a.strict {
                return Err(e);
            }
        }
    }

    println!("\n{}", setup_completion_text(doctor_had_issues));
    println!("{}", setup_next_steps());
    Ok(())
}

pub(crate) fn setup_completion_text(doctor_had_issues: bool) -> &'static str {
    if doctor_had_issues {
        "✓ Setup complete with doctor issues."
    } else {
        "✓ Setup complete."
    }
}

pub(crate) fn setup_next_steps() -> &'static str {
    "→ Next: maestro demo --run      # execute a zero-config 2-task DAG\n→ Dry:  maestro demo            # inspect the generated prompts first\n→ Or:  maestro work \"<goal>\" --root /path/to/your/projects\n→ Example: maestro work \"update README\" --root examples --agent mock --dry"
}

pub(crate) async fn cmd_history(a: HistoryArgs) -> Result<()> {
    let listing = crate::external::list_sessions(a.source.as_deref()).await?;
    if listing.sessions.is_empty() {
        println!("(no external sessions detected)");
        return Ok(());
    }
    for sum in &listing.summaries {
        println!(
            "[{}]  {} session{} across {} workspace{}",
            sum.source,
            sum.session_count,
            if sum.session_count == 1 { "" } else { "s" },
            sum.workspace_count,
            if sum.workspace_count == 1 { "" } else { "s" },
        );
    }
    println!();
    println!(
        "{:<8} {:<24} {:<14} {:<10}  modified",
        "SOURCE", "WORKSPACE", "ID", "BYTES"
    );
    for s in listing.sessions.iter().take(a.limit) {
        let modified = s
            .modified_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".into());
        let id = s.id.chars().take(14).collect::<String>();
        let ws = s.workspace_id.chars().take(24).collect::<String>();
        println!(
            "{:<8} {:<24} {:<14} {:>10}  {}",
            s.source, ws, id, s.bytes, modified
        );
    }
    if listing.sessions.len() > a.limit {
        println!(
            "\n(+ {} more — use --limit N)",
            listing.sessions.len() - a.limit
        );
    }
    Ok(())
}

pub(crate) fn cmd_cancel_run(a: CancelRunArgs) -> Result<()> {
    let (target, run_dir) = match a.run_id.as_deref() {
        Some(id) => (id.to_string(), paths::run_dir_for_id(id).ok()),
        None => {
            let Some(run_dir) = paths::current_run_dir()? else {
                anyhow::bail!("no current run to cancel");
            };
            let id = run_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("current")
                .to_string();
            (id, Some(run_dir))
        }
    };
    let dir = paths::cancels_dir()?;
    paths::ensure_dir(&dir)?;
    let marker = paths::control_marker_path(&dir, "run id", &target)?;
    std::fs::write(marker, b"cancel")?;
    // Safety net for abandoned runs: when the owner process is dead the
    // marker file has nobody to read it, so we update the state directly.
    // Idempotent — a live owner short-circuits this back to Ok(false).
    let forced = run_dir
        .as_deref()
        .map(crate::scheduler::force_cancel_if_abandoned)
        .transpose()?
        .unwrap_or(false);
    if forced {
        println!(
            "cancelled {} (owner process was dead — state forced to Cancelled)",
            target
        );
    } else {
        println!(
            "cancelled {} (scheduler picks this up within ~1 dispatch tick)",
            target
        );
    }
    Ok(())
}

// `maestro doc` lives in cli::commands::doc

pub(crate) async fn cmd_open(a: OpenArgs) -> Result<()> {
    use std::process::{Command, Stdio};

    let url = format!("http://{}:{}", a.host, a.port);
    let exe = std::env::current_exe().context("locate maestro executable")?;
    let log_path = std::env::temp_dir().join("maestro-ui.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open log {:?}", log_path))?;

    Command::new(&exe)
        .args(["ui", "--host", &a.host, "--port"])
        .arg(a.port.to_string())
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()
        .context("spawn `maestro ui`")?;

    println!("→ dashboard:  {url}");
    println!("→ logs:       {}", log_path.display());

    // give axum a beat to bind the port before launching the browser
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    if !a.no_browser {
        let _ = util::open_in_browser(&url);
    }
    Ok(())
}

// `maestro chat` lives in cli::commands::chat
// Common helpers (warn_unknown_model, levenshtein, …) live in cli::util.

/// Diagnostic for "wait, why am I seeing previous run data?"
/// Prints the resolved workspace, the env vars that might be overriding
/// cwd, the contents of `.maestro/`, and any running `maestro` processes
/// that could be serving stale state on a forgotten port.
/// Bare `maestro` (no subcommand): detect workspace state and point the user at
/// the single most relevant next command, instead of dumping the full help.
pub(crate) fn cmd_home() -> Result<()> {
    let initialized = paths::maestro_dir().map(|d| d.exists()).unwrap_or(false);
    let project_count = if initialized {
        paths::projects_file()
            .ok()
            .filter(|p| p.exists())
            .and_then(|p| ProjectsConfig::load(&p).ok())
            .map(|c| c.projects.len())
            .unwrap_or(0)
    } else {
        0
    };
    let has_runs = paths::runs_dir()
        .ok()
        .and_then(|d| d.read_dir().ok())
        .map(|mut it| it.any(|e| e.is_ok()))
        .unwrap_or(false);
    for line in home_lines(initialized, project_count, has_runs) {
        println!("{line}");
    }
    Ok(())
}

/// The homepage guidance lines for a given workspace state. Pure for testing.
pub(crate) fn home_lines(initialized: bool, project_count: usize, has_runs: bool) -> Vec<String> {
    let mut out = vec![
        "maestro — multi-project agent orchestrator".to_string(),
        String::new(),
    ];
    if !initialized {
        out.push("this directory isn't set up yet. get started:".to_string());
        out.push("  maestro init      # scan for projects + scaffold .maestro/".to_string());
    } else if project_count == 0 {
        out.push("initialized, but no projects registered yet. next:".to_string());
        out.push("  maestro init --analyze   # discover projects in this dir".to_string());
        out.push("  maestro add <path>       # register one manually".to_string());
    } else {
        out.push(format!(
            "{project_count} project(s) registered. common next steps:"
        ));
        out.push(
            "  maestro work \"<what you want to change>\"   # plan + run a change".to_string(),
        );
        out.push("  maestro ls                                 # list projects".to_string());
        if has_runs {
            out.push("  maestro runs ls                            # past runs".to_string());
        }
    }
    out.push(String::new());
    out.push("run `maestro --help` for the full command list".to_string());
    out
}

pub(crate) fn cmd_where() -> Result<()> {
    use std::process::Command;

    println!("════ maestro workspace ════");
    let root = paths::workspace_root()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("?"));
    let env_override = std::env::var("MAESTRO_WORKSPACE_ROOT").ok();
    let source = if env_override.as_deref().filter(|s| !s.is_empty()).is_some() {
        "MAESTRO_WORKSPACE_ROOT env"
    } else {
        "current working directory"
    };
    println!("  workspace_root  : {}", root.display());
    println!("  resolved_from   : {source}");
    if cwd != root {
        println!(
            "  process cwd     : {}  (differs from workspace!)",
            cwd.display()
        );
    }
    println!(
        "  MAESTRO_WORKSPACE_ROOT : {}",
        env_override.as_deref().unwrap_or("(unset)")
    );

    println!();
    println!("════ .maestro/ ════");
    let dot = paths::maestro_dir()?;
    if !dot.exists() {
        println!("  (no .maestro/ here — `maestro init` will create one)");
    } else {
        println!("  path            : {}", dot.display());
        let runs_dir = paths::runs_dir()?;
        let runs = list_runs(&runs_dir);
        match runs {
            Ok(ids) if !ids.is_empty() => {
                println!("  runs ({})       : {}", ids.len(), short_runs(&ids, 5));
            }
            Ok(_) => println!("  runs            : (none)"),
            Err(e) => println!("  runs            : (read failed: {e:#})"),
        }
        let current_link = paths::current_run_link()?;
        if current_link.exists() {
            let target = std::fs::read_link(&current_link)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "(broken symlink)".into());
            println!("  current_run     : {target}");
        } else {
            println!("  current_run     : (no symlink)");
        }
        let projects_file = paths::projects_file()?;
        if projects_file.exists() {
            match crate::config::ProjectsConfig::load(&projects_file) {
                Ok(cfg) => println!(
                    "  projects.yaml   : {} project(s) registered",
                    cfg.projects.len()
                ),
                Err(e) => println!("  projects.yaml   : (parse failed: {e:#})"),
            }
        } else {
            println!("  projects.yaml   : (missing)");
        }
    }

    println!();
    println!("════ live maestro processes (this host) ════");
    let out = Command::new("ps")
        .args(["-ef"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let pid_self = std::process::id();
    let mut found = false;
    for line in out.lines() {
        if !looks_like_maestro_process(line) {
            continue;
        }
        if line.contains(" grep ") || line.contains("/grep ") {
            continue;
        }
        let pid = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u32>().ok());
        if pid == Some(pid_self) {
            continue;
        }
        let collapsed = collapse_ps_line(line);
        println!("  {collapsed}");
        found = true;
    }
    if !found {
        println!("  (none)");
    }

    // Legacy `.mux/` left behind by the pre-rename tool. If it exists
    // and `.maestro/` doesn't, that's almost certainly what the user is
    // missing — surface it loudly.
    let legacy_mux = root.join(".mux");
    if legacy_mux.exists() {
        println!();
        println!("════ legacy .mux/ detected ════");
        println!("  path  : {}", legacy_mux.display());
        if dot.exists() {
            println!("  state : BOTH .mux/ and .maestro/ exist — merge manually or rename one.");
        } else {
            println!("  state : `.maestro/` missing — run `maestro migrate` to recover.");
        }
    }

    println!();
    println!("════ tips ════");
    println!("  • run from a different workspace by `cd` into that directory first.");
    println!("  • a stale `maestro ui` from another cwd will keep serving its OWN .maestro;");
    println!("    run `maestro list` to see them and `maestro stop --all` to clear them.");
    println!("  • to clear THIS workspace's run history: `rm -rf .maestro/runs/`");
    Ok(())
}

/// Recover data left behind by the pre-rename tool by moving `.mux/` to
/// `.maestro/`. Conservative on purpose:
///   - never overwrites an existing `.maestro/`
///   - never rewrites file contents (old "mux" strings in REPORT.md /
///     comments are cosmetic, not functional)
///   - prints exactly what it did so you can audit later
pub(crate) fn cmd_migrate(args: MigrateArgs) -> Result<()> {
    let root = paths::workspace_root()?;
    let old = root.join(".mux");
    let new = root.join(".maestro");

    if !old.exists() {
        println!("no .mux/ found in {}", root.display());
        println!("nothing to migrate.");
        return Ok(());
    }

    // Conflict resolution. Three cases, in order of how invasive we get:
    //
    //   (a) `.maestro/` doesn't exist  → straight rename. easy.
    //   (b) `.maestro/` exists but is empty/stub  → silently move it aside
    //       (no data loss possible) and proceed.
    //   (c) `.maestro/` exists and has real content  → require --force,
    //       back it up to a timestamped path, then proceed.
    if new.exists() {
        let stub = is_essentially_empty(&new);
        if stub {
            // (b) stub case: tiny side-step, no --force needed
            let backup = backup_path_for(&new);
            std::fs::rename(&new, &backup)
                .with_context(|| format!("move stub {:?} aside to {:?}", new, backup))?;
            println!(
                "✓ existing .maestro/ was empty; moved it aside to {}",
                backup.file_name().unwrap_or_default().to_string_lossy()
            );
        } else if args.force {
            // (c) explicit override
            let backup = backup_path_for(&new);
            std::fs::rename(&new, &backup)
                .with_context(|| format!("move {:?} aside to {:?}", new, backup))?;
            println!(
                "⚠ --force: existing .maestro/ contained data; backed it up to {}",
                backup.file_name().unwrap_or_default().to_string_lossy()
            );
            println!("  inspect the backup later; you can `rm -rf` it once you're sure.");
        } else {
            anyhow::bail!(
                "refusing to migrate: both .mux/ and .maestro/ exist under {}\n\
                 and .maestro/ has content. Re-run with `--force` to back up\n\
                 the current .maestro/ to .maestro.bak-<ts>/ and recover .mux/.\n\
                 Or merge manually and `rm -rf .mux/` yourself.",
                root.display()
            );
        }
    }

    std::fs::rename(&old, &new).with_context(|| format!("rename {:?} -> {:?}", old, new))?;

    println!("✓ renamed .mux/ → .maestro/  (in {})", root.display());

    // Best-effort summary so the user knows what came back.
    let runs_dir = new.join("runs");
    let run_count = std::fs::read_dir(&runs_dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != "current"
                })
                .count()
        })
        .unwrap_or(0);
    let projects_yaml = new.join(paths::PROJECTS_FILE);
    let project_count = if projects_yaml.exists() {
        crate::config::ProjectsConfig::load(&projects_yaml)
            .map(|c| c.projects.len())
            .unwrap_or(0)
    } else {
        0
    };
    let chat_count = std::fs::read_dir(new.join("chat").join("sessions"))
        .map(|it| it.filter_map(|e| e.ok()).count())
        .unwrap_or(0);

    println!();
    println!("recovered:");
    println!("  - {run_count} run(s)");
    println!("  - {project_count} project(s) in projects.yaml");
    println!("  - {chat_count} chat session(s)");
    println!();
    println!("note: file CONTENTS may still mention \"mux\" in old REPORT.md");
    println!("      files and similar. These are cosmetic and don't affect");
    println!("      execution. Re-run `maestro run` will produce maestro-labeled");
    println!("      reports going forward.");
    println!();
    println!("next: `maestro where` to confirm, or `maestro ui` to view the dashboard.");
    Ok(())
}

/// True iff a `.maestro/` directory looks like a freshly-`init`ed stub
/// rather than a workspace with real history. We check the three things
/// users actually care about: runs, registered projects, chat sessions.
/// Seed skills / memory don't count — those come from `init` itself.
pub(crate) fn is_essentially_empty(dot: &std::path::Path) -> bool {
    // Any non-`current` directory under `runs/` counts as real history.
    let has_runs = std::fs::read_dir(dot.join("runs"))
        .map(|it| {
            it.filter_map(|e| e.ok()).any(|e| {
                e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != "current"
            })
        })
        .unwrap_or(false);

    // An init-created `projects.yaml` exists but is empty (just the
    // version header). Treat "no registered projects" as no real content
    // — that's exactly the case a fresh `maestro init` leaves behind.
    let pf = dot.join(paths::PROJECTS_FILE);
    let has_projects = pf.exists()
        && crate::config::ProjectsConfig::load(&pf)
            .map(|c| !c.projects.is_empty())
            .unwrap_or(true); // parse failure → assume there's something interesting
    let has_chats = std::fs::read_dir(dot.join("chat").join("sessions"))
        .map(|it| it.filter_map(|e| e.ok()).next().is_some())
        .unwrap_or(false);

    !(has_runs || has_projects || has_chats)
}

pub(crate) fn backup_path_for(dot: &std::path::Path) -> std::path::PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let parent = dot.parent().unwrap_or(std::path::Path::new("."));
    let mut candidate = parent.join(format!(".maestro.bak-{ts}"));
    let mut suffix = 0;
    while candidate.exists() {
        suffix += 1;
        candidate = parent.join(format!(".maestro.bak-{ts}-{suffix}"));
    }
    candidate
}

pub(crate) fn list_runs(dir: &std::path::Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        // Skip the `current` symlink (already shown separately) and any
        // dotfiles we might have stashed.
        if name == "current" || name.starts_with('.') {
            continue;
        }
        out.push(name);
    }
    out.sort();
    out.reverse(); // newest first (timestamps in name)
    Ok(out)
}

pub(crate) fn short_runs(ids: &[String], n: usize) -> String {
    if ids.len() <= n {
        return ids.join(", ");
    }
    let head: Vec<_> = ids.iter().take(n).cloned().collect();
    format!("{}, … (+{} older)", head.join(", "), ids.len() - n)
}

/// Heuristic to tell a real `maestro` process from a process that just
/// happens to mention "maestro" in its argv (e.g. a shell whose cwd is a
/// directory named `maestro-foo`). We only count it if some argv token
/// either equals `maestro` or ends with `/maestro`.
pub(crate) fn looks_like_maestro_process(ps_line: &str) -> bool {
    // ps -ef columns: UID PID PPID C STIME TTY TIME CMD…
    // Skip the first 7 fields; everything after is argv.
    let mut argv = ps_line.split_whitespace().skip(7);
    argv.any(|tok| tok == "maestro" || tok.ends_with("/maestro"))
}

pub(crate) fn collapse_ps_line(line: &str) -> String {
    // `ps -ef` format: UID PID PPID C STIME TTY TIME CMD…
    // We just want PID + the trailing command, abridged.
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        return line.to_string();
    }
    let pid = parts[1];
    let cmd = parts[7..].join(" ");
    let cmd = if cmd.len() > 100 {
        format!("{}…", &cmd[..100])
    } else {
        cmd
    };
    format!("pid {pid:>6}  {cmd}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitAnalysisMode {
    Run,
    Skip,
    Prompt,
}

pub(crate) fn init_analysis_mode(a: &InitArgs, stdin_is_terminal: bool) -> InitAnalysisMode {
    if a.analyze {
        return InitAnalysisMode::Run;
    }
    if a.no_analyze || !stdin_is_terminal {
        return InitAnalysisMode::Skip;
    }
    InitAnalysisMode::Prompt
}

pub(crate) fn prompt_yes_no(prompt: &str, default: bool) -> Result<bool> {
    // Routed through the shared `ask` primitive so every y/n prompt has the
    // same structure and the same headless fallback (no TTY → default).
    let ask = crate::ask::Ask::yes_no(prompt.trim_end(), default);
    Ok(crate::ask::resolve(&ask) == vec!["yes".to_string()])
}

/// Zero-config discovery: scan + register projects (no DAG synthesis) so a
/// plain `maestro init` leaves the workspace ready to `work` in one step
/// instead of two. Non-fatal — discovery problems shouldn't fail `init`.
pub(crate) fn run_init_discovery_only(a: &InitArgs) {
    let root = match a.root.clone() {
        Some(root) => root,
        None => match paths::workspace_root() {
            Ok(r) => r,
            Err(_) => return,
        },
    };
    if let Err(e) = commands::work::discover_and_apply(&root, a.max_depth, &a.agent, false) {
        tracing::warn!("init project discovery skipped: {e:#}");
    }
}

pub(crate) fn run_init_analysis(a: &InitArgs) -> Result<()> {
    let root = match &a.root {
        Some(root) => root.clone(),
        None => paths::workspace_root()?,
    };
    let spec = format!(
        "Analyze and verify the discovered project dependency DAG under {}",
        root.display()
    );

    println!(
        "→ analyzing projects under {} with agent `{}`",
        root.display(),
        a.agent
    );
    commands::work::discover_and_apply(&root, a.max_depth, &a.agent, false)?;
    let projects = ProjectsConfig::load(&paths::projects_file()?)?;
    if projects.projects.is_empty() {
        println!("→ no projects discovered; register one with `maestro add <path>` or rerun with a different --root");
        return Ok(());
    }

    // Scope the audit to `--project` when given; otherwise audit every
    // discovered project (synthesize_audit_file keeps the full registry on an
    // empty selection — F-AUDIT-001). This is what the audit-size warning's
    // "scope with `--project`" advice refers to.
    let plan_path =
        commands::plan::synthesize_audit_file(&spec, a.out.clone(), a.projects.clone())?;
    commands::work::validate_generated_plan(&plan_path)?;

    println!("→ generated dependency DAG plan: {}", plan_path.display());
    println!("next:");
    println!("  maestro plan validate {}", plan_path.display());
    println!("  maestro run {} --dry", plan_path.display());
    println!("  maestro open --no-browser   # inspect the DAG in the dashboard");
    Ok(())
}

pub(crate) fn cmd_init(a: InitArgs) -> Result<()> {
    let dir = paths::maestro_dir()?;
    paths::ensure_dir(&dir)?;
    paths::ensure_dir(&paths::runs_dir()?)?;
    paths::ensure_dir(&paths::approvals_dir()?)?;
    paths::ensure_dir(&paths::cancels_dir()?)?;
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        let cfg = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        cfg.save(&pfile)?;
    }

    if !a.bare {
        let mut wrote = 0usize;
        for (scope, name, content) in samples::init_skill_samples() {
            let target =
                crate::skills::skill_path(&crate::skills::SkillScope::from_dir(scope), name)?;
            if !target.exists() {
                let _ =
                    crate::skills::save(&crate::skills::SkillScope::from_dir(scope), name, content);
                wrote += 1;
            }
        }
        let store = crate::memory::MemoryStore::open()?;
        for (topic, name, content) in samples::init_memory_samples() {
            let topic_file = store.l1_root().join(topic).join(name);
            if !topic_file.exists() {
                let _ = store.add(topic, name, content);
                wrote += 1;
            }
        }
        if wrote > 0 {
            println!(
                "  seeded {wrote} sample(s) under .maestro/skills/ and .maestro/memory/  (use --bare to skip)"
            );
        }
    }

    println!("initialized .maestro at {:?}", dir);
    let mut analyzed = false;
    match init_analysis_mode(&a, std::io::stdin().is_terminal()) {
        InitAnalysisMode::Run => {
            run_init_analysis(&a)?;
            analyzed = true;
        }
        // Even when the DAG synthesis is skipped, still discover + register
        // projects (zero-config) so `init` leaves the workspace ready to run.
        // `--bare` opts out for pure scaffolding.
        InitAnalysisMode::Skip => {
            if !a.bare {
                run_init_discovery_only(&a);
            }
        }
        InitAnalysisMode::Prompt => {
            if prompt_yes_no("Also build a dependency-verification DAG now? [Y/n] ", true)? {
                run_init_analysis(&a)?;
                analyzed = true;
            } else if !a.bare {
                run_init_discovery_only(&a);
            }
        }
    }
    // Always leave the user with a clear next step — the #1 onboarding cliff is
    // a freshly-initialized workspace with no registered projects and no hint.
    if !analyzed {
        print_init_next_steps(&a);
    }
    Ok(())
}

/// State-aware "what do I do now?" guidance printed at the end of `init` when
/// the optional analyze step didn't run. Reads projects.yaml so the advice
/// matches reality (empty → how to discover; populated → how to start a run).
pub(crate) fn print_init_next_steps(a: &InitArgs) {
    let count = paths::projects_file()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| ProjectsConfig::load(&p).ok())
        .map(|c| c.projects.len())
        .unwrap_or(0);
    let root = a
        .root
        .as_ref()
        .map(|r| r.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    for line in init_next_steps_lines(count, &root) {
        println!("{line}");
    }
}

/// The "next:" guidance lines for a freshly-initialized workspace, branching on
/// whether any projects are registered. Pure so it can be unit-tested.
pub(crate) fn init_next_steps_lines(project_count: usize, root: &str) -> Vec<String> {
    let mut out = vec![String::new(), "next:".to_string()];
    if project_count == 0 {
        out.push("  no projects registered yet — discover them in one step:".to_string());
        out.push(format!(
            "    maestro init --analyze --root {root}   # scan for projects + build a DAG"
        ));
        out.push("  or register one manually:".to_string());
        out.push("    maestro add <path>".to_string());
    } else {
        out.push(format!(
            "  {project_count} project(s) registered. start a workflow:"
        ));
        out.push("    maestro work \"<what you want to change>\"".to_string());
        out.push(
            "    maestro ls                              # list registered projects".to_string(),
        );
    }
    out
}

pub(crate) fn cmd_add(a: AddArgs) -> Result<()> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` first");
    }
    let mut cfg = ProjectsConfig::load(&pfile)?;
    let abs = paths::expand(&a.path)?;
    let name = a.name.unwrap_or_else(|| {
        abs.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into())
    });
    let project = Project {
        path: abs.to_string_lossy().to_string(),
        r#type: a.r#type,
        stack: a.stack,
        commands: Default::default(),
        contracts: Default::default(),
        dependencies: Vec::new(),
        memory_scope: vec![],
        agent: a.agent,
        agent_model: None,
        cursor_model: None,
        model_profile: None,
        role: a.role,
        agent_profile: None,
        review_profile: None,
        copy_files: Vec::new(),
    };
    cfg.projects.insert(name.clone(), project);
    cfg.save(&pfile)?;
    println!("added project '{name}' → {:?}", abs);
    Ok(())
}

pub(crate) fn cmd_ls() -> Result<()> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` first");
    }
    let cfg = ProjectsConfig::load(&pfile)?;
    if cfg.projects.is_empty() {
        println!("(no projects registered — `maestro add <path>`)");
        return Ok(());
    }
    println!("{:<20} {:<10} {:<20} PATH", "NAME", "TYPE", "AGENT");
    for (name, p) in &cfg.projects {
        let agent = p
            .agent
            .clone()
            .unwrap_or_else(|| cfg.defaults.agent.clone());
        let ty = p.r#type.clone().unwrap_or_else(|| "—".into());
        println!("{name:<20} {ty:<10} {agent:<20} {}", p.path);
    }
    Ok(())
}

/// Resolve the external `codegraph` CLI path (`~/.local/bin` first, then PATH).
pub(crate) fn cmd_codegraph(a: CodegraphArgs) -> Result<()> {
    use crate::codegraph::{engine_status, find_repo_root};
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = find_repo_root(&root);

    match a.subcmd.unwrap_or(CodegraphSubcmd::Status) {
        CodegraphSubcmd::Status => {
            println!("code-graph engines (richest first; ★ = currently served):\n");
            for e in engine_status(&root) {
                let star = if e.active { "★" } else { " " };
                let state = if e.built {
                    "built"
                } else if e.installed {
                    "installed, not built"
                } else {
                    "not installed"
                };
                println!("{star} {:<22} [{}] {}", e.engine, e.cost, state);
                println!("    {}", e.hint);
            }
            Ok(())
        }
        CodegraphSubcmd::Build { engine, root: r } => {
            let root = r.map(|p| find_repo_root(&p)).unwrap_or(root);
            match engine.as_str() {
                "native" => {
                    println!("→ native graph needs no build — it's computed on demand and always active when no richer graph exists.");
                    Ok(())
                }
                "understand" | "understand-anything" => {
                    println!(
                        "→ Understand-Anything is a Claude Code plugin; maestro can't run it for you.\n  In Claude Code, run:  /understand {}\n  ⚠ this does an LLM pass over the repo — it uses tokens.\n  Once it writes .understand-anything/knowledge-graph.json, the dashboard upgrades automatically.",
                        root.display()
                    );
                    Ok(())
                }
                "codegraph" => {
                    if !crate::codegraph::codegraph_installed() {
                        anyhow::bail!(
                            "codegraph CLI not found (looked in PATH and ~/.local/bin).\n  Install it, then re-run `maestro codegraph build --engine codegraph`."
                        );
                    }
                    let bin = crate::codegraph::codegraph_bin();
                    // init is idempotent-ish; ignore its error if already initialized.
                    if crate::codegraph::find_db(&root).is_none() {
                        println!("→ codegraph init {}", root.display());
                        let _ = std::process::Command::new(&bin)
                            .arg("init")
                            .arg(&root)
                            .status();
                    }
                    println!("→ codegraph index {}", root.display());
                    let status = std::process::Command::new(&bin)
                        .arg("index")
                        .arg(&root)
                        .status()
                        .with_context(|| "spawn codegraph index")?;
                    if !status.success() {
                        anyhow::bail!("codegraph index failed");
                    }
                    println!("✓ built codegraph index — the code-graph tab will now use it (free, no tokens).");
                    Ok(())
                }
                other => {
                    anyhow::bail!("unknown engine `{other}` (use: codegraph | understand | native)")
                }
            }
        }
    }
}

pub(crate) fn cmd_brief() -> Result<()> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` first");
    }
    let cfg = ProjectsConfig::load(&pfile)?;
    // Fold in code-graph-derived module imports so the brief shows real
    // dependency flow on a monorepo whose modules declare none.
    let derived = match paths::workspace_root() {
        Ok(root) => {
            let module_paths: Vec<(String, String)> = cfg
                .projects
                .iter()
                .map(|(name, p)| (name.clone(), p.path.clone()))
                .collect();
            crate::codegraph::load_derived_module_deps(&root, &module_paths, 1)
        }
        Err(_) => std::collections::BTreeMap::new(),
    };
    println!("{}", crate::config::architecture_brief(&cfg, &derived));
    Ok(())
}

pub(crate) fn cmd_remove(a: RemoveArgs) -> Result<()> {
    let pfile = paths::projects_file()?;
    let mut cfg = ProjectsConfig::load(&pfile)?;
    if cfg.projects.remove(&a.name).is_none() {
        anyhow::bail!("project '{}' not found", a.name);
    }
    cfg.save(&pfile)?;
    println!("removed '{}'", a.name);
    Ok(())
}

pub(crate) fn cmd_validate() -> Result<()> {
    let cfg = ProjectsConfig::load(&paths::projects_file()?)?;
    let mut errors = vec![];
    for (name, p) in &cfg.projects {
        let abs = paths::expand(&p.path)?;
        if !abs.exists() {
            errors.push(format!("project '{name}': path does not exist: {:?}", abs));
        }
    }
    // F-114: agent-profile structure + cross-config references (pure).
    errors.extend(cfg.agent_profile_issues());
    // F-114: role + skill existence against the on-disk registries — the shared
    // `crate::profile_visibility` source of truth (also used by F-118 runtime health).
    let existence = crate::profile_visibility::profile_existence_report(&cfg);
    errors.extend(existence.role_issues);
    errors.extend(existence.skill_issues);
    if errors.is_empty() {
        println!(
            "ok ({} project{})",
            cfg.projects.len(),
            if cfg.projects.len() == 1 { "" } else { "s" }
        );
        Ok(())
    } else {
        for e in &errors {
            eprintln!("{e}");
        }
        anyhow::bail!("{} validation error(s)", errors.len())
    }
}

// `maestro run` lives in cli::commands::run

pub(crate) fn cmd_approve(a: ApproveArgs) -> Result<()> {
    let dir = paths::approvals_dir()?;
    paths::ensure_dir(&dir)?;
    let f = paths::control_marker_path(&dir, "task id", &a.task_id)?;
    std::fs::write(&f, b"approved").context("write approval marker")?;
    println!("approved {}", a.task_id);
    // Guide the user against the current run — a typo'd id otherwise "succeeds"
    // silently (the marker is written but never consumed). The marker is still
    // written above to preserve legitimate pre-approval before a task runs.
    if let Ok(Some(run_dir)) = paths::current_run_dir() {
        if let Ok(state) = crate::scheduler::RunState::load(&run_dir) {
            let known = state.tasks.contains_key(&a.task_id);
            if let Some(msg) = approve_guidance(known, &state.approvals_pending, &a.task_id) {
                println!("{msg}");
            }
        }
    }
    Ok(())
}

/// Advisory line printed after `approve`, based on the current run's state.
/// `None` when the id is a known task that simply isn't awaiting approval yet
/// (a valid pre-approval — no need to nag).
pub(crate) fn approve_guidance(known: bool, awaiting: &[String], task_id: &str) -> Option<String> {
    if !known {
        return Some(if awaiting.is_empty() {
            format!("note: no task `{task_id}` in the current run, and nothing is awaiting approval right now (see: maestro status)")
        } else {
            format!(
                "note: no task `{task_id}` in the current run — awaiting approval: {} (see: maestro status)",
                awaiting.join(", ")
            )
        });
    }
    awaiting
        .iter()
        .any(|t| t == task_id)
        .then(|| "→ that task was awaiting approval; the run will now continue".to_string())
}

pub(crate) fn cmd_status() -> Result<()> {
    let Some(dir) = paths::current_run_dir()? else {
        println!("(no current run)");
        return Ok(());
    };
    let state = crate::scheduler::RunState::load(&dir)?;
    println!("run    {}", state.run_id);
    println!("spec   {}", state.spec);
    println!("status {:?}", state.status);
    println!();
    println!("{:<28} {:<14} {:<10} PROJECT", "ID", "STATUS", "AGENT");
    for id in &state.task_order {
        // Guarded lookup: a hand-edited / partially-written / forward-compat
        // RUN_STATE.json can list a task_order id with no matching tasks entry;
        // indexing the BTreeMap would panic the whole command (the dashboard
        // renderer already guards this). Render a partial table instead.
        let Some(t) = state.tasks.get(id) else {
            continue;
        };
        println!(
            "{:<28} {:<14} {:<10} {}",
            t.id,
            format!("{:?}", t.status),
            t.agent,
            t.project
        );
    }
    if !state.approvals_pending.is_empty() {
        println!();
        println!("pending approval: {}", state.approvals_pending.join(", "));
    }
    Ok(())
}

pub(crate) async fn cmd_logs(a: LogsArgs) -> Result<()> {
    let dir = match a.run.as_deref() {
        Some("current") | None => {
            paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run"))?
        }
        Some(name) => paths::run_dir_for_id(name)?,
    };
    paths::validate_path_component("task id", &a.task_id)?;
    let log_path = dir.join("logs").join(format!("{}.log", a.task_id));

    // Wait briefly for the file to appear if --follow and the task hasn't started yet.
    if a.follow && !log_path.exists() {
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if log_path.exists() {
                break;
            }
        }
    }
    if !log_path.exists() {
        let mut available: Vec<String> = std::fs::read_dir(dir.join("logs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "log") {
                    p.file_stem().map(|s| s.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .collect();
        available.sort();
        if available.is_empty() {
            anyhow::bail!(
                "no log for task `{}` (no task logs in this run yet — see: maestro status)",
                a.task_id
            );
        }
        anyhow::bail!(
            "no log for task `{}`. available task logs: {} (see: maestro status)",
            a.task_id,
            available.join(", ")
        );
    }

    let text = std::fs::read_to_string(&log_path)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(a.tail);
    for l in &lines[start..] {
        println!("{l}");
    }
    let mut last_size = text.len() as u64;

    if !a.follow {
        return Ok(());
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let Ok(meta) = std::fs::metadata(&log_path) else {
            continue;
        };
        let size = meta.len();
        if size > last_size {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = std::fs::File::open(&log_path)?;
            f.seek(SeekFrom::Start(last_size))?;
            let mut buf = String::new();
            f.read_to_string(&mut buf)?;
            print!("{buf}");
            last_size = size;
        } else if size < last_size {
            last_size = 0;
        }
    }
}

pub(crate) async fn cmd_ui(a: UiArgs) -> Result<()> {
    crate::server::serve(&a.host, a.port).await
}

/// Lines of fixed chrome (header block + table header + footer) reserved when
/// fitting the task list to the terminal height in live mode.
const TUI_CHROME_LINES: usize = 18;

/// Restores the terminal on drop: shows the cursor and leaves the alternate
/// screen buffer. Held for the lifetime of an interactive `maestro tui` so the
/// user's scrollback is untouched and the cursor returns even on Ctrl-C.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Self {
        let mut out = std::io::stdout();
        // Enter alternate screen + hide cursor.
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l");
        let _ = out.flush();
        TerminalGuard
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        // Show cursor + leave alternate screen.
        let _ = out.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    }
}

/// Real terminal size via TIOCGWINSZ, falling back to `COLUMNS`/`LINES` and
/// finally sane defaults. `COLUMNS` alone is unreliable because shells rarely
/// export it to child processes.
pub(crate) fn terminal_size() -> (usize, usize) {
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return (ws.ws_col as usize, (ws.ws_row as usize).max(1));
        }
    }
    let cols = dashboard_width_from_columns(std::env::var("COLUMNS").ok().as_deref());
    let rows = std::env::var("LINES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(40);
    (cols, rows)
}

pub(crate) async fn cmd_tui(a: TuiArgs) -> Result<()> {
    // Default routing: interactive ratatui frontend when both stdin
    // and stdout are real terminals; otherwise the classic line-printer
    // dashboard (CI / scripts / pipe-to-file consumers expect plain
    // ANSI, not raw-mode + alternate-screen sequences).
    //
    // Explicit flags override the auto-detect: --interactive forces
    // ratatui (errors out if stdin/stdout isn't a TTY); --classic
    // forces the line-printer even in a real terminal. --once implies
    // --classic since the whole point of --once is a one-shot dump.
    if a.interactive && a.classic {
        anyhow::bail!("--interactive and --classic are mutually exclusive");
    }
    if a.interactive && a.once {
        anyhow::bail!("--interactive and --once are mutually exclusive");
    }
    let stdin_tty = std::io::stdin().is_terminal();
    let stdout_tty = std::io::stdout().is_terminal();
    let want_interactive = !a.classic && !a.once && (a.interactive || (stdin_tty && stdout_tty));
    if want_interactive {
        return super::chat_tui::run(a.run.clone(), a.interval_ms).await;
    }
    use std::collections::{HashMap, HashSet};
    // Only take over the screen when we're refreshing into a real terminal.
    // `--once` and piped output stay inline so they remain scriptable.
    let interactive = !a.once && std::io::stdout().is_terminal();
    let _guard = interactive.then(TerminalGuard::enter);
    // Carry the previous tick's per-task status across the loop so we can
    // briefly highlight transitions — the k9s / lazygit pattern: the user's
    // eye is drawn to what just *changed*, not to the whole table.
    let mut prev_statuses: HashMap<String, crate::scheduler::state::TaskStatus> = HashMap::new();
    let interval_ms = a.interval_ms.max(250);
    loop {
        let (id, dir) = commands::runs::resolve_run_dir_arg(a.run.as_deref())?;
        let state = crate::scheduler::RunState::load(&dir)?;
        let evidence = crate::scheduler::evidence::read_evidence_summary(&dir)
            .unwrap_or_else(|| crate::scheduler::evidence::build_run_evidence(&state));
        let mut transitioned: HashSet<String> = HashSet::new();
        for (task_id, task) in &state.tasks {
            match prev_statuses.get(task_id) {
                Some(prev) if *prev == task.status => {}
                Some(_) => {
                    transitioned.insert(task_id.clone());
                }
                None => {
                    // First sighting: don't flash on the initial render — the
                    // whole table is "new" then and flashing every row reads
                    // like noise. Only highlight after we've seen the task at
                    // least once.
                }
            }
        }
        let (cols, rows) = terminal_size();
        let width = cols.clamp(DASHBOARD_MIN_WIDTH, DASHBOARD_MAX_WIDTH);
        let max_tasks = interactive.then(|| rows.saturating_sub(TUI_CHROME_LINES).max(3));
        let lines = render_dashboard_lines(&id, &state, &evidence, width, max_tasks, &transitioned);

        let mut out = std::io::stdout();
        if interactive {
            // Repaint in place: home the cursor, clear each line to its end as
            // we overwrite it, then clear anything left below. This avoids the
            // full-screen `\x1B[2J` flash on every tick.
            let mut buf = String::from("\x1b[H");
            for line in &lines {
                buf.push_str(line);
                buf.push_str("\x1b[K\n");
            }
            buf.push_str("\x1b[0J");
            // Beefier footer: explicit refresh cadence + last-tick timestamp
            // so the user can tell the dashboard is alive even when nothing's
            // changing (no more "is this frozen?").
            let cadence = format!(
                "\nctrl-c to quit  ·  refreshing every {:.1}s  ·  {}",
                (interval_ms as f64) / 1000.0,
                chrono::Local::now().format("%H:%M:%S"),
            );
            buf.push_str(&colorize(&cadence, "90"));
            buf.push_str("\x1b[K");
            let _ = out.write_all(buf.as_bytes());
        } else {
            for line in &lines {
                let _ = writeln!(out, "{line}");
            }
        }
        let _ = out.flush();

        // Roll the previous-statuses snapshot forward for next tick's diff.
        prev_statuses = state
            .tasks
            .iter()
            .map(|(k, v)| (k.clone(), v.status))
            .collect();

        if a.once {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(interval_ms)) => {}
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

pub(crate) fn render_dashboard_lines(
    id: &str,
    state: &crate::scheduler::RunState,
    evidence: &crate::scheduler::evidence::RunEvidence,
    width: usize,
    max_tasks: Option<usize>,
    // Task ids whose status changed since the previous tick. Used to flash
    // the affected rows so the user's eye is drawn to what just moved —
    // the k9s/lazygit pattern. Empty set ⇒ no flash (e.g. `--once`).
    transitioned: &std::collections::HashSet<String>,
) -> Vec<String> {
    use crate::scheduler::TaskStatus;

    let progress_width = width.saturating_sub(62).clamp(18, 42);
    let task_width = if width < 92 { 22 } else { 28 };
    let project_width = if width < 92 { 10 } else { 13 };
    let status_width = 15;
    let duration_width = 10;
    let workspace_width = width
        .saturating_sub(task_width + project_width + status_width + duration_width + 6)
        .clamp(18, 44);
    let done = state
        .tasks
        .values()
        .filter(|task| {
            matches!(
                task.status,
                crate::scheduler::TaskStatus::Done
                    | crate::scheduler::TaskStatus::Failed
                    | crate::scheduler::TaskStatus::Skipped
                    | crate::scheduler::TaskStatus::Cancelled
            )
        })
        .count();
    let total = state.tasks.len().max(1);
    let running = state
        .tasks
        .values()
        .filter(|task| matches!(task.status, TaskStatus::Running))
        .count();
    let failed = state
        .tasks
        .values()
        .filter(|task| matches!(task.status, TaskStatus::Failed))
        .count();
    let waiting = state
        .tasks
        .values()
        .filter(|task| matches!(task.status, TaskStatus::AwaitingApproval))
        .count();
    let status = format!("{:?}", state.status).to_lowercase();
    let status = colorize(&status, run_status_color(&status));
    let verified = if state.verified {
        colorize("verified", "32")
    } else {
        colorize("not verified", "90")
    };
    let percent = dashboard_percent(done, total);

    let mut lines: Vec<String> = Vec::new();
    lines.push(colorize(
        &format!(
            "maestro tui  {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ),
        "36",
    ));
    lines.push(dashboard_rule(width));
    lines.push(format!("{} {}", dashboard_label("run"), id));
    // Clip the spec to the line width so a long goal can't wrap and shove the
    // rest of the dashboard around.
    let spec_budget = width.saturating_sub(DASHBOARD_LABEL_WIDTH + 1).max(8);
    lines.push(format!(
        "{} {}",
        dashboard_label("spec"),
        truncate_right(&state.spec, spec_budget)
    ));
    lines.push(format!(
        "{} {}  {}",
        dashboard_label("status"),
        status,
        verified
    ));
    lines.push(format!(
        "{} [{}] {done}/{total} ({percent:>3}%)  {} running  {} failed  {} waiting",
        dashboard_label("progress"),
        progress_bar(done, total, progress_width),
        colorize(&running.to_string(), "34"),
        colorize(&failed.to_string(), if failed > 0 { "31" } else { "90" }),
        colorize(&waiting.to_string(), if waiting > 0 { "33" } else { "90" })
    ));
    lines.push(format!(
        "{} observed={}/{}  overlaps={}  browser_evidence={}",
        dashboard_label("parallel"),
        evidence.max_observed_parallelism,
        evidence.max_parallel,
        evidence.parallel_windows.len(),
        if evidence.browser.present {
            "yes"
        } else {
            "no"
        }
    ));
    if let Some((passed, checks)) = state.acceptance_summary() {
        let code = if passed == checks { "32" } else { "31" };
        lines.push(format!(
            "{} {}",
            dashboard_label("acceptance"),
            colorize(&format!("{passed}/{checks}"), code)
        ));
    }
    lines.push(String::new());
    lines.push(colorize(
        &format!(
            "{:<task_width$} {:<project_width$} {:<status_width$} {:>duration_width$}  workspace",
            "TASK", "PROJECT", "STATUS", "DURATION"
        ),
        "90",
    ));
    lines.push(dashboard_rule(width));

    let order: Vec<&String> = state
        .task_order
        .iter()
        .filter(|id| state.tasks.contains_key(*id))
        .collect();
    // When fitting to the terminal height, keep room for a "+N more" line.
    let (shown, hidden) = match max_tasks {
        Some(max) if order.len() > max => {
            let keep = max.saturating_sub(1).max(1);
            (keep, order.len() - keep)
        }
        _ => (order.len(), 0),
    };
    for id in order.iter().take(shown) {
        let Some(task) = state.tasks.get(*id) else {
            continue;
        };
        let duration = task
            .started_at
            .zip(task.ended_at)
            .map(|(start, end)| format!("{}ms", (end - start).num_milliseconds().max(0)))
            .unwrap_or_else(|| "-".to_string());
        let workspace = task
            .worktree_path
            .as_deref()
            .or(task.workspace_path.as_deref())
            .unwrap_or("-");
        // Canonical snake_case key (matches `status_symbol`/`task_status_color`
        // and serde) — Debug-formatting drops the underscore, e.g.
        // `awaiting_approval` → `awaitingapproval`, which also lost the ⏸ icon
        // and amber colour. Display with spaces for readability.
        let status_key = status_key(&task.status);
        // A row that just transitioned this tick gets a leading ▸ marker so
        // the eye finds it instantly — even a glance at a 30-row dashboard
        // surfaces "what just moved".
        let flash = transitioned.contains(&task.id);
        let status_text = format!(
            "{}{} {}",
            if flash { "▸ " } else { "  " },
            status_symbol(status_key),
            status_key.replace('_', " ")
        );
        let mut status = colorize(
            &format!("{status_text:<status_width$}"),
            task_status_color(status_key),
        );
        if flash {
            // Bold for one tick; the next render won't include this id in the
            // set, so the row settles back to normal weight automatically.
            status = format!("\x1b[1m{status}\x1b[22m");
        }
        lines.push(format!(
            "{:<task_width$} {:<project_width$} {} {:>duration_width$}  {}",
            truncate_right(&task.id, task_width),
            truncate_right(&task.project, project_width),
            status,
            duration,
            truncate_middle(workspace, workspace_width)
        ));
    }
    if hidden > 0 {
        lines.push(colorize(&format!("… +{hidden} more"), "90"));
    }
    lines.push(String::new());
    lines.push(colorize("next actions", "36"));
    // Surface the approve command first when tasks are waiting on a gate —
    // that's the action that actually unblocks the run.
    let awaiting: Vec<&str> = state
        .task_order
        .iter()
        .filter(|id| {
            state
                .tasks
                .get(*id)
                .is_some_and(|t| t.status == crate::scheduler::state::TaskStatus::AwaitingApproval)
        })
        .map(|s| s.as_str())
        .collect();
    for task_id in &awaiting {
        lines.push(colorize(&format!("  maestro approve {task_id}"), "33"));
    }
    lines.push(format!("  maestro runs replay {id}"));
    lines.push(format!("  maestro runs evidence {id}"));
    lines.push(format!("  maestro runs pr-body {id} --write"));
    lines
}

pub(crate) fn status_key(status: &crate::scheduler::state::TaskStatus) -> &'static str {
    use crate::scheduler::state::TaskStatus::*;
    match status {
        Pending => "pending",
        Running => "running",
        AwaitingApproval => "awaiting_approval",
        Done => "done",
        Failed => "failed",
        Skipped => "skipped",
        Cancelled => "cancelled",
    }
}

pub(crate) fn progress_bar(done: usize, total: usize, width: usize) -> String {
    let filled = if total == 0 {
        0
    } else {
        done.saturating_mul(width) / total
    };
    let bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled))
    );
    if done == total {
        colorize(&bar, "32")
    } else {
        colorize(&bar, "34")
    }
}

const DASHBOARD_DEFAULT_WIDTH: usize = 96;
const DASHBOARD_MIN_WIDTH: usize = 80;
const DASHBOARD_MAX_WIDTH: usize = 120;
const DASHBOARD_LABEL_WIDTH: usize = 10;

pub(crate) fn dashboard_width_from_columns(columns: Option<&str>) -> usize {
    columns
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DASHBOARD_DEFAULT_WIDTH)
        .clamp(DASHBOARD_MIN_WIDTH, DASHBOARD_MAX_WIDTH)
}

pub(crate) fn dashboard_percent(done: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    done.saturating_mul(100).saturating_div(total).min(100)
}

pub(crate) fn dashboard_label(text: &str) -> String {
    colorize(&format!("{text:<DASHBOARD_LABEL_WIDTH$}"), "90")
}

pub(crate) fn dashboard_rule(width: usize) -> String {
    colorize(&"─".repeat(width), "90")
}

pub(crate) fn truncate_middle(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 3 {
        return "...".chars().take(max).collect();
    }
    let left = (max - 3) / 2;
    let right = max - 3 - left;
    let start = s.chars().take(left).collect::<String>();
    let end = s
        .chars()
        .rev()
        .take(right)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{start}...{end}")
}

pub(crate) fn truncate_right(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    format!("{}…", s.chars().take(max - 1).collect::<String>())
}

pub(crate) fn status_symbol(status: &str) -> &'static str {
    match status {
        "done" => "✓",
        "running" => "⟳",
        "failed" => "✗",
        "awaiting_approval" => "⏸",
        "skipped" => "–",
        "cancelled" => "⊘",
        _ => "○",
    }
}

pub(crate) fn run_status_color(status: &str) -> &'static str {
    match status {
        "done" => "32",
        "running" => "34",
        "failed" => "31",
        "cancelled" => "90",
        _ => "37",
    }
}

pub(crate) fn task_status_color(status: &str) -> &'static str {
    match status {
        "done" => "32",
        "running" => "34",
        "failed" => "31",
        "awaiting_approval" => "33",
        "skipped" | "cancelled" => "90",
        _ => "37",
    }
}

pub(crate) fn colorize(text: &str, code: &str) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
        text.to_string()
    } else {
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

#[cfg(test)]
mod render_dashboard_tests {
    //! Snapshot-style tests for the TUI render. The render is the most
    //! visible UX surface in the terminal and has been edited several
    //! times; locking the visual contract keeps future refactors honest.
    //! Each test builds a minimal RunState fixture, calls
    //! `render_dashboard_lines`, and asserts on key strings — not exact
    //! whole-output equality (escape codes + widths make that brittle).
    use super::*;
    use crate::scheduler::evidence::{BrowserEvidenceSummary, RunEvidence};
    use crate::scheduler::state::{RunState, RunStatus, TaskState, TaskStatus};
    use chrono::Utc;
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;

    fn empty_evidence() -> RunEvidence {
        // RunEvidence has no Default impl; we build a zeroed one explicitly so
        // the test stays honest about what render_dashboard_lines actually
        // reads from evidence (parallel windows + browser summary).
        RunEvidence {
            run_id: "r-test".into(),
            spec: "demo spec".into(),
            status: "running".into(),
            verified: false,
            max_parallel: 4,
            max_observed_parallelism: 0,
            task_count: 0,
            tasks: vec![],
            parallel_windows: vec![],
            acceptance: vec![],
            artifact_refs: vec![],
            browser: BrowserEvidenceSummary::default(),
            auto_actions: vec![],
        }
    }

    fn task(id: &str, status: TaskStatus) -> TaskState {
        TaskState {
            id: id.into(),
            project: "demo".into(),
            agent: "shell".into(),
            status,
            started_at: None,
            ended_at: None,
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
            kind: "agent".into(),
            memory_used: vec![],
            context_bytes: None,
            skills_triggered: vec![],
            usage: None,
            steps: None,
            role: None,
            resolved_agent_profile: None,
            resolved_review_profile: None,
            workspace_path: Some("/tmp/demo".into()),
            worktree_path: None,
        }
    }

    fn state_with(tasks: &[(&str, TaskStatus)]) -> RunState {
        let mut s = RunState {
            run_id: "r-test".into(),
            spec: "demo spec".into(),
            started_at: Utc::now(),
            ended_at: None,
            status: RunStatus::Running,
            max_parallel: 4,
            pid: 0,
            tasks: BTreeMap::new(),
            approvals_pending: vec![],
            task_order: vec![],
            session_id: None,
            delivery_id: None,
            usage: Default::default(),
            budget_tokens: None,
            pending_gate: None,
            goal: None,
            acceptance_results: vec![],
            verified: false,
            auto_actions: vec![],
            run_dir: PathBuf::new(),
        };
        for (id, st) in tasks {
            s.tasks.insert((*id).into(), task(id, *st));
            s.task_order.push((*id).into());
        }
        s
    }

    fn render(state: &RunState, transitioned: &HashSet<String>) -> String {
        let evidence = empty_evidence();
        // Wide width + no row cap so nothing gets trimmed away under us.
        render_dashboard_lines(&state.run_id, state, &evidence, 120, None, transitioned).join("\n")
    }

    fn strip_ansi(s: &str) -> String {
        // Crude but enough for assertions: drop CSI sequences like \x1b[NN(m|K).
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // [
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn empty_run_shows_chrome_but_no_task_rows() {
        let s = state_with(&[]);
        let out = strip_ansi(&render(&s, &HashSet::new()));
        assert!(out.contains("maestro tui"), "header missing:\n{out}");
        assert!(out.contains("demo spec"), "spec missing:\n{out}");
        assert!(
            out.contains("0/1"),
            "progress line should report 0/1 even with no tasks:\n{out}"
        );
        assert!(!out.contains("T_"), "no task rows expected:\n{out}");
    }

    #[test]
    fn mixed_statuses_render_each_row_with_its_label() {
        let s = state_with(&[
            ("T_a", TaskStatus::Done),
            ("T_b", TaskStatus::Running),
            ("T_c", TaskStatus::Failed),
        ]);
        let out = strip_ansi(&render(&s, &HashSet::new()));
        // Each row carries the snake-cased status keyword (spaces, not underscores).
        for kw in ["done", "running", "failed"] {
            assert!(out.contains(kw), "missing status `{kw}`:\n{out}");
        }
        assert!(out.contains("T_a"), "T_a row missing");
        assert!(out.contains("T_b"), "T_b row missing");
        assert!(out.contains("T_c"), "T_c row missing");
    }

    #[test]
    fn transitioned_rows_get_a_flash_marker_and_bold() {
        let s = state_with(&[("T_a", TaskStatus::Done), ("T_b", TaskStatus::Running)]);
        let mut flash = HashSet::new();
        flash.insert("T_b".to_string());
        let raw = render(&s, &flash);
        // Bold ANSI present somewhere in the output for the flashed row.
        assert!(
            raw.contains("\x1b[1m"),
            "bold ANSI should appear when a row flashes"
        );
        // The ▸ marker should be on the T_b line specifically.
        let stripped = strip_ansi(&raw);
        let b_line = stripped
            .lines()
            .find(|l| l.contains("T_b"))
            .unwrap_or_else(|| panic!("T_b row missing:\n{stripped}"));
        assert!(
            b_line.contains("▸"),
            "T_b should carry the ▸ flash marker:\n{b_line}"
        );
        let a_line = stripped.lines().find(|l| l.contains("T_a")).unwrap();
        assert!(
            !a_line.contains("▸"),
            "T_a should NOT flash (not in set):\n{a_line}"
        );
    }

    #[test]
    fn awaiting_approval_surfaces_the_approve_command() {
        let s = state_with(&[("T_gate", TaskStatus::AwaitingApproval)]);
        let out = strip_ansi(&render(&s, &HashSet::new()));
        assert!(
            out.contains("maestro approve T_gate"),
            "next-actions should suggest approving the waiting task:\n{out}",
        );
        assert!(
            out.contains("awaiting approval"),
            "status keyword wrong:\n{out}"
        );
    }
}

#[cfg(test)]
mod runstate_serde_tests {
    //! Lock the back-compat invariant: a RunState JSON written by an
    //! older maestro (missing today's optional fields) must still
    //! deserialize cleanly. This is the central run-state contract and
    //! we have several `#[serde(default)]` fields whose protection is
    //! easy to break with a careless rename.
    use crate::scheduler::state::{RunState, RunStatus, TaskState, TaskStatus};

    #[test]
    fn older_minimal_taskstate_deserializes_with_defaults() {
        // Pre-2026 TaskState: just id/project/agent/status/log_path. Every
        // newer field carries `#[serde(default)]`; this test fires if anyone
        // accidentally drops the default attribute during a rename.
        let json = r#"{
            "id": "T_old",
            "project": "demo",
            "agent": "shell",
            "status": "done",
            "log_path": "T_old.log"
        }"#;
        let t: TaskState =
            serde_json::from_str(json).expect("older TaskState JSON must still deserialize");
        assert_eq!(t.id, "T_old");
        assert_eq!(t.status, TaskStatus::Done);
        assert_eq!(t.attempts, 0);
        assert_eq!(t.kind, ""); // default String — UI handles this
        assert!(t.depends_on.is_empty());
        assert!(t.memory_used.is_empty());
        assert!(t.skills_triggered.is_empty());
        assert!(t.role.is_none());
        assert!(t.workspace_path.is_none());
        assert!(t.worktree_path.is_none());
        assert!(t.usage.is_none());
        assert!(t.steps.is_none());
    }

    #[test]
    fn taskstate_roundtrip_preserves_optional_fields() {
        // The inverse safety: if a task DID carry the optional fields, they
        // must survive a serialize → deserialize cycle. Catches a future
        // `#[serde(skip)]` that would silently drop user data on state writes.
        use std::collections::BTreeMap;
        let original = TaskState {
            id: "T_full".into(),
            project: "demo".into(),
            agent: "codex".into(),
            status: TaskStatus::Running,
            started_at: Some(chrono::Utc::now()),
            ended_at: None,
            chat_id: Some("chat-7".into()),
            error: None,
            attempts: 2,
            risk_level: Some("low".into()),
            artifacts: Default::default(),
            permission: None,
            workflow_outputs: BTreeMap::new(),
            log_path: "T_full.log".into(),
            trajectory_path: Some("trajectory.jsonl".into()),
            depends_on: vec!["T_a".into()],
            parallel_group: Some("g1".into()),
            requires_approval_after: true,
            kind: "agent".into(),
            memory_used: vec!["topic/file.md".into()],
            context_bytes: Some(4096),
            skills_triggered: vec!["plan-validate".into()],
            usage: None,
            steps: Some(12),
            role: Some("backend".into()),
            resolved_agent_profile: None,
            resolved_review_profile: None,
            workspace_path: Some("/tmp/demo".into()),
            worktree_path: Some("/tmp/demo-wt".into()),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: TaskState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, original.id);
        assert_eq!(back.attempts, 2);
        assert_eq!(back.risk_level.as_deref(), Some("low"));
        assert_eq!(back.depends_on, vec!["T_a"]);
        assert_eq!(back.memory_used, vec!["topic/file.md"]);
        assert_eq!(back.skills_triggered, vec!["plan-validate"]);
        assert_eq!(back.steps, Some(12));
        assert_eq!(back.role.as_deref(), Some("backend"));
        assert_eq!(back.workspace_path.as_deref(), Some("/tmp/demo"));
        assert_eq!(back.worktree_path.as_deref(), Some("/tmp/demo-wt"));
        assert!(back.requires_approval_after);
    }

    #[test]
    fn older_minimal_json_deserializes_with_defaults() {
        // Pre-2026 shape: just the required fields + an empty tasks map.
        let json = r#"{
            "run_id": "r-old",
            "spec": "older run",
            "started_at": "2026-01-01T00:00:00Z",
            "ended_at": null,
            "status": "running",
            "max_parallel": 1,
            "tasks": {},
            "approvals_pending": [],
            "task_order": []
        }"#;
        let s: RunState =
            serde_json::from_str(json).expect("older RunState JSON must still deserialize");
        assert_eq!(s.run_id, "r-old");
        assert_eq!(s.status, RunStatus::Running);
        assert!(s.tasks.is_empty());
        // All later fields should default:
        assert_eq!(s.pid, 0);
        assert!(s.session_id.is_none());
        assert!(!s.verified);
        assert!(s.auto_actions.is_empty());
    }
}
