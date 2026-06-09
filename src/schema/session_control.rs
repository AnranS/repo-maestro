//! F-119 local chat-session control (`maestro.session_control.v1`).
//!
//! A small, local-only ownership/reuse guard around the Chat surface. It is NOT a
//! transcript or a run ledger — the chat `Session` JSON keeps the messages, F-117
//! `RESUME.json` keeps run resume, F-115 `events.ndjson` keeps run events. This
//! file records only: the last provider-session reuse signature (hash-only), the
//! current owner (busy guard), and bounded first-turn/recent turn receipts. It
//! stores ids / hashes / counts / enums / timestamps / pid ONLY — never a raw
//! prompt, assistant body, thinking trace, provider stdout/stderr, absolute path,
//! or env key/value. This module owns the types + validation; the filesystem
//! helpers live in `crate::chat::control`; ownership/heartbeat wiring is Step 2+.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// Max length for a short single-line value (provider/model/symbolic strings).
const MAX_VALUE_BYTES: usize = 256;
/// Bounded recent-receipt window; the first-turn receipt is exempt from compaction.
pub const MAX_TURN_RECEIPTS: usize = 64;

/// Who owns a session right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    ChatTurn,
    Action,
}

/// Lifecycle phase of a turn receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnPhase {
    Accepted,
    Running,
    Done,
    Failed,
    Abandoned,
}

impl TurnPhase {
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            TurnPhase::Done | TurnPhase::Failed | TurnPhase::Abandoned
        )
    }
}

/// Chat assembly mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    Plan,
    Exec,
}

/// Turn author. v1 only records `user` receipts (the dedup guard is for the
/// user's own first-turn write/prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
}

/// A privacy-safe fingerprint of the local inputs that affect provider-session
/// reuse. All content-bearing inputs are hashed; no raw path/prompt/skill body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReuseSignature {
    /// `fnv1a64:*` over the canonical, sorted-key signature payload.
    pub hash: String,
    /// `cursor/codex/claude/...` — symbol only.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub mode: SessionMode,
    /// hash of normalized workspace identity — NEVER the raw path.
    pub workspace_hash: String,
    /// hash of the prompt-assembly contract/template version (NOT the data, which
    /// has its own hashes below).
    pub prompt_contract_hash: String,
    /// hash of project names/types/scopes — not raw paths.
    pub project_registry_hash: String,
    /// hash of memory topic names only.
    pub memory_topic_hash: String,
    /// hash of skill name/scope/trigger metadata only — not the skill body.
    pub skill_index_hash: String,
    pub created_at: String,
}

/// The current owner of a session (the busy guard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOwner {
    /// safe component; the turn id (chat_turn) or action id (action).
    pub owner_id: String,
    pub kind: OwnerKind,
    /// local process that owns the stream/action.
    pub pid: u32,
    pub started_at: String,
    /// refreshed on a fixed wall-clock cadence (Step 2); stale → recoverable.
    pub heartbeat_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_signature_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// A bounded record of an accepted turn — content-free, for idempotency + audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTurnReceipt {
    /// client-provided idempotency key, safe component.
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_nonce_hash: Option<String>,
    pub role: TurnRole,
    pub phase: TurnPhase,
    /// true when the session had no prior messages when this turn was accepted.
    pub first_turn: bool,
    pub user_message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    /// hash of the sanitized request envelope (text hash + mode + provider + model).
    pub request_hash: String,
    /// the reuse signature hash observed for this turn.
    pub signature_hash: String,
    pub accepted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
}

/// `maestro.session_control.v1` — the per-session control record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionControl {
    #[serde(default = "crate::schema::session_control_version")]
    pub schema_version: String,
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<SessionReuseSignature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_owner: Option<SessionOwner>,
    #[serde(default)]
    pub turns: Vec<SessionTurnReceipt>,
}

impl SessionControl {
    /// A fresh control record for `session_id` at `now` (RFC3339, caller-supplied).
    pub fn new(session_id: impl Into<String>, now: impl Into<String>) -> Self {
        let now = now.into();
        Self {
            schema_version: crate::schema::session_control_version(),
            session_id: session_id.into(),
            created_at: now.clone(),
            updated_at: now,
            signature: None,
            active_owner: None,
            turns: Vec::new(),
        }
    }

    /// The first-turn receipt, if present (retained for the life of the session).
    pub fn first_turn_receipt(&self) -> Option<&SessionTurnReceipt> {
        self.turns.iter().find(|t| t.first_turn)
    }

    /// Look up a receipt by its idempotency key.
    pub fn receipt(&self, turn_id: &str) -> Option<&SessionTurnReceipt> {
        self.turns.iter().find(|t| t.turn_id == turn_id)
    }
}

/// Validate a control record's shape + self-consistency + privacy. Called BEFORE
/// every write AND AFTER every read (corrupt control must refuse, not be silently
/// ignored). Mirrors the F-116/F-117/F-118 validate-before-write/after-read rule.
pub fn validate_control(c: &SessionControl, expected_session_id: &str) -> Result<()> {
    ensure!(
        c.schema_version == crate::schema::SESSION_CONTROL_V1,
        "session control schema_version {:?} is not {}",
        c.schema_version,
        crate::schema::SESSION_CONTROL_V1
    );
    validate_id("session_id", &c.session_id)?;
    ensure!(
        c.session_id == expected_session_id,
        "session control session_id {:?} != expected {:?}",
        c.session_id,
        expected_session_id
    );
    validate_rfc3339("created_at", &c.created_at)?;
    validate_rfc3339("updated_at", &c.updated_at)?;

    if let Some(sig) = &c.signature {
        validate_signature(sig)?;
    }
    if let Some(owner) = &c.active_owner {
        validate_owner(owner)?;
    }

    // Turn receipts: each valid, ids unique, first_turn at most once.
    let mut seen_turn_ids = std::collections::BTreeSet::new();
    let mut first_turns = 0usize;
    for receipt in &c.turns {
        validate_receipt(receipt)?;
        ensure!(
            seen_turn_ids.insert(receipt.turn_id.as_str()),
            "session control has a duplicate turn_id {:?}",
            receipt.turn_id
        );
        if receipt.first_turn {
            first_turns += 1;
        }
    }
    ensure!(
        first_turns <= 1,
        "session control has {first_turns} first_turn receipts (at most 1 allowed)"
    );

    // Bound the receipt window so control files do not grow with transcript length.
    // The first-turn receipt is exempt (it is the permanent dedup guard).
    let non_first = c.turns.iter().filter(|t| !t.first_turn).count();
    ensure!(
        non_first <= MAX_TURN_RECEIPTS,
        "session control has {non_first} non-first turn receipts (max {MAX_TURN_RECEIPTS})"
    );

    // A chat_turn owner must reference a LIVE turn receipt (accepted or running); a
    // settled receipt (done/failed/abandoned) means the owner should have been
    // cleared, so it must not validate. An action owner needs no receipt.
    if let Some(owner) = &c.active_owner {
        if owner.kind == OwnerKind::ChatTurn {
            let receipt = c
                .turns
                .iter()
                .find(|t| t.turn_id == owner.owner_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "active chat_turn owner {:?} has no matching turn receipt",
                        owner.owner_id
                    )
                })?;
            ensure!(
                !receipt.phase.is_settled(),
                "active chat_turn owner {:?} references a settled receipt (phase {:?})",
                owner.owner_id,
                receipt.phase
            );
        }
    }
    Ok(())
}

fn validate_signature(sig: &SessionReuseSignature) -> Result<()> {
    validate_hash("signature.hash", &sig.hash)?;
    validate_symbol("signature.provider", &sig.provider)?;
    if let Some(model) = &sig.model {
        validate_value("signature.model", model)?;
    }
    for (field, value) in [
        ("workspace_hash", &sig.workspace_hash),
        ("prompt_contract_hash", &sig.prompt_contract_hash),
        ("project_registry_hash", &sig.project_registry_hash),
        ("memory_topic_hash", &sig.memory_topic_hash),
        ("skill_index_hash", &sig.skill_index_hash),
    ] {
        validate_hash(field, value)?;
    }
    validate_rfc3339("signature.created_at", &sig.created_at)?;
    Ok(())
}

fn validate_owner(owner: &SessionOwner) -> Result<()> {
    validate_id("active_owner.owner_id", &owner.owner_id)?;
    validate_rfc3339("active_owner.started_at", &owner.started_at)?;
    validate_rfc3339("active_owner.heartbeat_at", &owner.heartbeat_at)?;
    for (field, value) in [
        ("active_owner.message_id", &owner.message_id),
        ("active_owner.action_id", &owner.action_id),
        ("active_owner.run_id", &owner.run_id),
    ] {
        if let Some(v) = value {
            validate_id(field, v)?;
        }
    }
    if let Some(h) = &owner.session_signature_hash {
        validate_hash("active_owner.session_signature_hash", h)?;
    }
    Ok(())
}

fn validate_receipt(r: &SessionTurnReceipt) -> Result<()> {
    validate_id("turn receipt turn_id", &r.turn_id)?;
    validate_id("turn receipt user_message_id", &r.user_message_id)?;
    if let Some(id) = &r.assistant_message_id {
        validate_id("turn receipt assistant_message_id", id)?;
    }
    if let Some(h) = &r.client_nonce_hash {
        validate_hash("turn receipt client_nonce_hash", h)?;
    }
    validate_hash("turn receipt request_hash", &r.request_hash)?;
    validate_hash("turn receipt signature_hash", &r.signature_hash)?;
    validate_rfc3339("turn receipt accepted_at", &r.accepted_at)?;
    if let Some(t) = &r.settled_at {
        validate_rfc3339("turn receipt settled_at", t)?;
    }
    Ok(())
}

/// A safe single path component (reuses the workspace-wide guard). Rejects `..`,
/// `/`, `\`, empty, and multi-segment ids before they ever reach the filesystem.
fn validate_id(field: &str, value: &str) -> Result<()> {
    crate::paths::validate_path_component(field, value)
}

fn validate_rfc3339(field: &str, value: &str) -> Result<()> {
    ensure!(
        chrono::DateTime::parse_from_rfc3339(value).is_ok(),
        "{field} {value:?} is not RFC3339"
    );
    Ok(())
}

/// `fnv1a64:` + exactly 16 lowercase hex characters (the `file_guard` format).
fn validate_hash(field: &str, value: &str) -> Result<()> {
    let rest = value
        .strip_prefix("fnv1a64:")
        .ok_or_else(|| anyhow::anyhow!("{field} {value:?} is not an fnv1a64: hash"))?;
    ensure!(
        rest.len() == 16
            && rest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{field} {value:?} is not fnv1a64: + 16 lowercase hex"
    );
    Ok(())
}

/// A tight symbol: non-empty, bounded, `[A-Za-z0-9._-]` only, and not a bare
/// `.`/`..` (so it can never name this/parent dir). Mirrors the F-118 rule.
fn validate_symbol(field: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'),
        "{field} {value:?} is not a symbol"
    );
    Ok(())
}

/// A short single-line value that is not a path / env leak (used for model ids).
fn validate_value(field: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{field} is empty");
    ensure!(
        value.len() <= MAX_VALUE_BYTES,
        "{field} exceeds {MAX_VALUE_BYTES} bytes"
    );
    ensure!(!value.contains(['\n', '\r']), "{field} must be single-line");
    // reject an env assignment (`KEY=VALUE`) — model ids never contain `=`, and a
    // leaked `MAESTRO_TOKEN=...` must not slip through as a "model".
    ensure!(
        !value.contains('='),
        "{field} must not contain an env assignment"
    );
    ensure!(
        !value.trim_start().to_ascii_lowercase().starts_with("file:"),
        "{field} must not be a file: uri"
    );
    ensure!(
        !value.starts_with('/') && !value.starts_with('\\'),
        "{field} must not be an absolute path"
    );
    ensure!(
        !value.split(['/', '\\']).any(|seg| seg == ".."),
        "{field} must not contain a parent-dir segment"
    );
    let b = value.as_bytes();
    ensure!(
        !(b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'),
        "{field} must not be a drive path"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "fnv1a64:0123456789abcdef";

    fn sig() -> SessionReuseSignature {
        SessionReuseSignature {
            hash: H.into(),
            provider: "cursor".into(),
            model: Some("gpt-5.2".into()),
            mode: SessionMode::Exec,
            workspace_hash: H.into(),
            prompt_contract_hash: H.into(),
            project_registry_hash: H.into(),
            memory_topic_hash: H.into(),
            skill_index_hash: H.into(),
            created_at: "2026-06-05T00:00:00Z".into(),
        }
    }

    fn receipt(turn_id: &str, first: bool) -> SessionTurnReceipt {
        SessionTurnReceipt {
            turn_id: turn_id.into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Running,
            first_turn: first,
            user_message_id: "m-user-1".into(),
            assistant_message_id: None,
            request_hash: H.into(),
            signature_hash: H.into(),
            accepted_at: "2026-06-05T00:00:00Z".into(),
            settled_at: None,
        }
    }

    fn control() -> SessionControl {
        let mut c = SessionControl::new("s-abc", "2026-06-05T00:00:00Z");
        c.signature = Some(sig());
        c.turns.push(receipt("t-1", true));
        c
    }

    #[test]
    fn round_trips_and_validates() {
        let c = control();
        assert!(validate_control(&c, "s-abc").is_ok());
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"schema_version\":\"maestro.session_control.v1\""));
        let back: SessionControl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn rejects_wrong_schema_and_session_mismatch() {
        let mut c = control();
        c.schema_version = "maestro.session_control.v2".into();
        assert!(validate_control(&c, "s-abc").is_err());

        let c = control();
        assert!(
            validate_control(&c, "s-other").is_err(),
            "session_id must match expected"
        );
    }

    #[test]
    fn rejects_unsafe_ids() {
        for bad in ["../escape", "a/b", "", ".."] {
            let mut c = control();
            c.session_id = bad.into();
            assert!(
                validate_control(&c, bad).is_err(),
                "session_id {bad:?} must reject"
            );
        }
        // unsafe owner / message / turn ids reject too
        let mut c = control();
        c.turns[0].user_message_id = "../m".into();
        assert!(validate_control(&c, "s-abc").is_err());
    }

    #[test]
    fn rejects_bad_hash_and_timestamp() {
        let mut c = control();
        c.signature.as_mut().unwrap().workspace_hash = "deadbeef".into(); // no prefix
        assert!(validate_control(&c, "s-abc").is_err());

        let mut c = control();
        c.signature.as_mut().unwrap().hash = "fnv1a64:NOTHEX0000000000".into();
        assert!(validate_control(&c, "s-abc").is_err());

        let mut c = control();
        c.updated_at = "not-a-date".into();
        assert!(validate_control(&c, "s-abc").is_err());
    }

    #[test]
    fn rejects_duplicate_turn_ids_and_two_first_turns() {
        let mut c = control();
        c.turns.push(receipt("t-1", false)); // duplicate id
        assert!(validate_control(&c, "s-abc").is_err());

        let mut c = control();
        c.turns.push(receipt("t-2", true)); // second first_turn
        assert!(validate_control(&c, "s-abc").is_err());
    }

    #[test]
    fn chat_turn_owner_needs_matching_receipt() {
        let mut c = control();
        c.active_owner = Some(SessionOwner {
            owner_id: "t-1".into(),
            kind: OwnerKind::ChatTurn,
            pid: 1234,
            started_at: "2026-06-05T00:00:00Z".into(),
            heartbeat_at: "2026-06-05T00:00:00Z".into(),
            session_signature_hash: Some(H.into()),
            message_id: Some("m-user-1".into()),
            action_id: None,
            run_id: None,
        });
        assert!(validate_control(&c, "s-abc").is_ok());

        // owner_id with no matching receipt -> reject
        c.active_owner.as_mut().unwrap().owner_id = "t-missing".into();
        assert!(validate_control(&c, "s-abc").is_err());

        // an action owner needs no receipt
        let mut c = control();
        c.active_owner = Some(SessionOwner {
            owner_id: "a-1".into(),
            kind: OwnerKind::Action,
            pid: 1234,
            started_at: "2026-06-05T00:00:00Z".into(),
            heartbeat_at: "2026-06-05T00:00:00Z".into(),
            session_signature_hash: None,
            message_id: None,
            action_id: Some("a-1".into()),
            run_id: Some("r-1".into()),
        });
        assert!(validate_control(&c, "s-abc").is_ok());
    }

    #[test]
    fn chat_turn_owner_rejects_settled_receipt() {
        // N2: an active owner pointing at a settled (done/failed/abandoned) receipt
        // means it should have been cleared — must not validate.
        for settled in [TurnPhase::Done, TurnPhase::Failed, TurnPhase::Abandoned] {
            let mut c = control();
            c.turns[0].phase = settled;
            c.turns[0].settled_at = Some("2026-06-05T00:01:00Z".into());
            c.active_owner = Some(SessionOwner {
                owner_id: "t-1".into(),
                kind: OwnerKind::ChatTurn,
                pid: 1234,
                started_at: "2026-06-05T00:00:00Z".into(),
                heartbeat_at: "2026-06-05T00:00:00Z".into(),
                session_signature_hash: None,
                message_id: None,
                action_id: None,
                run_id: None,
            });
            assert!(
                validate_control(&c, "s-abc").is_err(),
                "owner over a {settled:?} receipt must reject"
            );
        }
        // an Accepted (not-yet-running) receipt is still a live owner -> ok.
        let mut c = control();
        c.turns[0].phase = TurnPhase::Accepted;
        c.active_owner = Some(SessionOwner {
            owner_id: "t-1".into(),
            kind: OwnerKind::ChatTurn,
            pid: 1234,
            started_at: "2026-06-05T00:00:00Z".into(),
            heartbeat_at: "2026-06-05T00:00:00Z".into(),
            session_signature_hash: None,
            message_id: None,
            action_id: None,
            run_id: None,
        });
        assert!(validate_control(&c, "s-abc").is_ok());
    }

    #[test]
    fn rejects_overflowing_non_first_receipts() {
        // N3: bounded window — first-turn exempt, non-first capped at MAX_TURN_RECEIPTS.
        let mut c = control(); // 1 first-turn receipt
        for i in 0..MAX_TURN_RECEIPTS {
            c.turns.push(receipt(&format!("n-{i}"), false));
        }
        assert!(
            validate_control(&c, "s-abc").is_ok(),
            "{MAX_TURN_RECEIPTS} non-first ok"
        );
        c.turns
            .push(receipt(&format!("n-{MAX_TURN_RECEIPTS}"), false));
        assert!(
            validate_control(&c, "s-abc").is_err(),
            "{} non-first must reject",
            MAX_TURN_RECEIPTS + 1
        );
    }

    #[test]
    fn rejects_env_like_model_and_dot_symbols() {
        // N4: a model that smuggles an env assignment is rejected.
        let mut c = control();
        c.signature.as_mut().unwrap().model = Some("MAESTRO_TOKEN=abc".into());
        assert!(validate_control(&c, "s-abc").is_err());

        // a bare `.`/`..` provider symbol is rejected.
        for bad in [".", ".."] {
            let mut c = control();
            c.signature.as_mut().unwrap().provider = bad.into();
            assert!(
                validate_control(&c, "s-abc").is_err(),
                "provider {bad:?} must reject"
            );
        }
    }

    #[test]
    fn first_turn_and_receipt_lookups() {
        let c = control();
        assert_eq!(c.first_turn_receipt().unwrap().turn_id, "t-1");
        assert!(c.receipt("t-1").is_some());
        assert!(c.receipt("nope").is_none());
    }

    #[test]
    fn unknown_enum_variant_fails_to_parse() {
        // closed enums: an unknown phase must not deserialize (corrupt read -> Err).
        let json = serde_json::to_string(&control()).unwrap();
        let bad = json.replace("\"running\"", "\"paused\"");
        assert!(serde_json::from_str::<SessionControl>(&bad).is_err());
    }
}
