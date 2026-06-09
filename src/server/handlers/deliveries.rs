//! F-128 — read-only Delivery Web UI endpoints. `GET /api/deliveries` (list as
//! envelopes) + `GET /api/deliveries/:id` (the `DeliveryView` projection). No
//! mutations: v1 only surfaces the PM→Delivery chain, it never crosses
//! `spec_confirm`/`pm_accept` or starts a run.

use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;

use crate::paths;
use crate::schema::delivery::{DeliveryListEntry, DeliverySpec};

/// `GET /api/deliveries` — every delivery as an envelope: `ok` carries the projected
/// `DeliveryView`, `corrupt` carries a visible stub + reason. A corrupt
/// `DELIVERY.json` is SHOWN (never silently dropped); only failing to read the
/// deliveries directory itself is a 500. Ordered by `delivery_id` asc (from `list`).
pub async fn deliveries_list() -> Response {
    let ids = match crate::delivery::list() {
        Ok(ids) => ids,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let entries: Vec<DeliveryListEntry> = ids
        .into_iter()
        .map(|id| match crate::delivery::read(&id) {
            // A linked-run RUN_STATE that can't be projected (F-136a1 B1) surfaces as
            // a corrupt stub for THIS delivery — never a silent null, never a list 500.
            Ok(Some(spec)) => match crate::delivery::project_view(&spec) {
                Ok(view) => DeliveryListEntry::Ok {
                    delivery_id: id,
                    view: Box::new(view),
                },
                Err(e) => DeliveryListEntry::Corrupt {
                    delivery_id: id,
                    error: format!("{e:#}"),
                },
            },
            // A registered dir without a readable DELIVERY.json is an incomplete
            // record — shown as corrupt, not hidden.
            Ok(None) => DeliveryListEntry::Corrupt {
                delivery_id: id,
                error: "DELIVERY.json missing".into(),
            },
            Err(e) => DeliveryListEntry::Corrupt {
                delivery_id: id,
                error: format!("{e:#}"),
            },
        })
        .collect();
    Json(entries).into_response()
}

/// `GET /api/deliveries/:id` — the read-only `DeliveryView` projection. Three-state
/// mirrors the runs handlers: bad id → 400, missing → 404, corrupt/bad-schema/unsafe
/// ref → 500 (never a silent empty).
pub async fn delivery_get(Path(id): Path<String>) -> Response {
    if let Err(e) = paths::validate_path_component("delivery id", &id) {
        return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    match crate::delivery::read(&id) {
        Ok(Some(spec)) => view_response(&spec),
        Ok(None) => (StatusCode::NOT_FOUND, "delivery not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/deliveries/:id/run-status` — slim live run status for the Delivery detail
/// (F-131). Delivery three-state (bad id 400 / missing 404 / corrupt delivery 500) via
/// `read_for_action`; the resolver's failure (a LINKED run that's missing/corrupt) →
/// 500 (never silently shown as idle); genuinely no run → `200 {run:null}`.
pub async fn delivery_run_status(Path(id): Path<String>) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    match crate::delivery::run_status(&spec) {
        Ok(run) => Json(serde_json::json!({ "run": run })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/deliveries/:id/timeline` — the read-only Delivery audit timeline (F-132).
/// Delivery three-state via `read_for_action` (bad id 400 / missing 404 / corrupt 500).
/// The projection is fallible: an audit/node inconsistency (a transition whose produced
/// node is missing) → 500 — NEVER a silent `{events:[]}`. Refs-first (ids/refs/uris/
/// summaries only).
pub async fn delivery_timeline(Path(id): Path<String>) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    match crate::delivery::timeline(&spec) {
        Ok(events) => Json(serde_json::json!({ "events": events })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ── F-129: Web mutation (confirm-spec / plan / run) ──────────────────────────

/// The acting identity from the POST body (`{by?}`). Empty/absent → `"web-ui"`
/// (never inferred from a real user — local UI has no auth).
#[derive(Deserialize)]
pub struct ActorBody {
    #[serde(default)]
    by: Option<String>,
}

/// The acting identity from a raw `by` field — trimmed, defaulting to `"web-ui"`
/// (local UI has no auth). Shared by every mutation handler so the audit/provenance
/// is consistent (the F-129 B1 lesson: never UI-only).
fn actor_of(by: Option<&str>) -> String {
    match by.map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "web-ui".to_string(),
    }
}

fn actor(body: Option<&ActorBody>) -> Option<String> {
    Some(actor_of(body.and_then(|b| b.by.as_deref())))
}

/// Trim + drop blank tokens from a Web-supplied multi-value field. The forms send
/// tokenized textareas; the server never trusts the client to have cleaned them.
fn clean_tokens(v: Vec<String>) -> Vec<String> {
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Read a delivery for a mutation, returning the F-128 three-state Response on
/// failure (bad id 400 / missing 404 / corrupt 500) — the action only runs on a
/// valid record. A subsequent store refusal maps to 409 (see `refused`).
fn read_for_action(id: &str) -> Result<DeliverySpec, Box<Response>> {
    if let Err(e) = paths::validate_path_component("delivery id", id) {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        ));
    }
    match crate::delivery::read(id) {
        Ok(Some(spec)) => Ok(spec),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "delivery not found").into_response(),
        )),
        Err(e) => Err(Box::new(
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        )),
    }
}

/// A store action refusal (anyhow `bail!`: not-confirmed / incomplete / already-plan
/// / already-confirmed / illegal-stage / preflight-fail) → 409 with the message.
fn refused(e: anyhow::Error) -> Response {
    (StatusCode::CONFLICT, format!("{e:#}")).into_response()
}

/// Serve a single delivery's `DeliveryView`. `project_view` is fallible (F-136a1 B1):
/// a linked-run RUN_STATE that can't be projected → 500 (runtime profile unavailable),
/// NEVER a silent null. Mutation handlers preflight projectability first
/// (`ensure_projectable`), so here it is defense-in-depth.
fn view_response(spec: &DeliverySpec) -> Response {
    match crate::delivery::project_view(spec) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// Mutation preflight (F-136a1 B2): a mutation that returns a `DeliveryView` must NOT
/// write state and THEN 500 because the post-mutation projection fails — that leaves a
/// persisted state the caller was told failed (e.g. `closeout` writes `stage=Closeout`
/// without reading the run, then the projection of a corrupt linked run 500s). So if
/// the (pre-mutation) delivery already has a linked run, verify its RuntimeProfile
/// projection is possible BEFORE the store mutation; on failure → 500 and the caller
/// MUST NOT run the mutation. (validate/project errors preflight before side effects.)
fn ensure_projectable(spec: &DeliverySpec) -> Option<Response> {
    match crate::delivery::project_view(spec) {
        Ok(_) => None,
        Err(e) => Some((StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()),
    }
}

/// `POST /api/deliveries/:id/confirm-spec` — record the `spec_confirm` threshold.
pub async fn delivery_confirm_spec(
    Path(id): Path<String>,
    body: Option<Json<ActorBody>>,
) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if let Some(resp) = ensure_projectable(&spec) {
        return resp;
    }
    let by = actor(body.as_ref().map(|j| &j.0));
    match crate::delivery::confirm_spec(&id, by, now()) {
        Ok(spec) => view_response(&spec),
        Err(e) => refused(e),
    }
}

/// `POST /api/deliveries/:id/plan` — generate the PLAN (Spec→Plan). v1: no `--force`.
pub async fn delivery_plan(Path(id): Path<String>, body: Option<Json<ActorBody>>) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if let Some(resp) = ensure_projectable(&spec) {
        return resp;
    }
    let by = actor(body.as_ref().map(|j| &j.0));
    match crate::delivery::generate_plan(&id, now(), by, false) {
        Ok((spec, _path)) => view_response(&spec),
        Err(e) => refused(e),
    }
}

/// `POST /api/deliveries/:id/run` — start the run as a DETACHED subprocess. Runs the
/// full non-mutating preflight synchronously first (409 on any failure, no spawn),
/// takes the durable launch lock (409 if a run is already launching — server-side
/// duplicate-run guard), then spawns `maestro delivery run <id>` and returns 202
/// immediately. NEVER blocks the request on the run; the run surfaces live in Tasks
/// via the existing fs-watcher → SSE.
pub async fn delivery_run(Path(id): Path<String>, body: Option<Json<ActorBody>>) -> Response {
    if let Err(resp) = read_for_action(&id) {
        return *resp;
    }
    // The actor is recorded in the Execute audit row by the detached child.
    let by = actor(body.as_ref().map(|j| &j.0)).unwrap_or_else(|| "web-ui".to_string());
    // Full preflight → immediate 409 (no spawn) on any failure (the detached child
    // re-runs it anti-TOCTOU).
    if let Err(e) = crate::delivery::run_preflight(&id) {
        return refused(e);
    }
    // Durable duplicate-launch guard: a 2nd POST while a run is in flight → 409.
    match crate::delivery::acquire_run_launch(&id) {
        Ok(crate::delivery::LaunchAcquire::Acquired) => {}
        Ok(crate::delivery::LaunchAcquire::Busy) => {
            return (
                StatusCode::CONFLICT,
                format!("delivery {id}: a run is already launching / in flight"),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
    let bin = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("maestro"));
    let cwd = match paths::workspace_root() {
        Ok(r) => r,
        Err(e) => {
            crate::delivery::release_run_launch(&id);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
        }
    };
    match std::process::Command::new(&bin)
        .arg("delivery")
        .arg("run")
        .arg(&id)
        .arg("--by")
        .arg(&by)
        .current_dir(&cwd)
        .env(crate::delivery::RUN_LAUNCHED_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "ok": true, "pid": child.id() })),
        )
            .into_response(),
        Err(e) => {
            // Don't leave a stuck "launching" marker on a spawn failure.
            crate::delivery::release_run_launch(&id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn delivery run: {e:#}"),
            )
                .into_response()
        }
    }
}

// ── F-130: Web mutation slice 2 (accept / closeout) ──────────────────────────
// Both reuse the SYNC store fns (no detached spawn): `accept` reads the run outcome
// internally for the 5-state gate; `closeout`'s write-back is an in-process
// `OutboundReply` append. Same shape as confirm/plan above (read-first three-state +
// store refusal → 409); the run-outcome validation lives ENTIRELY in the store.

/// `POST /api/deliveries/:id/accept` body. `verdict` is required (string → parsed by
/// the SHARED `delivery::parse_verdict`); the rest default. `debt` is trimmed of
/// blanks; the failed/unverified-run rules are enforced by the store, not here.
#[derive(Deserialize)]
pub struct AcceptBody {
    verdict: String,
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    debt: Vec<String>,
    #[serde(default)]
    accept_failed_with_debt: bool,
}

/// `POST /api/deliveries/:id/accept` — record the PM verdict (the `pm_accept`
/// threshold). v1 surfaces NO run status here: the run-outcome five-state validation
/// is the store's job (a refusal → 409 with the verbatim message; the run itself is a
/// deep-link in the UI). An invalid verdict is a caller error → 400.
pub async fn delivery_accept(Path(id): Path<String>, body: Option<Json<AcceptBody>>) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if let Some(resp) = ensure_projectable(&spec) {
        return resp;
    }
    let Some(Json(b)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            "accept needs a JSON body with a verdict",
        )
            .into_response();
    };
    let verdict = match crate::delivery::parse_verdict(&b.verdict) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let by = actor_of(b.by.as_deref());
    let notes = b
        .notes
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let debt = clean_tokens(b.debt);
    match crate::delivery::accept(
        &id,
        verdict,
        by,
        notes,
        debt,
        b.accept_failed_with_debt,
        now(),
    ) {
        Ok(spec) => view_response(&spec),
        Err(e) => refused(e),
    }
}

/// `POST /api/deliveries/:id/closeout` body. The multi-value fields are tokenized
/// textareas from the form; the server trims/drops blanks. `evidence` strings go
/// through the SAME `delivery::parse_evidence_refs` helper as the CLI (no drift) and
/// are validated run-local by the store.
#[derive(Deserialize, Default)]
pub struct CloseoutBody {
    #[serde(default)]
    commits: Vec<String>,
    #[serde(default)]
    ci: Vec<String>,
    #[serde(default)]
    reviews: Vec<String>,
    #[serde(default)]
    doc_revisions: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    writeback: bool,
    #[serde(default)]
    by: Option<String>,
}

/// `POST /api/deliveries/:id/reopen` body — who/why for the rework audit.
#[derive(Deserialize)]
pub struct ReopenBody {
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/deliveries/:id/reopen` — reopen a `changes_requested` delivery for rework
/// (F-133). Supersedes the prior round (archived) + resets to `spec`. read-first
/// three-state + store refusal (stage ≠ changes_requested) → 409. `by` defaults web-ui.
pub async fn delivery_reopen(Path(id): Path<String>, body: Option<Json<ReopenBody>>) -> Response {
    if let Err(resp) = read_for_action(&id) {
        return *resp;
    }
    let b = body.map(|j| j.0);
    let by = Some(actor_of(b.as_ref().and_then(|b| b.by.as_deref())));
    let reason = b.and_then(|b| b.reason).filter(|s| !s.trim().is_empty());
    match crate::delivery::reopen(&id, by, reason, now()) {
        Ok(spec) => view_response(&spec),
        Err(e) => refused(e),
    }
}

/// `POST /api/deliveries/:id/closeout` — record evidence refs + optional write-back
/// intent (Accept→Closeout). Empty-closeout / wrong-stage / verdict-not-accepted /
/// unsafe-evidence / writeback-without-doc-uri / enqueue-failure are ALL store
/// refusals → 409 (the store guarantees no half-written closeout, no spurious
/// `intent_emitted` — F-127c semantics, constraint 7).
pub async fn delivery_closeout(
    Path(id): Path<String>,
    body: Option<Json<CloseoutBody>>,
) -> Response {
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if let Some(resp) = ensure_projectable(&spec) {
        return resp;
    }
    let b = body.map(|j| j.0).unwrap_or_default();
    let evidence_refs = crate::delivery::parse_evidence_refs(&b.evidence);
    let by = Some(actor_of(b.by.as_deref()));
    match crate::delivery::closeout(
        &id,
        clean_tokens(b.commits),
        clean_tokens(b.ci),
        clean_tokens(b.reviews),
        clean_tokens(b.doc_revisions),
        evidence_refs,
        b.writeback,
        now(),
        by,
    ) {
        Ok(spec) => view_response(&spec),
        Err(e) => refused(e),
    }
}

/// `POST /api/deliveries/:id/writeback-receipt` body — the EXTERNAL drainer's callback.
#[derive(Deserialize)]
pub struct WritebackReceiptBody {
    idempotency_key: String,
    status: String,
    #[serde(default)]
    message_ref: Option<String>,
    #[serde(default)]
    doc_revision: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `POST /api/deliveries/:id/writeback-receipt` (F-134) — the external lark drainer
/// reports the Feishu post outcome BACK (this is a DRAINER CALLBACK, not a user UI
/// action). maestro RECORDS the receipt (posted/failed) and posts nothing. read-first
/// three-state; invalid status → 400; store refusal (no intent / stale key / posted-
/// immutable conflict / missing durable ref / missing error) → 409.
pub async fn delivery_writeback_receipt(
    Path(id): Path<String>,
    Json(b): Json<WritebackReceiptBody>,
) -> Response {
    // F-134 B1: preflight projectability BEFORE the mutation — otherwise recording the
    // receipt then 500-ing on the post-mutation projection (a corrupt linked RUN_STATE)
    // would leave a persisted posted/failed the drainer was told failed (it might retry).
    // Same discipline as confirm/plan/accept/closeout (the F-136a1 B2 fix).
    let spec = match read_for_action(&id) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if let Some(resp) = ensure_projectable(&spec) {
        return resp;
    }
    let status = match b.status.as_str() {
        "posted" => crate::schema::delivery::WritebackStatus::Posted,
        "failed" => crate::schema::delivery::WritebackStatus::Failed,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid status {other:?}; expected posted|failed"),
            )
                .into_response()
        }
    };
    match crate::delivery::record_writeback_receipt(
        &id,
        &b.idempotency_key,
        status,
        b.message_ref,
        b.doc_revision,
        b.error,
        now(),
    ) {
        Ok(spec) => view_response(&spec),
        Err(e) => refused(e),
    }
}
