pub mod commands;
pub mod samples;
pub mod util;

use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};
use std::path::PathBuf;

// Built-in command bodies now live in `commands::builtins`; bring them into
// scope so the dispatch arms below read unchanged.
use commands::builtins::*;

#[derive(Parser, Debug)]
#[command(name = "maestro", version, about = "Multi-project Agent orchestrator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Initialize a .maestro/ directory in the current workspace.
    Init(InitArgs),

    /// Run first-time workspace setup: init, bundled skills, providers, models, and doctor.
    Setup(SetupArgs),

    /// Create and optionally run a zero-config two-task demo DAG.
    Demo(DemoArgs),

    /// Print which workspace maestro is using and what's inside it.
    ///
    /// Useful when you see "previous run data" you weren't expecting:
    /// `maestro where` shows the resolved workspace root (cwd or
    /// `$MAESTRO_WORKSPACE_ROOT`), whether `.maestro/` exists, recent
    /// runs, any leftover daemons listening, and the env vars that
    /// would override resolution.
    Where,

    /// Run workspace health diagnostics for config, tools, models, and run state.
    Doctor(DoctorArgs),

    /// List known agent providers, adapter coverage, and local install status.
    Providers(ProvidersArgs),

    /// Migrate a legacy `.mux/` directory (from before the rename) to
    /// `.maestro/`. Refuses to overwrite if both directories already
    /// exist unless `--force` is passed, in which case the existing
    /// `.maestro/` is moved aside to `.maestro.bak-<timestamp>/` first.
    /// File contents that mention "mux" are left alone — they're
    /// cosmetic labels (old REPORT titles, etc.) and don't affect
    /// runtime behavior.
    Migrate(MigrateArgs),

    /// Register a project under the current .maestro/.
    Add(AddArgs),

    /// One-command workflow: discover projects, synthesize a DAG, validate it, optionally run.
    Work(WorkArgs),

    /// Deliberate over a requirements document: each in-scope project states its
    /// position (changes, contracts, dependencies, concerns), conflicts are
    /// surfaced, and the resolved decomposition becomes the DAG. Records a
    /// transcript + posts positions to the coordination mailbox.
    Deliberate(DeliberateArgs),

    /// List registered projects.
    Ls,

    /// Inspect or build the code-graph engine (native / codegraph / understand).
    Codegraph(CodegraphArgs),

    /// Print a zero-token architecture brief of the workspace (types,
    /// contracts, dependency flow) — a quick "how does this fit together?".
    Brief,

    /// Remove a project by name.
    Remove(RemoveArgs),

    /// Validate projects.yaml.
    Validate,

    /// Run a PLAN.yaml against the registered projects.
    Run(RunArgs),

    /// Approve a task that is waiting on `requires_approval_after`.
    Approve(ApproveArgs),

    /// Show current run status.
    Status,

    /// Tail a task's log.
    Logs(LogsArgs),

    /// Start the read-only web dashboard.
    Ui(UiArgs),

    /// List the maestro services running on this host (UI servers, runs).
    #[command(visible_alias = "ps")]
    List,

    /// Stop running maestro services (by pid, --port, or --all).
    Stop(commands::services::StopArgs),

    /// Start a lightweight terminal dashboard for the current run.
    Tui(TuiArgs),

    /// List, inspect, and tail historical runs.
    Runs(RunsArgs),

    /// Rerun a finished run, optionally starting from a specific task.
    Rerun(RerunArgs),

    /// Resume an interrupted run (process died) without redoing completed work.
    Resume(ResumeArgs),

    /// Run a task on several agents and pick the best result (voting).
    Compare(CompareArgs),

    /// Manage L1 memory facts.
    #[command(subcommand)]
    Memory(MemoryCmd),

    /// Review and promote failure-driven learning proposals (guardrails).
    #[command(subcommand)]
    Learn(LearnCmd),

    /// Create GitHub pull request artifacts from a run.
    #[command(subcommand)]
    Pr(PrCmd),

    /// Scaffold a new PLAN.yaml draft and a Planner agent prompt.
    #[command(subcommand)]
    Plan(PlanCmd),

    /// Chat with the orchestrator Agent (Cursor-backed).
    #[command(subcommand)]
    Chat(ChatCmd),

    /// Local mailbox for cross-role and cross-project handoffs.
    #[command(subcommand)]
    Mailbox(MailboxCmd),

    /// Inspect and process external channel messages.
    #[command(subcommand)]
    Channels(ChannelsCmd),

    /// Run replay benchmark fixtures and scoring reports.
    #[command(subcommand)]
    Bench(BenchCmd),

    /// Manage skills (global + per-project markdown playbooks).
    #[command(subcommand)]
    Skill(SkillCmd),

    /// Cancel a running maestro run (the scheduler picks up the marker on its next tick).
    CancelRun(CancelRunArgs),

    /// Start `maestro ui` and open the dashboard in the default browser.
    Open(OpenArgs),

    /// List local sessions from other AI tools (read-only).
    History(HistoryArgs),

    /// List Cursor models (cached + optional refresh from `cursor-agent --list-models`).
    Models(ModelsArgs),

    /// Greenfield scaffolding: read an ARCHITECTURE.yaml and create N
    /// directories, init them as git repos, drop starter files + contract
    /// placeholders, and register everything in projects.yaml.
    Scaffold(ScaffoldArgs),

    /// Open the built-in user documentation site (or list/print pages).
    ///
    /// `maestro doc` with no subcommand launches the dashboard if needed and
    /// opens the browser on the docs tab. Use `maestro doc ls` for the TOC,
    /// `maestro doc show <page>` for stdout output.
    Doc(DocArgs),

    /// Inspect role personas the orchestrator can adopt per task.
    ///
    /// Roles bundle a system-prelude markdown + metadata. `maestro role ls`
    /// lists every builtin + workspace role; `maestro role show <name>`
    /// prints one role's body; `maestro role export <name>` copies a
    /// builtin into `.maestro/roles/` so you can customize it.
    Role(RoleArgs),

    /// Run a reviewer persona over the current git diff (or the whole
    /// workspace with --no-diff), then exit 0 on pass / 1 on fail.
    ///
    /// Designed for use inside `goal.acceptance[].check` so any role —
    /// including Cursor plugin personas like `ce-kieran-typescript-reviewer`
    /// — becomes one of the L4 verification gates.
    Review(ReviewArgs),

    /// Run maestro as an MCP server on stdio.
    ///
    /// Lets Cursor (or any MCP-capable agent) query maestro's run state,
    /// memory, and roles during a chat. Add to `~/.cursor/mcp.json`:
    ///
    ///   { "mcpServers": { "maestro": { "command": "maestro",
    ///                                  "args": ["mcp"] } } }
    ///
    /// Output is one JSON-RPC message per line on stdout; logs go to
    /// stderr to keep the stdio channel clean.
    Mcp,
}

#[derive(Parser, Debug)]
pub struct ReviewArgs {
    /// Reviewer / persona name. Resolved against `.maestro/roles/`,
    /// builtin roles, then `~/.cursor/plugins/.../agents/<name>.agent.md`.
    #[arg(long)]
    pub role: String,

    /// Git ref to diff against. Default `HEAD~1`. Ignored with `--no-diff`.
    #[arg(long, default_value = "HEAD~1")]
    pub since: String,

    /// Workspace directory the diff is computed in. Defaults to the
    /// resolved maestro workspace root.
    #[arg(long)]
    pub workspace: Option<std::path::PathBuf>,

    /// Skip diff capture and ask the reviewer to look at the project as
    /// a whole. Useful for architecture / docs / dependency reviewers.
    #[arg(long)]
    pub no_diff: bool,

    /// Render the review prompt to stdout and exit 0 without invoking
    /// the agent. Use to inspect the wiring without burning tokens.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RoleArgs {
    #[command(subcommand)]
    pub subcmd: RoleSubcmd,
}

#[derive(Parser, Debug)]
pub enum RoleSubcmd {
    /// List every role available in this workspace.
    Ls,
    /// Print one role's full prelude markdown.
    Show {
        /// Role name (e.g. backend_rust, frontend, designer).
        name: String,
    },
    /// Copy a builtin role into .maestro/roles/<name>.md so you can edit it.
    Export {
        /// Builtin role name to copy.
        name: String,
    },
}

#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DocArgs {
    #[command(subcommand)]
    pub subcmd: Option<DocSubcmd>,

    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value_t = 7777)]
    pub port: u16,

    /// Open a specific page by id (e.g. `quick-start`). See `maestro doc ls`.
    #[arg(long)]
    pub page: Option<String>,

    /// UI language for the docs site (e.g. `en`, `zh`). The browser also
    /// has a switcher; this just sets the initial value.
    #[arg(long)]
    pub lang: Option<String>,

    /// Don't actually open the browser, just print the URL.
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Subcommand, Debug)]
pub enum DocSubcmd {
    /// Print the docs table of contents to stdout.
    Ls {
        /// Print Chinese (or another) localised titles instead of English.
        #[arg(long)]
        lang: Option<String>,
    },
    /// Print a single page's raw markdown to stdout.
    Show {
        /// Page id, e.g. `quick-start`. Use `maestro doc ls` to list ids.
        page: String,
        #[arg(long)]
        lang: Option<String>,
    },
}

#[derive(Parser, Debug)]
pub struct ScaffoldArgs {
    /// Path to the ARCHITECTURE.yaml file.
    pub architecture: PathBuf,

    /// Workspace root under which to create the module directories.
    /// Defaults to the current directory.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Skip `git init` (e.g. when running inside an outer git repo).
    #[arg(long)]
    pub no_git: bool,

    /// Don't merge into `projects.yaml` — just create the files.
    #[arg(long)]
    pub no_register: bool,
}

#[derive(Parser, Debug)]
pub struct ModelsArgs {
    /// Re-fetch the model list from cursor-agent and overwrite the cache.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Parser, Debug)]
pub struct DoctorArgs {
    /// Print a machine-readable JSON report.
    #[arg(long)]
    pub json: bool,

    /// Include passing-check details in the text report.
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Parser, Debug)]
pub struct ProvidersArgs {
    /// Print machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    /// Only show providers that Maestro can run as task adapters today.
    #[arg(long)]
    pub adapters_only: bool,
}

#[derive(Parser, Debug)]
pub struct HistoryArgs {
    /// Filter by source: cursor | claude | codex.
    #[arg(long)]
    pub source: Option<String>,

    /// Maximum rows to show (default 25).
    #[arg(long, default_value_t = 25)]
    pub limit: usize,
}

#[derive(Parser, Debug)]
pub struct InitArgs {
    /// Pure scaffolding only: skip the bundled sample skills & memory facts AND
    /// the automatic project discovery. Default: seed samples and discover +
    /// register projects so the workspace is ready to `work` in one step.
    #[arg(long)]
    pub bare: bool,

    /// Scan the workspace immediately, infer project dependencies, and write a DAG plan.
    #[arg(long, conflicts_with = "no_analyze")]
    pub analyze: bool,

    /// Do not prompt for or run dependency analysis during init.
    #[arg(long, conflicts_with = "analyze")]
    pub no_analyze: bool,

    /// Default agent assigned to projects discovered during init analysis.
    #[arg(long, default_value = "codex")]
    pub agent: String,

    /// Root directory to scan when init analysis runs. Defaults to the workspace root.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Maximum directory depth for project discovery during init analysis.
    #[arg(long, default_value_t = 3)]
    pub max_depth: usize,

    /// Path for the generated dependency DAG plan.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Restrict the analysis audit to these projects (comma-separated or
    /// repeated `--project`). Default: audit every discovered project. Use
    /// this to scope an audit on a large workspace.
    #[arg(long = "project", value_delimiter = ',')]
    pub projects: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct SetupArgs {
    /// Skip bundled sample skills and memory facts during init.
    #[arg(long)]
    pub bare: bool,

    /// Overwrite existing bundled skill files with the current built-in copy.
    #[arg(long)]
    pub force_skills: bool,

    /// Refresh the Cursor model cache during setup.
    #[arg(long)]
    pub refresh_models: bool,

    /// Skip the final doctor diagnostics pass.
    #[arg(long)]
    pub skip_doctor: bool,

    /// Return a failing exit code if doctor reports failures.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct DemoArgs {
    /// Directory to create. Defaults to ./maestro-demo.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Execute the shell-only DAG instead of rendering a dry-run preview.
    #[arg(long)]
    pub run: bool,
}

#[derive(Parser, Debug)]
pub struct MigrateArgs {
    /// When `.maestro/` already exists, back it up to `.maestro.bak-<ts>/`
    /// and proceed. Without this flag, migrate refuses to clobber.
    ///
    /// An exception: if the existing `.maestro/` is essentially empty
    /// (no runs, no projects.yaml, no chat sessions — i.e. it's a fresh
    /// `maestro init` stub), it's auto-replaced without needing --force.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Local path to the repo.
    pub path: String,

    /// Logical project name. Defaults to the directory basename.
    #[arg(long)]
    pub name: Option<String>,

    /// Project type tag (backend, frontend, mobile, ...).
    #[arg(long, alias = "type")]
    pub r#type: Option<String>,

    /// Comma-separated stack tags.
    #[arg(long, value_delimiter = ',')]
    pub stack: Vec<String>,

    /// Which Agent adapter to use for this project. Defaults to global default.
    #[arg(long)]
    pub agent: Option<String>,

    /// Default role for tasks in this project (e.g. backend_rust, frontend,
    /// mobile_rn, game_unity, game_cocos). Run `maestro role ls` to see all
    /// builtin and user-defined roles.
    #[arg(long)]
    pub role: Option<String>,
}

#[derive(Parser, Debug)]
pub struct DeliberateArgs {
    /// Path to the requirements document (markdown/plain text).
    pub doc: PathBuf,

    /// Optional folder to scan before deliberating. Discovered projects are applied.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Maximum directory depth to scan below --root.
    #[arg(long, default_value_t = 3)]
    pub max_depth: usize,

    /// Agent adapter to set on discovered projects.
    #[arg(long, default_value = "codex")]
    pub agent: String,

    /// Limit the deliberation to one or more project ids.
    #[arg(long = "project", value_delimiter = ',')]
    pub projects: Vec<String>,

    /// Enable LLM discussion mode with the given local agent backend
    /// (`codex` | `claude` | `cursor`). Each project's agent states its position
    /// via the real CLI and the takes are posted live to the coordination
    /// mailbox. Omit for the deterministic, zero-token planner.
    #[arg(long)]
    pub discuss: Option<String>,

    /// After deliberating + synthesizing the plan, run it immediately.
    #[arg(long)]
    pub run: bool,

    /// Build prompts/dry-run artifacts instead of dispatching agents.
    #[arg(long)]
    pub dry: bool,

    /// Maximum number of concurrent tasks when using --run/--dry.
    #[arg(long)]
    pub max_parallel: Option<usize>,

    /// Token budget for the run (escalate + stop when crossed).
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// Boundary review gates (start/end/both). Default `plan` — deliberation
    /// produces a plan you approve before it runs.
    #[arg(long, value_enum, default_value_t = ReviewGates::Plan)]
    pub gates: ReviewGates,
}

#[derive(Parser, Debug)]
pub struct WorkArgs {
    /// One-line goal to turn into a dependency-aware workflow.
    pub spec: String,

    /// Optional folder to scan before planning. When present, discovered projects are applied.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Maximum directory depth to scan below --root.
    #[arg(long, default_value_t = 3)]
    pub max_depth: usize,

    /// Agent adapter to set on discovered projects.
    #[arg(long, default_value = "codex")]
    pub agent: String,

    /// Output PLAN.yaml path. Defaults to plans/<date>-<slug>.yaml.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Limit the workflow to one or more project ids after discovery.
    #[arg(long = "project", value_delimiter = ',')]
    pub projects: Vec<String>,

    /// After generating and validating the plan, run it immediately.
    #[arg(long)]
    pub run: bool,

    /// Start a new run even when a matching previous run still appears active.
    #[arg(long)]
    pub force_new: bool,

    /// Build prompts and dry-run artifacts instead of dispatching agents.
    #[arg(long)]
    pub dry: bool,

    /// Maximum number of concurrent tasks when using --run or --dry.
    #[arg(long)]
    pub max_parallel: Option<usize>,

    /// Agent model override when using --run or --dry.
    #[arg(long)]
    pub model: Option<String>,

    /// Token budget for the run (escalate + stop when crossed).
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// Boundary review gates (anti decision-fatigue): concentrate approval at
    /// the start (`plan`), the end (`outcome`), or `both`, instead of on every
    /// task. Default `none`.
    #[arg(long, value_enum, default_value_t = ReviewGates::None)]
    pub gates: ReviewGates,
}

/// Where to place human-approval gates for a run. See `WorkArgs::gates`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewGates {
    /// No run-level gates (per-task `requires_approval_after` still applies).
    #[default]
    None,
    /// Approve the plan before any task runs (intent boundary).
    Plan,
    /// Approve the result after the work + acceptance checks (outcome boundary).
    Outcome,
    /// Both the plan and outcome boundaries.
    Both,
}

impl ReviewGates {
    /// `(plan_gate, outcome_gate)` for `ExecConfig`.
    pub fn flags(self) -> (bool, bool) {
        match self {
            ReviewGates::None => (false, false),
            ReviewGates::Plan => (true, false),
            ReviewGates::Outcome => (false, true),
            ReviewGates::Both => (true, true),
        }
    }
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    pub name: String,
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    pub plan: PathBuf,

    /// Only run these task ids (comma separated). Other tasks are skipped.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,

    /// Skip these task ids.
    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,

    /// Maximum number of concurrent tasks. Overrides projects.yaml default.
    #[arg(long)]
    pub max_parallel: Option<usize>,

    /// Continue after a task failure instead of aborting the run.
    #[arg(long)]
    pub continue_on_error: bool,

    /// Originating chat session id (also accepts MAESTRO_SESSION_ID env var).
    #[arg(long)]
    pub session_id: Option<String>,

    /// Agent model override applied to every task in this run (per-task
    /// `model:` in the PLAN.yaml still wins).
    #[arg(long)]
    pub model: Option<String>,

    /// Don't actually dispatch agents — assemble each task's full prompt
    /// (with memory + skills + contracts) and write them to
    /// `.maestro/runs/<id>/dry/<task>.prompt.md` instead. Use this to
    /// review what would be sent before burning real tokens.
    #[arg(long)]
    pub dry: bool,

    /// Don't auto-add the producer→consumer dependency edges implied by
    /// declared contracts. By default maestro wires them so consumers can't
    /// race ahead of a contract change; pass this to run the plan verbatim.
    #[arg(long)]
    pub no_wire_contracts: bool,

    /// Token budget for the run. When cumulative agent usage crosses it,
    /// maestro escalates and stops (unless --continue-on-error), so a run can't
    /// silently burn unbounded tokens.
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// Boundary review gates (anti decision-fatigue): concentrate approval at
    /// the start (`plan`), the end (`outcome`), or `both`, instead of on every
    /// task. Default `none`.
    #[arg(long, value_enum, default_value_t = ReviewGates::None)]
    pub gates: ReviewGates,

    /// Suppress the plan-impact preview (set when `maestro work` already
    /// printed it during validation, to avoid a duplicate). Not a CLI flag.
    #[clap(skip)]
    pub quiet_impact: bool,
}

#[derive(Parser, Debug)]
pub struct ApproveArgs {
    pub task_id: String,
}

#[derive(Parser, Debug)]
pub struct LogsArgs {
    pub task_id: String,

    #[arg(long, default_value_t = 200)]
    pub tail: usize,

    #[arg(long)]
    pub run: Option<String>,

    /// Follow new lines as they are appended (like tail -f).
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CodegraphArgs {
    #[command(subcommand)]
    pub subcmd: Option<CodegraphSubcmd>,
}

#[derive(Subcommand, Debug)]
pub enum CodegraphSubcmd {
    /// Show which code-graph engines are available, built, and active.
    Status,
    /// Build a richer code graph. `--engine codegraph` runs the tree-sitter
    /// indexer here (free); `--engine understand` prints how to run the
    /// LLM-based Understand-Anything graph (uses tokens).
    Build {
        /// Which engine to build: `codegraph` | `understand` | `native`.
        #[arg(long, default_value = "codegraph")]
        engine: String,
        /// Root to index. Defaults to the workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RunsArgs {
    #[command(subcommand)]
    pub subcmd: Option<RunsSubcmd>,
}

#[derive(Subcommand, Debug)]
pub enum RunsSubcmd {
    /// List historical runs.
    Ls,
    /// Show a run summary. Defaults to the current run.
    Show {
        /// Run id, or "current". Defaults to current.
        run_id: Option<String>,
    },
    /// Print the append-only run event stream.
    Events {
        /// Run id, or "current". Defaults to current.
        run_id: Option<String>,

        /// Number of historical events to print before returning/following.
        #[arg(long, default_value_t = 200)]
        tail: usize,

        /// Follow new events as they are appended.
        #[arg(short, long)]
        follow: bool,

        /// Print each event as raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show the durable evidence summary for a run.
    Evidence {
        /// Run id, or "current". Defaults to current.
        run_id: Option<String>,

        /// Print the evidence summary as raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reconstruct a run timeline from events + evidence.
    Replay {
        /// Run id, or "current". Defaults to current.
        run_id: Option<String>,

        /// Print the replay as raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Generate a draft PR body from report, acceptance, and evidence.
    PrBody {
        /// Run id, or "current". Defaults to current.
        run_id: Option<String>,

        /// Write PR_BODY.md under the run directory instead of printing only.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Parser, Debug)]
pub struct UiArgs {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value_t = 7777)]
    pub port: u16,
}

#[derive(Parser, Debug)]
pub struct TuiArgs {
    /// Run id, or "current". Defaults to current.
    #[arg(long)]
    pub run: Option<String>,

    /// Render once and exit instead of refreshing.
    #[arg(long)]
    pub once: bool,

    /// Refresh interval for the terminal dashboard.
    #[arg(long, default_value_t = 1000)]
    pub interval_ms: u64,

    /// Launch the interactive ratatui frontend (j/k navigate tasks,
    /// chat with the orchestrator, approve / reject inline actions,
    /// see the diff side-panel + live token & context meters).
    /// As of the Phase 5 chat-tui rollout this is the DEFAULT when
    /// stdin and stdout are both real terminals; the flag is kept as
    /// an explicit opt-in for callers that want to force interactive
    /// mode (e.g. when stdin is a pipe but stdout is the user's
    /// terminal).
    #[arg(short = 'i', long = "interactive")]
    pub interactive: bool,

    /// Force the classic auto-refreshing line-printer dashboard even
    /// in a real terminal. Useful when piping the TUI output through
    /// a recorder or when an old terminal can't handle ratatui's
    /// alternate-screen + raw-mode setup. Mutually exclusive with
    /// `--interactive` and implied by `--once`.
    #[arg(long = "classic")]
    pub classic: bool,
}

#[derive(Parser, Debug)]
pub struct RerunArgs {
    pub plan: PathBuf,

    /// Start from this task id; completed tasks outside its downstream slice
    /// are seeded as skipped. Useful when iterating after a fix.
    #[arg(long)]
    pub from: Option<String>,

    /// Only rerun these task ids.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,

    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,

    #[arg(long)]
    pub max_parallel: Option<usize>,
}

/// `maestro compare`: run the same task on several agents and pick the best.
#[derive(Parser, Debug)]
pub struct CompareArgs {
    /// The task prompt to give every agent.
    pub prompt: String,

    /// Project to run in (must declare a `check` command).
    #[arg(long)]
    pub project: String,

    /// Agents to compare, e.g. `--agents codex,cursor,mock`.
    #[arg(long, value_delimiter = ',')]
    pub agents: Vec<String>,

    /// Per-agent timeout in minutes.
    #[arg(long, default_value_t = 20)]
    pub timeout_minutes: u64,

    /// Keep all candidate worktrees (default: keep only the winner's).
    #[arg(long)]
    pub keep: bool,
}

/// `maestro resume`: continue an interrupted run (process died mid-flight)
/// without redoing completed work.
#[derive(Parser, Debug)]
pub struct ResumeArgs {
    /// Run id to resume. Defaults to the current run.
    #[arg(long)]
    pub run: Option<String>,

    #[arg(long)]
    pub max_parallel: Option<usize>,

    /// Resume even if the run still looks alive (its pid responds).
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct CancelRunArgs {
    /// Run id to cancel. Defaults to the current run.
    pub run_id: Option<String>,
}

#[derive(Parser, Debug)]
pub struct OpenArgs {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value_t = 7777)]
    pub port: u16,

    /// Don't actually open the browser, just print the URL.
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// List all skills grouped by scope.
    Ls,
    /// Mirror every skill into `.cursor/rules/*.mdc` and `.claude/skills/*/SKILL.md`
    /// so Cursor IDE and Claude Code pick them up natively.
    Sync,
    /// Install or refresh bundled Maestro skills. Existing local edits are skipped unless --force.
    Update {
        /// Optional bundled skill name, e.g. `qa-web-flow` or `_global/qa-web-flow`.
        name: Option<String>,
        /// Overwrite existing local skill files with the bundled copy.
        #[arg(long)]
        force: bool,
    },
    /// Print one skill's content.
    Show {
        /// Scope: `_global` or a project name.
        scope: String,
        name: String,
    },
    /// Create or replace a skill from a local file.
    Add {
        scope: String,
        name: String,
        file: PathBuf,
    },
    /// Create a fresh skill skeleton in `.maestro/skills/<scope>/<name>.md` and print its path.
    New {
        scope: String,
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a skill.
    Rm { scope: String, name: String },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCmd {
    /// List facts grouped by topic.
    #[command(alias = "ls")]
    List,
    /// Print the content of one fact file.
    Show { topic: String, name: String },
    /// Add a fact: `maestro memory add api openapi.yaml ./openapi.yaml`
    Add {
        topic: String,
        name: String,
        file: PathBuf,
    },
    /// Print the resolved memory directory.
    Path,
}

#[derive(Subcommand, Debug)]
pub enum LearnCmd {
    /// List learning proposals (failure-driven guardrails), highest recurrence first.
    #[command(alias = "ls")]
    List,
    /// Show one proposal's full body + provenance.
    Show { fingerprint: String },
    /// Promote a proposal into a live skill (reviewable; mirrors to .cursor/.claude).
    /// A trigger is required so the guardrail can't fire on unrelated tasks.
    Promote {
        fingerprint: String,
        /// Skill scope: `global` (default) or a project name. Defaults to the
        /// proposal's own scope.
        #[arg(long)]
        scope: Option<String>,
        /// The phrase the promoted skill fires on. Required when the proposal
        /// has no (conservative) suggested trigger.
        #[arg(long)]
        trigger: Option<String>,
    },
    /// Reject a proposal (kept as an audit record; never re-proposed).
    Reject { fingerprint: String },
    /// Scan archived L2 decisions for near-duplicate clusters and draft
    /// memory-curation proposals (review/promote like the others).
    ScanMemory,
}

#[derive(Subcommand, Debug)]
pub enum PrCmd {
    /// Create a GitHub draft PR from a run's PR body.
    Draft(PrDraftArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct PrDraftArgs {
    /// Run id, or "current". Defaults to current.
    #[arg(long)]
    pub run: Option<String>,
    /// Project to draft from when a run has multiple completed projects.
    #[arg(long)]
    pub project: Option<String>,
    /// GitHub repo owner/name. Defaults to gh's current repo inference.
    #[arg(long)]
    pub repo: Option<String>,
    /// Base branch. Defaults to main.
    #[arg(long)]
    pub base: Option<String>,
    /// Push the head branch to the remote before opening the PR. `gh pr create`
    /// needs the branch on the remote; no-ops with a warning if there's none.
    #[arg(long)]
    pub push: bool,
}

#[derive(Subcommand, Debug)]
pub enum ChatCmd {
    /// List chat sessions.
    #[command(alias = "list")]
    Ls,
    /// Create a fresh session and switch to it.
    New,
    /// Switch the current session.
    Switch { id: String },
    /// Delete a session.
    Rm { id: String },
    /// Send a one-shot message to the current session and stream the reply.
    Send {
        text: String,
        /// Per-message Cursor model override.
        #[arg(long)]
        model: Option<String>,
        /// Chat provider for this message: cursor, codex, or claude.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Pin a model to a session (UI dropdown writes this). Use empty string to clear.
    Model { id: String, model: String },
    /// Print the full message history of the current session.
    Show {
        #[arg(long)]
        tail: Option<usize>,
    },
    /// Write a local handoff brief for continuing this session elsewhere.
    Brief {
        /// Session id prefix. Defaults to current session.
        id: Option<String>,
        /// Output path. Defaults to `.maestro/chat/continuations/<id>.brief.md`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print the brief to stdout instead of writing a file.
        #[arg(long)]
        stdout: bool,
    },
    /// Open an interactive REPL on the current session.
    Repl,
    /// Add a tag to a session (creates if missing).
    Tag { id: String, tag: String },
    /// Remove a tag.
    Untag { id: String, tag: String },
    /// Ask the AI tagger to label one session (or all) using only user messages.
    AutoTag {
        /// Session id prefix. Use `--all` to tag every session that has none.
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PlanCmd {
    /// Validate a PLAN.yaml.
    Validate { plan: PathBuf },
    /// Print the stable hash used by guarded maestro-action blocks.
    Hash { plan: PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum MailboxCmd {
    /// Send a message to another role, project, or task owner.
    Send {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        /// Mark blocking: the recipient must resolve it before proceeding.
        #[arg(long)]
        blocking: bool,
    },
    /// List mailbox messages. Defaults to open messages only.
    Ls {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Show a full message by id or unique prefix.
    Show { id: String },
    /// Mark a message resolved.
    Resolve {
        id: String,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ChannelsCmd {
    /// Read one external channel message and print the route decision.
    Listen(ListenArgs),
    /// Drain outbound channel replies for a run.
    Drain(DrainArgs),
}

/// ListenArgs covers both stdin and poll-based channel ingestion.
#[derive(Parser, Debug, Clone)]
pub struct ListenArgs {
    #[arg(long)]
    pub once: bool,
    #[arg(long, requires = "once")]
    pub from_stdin: bool,
    #[arg(long, conflicts_with_all = ["once", "from_stdin"])]
    pub poll: bool,
    #[arg(long)]
    pub channel: Option<String>,
    #[arg(long, default_value = "5")]
    pub interval_secs: u64,
    #[arg(long, default_value = "20")]
    pub poll_limit: usize,
}

#[derive(Parser, Debug, Clone)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .args(["once", "watch"])
))]
pub struct DrainArgs {
    #[arg(long)]
    pub run_dir: PathBuf,
    #[arg(long, conflicts_with = "watch")]
    pub once: bool,
    #[arg(long, conflicts_with = "once")]
    pub watch: bool,
    #[arg(long, default_value = "5")]
    pub interval_secs: u64,
}

#[derive(Subcommand, Debug)]
pub enum BenchCmd {
    /// List all benchmark fixtures.
    List,
    /// Run one benchmark fixture.
    Run {
        id: String,
        #[arg(long)]
        offline: bool,
    },
    /// Run all benchmark fixtures.
    All {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        offline: bool,
    },
    /// Populate local cache for replay benchmark fixtures.
    Hydrate {
        /// Hydrate only one fixture id.
        #[arg(long)]
        fixture: Option<String>,
        /// Hydrate only one fixture kind, e.g. oss-replay.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Print a historical bench run result.
    Report { run_id: String },
}

/// Shared binary entrypoint: install tracing, parse argv, dispatch. Both the
/// `maestro` binary and its short `mst` alias call this so they stay identical.
pub async fn main_entry() -> Result<()> {
    use clap::Parser;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,maestro=debug".into()),
        )
        .with_target(false)
        .init();

    run_cli(Cli::parse()).await
}

pub async fn run_cli(cli: Cli) -> Result<()> {
    // No subcommand → a state-aware "what should I run next?" homepage instead
    // of a 20-command wall of clap help.
    let Some(command) = cli.command else {
        return cmd_home();
    };
    match command {
        Cmd::Init(a) => cmd_init(a),
        Cmd::Setup(a) => cmd_setup(a).await,
        Cmd::Demo(a) => commands::demo::run(&a).await,
        Cmd::Where => cmd_where(),
        Cmd::Doctor(a) => commands::doctor::run(a).await,
        Cmd::Providers(a) => commands::providers::run(a),
        Cmd::Migrate(a) => cmd_migrate(a),
        Cmd::Add(a) => cmd_add(a),
        Cmd::Work(a) => commands::work::run(a).await,
        Cmd::Deliberate(a) => commands::deliberate::run(a).await,
        Cmd::Ls => cmd_ls(),
        Cmd::Codegraph(a) => cmd_codegraph(a),
        Cmd::Brief => cmd_brief(),
        Cmd::Remove(a) => cmd_remove(a),
        Cmd::Validate => cmd_validate(),
        Cmd::Run(a) => commands::run::run(a).await,
        Cmd::Rerun(a) => commands::run::rerun(a).await,
        Cmd::Resume(a) => commands::run::resume(a).await,
        Cmd::Compare(a) => commands::compare::compare(a).await,
        Cmd::Approve(a) => cmd_approve(a),
        Cmd::Status => cmd_status(),
        Cmd::Logs(a) => cmd_logs(a).await,
        Cmd::Ui(a) => cmd_ui(a).await,
        Cmd::List => commands::services::cmd_list(),
        Cmd::Stop(a) => commands::services::cmd_stop(a),
        Cmd::Tui(a) => cmd_tui(a).await,
        Cmd::Runs(a) => commands::runs::run(a).await,
        Cmd::Memory(a) => commands::memory::run(a),
        Cmd::Learn(a) => commands::learn::run(a),
        Cmd::Pr(a) => match a {
            PrCmd::Draft(args) => commands::pr::draft(args).await,
        },
        Cmd::Plan(a) => commands::plan::run(a),
        Cmd::Chat(a) => commands::chat::run(a).await,
        Cmd::Mailbox(a) => commands::mailbox::run(a),
        Cmd::Channels(a) => commands::channels::run(a).await,
        Cmd::Bench(a) => commands::bench::run(a).await,
        Cmd::Skill(a) => commands::skills::run(a),
        Cmd::CancelRun(a) => cmd_cancel_run(a),
        Cmd::Open(a) => cmd_open(a).await,
        Cmd::History(a) => cmd_history(a).await,
        Cmd::Models(a) => commands::models::run(a).await,
        Cmd::Scaffold(a) => cmd_scaffold(a),
        Cmd::Doc(a) => commands::doc::run(a).await,
        Cmd::Role(a) => match a.subcmd {
            RoleSubcmd::Ls => commands::role::cmd_role_ls(),
            RoleSubcmd::Show { name } => commands::role::cmd_role_show(&name),
            RoleSubcmd::Export { name } => commands::role::cmd_role_export(&name),
        },
        Cmd::Review(a) => {
            commands::review::run(commands::review::ReviewArgs {
                role: a.role,
                since: Some(a.since),
                workspace: a.workspace,
                no_diff: a.no_diff,
                dry_run: a.dry_run,
            })
            .await
        }
        Cmd::Mcp => crate::mcp::run().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_width_defaults_and_clamps_columns() {
        assert_eq!(dashboard_width_from_columns(None), 96);
        assert_eq!(dashboard_width_from_columns(Some("40")), 80);
        assert_eq!(dashboard_width_from_columns(Some("101")), 101);
        assert_eq!(dashboard_width_from_columns(Some("180")), 120);
        assert_eq!(dashboard_width_from_columns(Some("not-a-number")), 96);
    }

    #[test]
    fn dashboard_percent_handles_empty_and_finished_runs() {
        assert_eq!(dashboard_percent(0, 0), 0);
        assert_eq!(dashboard_percent(1, 4), 25);
        assert_eq!(dashboard_percent(4, 4), 100);
    }

    #[test]
    fn init_accepts_analyze_agent_root_and_depth_flags() {
        let cli = Cli::try_parse_from([
            "maestro",
            "init",
            "--analyze",
            "--agent",
            "claude",
            "--root",
            "examples",
            "--max-depth",
            "4",
        ])
        .expect("parse init args");

        let Some(Cmd::Init(args)) = cli.command else {
            panic!("expected init command");
        };
        assert!(args.analyze);
        assert!(!args.no_analyze);
        assert_eq!(args.agent, "claude");
        assert_eq!(args.root, Some(PathBuf::from("examples")));
        assert_eq!(args.max_depth, 4);
    }

    #[test]
    fn work_accepts_force_new_rerun_override() {
        let cli = Cli::try_parse_from(["maestro", "work", "same goal", "--run", "--force-new"])
            .expect("parse work args");

        let Some(Cmd::Work(args)) = cli.command else {
            panic!("expected work command");
        };
        assert_eq!(args.spec, "same goal");
        assert!(args.run);
        assert!(args.force_new);
    }

    #[test]
    fn review_gates_map_to_exec_flags() {
        assert_eq!(ReviewGates::None.flags(), (false, false));
        assert_eq!(ReviewGates::Plan.flags(), (true, false));
        assert_eq!(ReviewGates::Outcome.flags(), (false, true));
        assert_eq!(ReviewGates::Both.flags(), (true, true));
    }

    #[test]
    fn run_parses_gates_both() {
        let cli = Cli::try_parse_from(["maestro", "run", "PLAN.yaml", "--gates", "both"])
            .expect("parse run args");
        let Some(Cmd::Run(args)) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(args.gates, ReviewGates::Both);
    }

    #[test]
    fn listen_args_parse_with_once_and_from_stdin() {
        let cli = Cli::try_parse_from(["maestro", "channels", "listen", "--once", "--from-stdin"])
            .expect("parse channels listen args");

        let Some(Cmd::Channels(ChannelsCmd::Listen(args))) = cli.command else {
            panic!("expected channels listen command");
        };
        assert!(args.once);
        assert!(args.from_stdin);
        assert!(!args.poll);
        assert_eq!(args.interval_secs, 5);
        assert_eq!(args.poll_limit, 20);
    }

    #[test]
    fn listen_args_parse_with_poll() {
        let cli = Cli::try_parse_from([
            "maestro",
            "channels",
            "listen",
            "--poll",
            "--channel",
            "feishu",
            "--interval-secs",
            "2",
            "--poll-limit",
            "3",
        ])
        .expect("parse poll args");

        let Some(Cmd::Channels(ChannelsCmd::Listen(args))) = cli.command else {
            panic!("expected channels listen command");
        };
        assert!(args.poll);
        assert_eq!(args.channel.as_deref(), Some("feishu"));
        assert_eq!(args.interval_secs, 2);
        assert_eq!(args.poll_limit, 3);
    }

    #[test]
    fn listen_args_reject_poll_with_from_stdin() {
        assert!(Cli::try_parse_from([
            "maestro",
            "channels",
            "listen",
            "--poll",
            "--once",
            "--from-stdin",
        ])
        .is_err());
    }

    #[test]
    fn drain_args_parse_once_and_reject_conflicting_modes() {
        let cli = Cli::try_parse_from([
            "maestro",
            "channels",
            "drain",
            "--run-dir",
            "/tmp/run",
            "--once",
        ])
        .expect("parse drain args");

        let Some(Cmd::Channels(ChannelsCmd::Drain(args))) = cli.command else {
            panic!("expected channels drain command");
        };
        assert_eq!(args.run_dir, PathBuf::from("/tmp/run"));
        assert!(args.once);
        assert!(!args.watch);

        assert!(Cli::try_parse_from([
            "maestro",
            "channels",
            "drain",
            "--run-dir",
            "/tmp/run",
            "--once",
            "--watch",
        ])
        .is_err());
    }

    #[test]
    fn bench_hydrate_args_parse_fixture_and_kind_filters() {
        let cli = Cli::try_parse_from([
            "maestro",
            "bench",
            "hydrate",
            "--fixture",
            "fixture-one",
            "--kind",
            "oss-replay",
        ])
        .expect("parse bench hydrate args");

        let Some(Cmd::Bench(BenchCmd::Hydrate { fixture, kind })) = cli.command else {
            panic!("expected bench hydrate command");
        };
        assert_eq!(fixture.as_deref(), Some("fixture-one"));
        assert_eq!(kind.as_deref(), Some("oss-replay"));
    }

    #[test]
    fn init_analysis_mode_prompts_only_for_interactive_default() {
        let args = InitArgs {
            bare: false,
            analyze: false,
            no_analyze: false,
            agent: "codex".into(),
            root: None,
            max_depth: 3,
            out: None,
            projects: vec![],
        };

        assert_eq!(init_analysis_mode(&args, true), InitAnalysisMode::Prompt);
        assert_eq!(init_analysis_mode(&args, false), InitAnalysisMode::Skip);
    }

    #[test]
    fn init_analysis_mode_honors_explicit_flags() {
        let mut args = InitArgs {
            bare: false,
            analyze: true,
            no_analyze: false,
            agent: "codex".into(),
            root: None,
            max_depth: 3,
            out: None,
            projects: vec![],
        };
        assert_eq!(init_analysis_mode(&args, false), InitAnalysisMode::Run);

        args.analyze = false;
        args.no_analyze = true;
        assert_eq!(init_analysis_mode(&args, true), InitAnalysisMode::Skip);
    }

    #[test]
    fn approve_guidance_flags_unknown_task_and_confirms_awaiting() {
        let awaiting = vec!["T_change_shared".to_string()];
        // Typo'd / unknown id while something is awaiting → name the real ones.
        let unknown = approve_guidance(false, &awaiting, "T_typo").unwrap();
        assert!(unknown.contains("no task `T_typo`"));
        assert!(unknown.contains("T_change_shared"));
        // Unknown id and nothing awaiting → say so.
        let none = approve_guidance(false, &[], "T_x").unwrap();
        assert!(none.contains("nothing is awaiting approval"));
        // Known + awaiting → confirm the run continues.
        assert!(approve_guidance(true, &awaiting, "T_change_shared")
            .unwrap()
            .contains("run will now continue"));
        // Known but not awaiting (valid pre-approval) → no nag.
        assert!(approve_guidance(true, &[], "T_future").is_none());
    }

    #[test]
    fn home_lines_adapt_to_workspace_state() {
        let fresh = home_lines(false, 0, false).join("\n");
        assert!(fresh.contains("isn't set up yet"));
        assert!(fresh.contains("maestro init"));

        let empty = home_lines(true, 0, false).join("\n");
        assert!(empty.contains("no projects registered"));
        assert!(empty.contains("maestro init --analyze"));

        let ready = home_lines(true, 2, false).join("\n");
        assert!(ready.contains("2 project(s) registered"));
        assert!(ready.contains("maestro work"));
        assert!(
            !ready.contains("runs ls"),
            "no runs yet → no past-runs hint"
        );

        assert!(home_lines(true, 2, true)
            .join("\n")
            .contains("maestro runs ls"));
    }

    #[test]
    fn init_next_steps_guide_matches_project_state() {
        // Empty workspace → tells the user how to discover/add projects.
        let empty = init_next_steps_lines(0, ".").join("\n");
        assert!(empty.contains("no projects registered yet"));
        assert!(empty.contains("maestro init --analyze"));
        assert!(empty.contains("maestro add"));

        // Populated workspace → tells the user how to start a run.
        let populated = init_next_steps_lines(3, ".").join("\n");
        assert!(populated.contains("3 project(s) registered"));
        assert!(populated.contains("maestro work"));
        assert!(!populated.contains("no projects registered"));
    }

    #[test]
    fn setup_completion_text_distinguishes_doctor_issues() {
        assert_eq!(setup_completion_text(false), "✓ Setup complete.");
        assert_eq!(
            setup_completion_text(true),
            "✓ Setup complete with doctor issues."
        );
    }

    #[test]
    fn setup_next_steps_include_concrete_example_and_placeholder_form() {
        let steps = setup_next_steps();

        assert!(steps.contains("maestro demo --run"));
        assert!(steps.contains("maestro work \"<goal>\" --root /path/to/your/projects"));
        assert!(steps.contains("maestro work \"update README\" --root examples --agent mock --dry"));
    }
}

// `maestro rerun` lives in cli::commands::run
// `maestro plan` lives in cli::commands::plan
