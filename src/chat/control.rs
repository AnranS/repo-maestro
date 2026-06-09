//! F-119 filesystem helpers for local chat-session control.
//!
//! Step 1 owns the storage primitives only: path resolution (id-guarded before any
//! filesystem access), atomic validated read/write, and the control/lock layout
//! under `.maestro/chat/`. Ownership acquisition, the short lock lifecycle,
//! heartbeats, and signature/turn wiring arrive in Step 2+. A missing control file
//! is `None` (an old session simply has no control yet); a corrupt or
//! schema-invalid file is an `Err` (the guard refuses rather than being ignored).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::chat::sessions::chat_dir;
use crate::paths;
use crate::schema::session_control::{
    validate_control, OwnerKind, SessionControl, SessionOwner, SessionReuseSignature,
    SessionTurnReceipt, TurnPhase, MAX_TURN_RECEIPTS,
};

const CONTROL_DIR: &str = "control";
const LOCKS_DIR: &str = "locks";

/// Fixed heartbeat cadence (Step 2): refresh every 30s, abandon after 3min of
/// silence. Wall-clock driven, NOT delta-driven — a slow provider that streams
/// nothing for a while is still alive.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const HEARTBEAT_STALE_SECS: i64 = 180;

/// The short control-file mutex is held for sub-millisecond read/modify/write only.
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(25);
/// A lock older than this lost its holder (crash mid-section); steal it.
const STALE_LOCK: Duration = Duration::from_secs(30);

/// Outcome of trying to claim a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerOutcome {
    /// The caller now owns the session.
    Acquired,
    /// A live owner already holds it — refuse (`409 session.busy`).
    Busy,
}

/// Outcome of a heartbeat tick. Lock contention is TRANSIENT (retry next tick) and
/// must NOT be confused with losing ownership, or one momentary blip would stop the
/// heartbeat and let a still-running slow provider be reaped as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    /// `heartbeat_at` was refreshed; keep beating.
    Updated,
    /// The lock was contended this tick; keep beating (try again next tick).
    Contended,
    /// We no longer own the session (replaced or control gone); stop beating.
    NotOwner,
}

/// `.maestro/chat/control/<session-id>.json`. Validates the id before touching FS.
pub fn control_path(session_id: &str) -> Result<PathBuf> {
    paths::validate_path_component("chat session id", session_id)?;
    let dir = chat_dir()?.join(CONTROL_DIR);
    paths::ensure_dir(&dir)?;
    Ok(dir.join(format!("{session_id}.json")))
}

/// `.maestro/chat/locks/<session-id>.lock`. The lock lifecycle itself is Step 2.
pub fn lock_path(session_id: &str) -> Result<PathBuf> {
    paths::validate_path_component("chat session id", session_id)?;
    let dir = chat_dir()?.join(LOCKS_DIR);
    paths::ensure_dir(&dir)?;
    Ok(dir.join(format!("{session_id}.lock")))
}

/// Load + validate the control record. Missing file → `Ok(None)`; corrupt JSON or
/// a schema/consistency violation → `Err` (after-read validation, never ignored).
pub fn load_control(session_id: &str) -> Result<Option<SessionControl>> {
    let path = control_path(session_id)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read session control {session_id:?}")),
    };
    let control: SessionControl = serde_json::from_str(&text)
        .with_context(|| format!("parse session control {session_id:?}"))?;
    validate_control(&control, session_id)
        .with_context(|| format!("validate session control {session_id:?}"))?;
    Ok(Some(control))
}

/// Validate (before-write) then atomically persist (`tmp + fsync + rename`).
pub fn save_control(control: &SessionControl) -> Result<()> {
    validate_control(control, &control.session_id)?;
    let path = control_path(&control.session_id)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;
        file.write_all(serde_json::to_string_pretty(control)?.as_bytes())?;
        file.sync_all()?; // durable before rename
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2 — busy ownership guard (sync; callers wrap in spawn_blocking).
// ---------------------------------------------------------------------------

/// RAII holder for the short per-session control lock. Dropping it releases the
/// lock even if the critical section panics.
struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Acquire the short control-file lock (atomic `create_new`). A lock left behind by
/// a crashed holder (older than `STALE_LOCK`) is stolen. `Ok(None)` means genuine
/// contention (another live owner is mid-mutation) — callers treat that as BUSY,
/// never an internal error. `Err` is a real filesystem failure.
fn acquire_lock(session_id: &str) -> Result<Option<LockGuard>> {
    let path = lock_path(session_id)?;
    let start = Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                let _ = file.sync_all();
                return Ok(Some(LockGuard { path }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|m| m.elapsed().ok())
                    .map(|age| age > STALE_LOCK)
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if start.elapsed() > LOCK_TIMEOUT {
                    return Ok(None); // contended → busy, not an error
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(e) => {
                return Err(e).with_context(|| format!("acquire session lock {session_id:?}"))
            }
        }
    }
}

/// An owner is recoverable (its claim can be taken over) when its local process is
/// gone OR its heartbeat is stale beyond the threshold. A live pid with a fresh
/// heartbeat is NOT recoverable.
fn is_owner_recoverable(owner: &SessionOwner, now: &str) -> bool {
    use crate::scheduler::liveness::{classify_pid, RunLiveness};
    if matches!(classify_pid(owner.pid), RunLiveness::Abandoned) {
        return true;
    }
    if let (Ok(hb), Ok(now_t)) = (
        chrono::DateTime::parse_from_rfc3339(&owner.heartbeat_at),
        chrono::DateTime::parse_from_rfc3339(now),
    ) {
        if now_t.signed_duration_since(hb).num_seconds() > HEARTBEAT_STALE_SECS {
            return true;
        }
    }
    false
}

/// Keep the receipt window bounded (first-turn receipt exempt) so it stays within
/// the schema cap before a write.
fn compact_receipts(control: &mut SessionControl) {
    let non_first = control.turns.iter().filter(|t| !t.first_turn).count();
    if non_first <= MAX_TURN_RECEIPTS {
        return;
    }
    let mut drop_count = non_first - MAX_TURN_RECEIPTS;
    control.turns.retain(|t| {
        if drop_count > 0 && !t.first_turn {
            drop_count -= 1;
            false
        } else {
            true
        }
    });
}

/// Claim the session for `owner` (and, for a chat_turn, its live `receipt`). Under
/// the short lock: refuse if a live owner holds it, otherwise recover an
/// abandoned/stale owner and write the new one. `now` is caller-supplied RFC3339.
pub fn prepare_session_owner(
    owner: SessionOwner,
    receipt: Option<SessionTurnReceipt>,
    session_id: &str,
    now: &str,
) -> Result<OwnerOutcome> {
    let Some(_lock) = acquire_lock(session_id)? else {
        return Ok(OwnerOutcome::Busy); // contended lock → busy, not 500
    };
    let mut control =
        load_control(session_id)?.unwrap_or_else(|| SessionControl::new(session_id, now));

    if let Some(existing) = control.active_owner.clone() {
        if !is_owner_recoverable(&existing, now) {
            return Ok(OwnerOutcome::Busy);
        }
        // recover: mark the abandoned owner's live receipt abandoned, drop the owner.
        if existing.kind == OwnerKind::ChatTurn {
            if let Some(r) = control
                .turns
                .iter_mut()
                .find(|t| t.turn_id == existing.owner_id && !t.phase.is_settled())
            {
                r.phase = TurnPhase::Abandoned;
                r.settled_at = Some(now.to_string());
            }
        }
        control.active_owner = None;
    }

    if let Some(receipt) = receipt {
        control.turns.push(receipt);
        compact_receipts(&mut control);
    }
    control.active_owner = Some(owner);
    control.updated_at = now.to_string();
    save_control(&control)?;
    Ok(OwnerOutcome::Acquired)
}

/// Refresh the owner heartbeat IFF we still own the session. Returns `Updated` on a
/// refresh, `Contended` if the lock was briefly busy (the caller retries next tick),
/// and `NotOwner` when the control file is gone or another owner replaced us (the
/// caller stops heartbeating).
pub fn heartbeat_session_owner(
    session_id: &str,
    owner_id: &str,
    now: &str,
) -> Result<HeartbeatOutcome> {
    let Some(_lock) = acquire_lock(session_id)? else {
        return Ok(HeartbeatOutcome::Contended); // transient → retry next tick, do NOT stop
    };
    let Some(mut control) = load_control(session_id)? else {
        return Ok(HeartbeatOutcome::NotOwner); // control gone → stop
    };
    match control.active_owner.as_mut() {
        Some(owner) if owner.owner_id == owner_id => {
            owner.heartbeat_at = now.to_string();
            control.updated_at = now.to_string();
            save_control(&control)?;
            Ok(HeartbeatOutcome::Updated)
        }
        _ => Ok(HeartbeatOutcome::NotOwner), // replaced → stop
    }
}

/// Release ownership and settle the turn receipt — but ONLY if we still own it (a
/// late finisher must not clear a newer owner that took over after stale recovery).
pub fn clear_session_owner(
    session_id: &str,
    owner_id: &str,
    settle: TurnPhase,
    assistant_message_id: Option<String>,
    now: &str,
) -> Result<()> {
    let Some(_lock) = acquire_lock(session_id)? else {
        return Ok(()); // contended → leave it; heartbeat-stale recovery will clean up
    };
    let Some(mut control) = load_control(session_id)? else {
        return Ok(());
    };
    let still_ours = control
        .active_owner
        .as_ref()
        .map(|o| o.owner_id == owner_id)
        .unwrap_or(false);
    if !still_ours {
        return Ok(());
    }
    control.active_owner = None;
    if let Some(r) = control.turns.iter_mut().find(|t| t.turn_id == owner_id) {
        if !r.phase.is_settled() {
            r.phase = settle;
            r.settled_at = Some(now.to_string());
        }
        if assistant_message_id.is_some() {
            r.assistant_message_id = assistant_message_id;
        }
    }
    control.updated_at = now.to_string();
    save_control(&control)?;
    Ok(())
}

/// Idempotency verdict for a client `turn_id` (F-119 Step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDedup {
    /// First time we have seen this turn_id — accept and run it.
    New,
    /// The same turn_id is still in flight (accepted/running) — refuse (409).
    Running,
    /// The turn already completed with the SAME request payload — replay the
    /// persisted assistant message (no provider call, no new user message).
    DoneReplay {
        assistant_message_id: Option<String>,
    },
    /// Same turn_id but a DIFFERENT request payload — refuse (409); do not replay.
    PayloadMismatch,
    /// The turn already failed/abandoned — refuse (409); do not silently re-send.
    NotRetriable,
}

/// Decide what to do with an incoming `(turn_id, request_hash)` against the recorded
/// receipts. A contended lock is treated as `Running` (refuse) — safe, never a
/// double-send. The raw request text is NOT needed: receipts carry only its hash.
pub fn check_turn_idempotency(
    session_id: &str,
    turn_id: &str,
    request_hash: &str,
) -> Result<TurnDedup> {
    let Some(_lock) = acquire_lock(session_id)? else {
        return Ok(TurnDedup::Running); // can't verify → refuse, don't risk a double-send
    };
    let Some(control) = load_control(session_id)? else {
        return Ok(TurnDedup::New);
    };
    let Some(receipt) = control.receipt(turn_id) else {
        return Ok(TurnDedup::New);
    };
    Ok(match receipt.phase {
        TurnPhase::Accepted | TurnPhase::Running => TurnDedup::Running,
        TurnPhase::Done => {
            if receipt.request_hash == request_hash {
                TurnDedup::DoneReplay {
                    assistant_message_id: receipt.assistant_message_id.clone(),
                }
            } else {
                TurnDedup::PayloadMismatch
            }
        }
        TurnPhase::Failed | TurnPhase::Abandoned => TurnDedup::NotRetriable,
    })
}

/// Compare a freshly-computed reuse signature to the session's stored one, store the
/// new signature (and refresh the active owner's / its receipt's signature hash so
/// the receipt reflects the real signature), and return whether the previous
/// provider session may be reused. A signature drift, a first-ever signature, or a
/// contended lock all return `false` → open a fresh provider session rather than
/// silently resuming a drifted one.
pub fn update_signature_and_decide_reuse(
    session_id: &str,
    new_signature: SessionReuseSignature,
    now: &str,
) -> Result<bool> {
    let Some(_lock) = acquire_lock(session_id)? else {
        return Ok(false); // can't verify under contention → safe reopen
    };
    let mut control =
        load_control(session_id)?.unwrap_or_else(|| SessionControl::new(session_id, now));
    let reuse = control
        .signature
        .as_ref()
        .map(|s| s.hash == new_signature.hash)
        .unwrap_or(false);

    let new_hash = new_signature.hash.clone();
    if let Some(owner) = control.active_owner.as_mut() {
        owner.session_signature_hash = Some(new_hash.clone());
        if owner.kind == OwnerKind::ChatTurn {
            let owner_id = owner.owner_id.clone();
            if let Some(receipt) = control.turns.iter_mut().find(|t| t.turn_id == owner_id) {
                receipt.signature_hash = new_hash;
            }
        }
    }
    control.signature = Some(new_signature);
    control.updated_at = now.to_string();
    save_control(&control)?;
    Ok(reuse)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    #[serial]
    fn path_helpers_reject_unsafe_ids_before_fs() {
        let _w = workspace();
        for bad in ["../escape", "a/b", "..", ""] {
            assert!(
                control_path(bad).is_err(),
                "control_path({bad:?}) must reject"
            );
            assert!(lock_path(bad).is_err(), "lock_path({bad:?}) must reject");
        }
        clear();
    }

    #[test]
    #[serial]
    fn missing_is_none_roundtrips_and_corrupt_is_err() {
        let _w = workspace();
        // missing -> None
        assert!(load_control("s-fresh").unwrap().is_none());

        // write + read back
        let control = SessionControl::new("s-fresh", "2026-06-05T00:00:00Z");
        save_control(&control).unwrap();
        assert_eq!(load_control("s-fresh").unwrap().unwrap(), control);

        // corrupt JSON -> Err (not silently ignored)
        std::fs::write(control_path("s-fresh").unwrap(), "{not json").unwrap();
        assert!(load_control("s-fresh").is_err());

        // valid JSON but wrong schema_version -> Err on validate-after-read
        let mut bad = SessionControl::new("s-bad", "2026-06-05T00:00:00Z");
        bad.schema_version = "maestro.session_control.v2".into();
        std::fs::write(
            control_path("s-bad").unwrap(),
            serde_json::to_string(&bad).unwrap(),
        )
        .unwrap();
        assert!(load_control("s-bad").is_err());
        clear();
    }

    #[test]
    #[serial]
    fn save_refuses_session_id_filename_mismatch() {
        // a control whose own session_id is unsafe cannot be written.
        let _w = workspace();
        let mut control = SessionControl::new("ok-id", "2026-06-05T00:00:00Z");
        control.session_id = "../evil".into();
        assert!(save_control(&control).is_err());
        clear();
    }

    // --- Step 2 ownership ---------------------------------------------------

    const H: &str = "fnv1a64:0123456789abcdef";

    fn owner(kind: OwnerKind, owner_id: &str, pid: u32, heartbeat_at: &str) -> SessionOwner {
        SessionOwner {
            owner_id: owner_id.into(),
            kind,
            pid,
            started_at: "2026-06-05T00:00:00Z".into(),
            heartbeat_at: heartbeat_at.into(),
            session_signature_hash: None,
            message_id: None,
            action_id: (kind == OwnerKind::Action).then(|| owner_id.into()),
            run_id: None,
        }
    }

    fn running_receipt(turn_id: &str) -> SessionTurnReceipt {
        use crate::schema::session_control::TurnRole;
        SessionTurnReceipt {
            turn_id: turn_id.into(),
            client_nonce_hash: None,
            role: TurnRole::User,
            phase: TurnPhase::Running,
            first_turn: false,
            user_message_id: "m-1".into(),
            assistant_message_id: None,
            request_hash: H.into(),
            signature_hash: H.into(),
            accepted_at: "2026-06-05T00:00:00Z".into(),
            settled_at: None,
        }
    }

    #[test]
    #[serial]
    fn busy_when_live_owner_then_clear_releases() {
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        let me = std::process::id();
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-1", me, now),
                Some(running_receipt("t-1")),
                "s1",
                now
            )
            .unwrap(),
            OwnerOutcome::Acquired
        );
        // a second live claim is refused
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-2", me, now),
                Some(running_receipt("t-2")),
                "s1",
                now
            )
            .unwrap(),
            OwnerOutcome::Busy
        );
        // clearing the owner releases the session
        clear_session_owner("s1", "t-1", TurnPhase::Done, None, now).unwrap();
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-3", me, now),
                Some(running_receipt("t-3")),
                "s1",
                now
            )
            .unwrap(),
            OwnerOutcome::Acquired
        );
        clear();
    }

    #[test]
    #[serial]
    fn recovers_abandoned_pid_owner() {
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-old", 999_999, now),
            Some(running_receipt("t-old")),
            "s2",
            now,
        )
        .unwrap();
        // dead pid -> recoverable -> new owner acquires, old receipt abandoned
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-new", std::process::id(), now),
                Some(running_receipt("t-new")),
                "s2",
                now
            )
            .unwrap(),
            OwnerOutcome::Acquired
        );
        let c = load_control("s2").unwrap().unwrap();
        assert_eq!(c.receipt("t-old").unwrap().phase, TurnPhase::Abandoned);
        assert_eq!(c.active_owner.unwrap().owner_id, "t-new");
        clear();
    }

    #[test]
    #[serial]
    fn recovers_stale_heartbeat_owner_but_not_fresh() {
        let _w = workspace();
        let me = std::process::id();
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-old", me, "2026-06-05T00:00:00Z"),
            Some(running_receipt("t-old")),
            "s3",
            "2026-06-05T00:00:00Z",
        )
        .unwrap();
        // 4 minutes later, heartbeat is stale (live pid, but > 3min) -> recoverable
        let now = "2026-06-05T00:04:00Z";
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-new", me, now),
                Some(running_receipt("t-new")),
                "s3",
                now
            )
            .unwrap(),
            OwnerOutcome::Acquired
        );

        // but a freshly-heartbeated owner is NOT abandoned (slow provider stays live)
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "h-1", me, "2026-06-05T00:00:00Z"),
            Some(running_receipt("h-1")),
            "s3b",
            "2026-06-05T00:00:00Z",
        )
        .unwrap();
        heartbeat_session_owner("s3b", "h-1", "2026-06-05T00:03:30Z").unwrap();
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "h-2", me, "2026-06-05T00:03:40Z"),
                Some(running_receipt("h-2")),
                "s3b",
                "2026-06-05T00:03:40Z"
            )
            .unwrap(),
            OwnerOutcome::Busy,
            "10s after a heartbeat the owner is still live"
        );
        clear();
    }

    #[test]
    #[serial]
    fn heartbeat_updates_then_stops_when_replaced() {
        let _w = workspace();
        let me = std::process::id();
        let now = "2026-06-05T00:00:00Z";
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-1", me, now),
            Some(running_receipt("t-1")),
            "s4",
            now,
        )
        .unwrap();
        let later = "2026-06-05T00:00:30Z";
        assert_eq!(
            heartbeat_session_owner("s4", "t-1", later).unwrap(),
            HeartbeatOutcome::Updated
        );
        assert_eq!(
            load_control("s4")
                .unwrap()
                .unwrap()
                .active_owner
                .unwrap()
                .heartbeat_at,
            later
        );
        // a heartbeat for a different owner id stops (NotOwner), not Updated
        assert_eq!(
            heartbeat_session_owner("s4", "t-other", later).unwrap(),
            HeartbeatOutcome::NotOwner
        );
        clear();
    }

    #[test]
    #[serial]
    fn heartbeat_contention_continues_but_replaced_stops() {
        // N4: a transient lock contention must NOT stop the heartbeat (it would
        // falsely abandon a still-running slow provider); only losing ownership does.
        let _w = workspace();
        let me = std::process::id();
        let now = "2026-06-05T00:00:00Z";
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-1", me, now),
            Some(running_receipt("t-1")),
            "shb",
            now,
        )
        .unwrap();
        // hold a fresh lock → the next heartbeat is Contended (keep beating)
        let lock = lock_path("shb").unwrap();
        std::fs::write(&lock, "99999").unwrap();
        assert_eq!(
            heartbeat_session_owner("shb", "t-1", now).unwrap(),
            HeartbeatOutcome::Contended
        );
        let _ = std::fs::remove_file(&lock);
        // a replaced/unknown owner → NotOwner (stop)
        assert_eq!(
            heartbeat_session_owner("shb", "t-other", now).unwrap(),
            HeartbeatOutcome::NotOwner
        );
        clear();
    }

    #[test]
    #[serial]
    fn late_finisher_cannot_clear_newer_owner() {
        let _w = workspace();
        let me = std::process::id();
        let now = "2026-06-05T00:00:00Z";
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-A", me, now),
            Some(running_receipt("t-A")),
            "s5",
            now,
        )
        .unwrap();
        clear_session_owner("s5", "t-A", TurnPhase::Done, None, now).unwrap();
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-B", me, now),
            Some(running_receipt("t-B")),
            "s5",
            now,
        )
        .unwrap();
        // a late clear from the previous owner must not clear B
        clear_session_owner("s5", "t-A", TurnPhase::Done, None, now).unwrap();
        assert_eq!(
            load_control("s5")
                .unwrap()
                .unwrap()
                .active_owner
                .unwrap()
                .owner_id,
            "t-B"
        );
        clear();
    }

    #[test]
    #[serial]
    fn action_owner_blocks_chat_turn_and_vice_versa() {
        let _w = workspace();
        let me = std::process::id();
        let now = "2026-06-05T00:00:00Z";
        assert_eq!(
            prepare_session_owner(owner(OwnerKind::Action, "a-1", me, now), None, "s6", now)
                .unwrap(),
            OwnerOutcome::Acquired
        );
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-1", me, now),
                Some(running_receipt("t-1")),
                "s6",
                now
            )
            .unwrap(),
            OwnerOutcome::Busy
        );
        clear_session_owner("s6", "a-1", TurnPhase::Done, None, now).unwrap();
        assert_eq!(
            prepare_session_owner(
                owner(OwnerKind::ChatTurn, "t-2", me, now),
                Some(running_receipt("t-2")),
                "s6",
                now
            )
            .unwrap(),
            OwnerOutcome::Acquired
        );
        clear();
    }

    fn sig(hash: &str) -> SessionReuseSignature {
        use crate::schema::session_control::SessionMode;
        SessionReuseSignature {
            hash: hash.into(),
            provider: "cursor".into(),
            model: None,
            mode: SessionMode::Exec,
            workspace_hash: H.into(),
            prompt_contract_hash: H.into(),
            project_registry_hash: H.into(),
            memory_topic_hash: H.into(),
            skill_index_hash: H.into(),
            created_at: "2026-06-05T00:00:00Z".into(),
        }
    }

    #[test]
    #[serial]
    fn signature_reuse_decision_matches_then_drifts() {
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        // first-ever signature → no reuse; same again → reuse; drift → no reuse.
        assert!(
            !update_signature_and_decide_reuse("sg", sig("fnv1a64:1111111111111111"), now).unwrap()
        );
        assert!(
            update_signature_and_decide_reuse("sg", sig("fnv1a64:1111111111111111"), now).unwrap()
        );
        assert!(
            !update_signature_and_decide_reuse("sg", sig("fnv1a64:2222222222222222"), now).unwrap()
        );
        assert!(
            update_signature_and_decide_reuse("sg", sig("fnv1a64:2222222222222222"), now).unwrap()
        );

        // with an active chat_turn owner, the owner + its receipt signature_hash track it
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-1", std::process::id(), now),
            Some(running_receipt("t-1")),
            "sg2",
            now,
        )
        .unwrap();
        update_signature_and_decide_reuse("sg2", sig("fnv1a64:3333333333333333"), now).unwrap();
        let c = load_control("sg2").unwrap().unwrap();
        assert_eq!(
            c.active_owner
                .as_ref()
                .unwrap()
                .session_signature_hash
                .as_deref(),
            Some("fnv1a64:3333333333333333")
        );
        assert_eq!(
            c.receipt("t-1").unwrap().signature_hash,
            "fnv1a64:3333333333333333"
        );
        clear();
    }

    #[test]
    #[serial]
    fn turn_idempotency_covers_all_branches() {
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        // unknown turn_id → New
        assert_eq!(
            check_turn_idempotency("td", "t-x", H).unwrap(),
            TurnDedup::New
        );

        // a running receipt → Running
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-run", std::process::id(), now),
            Some(running_receipt("t-run")),
            "td",
            now,
        )
        .unwrap();
        assert_eq!(
            check_turn_idempotency("td", "t-run", H).unwrap(),
            TurnDedup::Running
        );

        // settle it Done with an assistant id → same hash replays, different hash mismatches
        clear_session_owner("td", "t-run", TurnPhase::Done, Some("a-run".into()), now).unwrap();
        assert_eq!(
            check_turn_idempotency("td", "t-run", H).unwrap(),
            TurnDedup::DoneReplay {
                assistant_message_id: Some("a-run".into())
            }
        );
        assert_eq!(
            check_turn_idempotency("td", "t-run", "fnv1a64:9999999999999999").unwrap(),
            TurnDedup::PayloadMismatch
        );

        // a failed/abandoned receipt → NotRetriable
        prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-fail", std::process::id(), now),
            Some(running_receipt("t-fail")),
            "td",
            now,
        )
        .unwrap();
        clear_session_owner("td", "t-fail", TurnPhase::Failed, None, now).unwrap();
        assert_eq!(
            check_turn_idempotency("td", "t-fail", H).unwrap(),
            TurnDedup::NotRetriable
        );
        clear();
    }

    #[test]
    #[serial]
    fn signature_reuse_under_contention_is_safe_reopen() {
        // a contended lock can't verify the signature → return false (open fresh).
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        let lock = lock_path("sgc").unwrap();
        std::fs::write(&lock, "99999").unwrap();
        assert!(
            !update_signature_and_decide_reuse("sgc", sig("fnv1a64:4444444444444444"), now)
                .unwrap()
        );
        let _ = std::fs::remove_file(&lock);
        clear();
    }

    #[test]
    #[serial]
    fn contended_lock_is_busy_not_error() {
        // N2: a fresh (non-stale) lock file held by another holder makes a claim
        // BUSY after the timeout — never an Err that callers would surface as 500.
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        let lock = lock_path("sc").unwrap();
        std::fs::write(&lock, "99999").unwrap(); // a live (just-written) lock
        let out = prepare_session_owner(
            owner(OwnerKind::ChatTurn, "t-1", std::process::id(), now),
            Some(running_receipt("t-1")),
            "sc",
            now,
        )
        .expect("contention must not be an error");
        assert_eq!(out, OwnerOutcome::Busy);
        let _ = std::fs::remove_file(&lock);
        clear();
    }
}
