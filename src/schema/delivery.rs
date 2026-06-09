//! F-127 — the `DeliverySpec`: a single machine-readable record binding a PM
//! requirement, PRD, plan, run, evidence, acceptance, and closeout together.
//!
//! Refs-first: it stores ids / run-local paths / external uris, NEVER a copy of
//! the Feishu doc body or the run state. F-127a fills the discovery half
//! (intake / triage / clarify) + the stage cursor; the spec / plan / execute /
//! accept / closeout sections are the data spine for F-127b/c.

use serde::{Deserialize, Serialize};

use crate::schema::artifacts::{ref_path_is_unsafe, ref_uri_is_unsafe};

/// The 7-state lifecycle cursor plus the bypass states. Forward transitions are
/// gated by recorded human thresholds (`spec_confirm`, `pm_accept`) and the
/// existing run gates — there is no new gate engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage {
    Intake,
    Clarify,
    Spec,
    Plan,
    Execute,
    Accept,
    Closeout,
    // bypass
    Rejected,
    Parked,
    Duplicate,
    ChangesRequested,
    Cancelled,
}

impl DeliveryStage {
    /// An active stage in the forward chain — one the delivery can still move out
    /// of. NOTE: `Closeout` is included and is treated as "closeout in progress",
    /// NOT a terminal state: it is the last forward stage and may still bypass to
    /// `Cancelled` etc. The forward chain has no terminal stage of its own;
    /// "delivered" is recorded by the `pm_accept` threshold + `Closeout` contents,
    /// not by an `is_active()==false` stage. The bypass states
    /// (`Rejected`/`Parked`/`Duplicate`/`ChangesRequested`/`Cancelled`) are the
    /// only inactive ones.
    pub fn is_active(self) -> bool {
        use DeliveryStage::*;
        matches!(
            self,
            Intake | Clarify | Spec | Plan | Execute | Accept | Closeout
        )
    }

    /// Is `next` a legal transition from `self`? Forward chain is strict
    /// (each stage → only the next); any active stage may bypass to
    /// rejected/parked/duplicate/cancelled; `accept` may flag
    /// `changes_requested`, which re-opens to `clarify`/`spec`.
    pub fn can_transition_to(self, next: DeliveryStage) -> bool {
        use DeliveryStage::*;
        let forward = matches!(
            (self, next),
            (Intake, Clarify)
                | (Clarify, Spec)
                | (Spec, Plan)
                | (Plan, Execute)
                | (Execute, Accept)
                | (Accept, Closeout)
        );
        let bypass = self.is_active() && matches!(next, Rejected | Parked | Duplicate | Cancelled);
        let changes = matches!((self, next), (Accept, ChangesRequested))
            || matches!(
                (self, next),
                (ChangesRequested, Clarify) | (ChangesRequested, Spec)
            );
        forward || bypass || changes
    }
}

/// One PM-stated acceptance criterion. Same shape as `config::Goal.acceptance`
/// (`describe` + a shell `check`); at intake the `check` may be unset (it becomes
/// concrete when the spec is shaped into a PLAN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub describe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// A run-local path or external uri reference (never an inline body). Reuses the
/// F-124 run-local rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRef {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl DeliveryRef {
    /// `Some(reason)` if this ref is not run-local: an unsafe `path` (absolute /
    /// UNC / drive / `..`) or a `file:` `uri`. A name-only ref (no path/uri) is an
    /// opaque named reference and is allowed.
    pub fn violation(&self) -> Option<String> {
        if let Some(p) = self.path.as_deref() {
            if ref_path_is_unsafe(p) {
                return Some(format!("ref path must be run-relative: {p:?}"));
            }
        }
        if let Some(u) = self.uri.as_deref() {
            if ref_uri_is_unsafe(u) {
                return Some(format!("ref uri must not use a file: scheme: {u:?}"));
            }
        }
        None
    }
}

/// A free-string ref (`plan_path` / `preview_ref` / `acceptance_results_ref`) must
/// be run-local too. A bare string's nature (path vs uri) is ambiguous, so BOTH
/// rules apply: an unsafe path (absolute / UNC / drive-letter / `..`) AND a `file:`
/// uri are rejected. Legit run-relative paths and remote (`https:`) uris pass both.
fn string_ref_violation(label: &str, value: &str) -> Option<String> {
    if ref_path_is_unsafe(value) {
        return Some(format!(
            "{label} must be run-relative (no absolute / UNC / drive-letter / '..'): {value:?}"
        ));
    }
    if ref_uri_is_unsafe(value) {
        return Some(format!("{label} must not use a file: scheme: {value:?}"));
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intake {
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_users: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_goal: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<AcceptanceCriterion>,
    /// The inbound requirement (Feishu/doc/markdown) as a ref, NEVER its body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<DeliveryRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageDecision {
    AcceptedForDiscovery,
    Parked,
    Rejected,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    pub decision: TriageDecision,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarifyQuestion {
    pub q: String,
    /// A blocking question must be answered before `clarify → spec`.
    pub blocking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clarify {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<ClarifyQuestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred: Vec<String>,
}

impl Clarify {
    /// Blocking questions that are still unanswered (these refuse `clarify→spec`).
    pub fn open_blocking(&self) -> usize {
        self.questions
            .iter()
            .filter(|q| q.blocking && q.answer.as_deref().map(str::trim).unwrap_or("").is_empty())
            .count()
    }
}

// ── data spine for F-127b/c (defined now, populated later) ───────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub prd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<String>,
    /// Registered project names this delivery targets — the `selected` input to
    /// `plan::synthesize`. Empty = incomplete (a plan can't be scoped). Each must
    /// exist in `projects.yaml` (validated by the store, not the schema).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_goals: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appetite: Option<String>,
}

/// Recorded threshold for `spec → plan` (audit row + guard, NOT a gate engine).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecConfirm {
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRef {
    pub plan_path: String,
    pub plan_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptVerdict {
    Pending,
    Accepted,
    ChangesRequested,
    Partial,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accept {
    pub verdict: AcceptVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_results_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub debt: Vec<String>,
}

/// Recorded threshold for `accept → closeout` (CI-green alone is not "delivered").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmAccept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closeout {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ci: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doc_revisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<DeliveryRef>,
    /// Status of the optional Feishu/doc write-back (F-127c `--writeback`). maestro
    /// has NO in-process doc write — this records that an `OutboundReply` INTENT was
    /// emitted (or skipped), never that the doc was actually written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeback: Option<Writeback>,
}

/// F-134: the DURABLE terminal states maestro records. The drainer's transient steps
/// (drained / posting) stay drainer-side; maestro records only what it can audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritebackStatus {
    /// No `--writeback` was requested.
    Skipped,
    /// A `delivery.closeout` `OutboundReply` was appended to the run's outbound
    /// queue for an external skill to post. The actual Feishu post is async/external
    /// — this is NOT a claim that the doc was written.
    IntentEmitted,
    /// F-134: the external drainer reported a successful post — `receipt` carries a
    /// durable external ref (`message_ref` and/or `doc_revision`).
    Posted,
    /// F-134: the external drainer reported a failed post — `receipt.error` carries why.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writeback {
    pub status: WritebackStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The target doc the intent addressed (the intake source doc). A ref only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_ref: Option<DeliveryRef>,
    /// F-134: the stable content hash of THIS write-back intent (computed + persisted at
    /// emit). A receipt is only applied if its `idempotency_key` matches this — so a
    /// stale receipt (after a later closeout/edit) can't be applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// F-134: the receipt the external drainer reported (posted/failed). Refs only —
    /// never the Feishu body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<WritebackReceipt>,
}

/// F-134: the external post receipt — durable refs only (no Feishu body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritebackReceipt {
    /// The posted message's external ref (e.g. a Feishu message id/url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// The updated doc's revision ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_revision: Option<String>,
    /// The failure reason (audit evidence) — required on a `failed` receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub stage: DeliveryStage,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The single delivery record (`maestro.delivery_spec.v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySpec {
    #[serde(default = "crate::schema::delivery_spec_version")]
    pub schema_version: String,
    pub delivery_id: String,
    pub stage: DeliveryStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    pub created_at: String,
    pub intake: Intake,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<Triage>,
    #[serde(default)]
    pub clarify: Clarify,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Spec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_confirm: Option<SpecConfirm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute: Option<ExecuteRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Accept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pm_accept: Option<PmAccept>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<DeliveryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closeout: Option<Closeout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audit: Vec<AuditEntry>,
    /// F-133: archived prior rounds (the reopen/rework loop). On reopen the live
    /// `{plan,execute,accept,pm_accept,closeout}` are MOVED here (refs-first) before the
    /// live fields reset — the prior round's `run_id`/`plan_hash`/verdict are preserved,
    /// never silently deleted, and never polluted by a later round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_rounds: Vec<SupersededRound>,
}

/// F-133: one archived round, kept by reference (no RUN_STATE / Feishu body / evidence
/// copy — these are the same refs-first records the live spine holds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersededRound {
    /// 1-based number of the round being superseded.
    pub round: u32,
    pub superseded_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute: Option<ExecuteRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Accept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pm_accept: Option<PmAccept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closeout: Option<Closeout>,
}

impl DeliverySpec {
    /// Validate the machine-readable contract: a supported `schema_version` AND
    /// every ref run-local. Used on read so a tampered or foreign `DELIVERY.json`
    /// (wrong schema, or a ref smuggling an absolute path / `file:` uri) is an
    /// explicit error, never silently projected as a current-version record.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != crate::schema::DELIVERY_SPEC_V1 {
            return Err(format!(
                "unsupported schema_version {:?} (expected {})",
                self.schema_version,
                crate::schema::DELIVERY_SPEC_V1
            ));
        }
        if let Some(reason) = self.ref_violation() {
            return Err(reason);
        }
        Ok(())
    }

    /// Every ref must be run-local — used on read AND write so a tampered
    /// `DELIVERY.json` can't smuggle a non-run-local path/uri. Covers both the
    /// `DeliveryRef`-typed refs (intake/evidence/closeout) AND the free-string
    /// refs in the F-127b/c spine (`plan.plan_path`, `plan.preview_ref`,
    /// `accept.acceptance_results_ref`).
    pub fn ref_violation(&self) -> Option<String> {
        let typed = self
            .intake
            .source_refs
            .iter()
            .chain(self.evidence_refs.iter())
            .chain(self.closeout.iter().flat_map(|c| c.evidence_refs.iter()))
            .chain(
                self.closeout
                    .iter()
                    .flat_map(|c| c.writeback.iter())
                    .flat_map(|w| w.doc_ref.iter()),
            )
            .find_map(DeliveryRef::violation);
        if typed.is_some() {
            return typed;
        }
        if let Some(p) = self.plan.as_ref() {
            if let Some(v) = string_ref_violation("plan.plan_path", &p.plan_path) {
                return Some(v);
            }
            if let Some(r) = p.preview_ref.as_deref() {
                if let Some(v) = string_ref_violation("plan.preview_ref", r) {
                    return Some(v);
                }
            }
        }
        if let Some(a) = self.accept.as_ref() {
            if let Some(r) = a.acceptance_results_ref.as_deref() {
                if let Some(v) = string_ref_violation("accept.acceptance_results_ref", r) {
                    return Some(v);
                }
            }
        }
        // F-133: archived rounds carry the same refs — validate them too, so a tampered
        // `superseded_rounds` can't smuggle a non-run-local path/uri (F-127a lesson).
        for sr in &self.superseded_rounds {
            if let Some(c) = sr.closeout.as_ref() {
                if let Some(v) = c
                    .evidence_refs
                    .iter()
                    .chain(c.writeback.iter().flat_map(|w| w.doc_ref.iter()))
                    .find_map(DeliveryRef::violation)
                {
                    return Some(v);
                }
            }
            if let Some(p) = sr.plan.as_ref() {
                if let Some(v) = string_ref_violation("superseded plan.plan_path", &p.plan_path) {
                    return Some(v);
                }
                if let Some(r) = p.preview_ref.as_deref() {
                    if let Some(v) = string_ref_violation("superseded plan.preview_ref", r) {
                        return Some(v);
                    }
                }
            }
            if let Some(a) = sr.accept.as_ref() {
                if let Some(r) = a.acceptance_results_ref.as_deref() {
                    if let Some(v) =
                        string_ref_violation("superseded accept.acceptance_results_ref", r)
                    {
                        return Some(v);
                    }
                }
            }
        }
        None
    }

    /// Structural gaps that stop `spec` from shaping a runnable plan (pure — the
    /// store ALSO checks each target project exists in `projects.yaml`). Empty =
    /// structurally complete. `confirm-spec` refuses + records these as blocking
    /// clarify questions; `delivery plan` re-checks them (anti-tamper). Never
    /// fabricates a missing field.
    pub fn spec_structural_gaps(&self) -> Vec<String> {
        let mut gaps = Vec::new();
        let spec = self.spec.as_ref();
        let has_objective = spec.map(|s| !s.prd.trim().is_empty()).unwrap_or(false)
            || !self.intake.objective.trim().is_empty();
        if !has_objective {
            gaps.push("missing prd/objective — what is being built?".into());
        }
        match spec {
            None => gaps.push("no spec recorded — run `delivery spec` first".into()),
            Some(s) => {
                if s.target_projects.is_empty() {
                    gaps.push(
                        "missing target_projects — which registered projects does this touch?"
                            .into(),
                    );
                }
                if s.acceptance.is_empty() {
                    gaps.push("missing acceptance — what is the success check?".into());
                }
                for a in &s.acceptance {
                    if a.check.as_deref().map(str::trim).unwrap_or("").is_empty() {
                        gaps.push(format!("acceptance '{}' has no runnable check", a.describe));
                    }
                }
            }
        }
        gaps
    }
}

// ── read-only stage projection ───────────────────────────────────────────────

/// What the delivery is blocked on at its current stage (read-only audit signal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedOn {
    /// `clarify`: N blocking questions unanswered.
    OpenClarifyQuestions { count: u32 },
    /// `spec`: awaiting human `spec_confirm` before a plan is generated.
    SpecConfirm,
    /// `accept`: awaiting the PM `pm_accept` verdict.
    PmAccept,
    /// Nothing blocks the current stage's forward transition.
    Nothing,
}

/// Read-only projection of a delivery's lifecycle state. Pure over the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryView {
    pub schema_version: String,
    pub delivery_id: String,
    pub stage: DeliveryStage,
    pub blocked_on: BlockedOn,
    pub source_refs: Vec<DeliveryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_verdict: Option<AcceptVerdict>,
    /// Who recorded the PM-accept verdict (the `pm_accept` threshold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pm_accepted_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closeout_summary: Option<String>,
    /// Status of the closeout Feishu/doc write-back (skipped/intent_emitted/posted/failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeback_status: Option<WritebackStatus>,
    /// F-134: the external post receipt (message/doc ref or error) — refs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeback_receipt: Option<WritebackReceipt>,
    /// F-136a1: worst-case RuntimeProfile + per-profile breakdown across the linked
    /// run's tasks. `None` when no run is linked yet (never fabricated). `project`
    /// leaves this `None` — it is enriched (loads the run) where the view is served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_profile_summary: Option<crate::schema::runtime_profile::RuntimeProfileSummary>,
    /// F-133: the current rework round (1-based; `superseded_rounds.len() + 1`).
    pub round: u32,
    /// F-133: how many prior rounds were superseded by reopen.
    pub superseded_count: u32,
}

impl DeliveryView {
    pub const SCHEMA_VERSION: &'static str = "maestro.delivery_view.v1";

    pub fn project(d: &DeliverySpec) -> Self {
        let blocked_on = match d.stage {
            DeliveryStage::Clarify => {
                let n = d.clarify.open_blocking() as u32;
                if n > 0 {
                    BlockedOn::OpenClarifyQuestions { count: n }
                } else {
                    BlockedOn::Nothing
                }
            }
            DeliveryStage::Spec
                if !d
                    .spec_confirm
                    .as_ref()
                    .map(|s| s.confirmed)
                    .unwrap_or(false) =>
            {
                BlockedOn::SpecConfirm
            }
            DeliveryStage::Accept
                if d.pm_accept.is_none()
                    || d.accept
                        .as_ref()
                        .map(|a| a.verdict == AcceptVerdict::Pending)
                        .unwrap_or(true) =>
            {
                BlockedOn::PmAccept
            }
            _ => BlockedOn::Nothing,
        };
        let closeout_summary = d.closeout.as_ref().map(|c| {
            format!(
                "{} commit(s), {} CI, {} review(s), {} doc revision(s)",
                c.commits.len(),
                c.ci.len(),
                c.reviews.len(),
                c.doc_revisions.len()
            )
        });
        DeliveryView {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            delivery_id: d.delivery_id.clone(),
            stage: d.stage,
            blocked_on,
            source_refs: d.intake.source_refs.clone(),
            run_id: d.execute.as_ref().and_then(|e| e.run_id.clone()),
            plan_path: d.plan.as_ref().map(|p| p.plan_path.clone()),
            accept_verdict: d.accept.as_ref().map(|a| a.verdict),
            pm_accepted_by: d.pm_accept.as_ref().and_then(|p| p.by.clone()),
            closeout_summary,
            writeback_status: d
                .closeout
                .as_ref()
                .and_then(|c| c.writeback.as_ref())
                .map(|w| w.status),
            writeback_receipt: d
                .closeout
                .as_ref()
                .and_then(|c| c.writeback.as_ref())
                .and_then(|w| w.receipt.clone()),
            // Pure projection leaves the run-derived summary empty; the serving
            // layer enriches it (`delivery::project_view`) since it needs run IO.
            runtime_profile_summary: None,
            superseded_count: d.superseded_rounds.len() as u32,
            round: d.superseded_rounds.len() as u32 + 1,
        }
    }
}

/// One entry in the `GET /api/deliveries` list (F-128). Either the projected `view`
/// (`status:"ok"`) or a visible corrupt stub (`status:"corrupt"`) — a corrupt /
/// unreadable `DELIVERY.json` is SHOWN in the list, never silently omitted; clicking
/// it fetches the detail endpoint which returns the 500 + reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeliveryListEntry {
    Ok {
        delivery_id: String,
        view: Box<DeliveryView>,
    },
    Corrupt {
        delivery_id: String,
        error: String,
    },
}

/// F-131: a SLIM run-status projection for the Delivery detail's inline live status —
/// deliberately NOT the full `RunMonitor`. `linked` distinguishes a run resolved from
/// `execute.run_id` (true) from one found via the in-flight `RunState.delivery_id`
/// back-ref (false — a detached run still mid-flight, before `start_run` writes the
/// ExecuteRef back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatusLite {
    pub run_id: String,
    /// `running | done | failed | cancelled` (the `RunStatus` serde token).
    pub status: String,
    pub linked: bool,
    pub progress: RunStatusProgress,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatusProgress {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
    pub running: u32,
    /// pending + awaiting_approval (work not yet settled).
    pub pending: u32,
}

/// F-132: one event in the Delivery audit timeline — a projection of an `AuditEntry`
/// (the chronological who/when/why spine) enriched with refs-first pointers from the
/// matching lifecycle node. NEVER carries copied RUN_STATE / Feishu body / evidence
/// blobs — only ids / refs / uris / summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub at: String,
    pub stage: DeliveryStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<TimelineRef>,
}

/// A refs-first pointer on a timeline event: `kind` + at most an id `value`, a `uri`,
/// and a short `summary`. No copied bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineRef {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intake_spec() -> DeliverySpec {
        DeliverySpec {
            schema_version: crate::schema::DELIVERY_SPEC_V1.to_string(),
            delivery_id: "d-1".into(),
            stage: DeliveryStage::Intake,
            proposer: Some("pm".into()),
            created_at: "2026-06-08T00:00:00Z".into(),
            intake: Intake {
                objective: "ship X".into(),
                source_refs: vec![DeliveryRef {
                    kind: "feishu_doc".into(),
                    path: None,
                    uri: Some("https://example.invalid/docx/abc".into()),
                    name: None,
                }],
                ..Default::default()
            },
            triage: None,
            clarify: Clarify::default(),
            spec: None,
            spec_confirm: None,
            plan: None,
            execute: None,
            accept: None,
            pm_accept: None,
            evidence_refs: vec![],
            closeout: None,
            audit: vec![],
            superseded_rounds: vec![],
        }
    }

    #[test]
    fn serde_round_trips() {
        let d = intake_spec();
        let back: DeliverySpec = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn legal_and_illegal_transitions() {
        use DeliveryStage::*;
        assert!(Intake.can_transition_to(Clarify));
        assert!(Clarify.can_transition_to(Spec));
        assert!(Accept.can_transition_to(Closeout));
        // illegal: skipping a stage
        assert!(!Intake.can_transition_to(Spec));
        assert!(!Intake.can_transition_to(Closeout));
        assert!(!Clarify.can_transition_to(Plan));
        // bypass from an active stage
        assert!(Intake.can_transition_to(Rejected));
        assert!(Clarify.can_transition_to(Parked));
        assert!(Accept.can_transition_to(ChangesRequested));
        assert!(ChangesRequested.can_transition_to(Clarify));
        // a bypass state can't jump forward
        assert!(!Rejected.can_transition_to(Clarify));
    }

    #[test]
    fn ref_violation_rejects_absolute_path_and_file_uri() {
        let mut d = intake_spec();
        assert!(d.ref_violation().is_none(), "https uri ref is fine");
        d.intake.source_refs.push(DeliveryRef {
            kind: "log".into(),
            path: Some("/abs/secret.md".into()),
            uri: None,
            name: None,
        });
        assert!(
            d.ref_violation().is_some(),
            "absolute path must be rejected"
        );

        let mut d2 = intake_spec();
        d2.evidence_refs.push(DeliveryRef {
            kind: "x".into(),
            path: None,
            uri: Some("file:///etc/passwd".into()),
            name: None,
        });
        assert!(d2.ref_violation().is_some(), "file: uri must be rejected");
    }

    #[test]
    fn ref_violation_rejects_non_run_local_plan_and_accept_string_refs() {
        // Run-local plan/accept string refs pass.
        let mut ok = intake_spec();
        ok.plan = Some(PlanRef {
            plan_path: "PLAN.yaml".into(),
            plan_hash: "abc".into(),
            preview_ref: Some("outputs/preview.json".into()),
        });
        ok.accept = Some(Accept {
            verdict: AcceptVerdict::Pending,
            acceptance_results_ref: Some("outputs/results.json".into()),
            debt: vec![],
        });
        assert!(
            ok.ref_violation().is_none(),
            "run-local plan/accept refs are fine"
        );

        // Blocker-1 repro: absolute plan_path must be rejected.
        let mut d = intake_spec();
        d.plan = Some(PlanRef {
            plan_path: "/abs/PLAN.yaml".into(),
            plan_hash: "abc".into(),
            preview_ref: None,
        });
        assert!(
            d.ref_violation().is_some(),
            "absolute plan_path must be rejected"
        );

        // file: preview_ref must be rejected.
        let mut d = intake_spec();
        d.plan = Some(PlanRef {
            plan_path: "PLAN.yaml".into(),
            plan_hash: "abc".into(),
            preview_ref: Some("file:///etc/passwd".into()),
        });
        assert!(
            d.ref_violation().is_some(),
            "file: preview_ref must be rejected"
        );

        // absolute acceptance_results_ref must be rejected.
        let mut d = intake_spec();
        d.accept = Some(Accept {
            verdict: AcceptVerdict::Accepted,
            acceptance_results_ref: Some("/var/results.json".into()),
            debt: vec![],
        });
        assert!(
            d.ref_violation().is_some(),
            "absolute acceptance_results_ref must be rejected"
        );
    }

    #[test]
    fn spec_structural_gaps_flags_each_missing_field() {
        // No spec at all → "no spec" gap (objective still satisfied by intake).
        let d = intake_spec();
        let gaps = d.spec_structural_gaps();
        assert!(
            gaps.iter().any(|g| g.contains("no spec recorded")),
            "{gaps:?}"
        );

        // A spec missing target_projects + acceptance, and no objective anywhere.
        let mut d = intake_spec();
        d.intake.objective = String::new();
        d.spec = Some(Spec {
            prd: String::new(),
            ..Default::default()
        });
        let gaps = d.spec_structural_gaps();
        assert!(gaps.iter().any(|g| g.contains("prd/objective")), "{gaps:?}");
        assert!(
            gaps.iter().any(|g| g.contains("target_projects")),
            "{gaps:?}"
        );
        assert!(gaps.iter().any(|g| g.contains("acceptance")), "{gaps:?}");

        // An acceptance criterion without a runnable check is a gap (not silent).
        let mut d = intake_spec();
        d.spec = Some(Spec {
            prd: "ship it".into(),
            target_projects: vec!["proj-a".into()],
            acceptance: vec![AcceptanceCriterion {
                describe: "works".into(),
                check: None,
            }],
            ..Default::default()
        });
        let gaps = d.spec_structural_gaps();
        assert!(
            gaps.iter().any(|g| g.contains("no runnable check")),
            "{gaps:?}"
        );

        // A structurally complete spec has no gaps.
        let mut d = intake_spec();
        d.spec = Some(Spec {
            prd: "ship it".into(),
            target_projects: vec!["proj-a".into()],
            acceptance: vec![AcceptanceCriterion {
                describe: "works".into(),
                check: Some("test 1 = 1".into()),
            }],
            ..Default::default()
        });
        assert!(
            d.spec_structural_gaps().is_empty(),
            "{:?}",
            d.spec_structural_gaps()
        );
    }

    #[test]
    fn closeout_writeback_serde_round_trips_and_legacy_is_none() {
        let mut d = intake_spec();
        d.closeout = Some(Closeout {
            commits: vec!["abc1234".into()],
            ci: vec!["https://ci/run/1".into()],
            writeback: Some(Writeback {
                status: WritebackStatus::IntentEmitted,
                at: Some("2026-06-08T00:00:00Z".into()),
                doc_ref: Some(DeliveryRef {
                    kind: "intake_doc".into(),
                    path: None,
                    uri: Some("https://example.invalid/docx/abc".into()),
                    name: None,
                }),
                idempotency_key: None,
                receipt: None,
            }),
            ..Default::default()
        });
        let back: DeliverySpec = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);

        // A legacy closeout without `writeback` deserializes to None.
        let c: Closeout = serde_json::from_str(r#"{"commits":["x"]}"#).unwrap();
        assert!(c.writeback.is_none());

        // A write-back doc_ref is validated run-local (a file: uri is rejected).
        let mut bad = intake_spec();
        bad.closeout = Some(Closeout {
            commits: vec!["c".into()],
            writeback: Some(Writeback {
                status: WritebackStatus::IntentEmitted,
                at: None,
                doc_ref: Some(DeliveryRef {
                    kind: "x".into(),
                    path: None,
                    uri: Some("file:///etc/passwd".into()),
                    name: None,
                }),
                idempotency_key: None,
                receipt: None,
            }),
            ..Default::default()
        });
        assert!(
            bad.ref_violation().is_some(),
            "file: writeback doc_ref rejected"
        );
    }

    #[test]
    fn validate_rejects_unsupported_schema_version() {
        assert!(intake_spec().validate().is_ok(), "v1 record validates");

        // Blocker-2 repro: a foreign schema_version is an explicit error.
        let mut bad = intake_spec();
        bad.schema_version = "maestro.delivery_spec.v999".into();
        let err = bad.validate().unwrap_err();
        assert!(err.contains("unsupported schema_version"), "got: {err}");

        // validate() also surfaces a ref violation.
        let mut bad_ref = intake_spec();
        bad_ref.intake.source_refs.push(DeliveryRef {
            kind: "x".into(),
            path: Some("/abs".into()),
            uri: None,
            name: None,
        });
        assert!(bad_ref.validate().is_err(), "unsafe ref fails validate too");
    }

    #[test]
    fn projection_blocked_on_three_signals() {
        // clarify with an open blocking question
        let mut d = intake_spec();
        d.stage = DeliveryStage::Clarify;
        d.clarify.questions.push(ClarifyQuestion {
            q: "who is the user?".into(),
            blocking: true,
            answer: None,
            by: None,
            at: None,
        });
        assert_eq!(
            DeliveryView::project(&d).blocked_on,
            BlockedOn::OpenClarifyQuestions { count: 1 }
        );

        // spec awaiting confirm
        let mut d = intake_spec();
        d.stage = DeliveryStage::Spec;
        assert_eq!(DeliveryView::project(&d).blocked_on, BlockedOn::SpecConfirm);

        // accept awaiting pm verdict
        let mut d = intake_spec();
        d.stage = DeliveryStage::Accept;
        assert_eq!(DeliveryView::project(&d).blocked_on, BlockedOn::PmAccept);

        // intake → nothing blocks the forward transition
        let d = intake_spec();
        assert_eq!(DeliveryView::project(&d).blocked_on, BlockedOn::Nothing);
    }
}
