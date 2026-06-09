//! `maestro delivery` CLI — the full PM-to-Delivery lifecycle. F-127a: intake →
//! DeliverySpec, read-only projection, list. F-127b: shape spec → confirm-spec (the
//! spec_confirm threshold) → generate PLAN (reuses plan::synthesize) → start + link
//! a run (delivery↔run). F-127c: accept (the PM verdict threshold) → closeout (+
//! optional Feishu/doc write-back intent — maestro emits the intent, never an
//! in-process post).

use anyhow::{Context, Result};

use crate::cli::{DeliveryArgs, DeliverySubcmd};
use crate::delivery;
use crate::schema::delivery::{AcceptanceCriterion, DeliveryRef};

pub async fn run(args: DeliveryArgs) -> Result<()> {
    match args.subcmd {
        DeliverySubcmd::Intake {
            file,
            id,
            proposer,
            source_uri,
        } => intake(&file, id, proposer, source_uri),
        DeliverySubcmd::Show { id, json } => show(&id, json),
        DeliverySubcmd::Ls => ls(),
        DeliverySubcmd::Spec {
            id,
            prd,
            projects,
            accept,
            by,
        } => spec(&id, prd, projects, accept, by),
        DeliverySubcmd::ConfirmSpec { id, by } => confirm_spec(&id, by),
        DeliverySubcmd::Plan { id, force } => plan(&id, force),
        DeliverySubcmd::Run { id, by } => start_run(&id, by).await,
        DeliverySubcmd::Accept {
            id,
            verdict,
            by,
            notes,
            debt,
            accept_failed_with_debt,
        } => accept(&id, &verdict, by, notes, debt, accept_failed_with_debt),
        DeliverySubcmd::Closeout {
            id,
            commits,
            ci,
            reviews,
            doc_revisions,
            evidence,
            writeback,
            by,
        } => closeout(
            &id,
            commits,
            ci,
            reviews,
            doc_revisions,
            evidence,
            writeback,
            by,
        ),
        DeliverySubcmd::Reopen { id, by, reason } => reopen(&id, by, reason),
    }
}

fn reopen(id: &str, by: Option<String>, reason: Option<String>) -> Result<()> {
    let d = delivery::reopen(id, by, reason, now())?;
    println!(
        "delivery {} reopened for rework (now round {}, {} superseded) — stage {:?}; \
         re-confirm-spec → re-plan → re-run",
        id,
        d.superseded_rounds.len() + 1,
        d.superseded_rounds.len(),
        d.stage
    );
    Ok(())
}

fn accept(
    id: &str,
    verdict: &str,
    by: String,
    notes: Option<String>,
    debt: Vec<String>,
    accept_failed_with_debt: bool,
) -> Result<()> {
    let v = delivery::parse_verdict(verdict)?;
    let d = delivery::accept(id, v, by, notes, debt, accept_failed_with_debt, now())?;
    println!(
        "delivery {} accept recorded: verdict {:?}, stage {:?}",
        id,
        d.accept.as_ref().map(|a| a.verdict),
        d.stage
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn closeout(
    id: &str,
    commits: Vec<String>,
    ci: Vec<String>,
    reviews: Vec<String>,
    doc_revisions: Vec<String>,
    evidence: Vec<String>,
    writeback: bool,
    by: Option<String>,
) -> Result<()> {
    let evidence_refs = delivery::parse_evidence_refs(&evidence);
    let d = delivery::closeout(
        id,
        commits,
        ci,
        reviews,
        doc_revisions,
        evidence_refs,
        writeback,
        now(),
        by,
    )?;
    let wb = d
        .closeout
        .as_ref()
        .and_then(|c| c.writeback.as_ref())
        .map(|w| format!("{:?}", w.status))
        .unwrap_or_else(|| "none".into());
    println!(
        "delivery {} closed out (stage {:?}); write-back: {}",
        id, d.stage, wb
    );
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Parse an `--accept` value: "describe :: shell check" → criterion with a check;
/// no "::" → describe only (no check — blocks confirm until a check is supplied).
fn parse_accept(raw: &str) -> AcceptanceCriterion {
    match raw.split_once("::") {
        Some((d, c)) => AcceptanceCriterion {
            describe: d.trim().to_string(),
            check: Some(c.trim().to_string()).filter(|s| !s.is_empty()),
        },
        None => AcceptanceCriterion {
            describe: raw.trim().to_string(),
            check: None,
        },
    }
}

fn spec(
    id: &str,
    prd: String,
    projects: Vec<String>,
    accept: Vec<String>,
    by: Option<String>,
) -> Result<()> {
    let acceptance: Vec<AcceptanceCriterion> = accept.iter().map(|a| parse_accept(a)).collect();
    let d = delivery::set_spec(id, prd, projects, acceptance, now(), by)?;
    println!("delivery {} spec recorded (stage: {:?})", id, d.stage);
    let gaps = d.spec_structural_gaps();
    if gaps.is_empty() {
        println!("spec structurally complete — run `delivery confirm-spec {id}` to confirm");
    } else {
        println!(
            "spec still incomplete ({} gap(s)) — confirm will refuse until fixed:",
            gaps.len()
        );
        for g in gaps {
            println!("  - {g}");
        }
    }
    Ok(())
}

fn confirm_spec(id: &str, by: Option<String>) -> Result<()> {
    let d = delivery::confirm_spec(id, by, now())?;
    println!(
        "delivery {} spec confirmed by {} — run `delivery plan {id}` to generate a PLAN",
        id,
        d.spec_confirm
            .as_ref()
            .and_then(|s| s.by.as_deref())
            .unwrap_or("(unknown)")
    );
    Ok(())
}

fn plan(id: &str, force: bool) -> Result<()> {
    let (d, path) = delivery::generate_plan(id, now(), None, force)?;
    let pr = d.plan.as_ref().expect("plan recorded");
    println!("delivery {} plan generated (stage: {:?})", id, d.stage);
    println!("  plan_path:  {}", pr.plan_path);
    println!("  plan_hash:  {}", pr.plan_hash);
    println!("  (written:   {})", path.display());
    println!("run `delivery run {id}` to execute + link it");
    Ok(())
}

async fn start_run(id: &str, by: Option<String>) -> Result<()> {
    let d = delivery::start_run(id, now(), by).await?;
    let ex = d.execute.as_ref().expect("execute recorded");
    println!(
        "delivery {} linked to run {} (status {}), stage: {:?}",
        id,
        ex.run_id.as_deref().unwrap_or("?"),
        ex.status.as_deref().unwrap_or("?"),
        d.stage
    );
    Ok(())
}

fn intake(
    file: &str,
    id: Option<String>,
    proposer: Option<String>,
    source_uri: Option<String>,
) -> Result<()> {
    let text = std::fs::read_to_string(file).with_context(|| format!("read {file}"))?;
    let delivery_id = id.unwrap_or_else(|| format!("d-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    // The source doc is referenced by name (+ optional remote uri) only — its body
    // is NOT copied in. The uri is what `closeout --writeback` targets.
    let name = std::path::Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("intake")
        .to_string();
    let source = DeliveryRef {
        kind: "intake_doc".into(),
        path: None,
        uri: source_uri,
        name: Some(name),
    };
    let created_at = chrono::Utc::now().to_rfc3339();
    let spec = delivery::parse_intake(&delivery_id, &text, source, created_at, proposer);
    let path = delivery::create(&spec)?;
    println!(
        "delivery {} created (stage: {:?}) — {}",
        spec.delivery_id,
        spec.stage,
        path.display()
    );
    let blocking: Vec<_> = spec
        .clarify
        .questions
        .iter()
        .filter(|q| q.blocking)
        .collect();
    if blocking.is_empty() {
        println!("intake complete — no blocking clarifications");
    } else {
        println!(
            "{} blocking clarify question(s) — not silently filled:",
            blocking.len()
        );
        for q in blocking {
            println!("  - {}", q.q);
        }
    }
    Ok(())
}

fn show(id: &str, json: bool) -> Result<()> {
    match delivery::read(id)? {
        None => {
            println!("delivery {id} not found");
            Ok(())
        }
        Some(spec) => {
            // Fallible (F-136a1 B1): a linked-run RUN_STATE that can't be projected is
            // an explicit error here too, never a silent null.
            let view = crate::delivery::project_view(&spec)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                println!("delivery {}", view.delivery_id);
                println!("  stage:      {:?}", view.stage);
                println!("  blocked_on: {:?}", view.blocked_on);
                println!("  source:     {} ref(s)", view.source_refs.len());
                if let Some(run_id) = &view.run_id {
                    println!("  run:        {run_id}");
                }
                if let Some(plan) = &view.plan_path {
                    println!("  plan:       {plan}");
                }
                if let Some(v) = &view.accept_verdict {
                    println!("  accept:     {v:?}");
                }
                if let Some(by) = &view.pm_accepted_by {
                    println!("  pm_accept:  {by}");
                }
                if let Some(c) = &view.closeout_summary {
                    println!("  closeout:   {c}");
                }
                if let Some(w) = &view.writeback_status {
                    println!("  writeback:  {w:?}");
                }
            }
            Ok(())
        }
    }
}

fn ls() -> Result<()> {
    let ids = delivery::list()?;
    if ids.is_empty() {
        println!("(no deliveries)");
        return Ok(());
    }
    for id in ids {
        let stage = delivery::read(&id)
            .ok()
            .flatten()
            .map(|s| format!("{:?}", s.stage))
            .unwrap_or_else(|| "<corrupt>".into());
        println!("{id}\t{stage}");
    }
    Ok(())
}
