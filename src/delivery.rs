//! The `DeliverySpec` file store + the PM-to-Delivery lifecycle operations.
//!
//! Store (F-127a): one `.maestro/deliveries/<id>/DELIVERY.json` per requirement. A
//! corrupt file is an explicit error, never a silent empty; refs are validated
//! run-local on read AND write.
//!
//! Lifecycle: intake parser (F-127a, best-effort markdown/plain text → spec; a
//! missing required field becomes a BLOCKING `clarify.question`, never silently
//! filled); spec shaping → confirm → plan → run linkage (F-127b); accept → closeout
//! → write-back intent (F-127c). The inbound doc is kept only as a `source_ref`,
//! never copied inline; runs are referenced by id, never copied.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::config::{Acceptance, Goal, Plan, ProjectsConfig};
use crate::paths;
use crate::schema::delivery::{
    Accept, AcceptVerdict, AcceptanceCriterion, AuditEntry, Clarify, ClarifyQuestion, Closeout,
    DeliveryRef, DeliverySpec, DeliveryStage, DeliveryView, ExecuteRef, Intake, PlanRef, PmAccept,
    RunStatusLite, RunStatusProgress, Spec, SpecConfirm, SupersededRound, TimelineEvent,
    TimelineRef, Writeback, WritebackReceipt, WritebackStatus,
};

fn delivery_file(delivery_id: &str) -> Result<PathBuf> {
    Ok(paths::delivery_dir(delivery_id)?.join(paths::DELIVERY_FILE))
}

/// Validate the machine contract (schema_version + run-local refs) and write.
/// Shared by `create` (new) and `save` (overwrite) so neither can persist a
/// record that `read` would later reject.
fn write_validated(spec: &DeliverySpec, dir: &Path, path: &Path) -> Result<()> {
    spec.validate()
        .map_err(|e| anyhow!("delivery {}: {e}", spec.delivery_id))?;
    paths::ensure_dir(dir)?;
    let body = serde_json::to_string_pretty(spec).context("serialize DeliverySpec")?;
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Persist a NEW delivery. Refuses to overwrite an existing record — a PM
/// requirement is a durable record, so a same-`id` re-intake is an explicit
/// error, never a silent data-losing overwrite. Use `update_stage` to evolve an
/// existing one.
pub fn create(spec: &DeliverySpec) -> Result<PathBuf> {
    let dir = paths::delivery_dir(&spec.delivery_id)?;
    let path = dir.join(paths::DELIVERY_FILE);
    if path.exists() {
        bail!(
            "delivery {} already exists ({}); refusing to overwrite an existing requirement \
             record — choose a new --id",
            spec.delivery_id,
            path.display()
        );
    }
    write_validated(spec, &dir, &path)?;
    Ok(path)
}

/// Overwrite an EXISTING delivery in place. Internal: callers must have read the
/// record first (this is how a stage transition is persisted), so it deliberately
/// allows overwrite where `create` does not.
fn save(spec: &DeliverySpec) -> Result<PathBuf> {
    let dir = paths::delivery_dir(&spec.delivery_id)?;
    let path = dir.join(paths::DELIVERY_FILE);
    write_validated(spec, &dir, &path)?;
    Ok(path)
}

/// Read a delivery. Missing → `Ok(None)`; a present-but-corrupt file, an
/// unsupported `schema_version`, OR an unsafe ref is an explicit error (never a
/// silent empty / silent down-version projection).
pub fn read(delivery_id: &str) -> Result<Option<DeliverySpec>> {
    let path = delivery_file(delivery_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let spec: DeliverySpec = serde_json::from_str(&text)
        .with_context(|| format!("corrupt DeliverySpec {}", path.display()))?;
    spec.validate()
        .map_err(|e| anyhow!("invalid DeliverySpec {}: {e}", path.display()))?;
    Ok(Some(spec))
}

/// List delivery ids (the subdir names under `.maestro/deliveries/`). A missing
/// dir (fresh workspace) → `Ok([])`, but ANY OTHER read failure (not-a-directory /
/// permission denied / …) is an explicit `Err` — a real storage problem must never
/// be shown as "no deliveries". A single unreadable entry is skipped, not fatal.
pub fn list() -> Result<Vec<String>> {
    let dir = paths::deliveries_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read deliveries dir {}", dir.display())),
    };
    let mut ids = Vec::new();
    for e in entries.flatten() {
        if e.path().is_dir() {
            if let Some(name) = e.file_name().to_str() {
                ids.push(name.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// Project a delivery to its `DeliveryView` AND enrich the F-136a1
/// `runtime_profile_summary` (the worst-case RuntimeProfile + per-profile breakdown
/// across the linked run's tasks). FALLIBLE on purpose (F-136a1 B1): a delivery with
/// a linked `execute.run_id` whose RUN_STATE is missing/corrupt is "runtime profile
/// UNAVAILABLE" — an explicit `Err` (the serving layer maps it to a detail 500 / a
/// list corrupt-stub), NEVER a silent `null`. `null` is reserved for "no run linked"
/// (and a run that loads but has no tasks). Read-only; no behavior change.
pub fn project_view(d: &DeliverySpec) -> Result<DeliveryView> {
    let mut view = DeliveryView::project(d);
    if let Some(run_id) = d.execute.as_ref().and_then(|e| e.run_id.as_deref()) {
        let run_dir = paths::run_dir_for_id(run_id)
            .with_context(|| format!("delivery {}: resolve linked run {run_id}", d.delivery_id))?;
        let state = crate::scheduler::state::RunState::load(&run_dir).with_context(|| {
            format!(
                "delivery {}: linked run {run_id} RUN_STATE unavailable (missing/corrupt) \
                 — runtime profile cannot be projected",
                d.delivery_id
            )
        })?;
        view.runtime_profile_summary = crate::schema::runtime_profile::summarize(&state);
    }
    Ok(view)
}

/// F-131: resolve the Delivery's run for the inline live status. A LINKED run
/// (`execute.run_id`) MUST load — a missing/corrupt linked run is an explicit error
/// (the handler maps it to 500), NEVER silent idle. With no linkage yet (a detached
/// run mid-flight, before `start_run` writes the ExecuteRef back), find the in-flight
/// run by the `RunState.delivery_id` back-ref. `Ok(None)` = genuinely no run.
pub fn run_status(d: &DeliverySpec) -> Result<Option<RunStatusLite>> {
    if let Some(run_id) = d.execute.as_ref().and_then(|e| e.run_id.as_deref()) {
        let run_dir = paths::run_dir_for_id(run_id)?;
        let state = crate::scheduler::state::RunState::load(&run_dir).with_context(|| {
            format!(
                "delivery {}: linked run {run_id} status unavailable (missing/corrupt)",
                d.delivery_id
            )
        })?;
        return Ok(Some(run_status_lite(run_id, &state, true)));
    }
    Ok(find_inflight_run(&d.delivery_id)?
        .map(|(run_id, state)| run_status_lite(&run_id, &state, false)))
}

/// Scan `.maestro/runs/*` for the most-recent RUNNING run whose `delivery_id` back-ref
/// matches. The back-ref path exists ONLY for the window where a detached run is
/// actively running but `start_run` hasn't written `execute.run_id` back yet — so it
/// filters to `RunStatus::Running` (F-131 B1). A completed / failed / cancelled orphan
/// run must NOT masquerade as in-flight; it's `None` here (orphan visibility is later
/// audit-timeline / recovery work). Unreadable RUN_STATE files are SKIPPED
/// (unattributable; a matched run is always readable — the linked path is where corrupt → 500).
fn find_inflight_run(
    delivery_id: &str,
) -> Result<Option<(String, crate::scheduler::state::RunState)>> {
    use crate::scheduler::state::RunStatus;
    let runs = paths::runs_dir()?;
    let entries = match std::fs::read_dir(&runs) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read runs dir {}", runs.display())),
    };
    let mut best: Option<(String, crate::scheduler::state::RunState)> = None;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Ok(state) = crate::scheduler::state::RunState::load(&entry.path()) else {
            continue; // unreadable / unrelated — unattributable, skip
        };
        if state.delivery_id.as_deref() != Some(delivery_id) {
            continue;
        }
        if state.status != RunStatus::Running {
            continue; // a non-running back-ref run is an orphan, not in-flight
        }
        let newer = best
            .as_ref()
            .map(|(_, b)| state.started_at > b.started_at)
            .unwrap_or(true);
        if newer {
            best = Some((state.run_id.clone(), state));
        }
    }
    Ok(best)
}

fn run_status_lite(
    run_id: &str,
    state: &crate::scheduler::state::RunState,
    linked: bool,
) -> RunStatusLite {
    use crate::scheduler::state::RunStatus;
    let p = crate::schema::monitor::RunProgress::from_state(state);
    let status = match state.status {
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    };
    RunStatusLite {
        run_id: run_id.to_string(),
        status: status.to_string(),
        linked,
        progress: RunStatusProgress {
            total: p.total,
            done: p.done,
            failed: p.failed,
            running: p.running,
            pending: p.pending + p.awaiting_approval,
        },
        started_at: state.started_at.to_rfc3339(),
        ended_at: state.ended_at.map(|t| t.to_rfc3339()),
    }
}

/// F-132: project `DeliverySpec.audit` (the chronological who/when/why spine) into the
/// audit timeline — one event per `AuditEntry`, enriched refs-first from the matching
/// lifecycle node. FALLIBLE: an audit entry whose stage REQUIRES a node that's missing
/// (a Plan transition with no `plan`, an Execute transition with no `execute.run_id`, an
/// Accept/terminal transition with no `accept`, a Closeout transition with no `closeout`)
/// is an inconsistent record → `Err` (the handler maps it to 500), NEVER fabricated as a
/// text-only event. Events are sorted by `(at, original_index)` — the index makes the
/// stable order EXPLICIT (same-instant rows keep `audit[]` append order), not reliant on
/// sort stability. Refs-first: ids / paths / hashes / counts / uris only — never RUN_STATE,
/// task lists, the Feishu body, or evidence blobs.
pub fn timeline(d: &DeliverySpec) -> Result<Vec<TimelineEvent>> {
    let mut indexed: Vec<(usize, TimelineEvent)> = Vec::with_capacity(d.audit.len());
    // F-133: walk the audit in append order, tracking the rework round. A node-producing
    // row of round R resolves its node from `superseded_rounds[R-1]` (a past round) or the
    // LIVE fields (the current round) — so an old Plan/Execute/Accept row whose node was
    // archived by reopen is NOT a false inconsistency. A reopen row (a Spec transition
    // "reopened for rework") starts the next round.
    let mut round_idx = 0usize;
    for (i, a) in d.audit.iter().enumerate() {
        indexed.push((
            i,
            TimelineEvent {
                at: a.at.clone(),
                stage: a.stage,
                by: a.by.clone(),
                reason: a.reason.clone(),
                refs: event_refs(d, a, round_idx)?,
            },
        ));
        if is_reopen_row(a) {
            round_idx += 1;
        }
    }
    indexed.sort_by(|(ia, ea), (ib, eb)| ea.at.cmp(&eb.at).then(ia.cmp(ib)));
    Ok(indexed.into_iter().map(|(_, e)| e).collect())
}

/// The reopen audit row (a Spec transition whose reason marks the rework). The reason
/// string is owned by `reopen()` (see the `"reopened for rework"` prefix).
fn is_reopen_row(a: &AuditEntry) -> bool {
    a.stage == DeliveryStage::Spec
        && a.reason
            .as_deref()
            .map(|r| r.starts_with("reopened for rework"))
            .unwrap_or(false)
}

/// The `{plan, execute, accept, closeout}` of round `round_idx` — a superseded round if
/// `round_idx < superseded_rounds.len()`, else the live (current) round.
struct RoundNodes<'a> {
    plan: Option<&'a PlanRef>,
    execute: Option<&'a ExecuteRef>,
    accept: Option<&'a Accept>,
    closeout: Option<&'a Closeout>,
}

fn round_nodes(d: &DeliverySpec, round_idx: usize) -> RoundNodes<'_> {
    if let Some(sr) = d.superseded_rounds.get(round_idx) {
        RoundNodes {
            plan: sr.plan.as_ref(),
            execute: sr.execute.as_ref(),
            accept: sr.accept.as_ref(),
            closeout: sr.closeout.as_ref(),
        }
    } else {
        RoundNodes {
            plan: d.plan.as_ref(),
            execute: d.execute.as_ref(),
            accept: d.accept.as_ref(),
            closeout: d.closeout.as_ref(),
        }
    }
}

fn tref(
    kind: &str,
    value: Option<String>,
    uri: Option<String>,
    summary: Option<String>,
) -> TimelineRef {
    TimelineRef {
        kind: kind.to_string(),
        value,
        uri,
        summary,
    }
}

/// Refs for one timeline event, by the entry's stage, resolved against round `round_idx`
/// (live or a superseded round). Forward stages whose transition PRODUCES a node require
/// that node to be present in that round (else the record is inconsistent → Err). Early
/// stages that simply haven't reached a node add no ref (not an error).
fn event_refs(d: &DeliverySpec, a: &AuditEntry, round_idx: usize) -> Result<Vec<TimelineRef>> {
    use DeliveryStage::*;
    let id = &d.delivery_id;
    let nodes = round_nodes(d, round_idx);
    let mut refs = Vec::new();
    match a.stage {
        Intake | Clarify => {
            for r in &d.intake.source_refs {
                refs.push(tref(
                    "source",
                    r.path.clone(),
                    r.uri.clone(),
                    r.name.clone(),
                ));
            }
        }
        // Spec covers both `set_spec` and `confirm_spec` rows. `spec_confirm` is not
        // archived per round, so only enrich the CURRENT round's Spec rows (and only if
        // confirmed); an old round's confirm row simply carries no spec_confirm ref.
        Spec => {
            let is_current = round_idx >= d.superseded_rounds.len();
            if is_current {
                if let Some(sc) = &d.spec_confirm {
                    if sc.confirmed {
                        refs.push(tref("spec_confirm", sc.by.clone(), None, sc.notes.clone()));
                    }
                }
            }
        }
        Plan => {
            let p = nodes.plan.ok_or_else(|| {
                anyhow!("delivery {id}: audit shows a Plan transition but plan ref is missing — inconsistent record")
            })?;
            refs.push(tref(
                "plan",
                Some(p.plan_path.clone()),
                None,
                Some(format!("hash {}", short_hash(&p.plan_hash))),
            ));
            if let Some(pr) = &p.preview_ref {
                refs.push(tref("preview", Some(pr.clone()), None, None));
            }
        }
        Execute => {
            let run_id = nodes
                .execute
                .and_then(|e| e.run_id.clone())
                .ok_or_else(|| {
                    anyhow!("delivery {id}: audit shows an Execute transition but execute/run_id is missing — inconsistent record")
                })?;
            let status = nodes.execute.and_then(|e| e.status.clone());
            refs.push(tref("run", Some(run_id), None, status));
        }
        // Accept and the terminal verdict transitions all imply an `accept` record.
        Accept | ChangesRequested | Rejected => {
            let ac = nodes.accept.ok_or_else(|| {
                anyhow!("delivery {id}: audit shows an accept/verdict transition but accept record is missing — inconsistent record")
            })?;
            let summary = if ac.debt.is_empty() {
                None
            } else {
                Some(format!("{} debt", ac.debt.len()))
            };
            refs.push(tref(
                "verdict",
                Some(verdict_str(ac.verdict).into()),
                None,
                summary,
            ));
        }
        Closeout => {
            let c = nodes.closeout.ok_or_else(|| {
                anyhow!("delivery {id}: audit shows a Closeout transition but closeout record is missing — inconsistent record")
            })?;
            refs.push(tref(
                "evidence",
                None,
                None,
                Some(format!(
                    "{} commit / {} ci / {} review / {} doc / {} evidence",
                    c.commits.len(),
                    c.ci.len(),
                    c.reviews.len(),
                    c.doc_revisions.len(),
                    c.evidence_refs.len()
                )),
            ));
            if let Some(wb) = &c.writeback {
                refs.push(tref(
                    "writeback",
                    Some(writeback_status_str(wb.status).into()),
                    wb.doc_ref.as_ref().and_then(|r| r.uri.clone()),
                    None,
                ));
            }
        }
        // Bypass stages (Parked / Duplicate / Cancelled) produce no node.
        _ => {}
    }
    Ok(refs)
}

fn short_hash(h: &str) -> String {
    h.chars().take(12).collect()
}

fn verdict_str(v: AcceptVerdict) -> &'static str {
    match v {
        AcceptVerdict::Pending => "pending",
        AcceptVerdict::Accepted => "accepted",
        AcceptVerdict::Partial => "partial",
        AcceptVerdict::ChangesRequested => "changes_requested",
        AcceptVerdict::Rejected => "rejected",
    }
}

fn writeback_status_str(s: WritebackStatus) -> &'static str {
    match s {
        WritebackStatus::Skipped => "skipped",
        WritebackStatus::IntentEmitted => "intent_emitted",
        WritebackStatus::Posted => "posted",
        WritebackStatus::Failed => "failed",
    }
}

/// Advance the stage, validating the transition (illegal → error) and appending
/// an audit row. `now` is supplied so callers control the clock.
pub fn update_stage(
    delivery_id: &str,
    next: DeliveryStage,
    now: String,
    by: Option<String>,
    reason: Option<String>,
) -> Result<DeliverySpec> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if !spec.stage.can_transition_to(next) {
        bail!(
            "illegal stage transition {:?} -> {:?} for delivery {delivery_id}",
            spec.stage,
            next
        );
    }
    spec.stage = next;
    spec.audit.push(AuditEntry {
        stage: next,
        at: now,
        by,
        reason,
    });
    save(&spec)?;
    Ok(spec)
}

// ── F-127b: spec shaping → plan generation → run linkage ─────────────────────

/// The `plan_hash` recorded in `PlanRef` AND pinned by F-122 at run start: the
/// hash of the run-NORMALIZED plan (`serde_yaml::to_string`), so a delivery's
/// `PlanRef.plan_hash` is same-source with the run's `PLAN_PREVIEW.json` pin. A
/// raw `file_hash` of the generated YAML would diverge (the run re-serializes the
/// loaded `Plan` — executor.rs snapshots via `serde_yaml::to_string`).
pub fn normalized_plan_hash(plan: &Plan) -> Result<String> {
    let yaml = serde_yaml::to_string(plan).context("serialize plan for hashing")?;
    Ok(crate::file_guard::stable_hash_bytes(yaml.as_bytes()))
}

/// Structural gaps (pure `spec_structural_gaps`) PLUS the runtime check that every
/// target project actually exists in `projects.yaml`. An unknown project is an
/// explicit gap — never handed to `synthesize` for a fuzzy failure.
fn spec_gaps_with_projects(spec: &DeliverySpec) -> Result<Vec<String>> {
    let mut gaps = spec.spec_structural_gaps();
    if let Some(s) = spec.spec.as_ref() {
        if !s.target_projects.is_empty() {
            let projects = ProjectsConfig::load(&paths::projects_file()?)
                .context("load projects.yaml to validate target_projects")?;
            for p in &s.target_projects {
                if !projects.projects.contains_key(p) {
                    gaps.push(format!(
                        "target project '{p}' is not registered in projects.yaml"
                    ));
                }
            }
        }
    }
    Ok(gaps)
}

/// Record each gap as a BLOCKING clarify question (dedup by text) — so `delivery
/// show` reflects exactly what stops a confirm. Never fabricates an answer.
fn record_blocking_clarify(spec: &mut DeliverySpec, gaps: &[String], now: &str) {
    for g in gaps {
        if spec.clarify.questions.iter().any(|q| q.q == *g) {
            continue;
        }
        spec.clarify.questions.push(ClarifyQuestion {
            q: g.clone(),
            blocking: true,
            answer: None,
            by: None,
            at: Some(now.to_string()),
        });
    }
}

/// Record/shape the `spec` and advance the delivery to `Spec`. Deterministic — no
/// AI/heuristics; the caller passes the fields. Refuses once a plan exists
/// (`stage` past `Spec`): a spec is frozen after planning. Walks the legal forward
/// chain (Intake→Clarify→Spec) with audit rows. NEVER writes `spec_confirm`, and if
/// the spec was already confirmed, editing it RESETS the confirmation (a human
/// confirmed a specific version) so `delivery plan` refuses until a re-confirm.
pub fn set_spec(
    delivery_id: &str,
    prd: String,
    target_projects: Vec<String>,
    acceptance: Vec<AcceptanceCriterion>,
    now: String,
    by: Option<String>,
) -> Result<DeliverySpec> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    use DeliveryStage::{Clarify as StClarify, Intake as StIntake, Spec as StSpec};
    if !matches!(spec.stage, StIntake | StClarify | StSpec) {
        bail!(
            "delivery {delivery_id} is at stage {:?}; the spec can't be set or edited after planning",
            spec.stage
        );
    }
    let was_confirmed = spec
        .spec_confirm
        .as_ref()
        .map(|s| s.confirmed)
        .unwrap_or(false);
    spec.spec = Some(Spec {
        prd,
        target_projects,
        acceptance,
        ..Default::default()
    });
    // Editing a confirmed spec invalidates the confirmation: `spec_confirm` is a
    // hard guard over a SPECIFIC version, not a blanket "edit freely after". Reset
    // it + audit; `delivery plan` will then refuse until a fresh `confirm-spec`.
    if was_confirmed {
        spec.spec_confirm = None;
        spec.audit.push(AuditEntry {
            stage: spec.stage,
            at: now.clone(),
            by: by.clone(),
            reason: Some("spec changed after confirmation; confirmation reset".into()),
        });
    }
    // Advance along the legal forward chain to Spec, one audit row per step.
    while spec.stage != StSpec {
        let next = match spec.stage {
            StIntake => StClarify,
            StClarify => StSpec,
            _ => break,
        };
        debug_assert!(spec.stage.can_transition_to(next));
        spec.stage = next;
        spec.audit.push(AuditEntry {
            stage: next,
            at: now.clone(),
            by: by.clone(),
            reason: Some("delivery spec set".into()),
        });
    }
    save(&spec)?;
    Ok(spec)
}

/// Record the `spec_confirm` threshold — the ONLY action that writes it
/// (`delivery plan`/`run` never do). Confirms ONLY a complete spec: any structural
/// gap or unknown/empty target project → refuse, and record the gaps as BLOCKING
/// clarify questions (never a "confirmed-but-unplannable" record). Requires
/// `stage == Spec`.
pub fn confirm_spec(delivery_id: &str, by: Option<String>, now: String) -> Result<DeliverySpec> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if spec.stage != DeliveryStage::Spec {
        bail!(
            "delivery {delivery_id} is at stage {:?}; confirm-spec requires stage spec (run `delivery spec` first)",
            spec.stage
        );
    }
    // No duplicate confirm (F-129 Web guard): an already-confirmed spec is a no-op
    // that would push a redundant audit row — refuse instead.
    if spec
        .spec_confirm
        .as_ref()
        .map(|s| s.confirmed)
        .unwrap_or(false)
    {
        bail!("delivery {delivery_id} spec is already confirmed");
    }
    let gaps = spec_gaps_with_projects(&spec)?;
    if !gaps.is_empty() {
        record_blocking_clarify(&mut spec, &gaps, &now);
        save(&spec)?;
        bail!(
            "delivery {delivery_id} spec is incomplete — refusing to confirm; recorded {} blocking clarify question(s):\n  - {}",
            gaps.len(),
            gaps.join("\n  - ")
        );
    }
    spec.spec_confirm = Some(SpecConfirm {
        confirmed: true,
        by: by.clone(),
        at: Some(now.clone()),
        notes: None,
    });
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Spec,
        at: now,
        by,
        reason: Some("spec confirmed".into()),
    });
    save(&spec)?;
    Ok(spec)
}

/// Generate a PLAN from the confirmed spec and record `PlanRef`, advancing
/// Spec→Plan. HARD guard: `spec_confirm.confirmed` (plan NEVER writes confirm).
/// Re-checks completeness (anti-tamper) before generating. Refuses to overwrite an
/// existing plan unless `force` (allowed only while still at `Plan`, never after
/// Execute). Reuses `plan::synthesize` + `config::{Goal, Acceptance}` — no new
/// planner. Returns the spec + the absolute plan path.
pub fn generate_plan(
    delivery_id: &str,
    now: String,
    by: Option<String>,
    force: bool,
) -> Result<(DeliverySpec, PathBuf)> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    match spec.stage {
        DeliveryStage::Spec => {}
        DeliveryStage::Plan if force => {} // regenerate in place (we are at Plan, pre-execute)
        DeliveryStage::Plan => bail!(
            "delivery {delivery_id} already has a plan ({}); re-run with --force to regenerate (only before execute)",
            spec.plan.as_ref().map(|p| p.plan_path.as_str()).unwrap_or("?")
        ),
        other => bail!(
            "delivery {delivery_id} is at stage {other:?}; plan generation requires stage spec (or plan + --force before execute)"
        ),
    }
    // HARD guard: spec_confirm. plan/run never auto-confirm.
    if !spec
        .spec_confirm
        .as_ref()
        .map(|s| s.confirmed)
        .unwrap_or(false)
    {
        bail!(
            "delivery {delivery_id} spec is not confirmed; run `delivery confirm-spec` first (plan never auto-confirms)"
        );
    }
    // Anti-tamper: a confirmed record must still be complete.
    let gaps = spec_gaps_with_projects(&spec)?;
    if !gaps.is_empty() {
        bail!(
            "delivery {delivery_id} is marked confirmed but its spec is incomplete (tampered?):\n  - {}",
            gaps.join("\n  - ")
        );
    }
    let s = spec.spec.clone().expect("spec present when gaps empty");
    let objective = if s.prd.trim().is_empty() {
        spec.intake.objective.clone()
    } else {
        s.prd.clone()
    };

    // Reuse the existing synthesizer for the task DAG, then graft the delivery's
    // own goal + acceptance onto the plan (reusing config::{Goal, Acceptance}).
    let rel = format!("plans/delivery-{delivery_id}.yaml");
    let out_abs = paths::workspace_root()?.join(&rel);
    crate::cli::commands::plan::synthesize_file(
        &objective,
        Some(out_abs.clone()),
        s.target_projects.clone(),
        None,
    )
    .with_context(|| format!("synthesize plan for delivery {delivery_id}"))?;
    let mut plan = Plan::read_only(&out_abs).context("load synthesized plan")?;
    plan.goal = Some(Goal {
        description: objective.clone(),
        acceptance: s
            .acceptance
            .iter()
            .map(|a| Acceptance {
                describe: a.describe.clone(),
                // gaps were empty → every acceptance has a check.
                check: a.check.clone().unwrap_or_default(),
            })
            .collect(),
    });
    std::fs::write(
        &out_abs,
        serde_yaml::to_string(&plan).context("serialize plan with grafted goal")?,
    )
    .with_context(|| format!("write {}", out_abs.display()))?;

    // Hash the run-NORMALIZED plan (Plan::load == the form the run will pin).
    let loaded = Plan::load(&out_abs).context("reload plan for hashing")?;
    let plan_hash = normalized_plan_hash(&loaded)?;
    spec.plan = Some(PlanRef {
        plan_path: rel,
        plan_hash,
        preview_ref: None, // set at execute (no run yet)
    });
    if spec.stage == DeliveryStage::Spec {
        spec.stage = DeliveryStage::Plan;
    }
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Plan,
        at: now,
        by,
        reason: Some(if force {
            "plan regenerated (--force)".into()
        } else {
            "plan generated".into()
        }),
    });
    save(&spec)?;
    Ok((spec, out_abs))
}

/// The loaded artifacts a `delivery run` preflight validated, returned so callers
/// don't re-load them.
pub struct RunPreflight {
    pub spec: DeliverySpec,
    pub plan: Plan,
    pub projects: ProjectsConfig,
    pub planref: PlanRef,
}

/// The FULL non-mutating preflight for `delivery run` (no side effects). Reused by
/// the HTTP handler (F-129 — synchronous refusal BEFORE spawning a detached run) AND
/// by `start_run` itself (anti-TOCTOU re-check just before executing). Validates:
/// `stage == Plan`, a plan present, no existing `execute`, the plan not drifted from
/// `PlanRef` (B1), project drift / task cap / plan-analysis errors (B2).
pub fn run_preflight(delivery_id: &str) -> Result<RunPreflight> {
    let spec = read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if let Some(ex) = spec.execute.as_ref() {
        bail!(
            "delivery {delivery_id} is already linked to run {} (status {}); not starting a second run",
            ex.run_id.as_deref().unwrap_or("?"),
            ex.status.as_deref().unwrap_or("?")
        );
    }
    if spec.stage != DeliveryStage::Plan {
        bail!(
            "delivery {delivery_id} is at stage {:?}; `delivery run` requires stage plan (run `delivery plan` first)",
            spec.stage
        );
    }
    let planref = spec
        .plan
        .clone()
        .with_context(|| format!("delivery {delivery_id} has no plan to run"))?;
    let plan_path = paths::workspace_root()?.join(&planref.plan_path);
    let plan =
        Plan::load(&plan_path).with_context(|| format!("load plan {}", plan_path.display()))?;
    let projects = ProjectsConfig::load(&paths::projects_file()?).context("load projects.yaml")?;

    // Drift guard: a tampered PLAN must NOT execute (B1).
    let preflight_hash = normalized_plan_hash(&plan)?;
    if preflight_hash != planref.plan_hash {
        bail!(
            "delivery {delivery_id}: plan {} has drifted from PlanRef ({} != {}); refusing to run a tampered plan",
            planref.plan_path,
            preflight_hash,
            planref.plan_hash
        );
    }
    // Mirror the normal `maestro run` pre-run defenses (B2): project drift, task cap,
    // plan-analysis errors — all non-mutating.
    crate::cli::commands::run::check_plan_project_drift(&plan, &projects)?;
    plan.enforce_task_cap(projects.defaults.max_total_tasks)?;
    let report = crate::config::analyze(&plan, &projects);
    if report.has_errors() {
        bail!(
            "delivery {delivery_id}: plan analysis found {} error(s) — refusing to run (run `maestro run` for details)",
            report.error_count()
        );
    }
    Ok(RunPreflight {
        spec,
        plan,
        projects,
        planref,
    })
}

/// Env var the server handler sets when it spawns `maestro delivery run <id>` — it
/// tells the child the launch marker is ALREADY held by the handler, so the child
/// does NOT re-acquire (but still releases on completion).
pub const RUN_LAUNCHED_ENV: &str = "MAESTRO_DELIVERY_RUN_LAUNCHED";

/// A run-launch marker is stale if its recorded pid is dead, or it is older than
/// this backstop (no plausible run lasts this long; covers a child SIGKILL'd before
/// its release).
const LAUNCH_MARKER_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

fn run_launch_marker(delivery_id: &str) -> Result<PathBuf> {
    Ok(paths::delivery_dir(delivery_id)?.join(".run-launch"))
}

/// Outcome of taking the run-launch lock for a delivery.
pub enum LaunchAcquire {
    Acquired,
    Busy,
}

/// Acquire the durable run-launch lock (F-129 server-side duplicate-run guard).
/// Atomic `create_new`; a present marker is reclaimed ONLY when its pid is dead or it
/// exceeds the age backstop, otherwise `Busy`. Records the acquiring process's pid so
/// a dead launcher self-heals. Released by `release_run_launch`.
pub fn acquire_run_launch(delivery_id: &str) -> Result<LaunchAcquire> {
    let path = run_launch_marker(delivery_id)?;
    paths::ensure_dir(&paths::delivery_dir(delivery_id)?)?;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = write!(f, "{}", std::process::id());
                return Ok(LaunchAcquire::Acquired);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if launch_marker_is_stale(&path) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                return Ok(LaunchAcquire::Busy);
            }
            Err(e) => {
                return Err(e).with_context(|| format!("acquire run launch {}", path.display()))
            }
        }
    }
}

fn launch_marker_is_stale(path: &Path) -> bool {
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if modified
                .elapsed()
                .map(|age| age > LAUNCH_MARKER_MAX_AGE)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match s.trim().parse::<u32>() {
            Ok(pid) => matches!(
                crate::scheduler::liveness::classify_pid(pid),
                crate::scheduler::liveness::RunLiveness::Abandoned
            ),
            Err(_) => true,
        },
        Err(_) => true,
    }
}

/// Release the run-launch lock (best-effort; no-op if absent).
pub fn release_run_launch(delivery_id: &str) {
    if let Ok(path) = run_launch_marker(delivery_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Removes the run-launch marker on drop, so `start_run` frees the lock on EVERY exit
/// path (success / bail / panic).
struct LaunchGuard<'a> {
    delivery_id: &'a str,
}
impl Drop for LaunchGuard<'_> {
    fn drop(&mut self) {
        release_run_launch(self.delivery_id);
    }
}

/// Start a run from the generated plan and record the linkage (`ExecuteRef` +
/// `RunState.delivery_id` + `plan.preview_ref`), advancing Plan→Execute. Runs the
/// shared `run_preflight` just before executing (anti-TOCTOU). Holds the durable
/// run-launch lock for the run's lifetime: a CLI-direct launch acquires it here, a
/// server-spawned child (`RUN_LAUNCHED_ENV` set) trusts the handler's acquire; either
/// way it's released on completion. The F-122 preview pin is REQUIRED here (not
/// best-effort): missing / parse-fail / hash mismatch → explicit error, NO linkage.
pub async fn start_run(delivery_id: &str, now: String, by: Option<String>) -> Result<DeliverySpec> {
    // Take the run-launch lock unless the server handler already holds it.
    if std::env::var(RUN_LAUNCHED_ENV).is_err() {
        match acquire_run_launch(delivery_id)? {
            LaunchAcquire::Acquired => {}
            LaunchAcquire::Busy => {
                bail!("delivery {delivery_id}: a run is already launching / in flight")
            }
        }
    }
    let _guard = LaunchGuard { delivery_id };

    // Anti-TOCTOU: re-run the FULL preflight just before executing.
    let RunPreflight {
        mut spec,
        plan,
        projects,
        planref,
    } = run_preflight(delivery_id)?;

    // Honor the workspace's configured default concurrency (not the bare default 4).
    let cfg = crate::scheduler::ExecConfig {
        max_parallel: projects.defaults.max_parallel.max(1),
        delivery_id: Some(delivery_id.to_string()),
        ..Default::default()
    };
    let state = crate::scheduler::run_plan(plan, projects, cfg)
        .await
        .with_context(|| format!("run delivery {delivery_id} plan"))?;
    let run_id = state.run_id.clone();

    // F-127b linkage REQUIRES the F-122 preview pin + a same-source hash. (The pin
    // is best-effort for an ordinary run; a delivery linkage cannot silently miss.)
    let preview_path = paths::run_dir_for_id(&run_id)?.join(paths::PLAN_PREVIEW_SNAPSHOT);
    let preview_text = std::fs::read_to_string(&preview_path).with_context(|| {
        format!("delivery {delivery_id}: run {run_id} produced no PLAN_PREVIEW.json (linkage requires the F-122 pin)")
    })?;
    let snapshot = crate::schema::preview::PlanPreviewSnapshot::from_json(&preview_text)
        .with_context(|| format!("parse {}", preview_path.display()))?;
    if snapshot.plan_hash != planref.plan_hash {
        bail!(
            "delivery {delivery_id}: run {run_id} plan_hash {} != PlanRef.plan_hash {} (plan drifted between generation and run)",
            snapshot.plan_hash,
            planref.plan_hash
        );
    }

    spec.execute = Some(ExecuteRef {
        run_id: Some(run_id.clone()),
        status: Some(format!("{:?}", state.status)),
    });
    if let Some(p) = spec.plan.as_mut() {
        // Run-relative to the run_dir of execute.run_id (resolve WITH the run_id).
        p.preview_ref = Some(paths::PLAN_PREVIEW_SNAPSHOT.to_string());
    }
    spec.stage = DeliveryStage::Execute;
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Execute,
        at: now,
        by,
        reason: Some(format!("linked to run {run_id}")),
    });
    save(&spec)?;
    Ok(spec)
}

// ── F-127c: accept + closeout + write-back ───────────────────────────────────

fn is_doc_uri(u: &str) -> bool {
    let u = u.trim().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// Parse a verdict string → `AcceptVerdict`. SHARED by the CLI (`delivery accept
/// --verdict`) and the Web handler (`POST /accept`) so the two never drift.
/// `pending` is intentionally NOT accepted here — `accept` rejects it as
/// non-recordable; an unknown verdict is a caller error.
pub fn parse_verdict(s: &str) -> Result<AcceptVerdict> {
    Ok(match s.trim() {
        "accepted" => AcceptVerdict::Accepted,
        "partial" => AcceptVerdict::Partial,
        "changes_requested" => AcceptVerdict::ChangesRequested,
        "rejected" => AcceptVerdict::Rejected,
        other => {
            bail!("invalid verdict {other:?}; expected accepted|partial|changes_requested|rejected")
        }
    })
}

/// One evidence string → a run-local `DeliveryRef`. A string with a URI scheme
/// (`http(s)://…`, `file:…`, any `scheme://…`) goes in `uri` so the store's uri rule
/// applies (`file:` is rejected); everything else is a `path` (absolute / UNC /
/// drive / `..` rejected). Routing `file:` to `uri` matters: a `file:` string
/// classified as a *path* would slip past `ref_path_is_unsafe` and masquerade as a
/// relative path. The input is trimmed; `closeout` does the run-local validation
/// (`DeliveryRef::violation`). SHARED by the CLI and the Web handler (no drift).
pub fn parse_evidence_ref(s: &str) -> DeliveryRef {
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();
    let is_uri = lower.contains("://") || lower.starts_with("file:");
    if is_uri {
        DeliveryRef {
            kind: "evidence".into(),
            path: None,
            uri: Some(trimmed.to_string()),
            name: None,
        }
    } else {
        DeliveryRef {
            kind: "evidence".into(),
            path: Some(trimmed.to_string()),
            uri: None,
            name: None,
        }
    }
}

/// Map raw evidence strings → refs, dropping blank/whitespace-only tokens (the Web
/// sends tokenized textareas; the CLI repeats `--evidence`). Unsafe refs survive to
/// be rejected by `closeout`, so a bad path is a refusal, not a silent drop.
pub fn parse_evidence_refs(raw: &[String]) -> Vec<DeliveryRef> {
    raw.iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse_evidence_ref(s))
        .collect()
}

fn closeout_summary(
    id: &str,
    commits: &[String],
    ci: &[String],
    reviews: &[String],
    doc_revisions: &[String],
) -> String {
    format!(
        "delivery {id} closed out: {} commit(s), {} CI, {} review(s), {} doc revision(s)",
        commits.len(),
        ci.len(),
        reviews.len(),
        doc_revisions.len()
    )
}

/// Record the PM accept verdict — the explicit human threshold (writes BOTH `Accept`
/// AND `PmAccept`; `by` is required, never inferred). Reads the run outcome via
/// `execute.run_id` and classifies it; `verified==true` is ONLY an input fact, never
/// an auto-accept. Verdict → stage. A failed/cancelled run allows only
/// `changes_requested`/`rejected`; a `Done && !verified` run requires explicit debt
/// to `accepted` (`accept_failed_with_debt` + non-empty `debt`) or `partial`
/// (non-empty `debt`).
#[allow(clippy::too_many_arguments)]
pub fn accept(
    delivery_id: &str,
    verdict: AcceptVerdict,
    by: String,
    notes: Option<String>,
    debt: Vec<String>,
    accept_failed_with_debt: bool,
    now: String,
) -> Result<DeliverySpec> {
    use crate::scheduler::state::{RunState, RunStatus};
    use AcceptVerdict::*;

    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if spec.stage != DeliveryStage::Execute {
        bail!(
            "delivery {delivery_id} is at stage {:?}; accept requires stage execute (run `delivery run` first)",
            spec.stage
        );
    }
    if verdict == Pending {
        bail!("`pending` is not a recordable verdict; choose accepted/partial/changes_requested/rejected");
    }

    // Load + classify the run outcome (missing/corrupt → error; running/gated → refuse).
    let run_id = spec
        .execute
        .as_ref()
        .and_then(|e| e.run_id.clone())
        .with_context(|| format!("delivery {delivery_id} has no linked run to accept"))?;
    let run_dir = paths::run_dir_for_id(&run_id)?;
    if !run_dir.join(paths::RUN_STATE_FILE).exists() {
        bail!(
            "delivery {delivery_id}: run {run_id} state is missing ({}); cannot accept",
            run_dir.display()
        );
    }
    let state = RunState::load(&run_dir)
        .with_context(|| format!("delivery {delivery_id}: run {run_id} RUN_STATE is corrupt"))?;

    if state.pending_gate.as_deref() == Some("outcome") {
        bail!("delivery {delivery_id}: run {run_id} is paused at the outcome gate; approve/cancel it before accepting");
    }
    match state.status {
        RunStatus::Running => {
            bail!("delivery {delivery_id}: run {run_id} is still running; cannot accept yet")
        }
        RunStatus::Failed | RunStatus::Cancelled => {
            if !matches!(verdict, ChangesRequested | Rejected) {
                bail!(
                    "delivery {delivery_id}: run {run_id} is {:?}; a failed/cancelled run can only be changes_requested or rejected (not {verdict:?})",
                    state.status
                );
            }
        }
        RunStatus::Done if !state.verified => match verdict {
            Accepted => {
                if !accept_failed_with_debt || debt.is_empty() {
                    bail!("delivery {delivery_id}: run {run_id} finished but acceptance did NOT pass (verified=false); to accept anyway pass --accept-failed-with-debt with a non-empty --debt");
                }
            }
            Partial => {
                if debt.is_empty() {
                    bail!("delivery {delivery_id}: a partial accept of an unverified run requires a non-empty --debt");
                }
            }
            _ => {} // changes_requested / rejected are fine
        },
        RunStatus::Done => {} // verified clean pass — any verdict
    }

    // Record Accept + PmAccept (the human threshold). acceptance_results_ref is
    // run-relative (resolve WITH execute.run_id).
    spec.accept = Some(Accept {
        verdict,
        acceptance_results_ref: Some(paths::RUN_STATE_FILE.to_string()),
        debt,
    });
    spec.pm_accept = Some(PmAccept {
        by: Some(by.clone()),
        at: Some(now.clone()),
        notes,
    });
    spec.stage = DeliveryStage::Accept;
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Accept,
        at: now.clone(),
        by: Some(by.clone()),
        reason: Some(format!("pm accept: {verdict:?}")),
    });
    // The verdict's terminal transition out of Accept.
    match verdict {
        ChangesRequested => {
            spec.stage = DeliveryStage::ChangesRequested;
            spec.audit.push(AuditEntry {
                stage: DeliveryStage::ChangesRequested,
                at: now,
                by: Some(by),
                reason: Some("changes requested".into()),
            });
        }
        Rejected => {
            spec.stage = DeliveryStage::Rejected;
            spec.audit.push(AuditEntry {
                stage: DeliveryStage::Rejected,
                at: now,
                by: Some(by),
                reason: Some("rejected".into()),
            });
        }
        _ => {} // accepted / partial stay at Accept (eligible for closeout)
    }
    save(&spec)?;
    Ok(spec)
}

/// Record the closeout (refs/summary only) and advance Accept→Closeout. Guard:
/// `stage == Accept` + `pm_accept` present + verdict ∈ {accepted, partial}. Refuses an
/// EMPTY closeout (needs ≥1 evidence ref). With `writeback`, emits a `delivery.closeout`
/// `OutboundReply` INTENT to the intake source doc (requires a locatable doc uri) via
/// the shared outbound helper; an enqueue failure fails the command (NO closeout
/// written, NO stage advance) — never a closeout that claims the doc was written.
#[allow(clippy::too_many_arguments)]
pub fn closeout(
    delivery_id: &str,
    commits: Vec<String>,
    ci: Vec<String>,
    reviews: Vec<String>,
    doc_revisions: Vec<String>,
    evidence_refs: Vec<DeliveryRef>,
    writeback: bool,
    now: String,
    by: Option<String>,
) -> Result<DeliverySpec> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if spec.stage != DeliveryStage::Accept {
        bail!(
            "delivery {delivery_id} is at stage {:?}; closeout requires stage accept (PM-accepted)",
            spec.stage
        );
    }
    if spec.pm_accept.is_none() {
        bail!("delivery {delivery_id} has no pm_accept; run `delivery accept` first");
    }
    let verdict = spec.accept.as_ref().map(|a| a.verdict);
    if !matches!(
        verdict,
        Some(AcceptVerdict::Accepted) | Some(AcceptVerdict::Partial)
    ) {
        bail!(
            "delivery {delivery_id} verdict is {verdict:?}; only accepted/partial can be closed out"
        );
    }
    // Empty-closeout guard: at least one evidence ref (writeback alone is not evidence).
    if commits.is_empty()
        && ci.is_empty()
        && reviews.is_empty()
        && doc_revisions.is_empty()
        && evidence_refs.is_empty()
    {
        bail!("delivery {delivery_id}: closeout needs at least one evidence ref (--commit/--ci/--review/--doc-revision/--evidence); --writeback alone is not evidence");
    }
    for r in &evidence_refs {
        if let Some(reason) = r.violation() {
            bail!("delivery {delivery_id}: closeout evidence ref invalid: {reason}");
        }
    }

    // Write-back: emit the intent BEFORE recording so an enqueue failure aborts the
    // whole closeout (no half-written record claiming the doc was updated).
    let wb = if writeback {
        let run_id = spec
            .execute
            .as_ref()
            .and_then(|e| e.run_id.clone())
            .with_context(|| format!("delivery {delivery_id} has no linked run for write-back"))?;
        let target = spec
            .intake
            .source_refs
            .iter()
            .find(|r| r.uri.as_deref().map(is_doc_uri).unwrap_or(false))
            .with_context(|| format!("delivery {delivery_id}: --writeback needs an intake source_ref with a Feishu/doc uri; none found"))?
            .clone();
        let run_dir = paths::run_dir_for_id(&run_id)?;
        let event_kind = "delivery.closeout";
        let title = format!("Delivery {delivery_id} closed out ({verdict:?})");
        let body = closeout_summary(delivery_id, &commits, &ci, &reviews, &doc_revisions);
        let doc_uri = target.uri.clone().unwrap_or_default();
        // F-134: compute + persist the idempotency key at emit (the SAME key on both the
        // OutboundReply the drainer reads and the Writeback the receipt is checked against).
        let idempotency_key =
            writeback_input_hash(delivery_id, &run_id, &doc_uri, event_kind, &title, &body);
        let reply = crate::channel::OutboundReply {
            channel: target.kind.clone(),
            run_id,
            event_kind: event_kind.into(),
            title,
            body,
            attachments: vec![crate::schema::artifacts::ArtifactRef {
                kind: "closeout_target_doc".into(),
                source: crate::schema::artifacts::ArtifactSource::External,
                task_id: None,
                path: None,
                uri: target.uri.clone(),
                name: target.name.clone(),
                bytes: None,
            }],
            delivery_id: Some(delivery_id.to_string()),
            idempotency_key: Some(idempotency_key.clone()),
        };
        crate::channel::append_outbound_reply(&run_dir, &reply).map_err(|e| {
            anyhow!("delivery {delivery_id}: --writeback failed to enqueue closeout intent: {e}")
        })?;
        Writeback {
            status: WritebackStatus::IntentEmitted,
            at: Some(now.clone()),
            doc_ref: Some(target),
            idempotency_key: Some(idempotency_key),
            receipt: None,
        }
    } else {
        Writeback {
            status: WritebackStatus::Skipped,
            at: None,
            doc_ref: None,
            idempotency_key: None,
            receipt: None,
        }
    };

    spec.closeout = Some(Closeout {
        commits,
        ci,
        reviews,
        doc_revisions,
        evidence_refs,
        writeback: Some(wb),
    });
    spec.stage = DeliveryStage::Closeout;
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Closeout,
        at: now,
        by,
        reason: Some("closed out".into()),
    });
    save(&spec)?;
    Ok(spec)
}

/// F-133: reopen a `changes_requested` delivery for rework (round N → N+1). The prior
/// round's `{plan,execute,accept,pm_accept,closeout}` are MOVED into `superseded_rounds`
/// (refs preserved — old `run_id`/`plan_hash`/verdict are NEVER lost or polluted by the
/// new round), then the live fields + `spec_confirm` reset so the new round MUST
/// re-confirm-spec + re-plan + re-run (the F-127b stale-linkage discipline). The `spec`
/// itself is kept (editable for the rework). All in-memory before a single `save` — a
/// write failure leaves the prior record intact (no half-reset state). Only reopenable
/// from `ChangesRequested`.
pub fn reopen(
    delivery_id: &str,
    by: Option<String>,
    reason: Option<String>,
    now: String,
) -> Result<DeliverySpec> {
    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    if spec.stage != DeliveryStage::ChangesRequested {
        bail!(
            "delivery {delivery_id} is at stage {:?}; reopen requires stage changes_requested \
             (only a PM changes_requested verdict can be reworked)",
            spec.stage
        );
    }
    let round = spec.superseded_rounds.len() as u32 + 1;
    // Archive the prior round FIRST (`.take()` moves the refs out AND clears the live
    // field in one step), so the old refs are captured before the reset.
    let superseded = SupersededRound {
        round,
        superseded_at: now.clone(),
        by: by.clone(),
        reason: reason.clone(),
        plan: spec.plan.take(),
        execute: spec.execute.take(),
        accept: spec.accept.take(),
        pm_accept: spec.pm_accept.take(),
        closeout: spec.closeout.take(),
    };
    spec.superseded_rounds.push(superseded);
    // Reset the confirmation threshold (the spec/PRD/acceptance is kept, editable).
    spec.spec_confirm = None;
    // Back to Spec — re-confirm + re-plan + re-run are required for the new round.
    spec.stage = DeliveryStage::Spec;
    let detail = match reason {
        Some(r) if !r.trim().is_empty() => format!(": {}", r.trim()),
        _ => String::new(),
    };
    spec.audit.push(AuditEntry {
        stage: DeliveryStage::Spec,
        at: now,
        by,
        reason: Some(format!(
            "reopened for rework (round {round}→{}){detail}",
            round + 1
        )),
    });
    save(&spec)?;
    Ok(spec)
}

/// F-134: the stable content hash identifying a closeout write-back intent. Canonical
/// payload — FIXED field order joined by the unit separator `\x1f`, with trimmed
/// title/body: `[delivery_id, run_id, doc_uri, event_kind, title, body]`. The body is
/// hashed at emit but NEVER stored in `DeliverySpec` (refs-first). Reuses the FNV1a
/// `stable_hash_bytes` (same hasher as plan/turn hashes).
fn writeback_input_hash(
    delivery_id: &str,
    run_id: &str,
    doc_uri: &str,
    event_kind: &str,
    title: &str,
    body: &str,
) -> String {
    let canonical = [
        delivery_id,
        run_id,
        doc_uri,
        event_kind,
        title.trim(),
        body.trim(),
    ]
    .join("\u{1f}");
    crate::file_guard::stable_hash_bytes(canonical.as_bytes())
}

/// F-134: record the write-back receipt the EXTERNAL drainer reports (maestro posts
/// nothing). The receipt is applied ONLY when `idempotency_key` matches the PERSISTED
/// `closeout.writeback.idempotency_key` (a stale receipt → error). `posted` requires a
/// durable external ref (`message_ref` or `doc_revision`); `failed` requires a non-empty
/// `error`. Reconcile: the same terminal + same payload re-report is a no-op; `failed →
/// posted` is allowed; `posted` is immutable (any differing receipt → error, surfacing a
/// double/wrong post). All checks run BEFORE the single `save` — no half-write.
#[allow(clippy::too_many_arguments)]
pub fn record_writeback_receipt(
    delivery_id: &str,
    idempotency_key: &str,
    status: WritebackStatus,
    message_ref: Option<String>,
    doc_revision: Option<String>,
    error: Option<String>,
    now: String,
) -> Result<DeliverySpec> {
    if !matches!(status, WritebackStatus::Posted | WritebackStatus::Failed) {
        bail!("delivery {delivery_id}: writeback receipt status must be posted or failed");
    }
    let norm = |s: Option<String>| s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let message_ref = norm(message_ref);
    let doc_revision = norm(doc_revision);
    let error = norm(error);
    if status == WritebackStatus::Posted && message_ref.is_none() && doc_revision.is_none() {
        bail!("delivery {delivery_id}: a posted receipt must carry a durable message_ref or doc_revision");
    }
    if status == WritebackStatus::Failed && error.is_none() {
        bail!("delivery {delivery_id}: a failed receipt must carry a non-empty error (audit evidence)");
    }

    let mut spec =
        read(delivery_id)?.with_context(|| format!("delivery {delivery_id} not found"))?;
    // Snapshot the current write-back (must exist + not be `skipped`).
    let (cur_status, cur_key, cur_refs) = {
        let wb = spec
            .closeout
            .as_ref()
            .and_then(|c| c.writeback.as_ref())
            .filter(|w| w.status != WritebackStatus::Skipped)
            .with_context(|| {
                format!("delivery {delivery_id}: no write-back intent to receipt (closeout missing or --writeback was skipped)")
            })?;
        let refs = wb.receipt.as_ref().map(|r| {
            (
                r.message_ref.clone(),
                r.doc_revision.clone(),
                r.error.clone(),
            )
        });
        (wb.status, wb.idempotency_key.clone(), refs)
    };
    // The receipt is checked against the PERSISTED key, never a recomputed one.
    if cur_key.as_deref() != Some(idempotency_key) {
        bail!("delivery {delivery_id}: receipt idempotency_key does not match the recorded write-back intent (stale receipt)");
    }
    let new_refs = (message_ref.clone(), doc_revision.clone(), error.clone());
    // Same terminal + same payload (ignoring the timestamp) → idempotent no-op.
    if cur_status == status && cur_refs.as_ref() == Some(&new_refs) {
        return Ok(spec);
    }
    // `posted` is immutable — any differing receipt is a conflict (double/wrong post).
    if cur_status == WritebackStatus::Posted {
        bail!("delivery {delivery_id}: write-back is already posted (immutable); a differing receipt is a conflict (possible double/wrong post)");
    }
    let wb = spec
        .closeout
        .as_mut()
        .and_then(|c| c.writeback.as_mut())
        .expect("write-back present (checked above)");
    wb.status = status;
    wb.receipt = Some(WritebackReceipt {
        message_ref,
        doc_revision,
        error,
        at: now,
    });
    save(&spec)?;
    Ok(spec)
}

// ── intake parser ────────────────────────────────────────────────────────────

/// Required intake fields. A missing one becomes a BLOCKING clarify question
/// instead of a silent empty (per the F-127a contract).
const REQUIRED: &[(&str, &str)] = &[
    ("objective", "What is the requirement / problem to solve?"),
    ("target_users", "Who are the target users?"),
    (
        "acceptance",
        "What is the success metric / acceptance criteria?",
    ),
];

/// Build a `DeliverySpec` (stage `intake`, or `clarify` when a required field is
/// missing) from a markdown / plain-text requirement. `source` is recorded as a
/// ref only — the body is never copied into the record.
pub fn parse_intake(
    delivery_id: impl Into<String>,
    text: &str,
    source: DeliveryRef,
    created_at: String,
    proposer: Option<String>,
) -> DeliverySpec {
    let objective = field(text, &["objective", "goal", "requirement", "需求", "目标"])
        .or_else(|| first_nonempty_line(text));
    let intake = Intake {
        objective: objective.clone().unwrap_or_default(),
        background: field(text, &["background", "背景", "context"]),
        target_users: field(
            text,
            &["target users", "target_users", "users", "用户", "目标用户"],
        ),
        business_goal: field(text, &["business goal", "business_goal", "业务目标"]),
        constraints: list_field(text, &["constraints", "约束", "限制"]),
        time_constraint: field(text, &["time", "timeline", "deadline", "时间", "排期"]),
        acceptance: list_field(
            text,
            &["acceptance", "acceptance criteria", "成功指标", "验收"],
        )
        .into_iter()
        .map(|describe| AcceptanceCriterion {
            describe,
            check: None,
        })
        .collect(),
        source_refs: vec![source],
    };

    // Missing required → BLOCKING clarify questions (never silent).
    let mut questions = Vec::new();
    for (key, q) in REQUIRED {
        let present = match *key {
            "objective" => !intake.objective.trim().is_empty(),
            "target_users" => intake.target_users.is_some(),
            "acceptance" => !intake.acceptance.is_empty(),
            _ => true,
        };
        if !present {
            questions.push(ClarifyQuestion {
                q: q.to_string(),
                blocking: true,
                answer: None,
                by: None,
                at: None,
            });
        }
    }

    let stage = if questions.is_empty() {
        DeliveryStage::Intake
    } else {
        DeliveryStage::Clarify
    };

    DeliverySpec {
        schema_version: crate::schema::DELIVERY_SPEC_V1.to_string(),
        delivery_id: delivery_id.into(),
        stage,
        proposer,
        created_at: created_at.clone(),
        intake,
        triage: None,
        clarify: Clarify {
            questions,
            ..Default::default()
        },
        spec: None,
        spec_confirm: None,
        plan: None,
        execute: None,
        accept: None,
        pm_accept: None,
        evidence_refs: vec![],
        closeout: None,
        audit: vec![AuditEntry {
            stage,
            at: created_at,
            by: None,
            reason: Some("intake parsed".into()),
        }],
        superseded_rounds: vec![],
    }
}

/// Find a single-value field: a `key: value` line (frontmatter or inline) OR the
/// first paragraph under a markdown heading whose text matches a key.
fn field(text: &str, keys: &[&str]) -> Option<String> {
    if let Some(v) = kv_line(text, keys) {
        return Some(v);
    }
    heading_section(text, keys).and_then(|body| {
        let first = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('-') && !l.starts_with('*'))
            .map(str::to_string);
        first
    })
}

/// Find a list field: bullet lines under a matching heading, or a `key: a, b` line.
fn list_field(text: &str, keys: &[&str]) -> Vec<String> {
    if let Some(body) = heading_section(text, keys) {
        let items: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix('-').or_else(|| l.strip_prefix('*')))
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if !items.is_empty() {
            return items;
        }
    }
    if let Some(v) = kv_line(text, keys) {
        return v
            .split([',', '，', ';', '；'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    Vec::new()
}

/// A `key: value` line (case-insensitive key match), e.g. frontmatter / inline.
fn kv_line(text: &str, keys: &[&str]) -> Option<String> {
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k
                .trim()
                .trim_start_matches(['#', '-', '*', ' '])
                .to_lowercase();
            if keys.iter().any(|key| k == key.to_lowercase()) {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// The body under a markdown heading (`#`..`######`) whose text matches a key,
/// up to the next heading.
fn heading_section(text: &str, keys: &[&str]) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix('#') {
            let title = h.trim_start_matches('#').trim().to_lowercase();
            if keys.iter().any(|key| title == key.to_lowercase()) {
                let body: String = lines[i + 1..]
                    .iter()
                    .take_while(|l| !l.trim_start().starts_with('#'))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                return Some(body);
            }
        }
    }
    None
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("---"))
        .map(|l| l.trim_start_matches(['#', ' ']).trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> DeliveryRef {
        DeliveryRef {
            kind: "doc".into(),
            path: None,
            uri: Some("https://example.invalid/docx/abc".into()),
            name: None,
        }
    }

    #[test]
    fn parses_structured_markdown_into_intake() {
        let md = "\
## Objective
Ship the export feature.

## Target Users
Analysts.

## Acceptance
- CSV downloads
- numbers match the dashboard

## Constraints
- no schema change
";
        let d = parse_intake("d-1", md, src(), "2026-06-08T00:00:00Z".into(), None);
        assert_eq!(d.stage, DeliveryStage::Intake);
        assert_eq!(d.intake.objective, "Ship the export feature.");
        assert_eq!(d.intake.target_users.as_deref(), Some("Analysts."));
        assert_eq!(d.intake.acceptance.len(), 2);
        assert_eq!(d.intake.constraints.len(), 1);
        assert!(d.clarify.questions.is_empty());
        assert_eq!(d.intake.source_refs.len(), 1);
    }

    #[test]
    fn missing_required_fields_become_blocking_clarify_not_silent() {
        let md = "## Objective\nDo a thing.\n"; // no target_users, no acceptance
        let d = parse_intake("d-2", md, src(), "2026-06-08T00:00:00Z".into(), None);
        assert_eq!(d.stage, DeliveryStage::Clarify, "blocked → clarify stage");
        assert!(d.intake.target_users.is_none());
        assert!(d.intake.acceptance.is_empty());
        let blocking: Vec<_> = d.clarify.questions.iter().filter(|q| q.blocking).collect();
        assert_eq!(blocking.len(), 2, "target_users + acceptance missing");
        // never silently faked
        assert!(blocking.iter().all(|q| q.answer.is_none()));
    }

    #[test]
    fn supports_frontmatter_kv_lines() {
        let txt = "objective: Add login\ntarget_users: end users\nacceptance: works, secure\n";
        let d = parse_intake("d-3", txt, src(), "2026-06-08T00:00:00Z".into(), None);
        assert_eq!(d.intake.objective, "Add login");
        assert_eq!(d.intake.acceptance.len(), 2);
        assert!(d.clarify.questions.is_empty());
    }
}
