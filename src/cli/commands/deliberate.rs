//! `maestro deliberate <requirements.md>` — the deliberation workflow.
//!
//! Reads a requirements document, has each in-scope project state its position
//! (changes, contracts, dependencies, concerns), surfaces contract conflicts,
//! records an auditable transcript + posts the positions to the coordination
//! mailbox, then synthesizes the contract-ordered DAG and (optionally) runs it
//! behind the plan gate.

use std::collections::BTreeSet;

use anyhow::{Context, Result};

use crate::cli::{DeliberateArgs, RunArgs};
use crate::config::{self, ProjectsConfig};
use crate::mailbox::{MailDraft, MailboxStore};
use crate::paths;

pub async fn run(args: DeliberateArgs) -> Result<()> {
    crate::cli::commands::work::ensure_initialized()?;

    if let Some(root) = args.root.as_deref() {
        crate::cli::commands::work::discover_and_apply(root, args.max_depth, &args.agent)?;
    }

    let spec = std::fs::read_to_string(&args.doc)
        .with_context(|| format!("read requirements doc {}", args.doc.display()))?;
    if spec.trim().is_empty() {
        anyhow::bail!("requirements doc {} is empty", args.doc.display());
    }

    let projects = ProjectsConfig::load(&paths::projects_file()?)?;
    if projects.projects.is_empty() {
        anyhow::bail!("no registered projects — run `maestro init`/`maestro add` first");
    }
    // Scope: explicit --project list, else every registered project.
    let scope: BTreeSet<String> = if args.projects.is_empty() {
        projects.projects.keys().cloned().collect()
    } else {
        for p in &args.projects {
            if !projects.projects.contains_key(p) {
                anyhow::bail!("unknown project `{p}`");
            }
        }
        args.projects.iter().cloned().collect()
    };

    // 1) Deliberate: positions + conflicts + resolved order (deterministic).
    //    Fold in code-graph-derived imports so a monorepo whose modules cross
    //    no contracts still deliberates with a real ordering, not all-flat.
    let derived = match paths::workspace_root() {
        Ok(root) => {
            let module_paths: Vec<(String, String)> = projects
                .projects
                .iter()
                .map(|(name, p)| (name.clone(), p.path.clone()))
                .collect();
            crate::codegraph::load_derived_module_deps(&root, &module_paths, 1)
        }
        Err(_) => std::collections::BTreeMap::new(),
    };
    let mut report = config::deliberate(&spec, &projects, &scope, &derived);

    // 2) Either run a live LLM discussion (real agents post takes to the
    //    mailbox), or post the deterministic positions to the mailbox.
    let mut lead_summary: Option<String> = None;
    if let Some(backend_name) = args.discuss.as_deref() {
        let backend = crate::cli::commands::discuss::DiscussBackend::parse(backend_name)?;
        println!(
            "→ discussion mode: {} agents stating positions (live in the coordination panel)\n",
            backend.label()
        );
        lead_summary = Some(
            crate::cli::commands::discuss::run_discussion(backend, &projects, &mut report, &spec)
                .await?,
        );
    } else {
        match MailboxStore::open() {
            Ok(store) => {
                for pos in &report.positions {
                    let (subject, body) = config::position_message(pos);
                    let _ = store.send(MailDraft {
                        from: format!("{}-agent", pos.project),
                        to: "lead".to_string(),
                        project: Some(pos.project.clone()),
                        task: None,
                        subject,
                        body,
                        blocking: false,
                        ask: None,
                    });
                }
            }
            Err(e) => tracing::warn!("could not open mailbox to post positions: {e:#}"),
        }
    }

    // 3) Write + print the transcript.
    let mut transcript = config::render_transcript(&report);
    if let Some(lead) = &lead_summary {
        transcript.push_str("\n## Lead synthesis\n\n");
        transcript.push_str(lead);
        transcript.push('\n');
    }
    let out = paths::maestro_dir()?.join("DELIBERATION.md");
    if let Err(e) = std::fs::write(&out, &transcript) {
        tracing::warn!("could not write {}: {e:#}", out.display());
    }
    println!("{transcript}");
    println!("\n→ deliberation transcript: {}", out.display());
    if !report.conflicts.is_empty() {
        println!(
            "→ {} contract coordination point(s) surfaced — ordering enforced in the plan",
            report.conflicts.len()
        );
    }

    // 4) Synthesize the contract-ordered DAG from the resolved decomposition.
    let selected: Vec<String> = report.order.clone();
    let plan_path = crate::cli::commands::plan::synthesize_file(&spec, None, selected, None)?;
    crate::cli::commands::work::validate_generated_plan(&plan_path)?;
    println!("→ plan synthesized: {}", plan_path.display());

    if !(args.run || args.dry) {
        println!("next:");
        println!("  maestro run {} --gates plan", plan_path.display());
        return Ok(());
    }

    // 5) Run behind the gate(s).
    let run_args = RunArgs {
        plan: plan_path,
        only: vec![],
        skip: vec![],
        max_parallel: args.max_parallel,
        continue_on_error: false,
        session_id: None,
        model: None,
        dry: args.dry,
        no_wire_contracts: false,
        max_tokens: args.max_tokens,
        gates: args.gates,
        quiet_impact: false,
    };
    crate::cli::commands::run::run(run_args).await
}
