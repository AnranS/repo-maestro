//! F-119 shared chat turn / action entrypoints.
//!
//! EVERY caller — the HTTP handler, the CLI chat path, and the TUI — starts a chat
//! turn or runs an approved action through these wrappers, so the busy-ownership
//! guard + fixed heartbeat apply uniformly. The lower
//! `stream::send_streaming_with_options` / `actions::execute_action_with_session`
//! are only reached from here (or from tests); a future caller must not bypass the
//! guard. Signature-based provider-session reuse (Step 3) and first-turn dedup /
//! replay (Step 4) layer onto this; Step 2 records a provisional receipt
//! `user_message_id` (the real persisted id is threaded in Step 4).

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::chat::actions::{Action, ActionStatus};
use crate::chat::control::{
    check_turn_idempotency, clear_session_owner, heartbeat_session_owner, prepare_session_owner,
    HeartbeatOutcome, OwnerOutcome, TurnDedup, HEARTBEAT_INTERVAL,
};
use crate::chat::sessions::{self, Session};
use crate::chat::StreamEvent;
use crate::file_guard::stable_hash_bytes;
use crate::paths;
use crate::schema::session_control::{
    OwnerKind, SessionMode, SessionOwner, SessionTurnReceipt, TurnPhase, TurnRole,
};

/// A chat turn request from any surface.
pub struct ChatTurnRequest {
    pub session_id: Option<String>,
    pub text: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// `plan` / `exec`; defaults to `exec`.
    pub mode: Option<String>,
    /// Client idempotency key (Step 4); Step 2 generates one when absent.
    pub turn_id: Option<String>,
}

/// Why a chat turn could not start.
#[derive(Debug)]
pub enum TurnError {
    /// session/turn id failed path-component validation → 400.
    InvalidId,
    /// a live owner already holds the session → 409.
    Busy,
    /// the same turn_id is still in flight → 409.
    TurnRunning,
    /// the same turn_id already ran with a DIFFERENT payload → 409 (no replay).
    TurnPayloadMismatch,
    /// the same turn_id already failed/abandoned → 409 (do not re-send).
    TurnNotRetriable,
    Internal(anyhow::Error),
}

/// Why an action could not run.
#[derive(Debug)]
pub enum ActionError {
    InvalidId,
    SessionNotFound,
    ActionNotFound,
    Busy,
    Internal(anyhow::Error),
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Hash of the sanitized request envelope (text hash + mode + provider + model).
/// Content-free: the raw text is never stored, only its hash.
fn request_hash(text: &str, mode: &str, provider: Option<&str>, model: Option<&str>) -> String {
    let payload = format!(
        "{}\n{}\n{}\n{}",
        stable_hash_bytes(text.as_bytes()),
        mode,
        provider.unwrap_or("default"),
        model.unwrap_or("default"),
    );
    stable_hash_bytes(payload.as_bytes())
}

/// Provisional reuse-signature hash for Step 2 (provider/model/mode + workspace
/// identity, all hashed — never the raw path). Step 3 replaces this with the full
/// multi-input signature; the receipt only needs a stable, valid hash here.
fn provisional_signature_hash(mode: &str, provider: Option<&str>, model: Option<&str>) -> String {
    let workspace = paths::workspace_root()
        .map(|p| stable_hash_bytes(p.display().to_string().as_bytes()))
        .unwrap_or_else(|_| stable_hash_bytes(b""));
    let payload = format!(
        "{}\n{}\n{}\n{}",
        provider.unwrap_or("default"),
        model.unwrap_or("default"),
        mode,
        workspace,
    );
    stable_hash_bytes(payload.as_bytes())
}

/// Load the named session, or create a fresh one (mirrors the prior handler).
fn resolve_session(session_id: Option<&str>) -> anyhow::Result<Session> {
    match session_id {
        Some(id) => sessions::load(id).or_else(|_| {
            let s = Session::new();
            sessions::save(&s)?;
            let _ = sessions::set_current(&s.id);
            Ok(s)
        }),
        None => sessions::ensure_current().or_else(|_| Ok(Session::new())),
    }
}

/// The fixed 30s heartbeat loop; stops as soon as another owner replaces us (or the
/// control file is gone). Runs the blocking control write off the async executor.
fn spawn_heartbeat(session_id: String, owner_id: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            let now = now_rfc3339();
            let (sid, oid) = (session_id.clone(), owner_id.clone());
            match tokio::task::spawn_blocking(move || heartbeat_session_owner(&sid, &oid, &now))
                .await
            {
                // refreshed OR a transient lock blip → keep beating
                Ok(Ok(HeartbeatOutcome::Updated)) | Ok(Ok(HeartbeatOutcome::Contended)) => continue,
                // replaced / control gone / error → stop heartbeating
                _ => break,
            }
        }
    })
}

/// Start a chat turn through the ownership guard. On success returns the
/// `StreamEvent` receiver for the caller to relay; ownership is held + heartbeated
/// until the turn settles, then released.
pub async fn start_chat_turn(
    req: ChatTurnRequest,
) -> Result<mpsc::Receiver<StreamEvent>, TurnError> {
    if let Some(id) = &req.session_id {
        if paths::validate_path_component("chat session id", id).is_err() {
            return Err(TurnError::InvalidId);
        }
    }
    if let Some(tid) = &req.turn_id {
        if paths::validate_path_component("chat turn id", tid).is_err() {
            return Err(TurnError::InvalidId);
        }
    }

    let mode = req.mode.clone().unwrap_or_else(|| "exec".into());
    // In plan mode, the constraint is prepended to the user's message (so it lands
    // in the transcript) — same behavior the HTTP handler had before.
    let text = if mode == "plan" {
        format!(
            "[plan mode] Please analyse and propose what you would do. Do NOT emit any `maestro-action` blocks; describe the actions in prose instead. The user will switch to exec mode when ready.\n\n{}",
            req.text
        )
    } else {
        req.text.clone()
    };

    let session = resolve_session(req.session_id.as_deref()).map_err(TurnError::Internal)?;
    let session_id = session.id.clone();
    let _ = sessions::set_current(&session_id);

    let is_first = session.messages.is_empty();
    let owner_id = req
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("t-{}", Uuid::new_v4()));
    let req_hash = request_hash(&text, &mode, req.provider.as_deref(), req.model.as_deref());

    // F-119 Step 4: turn idempotency by turn_id. A duplicate that already completed
    // with the SAME payload replays the persisted assistant (no provider call, no new
    // user message); a payload mismatch / still-running / failed / abandoned turn is
    // refused (409). New turns proceed.
    let (sid_d, tid_d, rh_d) = (session_id.clone(), owner_id.clone(), req_hash.clone());
    let dedup = tokio::task::spawn_blocking(move || check_turn_idempotency(&sid_d, &tid_d, &rh_d))
        .await
        .map_err(|e| TurnError::Internal(e.into()))?
        .map_err(TurnError::Internal)?;
    match dedup {
        TurnDedup::New => {}
        TurnDedup::Running => return Err(TurnError::TurnRunning),
        TurnDedup::PayloadMismatch => return Err(TurnError::TurnPayloadMismatch),
        TurnDedup::NotRetriable => return Err(TurnError::TurnNotRetriable),
        TurnDedup::DoneReplay {
            assistant_message_id,
        } => {
            return Ok(replay_done(session, assistant_message_id));
        }
    }

    let now = now_rfc3339();
    let sig_hash = provisional_signature_hash(&mode, req.provider.as_deref(), req.model.as_deref());
    // Real persisted message ids, threaded into the receipt (for dedup/replay) and
    // into send_streaming (so the persisted messages carry these exact ids).
    let user_message_id = format!("m-{}", Uuid::new_v4());
    let assistant_message_id = format!("a-{}", Uuid::new_v4());

    let receipt = SessionTurnReceipt {
        turn_id: owner_id.clone(),
        client_nonce_hash: None,
        role: TurnRole::User,
        phase: TurnPhase::Running,
        first_turn: is_first,
        user_message_id: user_message_id.clone(),
        assistant_message_id: Some(assistant_message_id.clone()),
        request_hash: req_hash,
        signature_hash: sig_hash.clone(),
        accepted_at: now.clone(),
        settled_at: None,
    };
    let owner = SessionOwner {
        owner_id: owner_id.clone(),
        kind: OwnerKind::ChatTurn,
        pid: std::process::id(),
        started_at: now.clone(),
        heartbeat_at: now.clone(),
        session_signature_hash: Some(sig_hash),
        message_id: Some(assistant_message_id.clone()),
        action_id: None,
        run_id: None,
    };

    let (sid, n) = (session_id.clone(), now.clone());
    let outcome =
        tokio::task::spawn_blocking(move || prepare_session_owner(owner, Some(receipt), &sid, &n))
            .await
            .map_err(|e| TurnError::Internal(e.into()))?
            .map_err(TurnError::Internal)?;
    if outcome == OwnerOutcome::Busy {
        return Err(TurnError::Busy);
    }

    let (tx, rx) = mpsc::channel::<StreamEvent>(64);
    let model = req.model.clone();
    let provider = req.provider.clone();
    let mode_enum = if mode == "plan" {
        SessionMode::Plan
    } else {
        SessionMode::Exec
    };
    tokio::spawn(async move {
        let hb = spawn_heartbeat(session_id.clone(), owner_id.clone());
        let result = crate::chat::stream::send_streaming_with_options(
            session,
            text,
            model,
            provider,
            mode_enum,
            user_message_id,
            assistant_message_id,
            tx.clone(),
        )
        .await;
        hb.abort();
        let phase = if result.is_ok() {
            TurnPhase::Done
        } else {
            TurnPhase::Failed
        };
        let (sid, oid, n) = (session_id.clone(), owner_id.clone(), now_rfc3339());
        let _ =
            tokio::task::spawn_blocking(move || clear_session_owner(&sid, &oid, phase, None, &n))
                .await;
        if let Err(e) = result {
            let _ = tx
                .send(StreamEvent::Error {
                    message: format!("{e:#}"),
                })
                .await;
        }
    });
    Ok(rx)
}

/// Replay a completed turn: emit the persisted assistant message as an SSE `done`,
/// with NO provider call and NO new user message (the original turn already ran and
/// its user message is already in the transcript).
fn replay_done(
    session: Session,
    assistant_message_id: Option<String>,
) -> mpsc::Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel::<StreamEvent>(8);
    tokio::spawn(async move {
        let message =
            assistant_message_id.and_then(|id| session.messages.into_iter().find(|m| m.id == id));
        let event = match message {
            Some(message) => StreamEvent::Done { message },
            None => StreamEvent::Error {
                message: "turn already completed".to_string(),
            },
        };
        let _ = tx.send(event).await;
    });
    rx
}

/// Run an approved/rejected action through the ownership guard. A `reject` never
/// executes anything, so it needs no ownership; an `approve` acquires the session
/// (kind=action) for the duration of execution and releases it after.
pub async fn run_chat_action_with_owner(
    session_id: &str,
    action_id: &str,
    decision: &str,
) -> Result<Action, ActionError> {
    if paths::validate_path_component("chat session id", session_id).is_err() {
        return Err(ActionError::InvalidId);
    }
    if paths::validate_path_component("chat action id", action_id).is_err() {
        return Err(ActionError::InvalidId);
    }
    let mut session = sessions::load(session_id).map_err(|_| ActionError::SessionNotFound)?;

    let found = session.messages.iter().enumerate().find_map(|(mi, m)| {
        m.actions
            .iter()
            .position(|a| a.id == action_id)
            .map(|ai| (mi, ai))
    });
    let Some((mi, ai)) = found else {
        return Err(ActionError::ActionNotFound);
    };

    if decision == "reject" {
        session.messages[mi].actions[ai].status = Some(ActionStatus::Rejected);
        let _ = sessions::save(&session);
        return Ok(session.messages[mi].actions[ai].clone());
    }

    // approve → acquire ownership before executing.
    let now = now_rfc3339();
    let owner = SessionOwner {
        owner_id: action_id.to_string(),
        kind: OwnerKind::Action,
        pid: std::process::id(),
        started_at: now.clone(),
        heartbeat_at: now.clone(),
        session_signature_hash: None,
        message_id: None,
        action_id: Some(action_id.to_string()),
        run_id: None,
    };
    let (sid, n) = (session_id.to_string(), now.clone());
    let outcome = tokio::task::spawn_blocking(move || prepare_session_owner(owner, None, &sid, &n))
        .await
        .map_err(|e| ActionError::Internal(e.into()))?
        .map_err(ActionError::Internal)?;
    if outcome == OwnerOutcome::Busy {
        return Err(ActionError::Busy);
    }

    session.messages[mi].actions[ai].status = Some(ActionStatus::Running);
    let _ = sessions::save(&session);
    let action = session.messages[mi].actions[ai].clone();

    let hb = spawn_heartbeat(session_id.to_string(), action_id.to_string());
    let res = crate::chat::actions::execute_action_with_session(&action, Some(session_id)).await;
    hb.abort();

    let mut session = sessions::load(session_id).unwrap_or(session);
    if let Some(act) = session
        .messages
        .iter_mut()
        .flat_map(|m| m.actions.iter_mut())
        .find(|a| a.id == action_id)
    {
        match &res {
            Ok(out) => {
                act.status = Some(ActionStatus::Done);
                act.output = Some(out.clone());
            }
            Err(e) => {
                act.status = Some(ActionStatus::Failed);
                act.output = Some(format!("{e:#}"));
            }
        }
    }
    let _ = sessions::save(&session);

    let phase = if res.is_ok() {
        TurnPhase::Done
    } else {
        TurnPhase::Failed
    };
    let (sid, aid, n) = (session_id.to_string(), action_id.to_string(), now_rfc3339());
    let _ =
        tokio::task::spawn_blocking(move || clear_session_owner(&sid, &aid, phase, None, &n)).await;

    Ok(session
        .messages
        .iter()
        .flat_map(|m| m.actions.iter())
        .find(|a| a.id == action_id)
        .cloned()
        .unwrap_or(action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::control::prepare_session_owner;
    use serial_test::serial;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
        }
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        dir
    }
    fn clear() {
        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[tokio::test]
    #[serial]
    async fn start_chat_turn_rejects_invalid_session_id() {
        let _w = workspace();
        let result = start_chat_turn(ChatTurnRequest {
            session_id: Some("../escape".into()),
            text: "hi".into(),
            model: None,
            provider: None,
            mode: None,
            turn_id: None,
        })
        .await;
        assert!(matches!(result, Err(TurnError::InvalidId)));
        clear();
    }

    #[tokio::test]
    #[serial]
    async fn start_chat_turn_refuses_a_busy_session() {
        // All three surfaces (HTTP / CLI / TUI) funnel through start_chat_turn, so a
        // busy refusal here is the shared guard for every entrypoint. Pre-claim the
        // session with a live owner, then a new turn must get Busy (no provider call).
        let _w = workspace();
        let s = Session::new();
        sessions::save(&s).unwrap();
        let now = now_rfc3339();
        const H: &str = "fnv1a64:0123456789abcdef";
        let owner = SessionOwner {
            owner_id: "t-live".into(),
            kind: OwnerKind::ChatTurn,
            pid: std::process::id(),
            started_at: now.clone(),
            heartbeat_at: now.clone(),
            session_signature_hash: None,
            message_id: None,
            action_id: None,
            run_id: None,
        };
        let receipt = SessionTurnReceipt {
            turn_id: "t-live".into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Running,
            first_turn: true,
            user_message_id: "m-1".into(),
            assistant_message_id: None,
            request_hash: H.into(),
            signature_hash: H.into(),
            accepted_at: now.clone(),
            settled_at: None,
        };
        prepare_session_owner(owner, Some(receipt), &s.id, &now).unwrap();

        let result = start_chat_turn(ChatTurnRequest {
            session_id: Some(s.id.clone()),
            text: "hello".into(),
            model: None,
            provider: None,
            mode: None,
            turn_id: None,
        })
        .await;
        assert!(matches!(result, Err(TurnError::Busy)));
        clear();
    }

    #[tokio::test]
    #[serial]
    async fn action_on_missing_session_is_not_found() {
        let _w = workspace();
        let result = run_chat_action_with_owner("s-nope", "a-1", "approve").await;
        assert!(matches!(result, Err(ActionError::SessionNotFound)));
        // an unsafe id is InvalidId, not SessionNotFound
        let result = run_chat_action_with_owner("../x", "a-1", "approve").await;
        assert!(matches!(result, Err(ActionError::InvalidId)));
        clear();
    }

    #[tokio::test]
    #[serial]
    async fn action_approve_refused_and_not_run_when_session_busy() {
        // N1: an approved action against a busy session is refused, and the action
        // is NOT stamped Running (it stays pending). Proves the TUI/HTTP approve
        // path can't bypass the guard.
        use crate::chat::actions::{Action, ActionStatus, ActionVerb};
        use crate::chat::sessions::{Message, Role};
        let _w = workspace();
        let mut s = Session::new();
        let mut msg = Message::new(Role::Assistant, "here".into());
        msg.actions.push(Action {
            id: "a-1".into(),
            verb: ActionVerb::Work,
            args: Default::default(),
            status: Some(ActionStatus::Pending),
            label: "do".into(),
            output: None,
        });
        s.messages.push(msg);
        sessions::save(&s).unwrap();

        // pre-claim a live chat owner
        let now = now_rfc3339();
        const H: &str = "fnv1a64:0123456789abcdef";
        let owner = SessionOwner {
            owner_id: "t-live".into(),
            kind: OwnerKind::ChatTurn,
            pid: std::process::id(),
            started_at: now.clone(),
            heartbeat_at: now.clone(),
            session_signature_hash: None,
            message_id: None,
            action_id: None,
            run_id: None,
        };
        let receipt = SessionTurnReceipt {
            turn_id: "t-live".into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Running,
            first_turn: true,
            user_message_id: "m-1".into(),
            assistant_message_id: None,
            request_hash: H.into(),
            signature_hash: H.into(),
            accepted_at: now.clone(),
            settled_at: None,
        };
        crate::chat::control::prepare_session_owner(owner, Some(receipt), &s.id, &now).unwrap();

        let result = run_chat_action_with_owner(&s.id, "a-1", "approve").await;
        assert!(matches!(result, Err(ActionError::Busy)));
        let reloaded = sessions::load(&s.id).unwrap();
        let status = reloaded
            .messages
            .iter()
            .flat_map(|m| m.actions.iter())
            .find(|a| a.id == "a-1")
            .unwrap()
            .status;
        assert_eq!(
            status,
            Some(ActionStatus::Pending),
            "busy approve must not mark Running"
        );
        clear();
    }

    fn done_receipt(
        turn_id: &str,
        request_hash: &str,
        assistant: &str,
        first: bool,
    ) -> SessionTurnReceipt {
        SessionTurnReceipt {
            turn_id: turn_id.into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Done,
            first_turn: first,
            user_message_id: format!("m-{turn_id}"),
            assistant_message_id: Some(assistant.into()),
            request_hash: request_hash.into(),
            signature_hash: "fnv1a64:0123456789abcdef".into(),
            accepted_at: "2026-06-05T00:00:00Z".into(),
            settled_at: Some("2026-06-05T00:00:01Z".into()),
        }
    }

    #[tokio::test]
    #[serial]
    async fn duplicate_done_turn_replays_assistant_without_new_user_message() {
        use crate::chat::sessions::{Message, Role};
        let _w = workspace();
        // a session whose transcript already holds the assistant reply "a-1"
        let mut s = Session::new();
        let mut amsg = Message::new(Role::Assistant, "the persisted answer".into());
        amsg.id = "a-1".into();
        s.messages.push(amsg);
        sessions::save(&s).unwrap();

        // a Done receipt for turn t-1 with the SAME request hash the retry will compute
        let rh = request_hash("hello", "exec", None, None);
        let mut control = crate::schema::session_control::SessionControl::new(&s.id, now_rfc3339());
        control.turns.push(done_receipt("t-1", &rh, "a-1", true));
        crate::chat::control::save_control(&control).unwrap();

        let mut rx = start_chat_turn(ChatTurnRequest {
            session_id: Some(s.id.clone()),
            text: "hello".into(),
            model: None,
            provider: None,
            mode: None,
            turn_id: Some("t-1".into()),
        })
        .await
        .expect("done replay returns a receiver");
        match rx.recv().await {
            Some(StreamEvent::Done { message }) => assert_eq!(message.id, "a-1"),
            _ => panic!("expected a replayed Done"),
        }
        // no new user message was appended
        assert_eq!(sessions::load(&s.id).unwrap().messages.len(), 1);
        clear();
    }

    #[tokio::test]
    #[serial]
    async fn duplicate_running_and_payload_mismatch_are_409() {
        let _w = workspace();
        let s = Session::new();
        sessions::save(&s).unwrap();
        let rh = request_hash("hello", "exec", None, None);

        let mut control = crate::schema::session_control::SessionControl::new(&s.id, now_rfc3339());
        // a still-running turn t-run, and a done turn t-done with a DIFFERENT payload
        control.turns.push(SessionTurnReceipt {
            turn_id: "t-run".into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Running,
            first_turn: true,
            user_message_id: "m-run".into(),
            assistant_message_id: None,
            request_hash: rh.clone(),
            signature_hash: "fnv1a64:0123456789abcdef".into(),
            accepted_at: "2026-06-05T00:00:00Z".into(),
            settled_at: None,
        });
        control.turns.push(done_receipt(
            "t-done",
            "fnv1a64:9999999999999999",
            "a-done",
            false,
        ));
        crate::chat::control::save_control(&control).unwrap();

        let running = start_chat_turn(ChatTurnRequest {
            session_id: Some(s.id.clone()),
            text: "hello".into(),
            model: None,
            provider: None,
            mode: None,
            turn_id: Some("t-run".into()),
        })
        .await;
        assert!(matches!(running, Err(TurnError::TurnRunning)));

        let mismatch = start_chat_turn(ChatTurnRequest {
            session_id: Some(s.id.clone()),
            text: "hello".into(), // hashes to rh, but t-done's receipt has a different hash
            model: None,
            provider: None,
            mode: None,
            turn_id: Some("t-done".into()),
        })
        .await;
        assert!(matches!(mismatch, Err(TurnError::TurnPayloadMismatch)));
        clear();
    }
}
