//! F-120 ack storage — per-run, per-consumer high-water files.
//!
//! Consumer state ONLY: this never trims `events.ndjson`, is never read by F-117
//! resume or F-112 monitor, and records only ids / seqs / enum / counters /
//! timestamp. Missing ack = "no cursor" (start at 0); a corrupt ack is an error for
//! ack-aware consumers, never silently treated as caught-up.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use crate::paths;
use crate::schema::event_delivery::{
    validate_ack, DeliveryMode, RunEventAck, RunEventAckRequest, RunEventAckStats,
};

const EVENT_ACK_DIR: &str = "event_ack";

/// The concrete run id from a run dir (its last path component).
fn run_id_of(run_dir: &Path) -> Option<&str> {
    run_dir.file_name().and_then(|n| n.to_str())
}

/// `.maestro/runs/<run>/event_ack/<consumer-id>.json` WITHOUT creating the dir, so a
/// read never litters an empty `event_ack/`.
fn ack_path_read(run_dir: &Path, consumer_id: &str) -> Result<PathBuf> {
    paths::validate_path_component("event consumer id", consumer_id)?;
    Ok(run_dir
        .join(EVENT_ACK_DIR)
        .join(format!("{consumer_id}.json")))
}

/// `.maestro/runs/<run>/event_ack/<consumer-id>.json` for writes/locks — ensures the
/// dir exists. Validates the consumer id before any filesystem access.
pub fn ack_path(run_dir: &Path, consumer_id: &str) -> Result<PathBuf> {
    paths::validate_path_component("event consumer id", consumer_id)?;
    let dir = run_dir.join(EVENT_ACK_DIR);
    paths::ensure_dir(&dir)?;
    Ok(dir.join(format!("{consumer_id}.json")))
}

/// Load + validate a consumer's ack. Missing file → `Ok(None)` (no cursor); corrupt
/// JSON, a schema/consistency violation, OR a file whose recorded identity does not
/// match where it lives → `Err` (never silently reset / never trusted as caught-up).
pub fn read_ack(run_dir: &Path, consumer_id: &str) -> Result<Option<RunEventAck>> {
    let path = ack_path_read(run_dir, consumer_id)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read event ack {consumer_id:?}")),
    };
    let ack: RunEventAck =
        serde_json::from_str(&text).with_context(|| format!("parse event ack {consumer_id:?}"))?;
    validate_ack(&ack).with_context(|| format!("validate event ack {consumer_id:?}"))?;
    // The file's recorded identity must match its location — a swapped/edited file
    // (wrong consumer_id or run_id) is corrupt, not a usable stored ack.
    ensure!(
        ack.consumer_id == consumer_id,
        "event ack file consumer_id does not match its path"
    );
    ensure!(
        Some(ack.run_id.as_str()) == run_id_of(run_dir),
        "event ack file run_id does not match its run directory"
    );
    Ok(Some(ack))
}

/// Validate (before-write) then atomically persist (`tmp + fsync + rename`).
pub fn write_ack_atomic(run_dir: &Path, ack: &RunEventAck) -> Result<()> {
    validate_ack(ack)?;
    let path = ack_path(run_dir, &ack.consumer_id)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;
        file.write_all(serde_json::to_string(ack)?.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Why an ack high-water update was refused (both → HTTP 409).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckUpdateError {
    /// `high_water_seq` is below the stored value (monotonic guard).
    Backwards,
    /// `high_water_seq` is beyond the current ledger `last_seq`.
    BeyondLedger,
}

/// Pure: validate the requested high-water against the stored ack + the ledger's
/// current `last_seq`, and build the new `RunEventAck`. The high-water must not move
/// backwards and must not exceed what the ledger actually contains. `now` is
/// caller-supplied RFC3339.
pub fn plan_ack(
    run_id: &str,
    consumer_id: &str,
    stored: Option<&RunEventAck>,
    req: &RunEventAckRequest,
    last_seq: u64,
    now: &str,
) -> Result<RunEventAck, AckUpdateError> {
    if let Some(stored) = stored {
        if req.high_water_seq < stored.high_water_seq {
            return Err(AckUpdateError::Backwards);
        }
    }
    if req.high_water_seq > last_seq {
        return Err(AckUpdateError::BeyondLedger);
    }
    let delivery = req
        .delivery
        .or_else(|| stored.map(|s| s.delivery))
        .unwrap_or(DeliveryMode::Lossless);
    Ok(RunEventAck {
        schema_version: crate::schema::run_event_ack_version(),
        run_id: run_id.to_string(),
        consumer_id: consumer_id.to_string(),
        high_water_seq: req.high_water_seq,
        last_seen_seq: last_seq,
        delivery,
        updated_at: now.to_string(),
        stats: RunEventAckStats {
            acked_events: req.acked_events.unwrap_or(0),
            acked_gaps: req.acked_gaps.unwrap_or(0),
            shed_events: req.shed_events.unwrap_or(0),
        },
    })
}

// --- atomic, monotonic commit under a per-consumer lock ---------------------

const ACK_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const ACK_LOCK_RETRY: Duration = Duration::from_millis(20);
const ACK_STALE_LOCK: Duration = Duration::from_secs(30);

/// RAII holder for the short per-consumer ack lock.
struct AckLockGuard {
    path: PathBuf,
}
impl Drop for AckLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Acquire the short `<consumer>.lock`. `Ok(None)` = genuine contention (another ack
/// in flight); a lock left by a crash (older than `ACK_STALE_LOCK`) is stolen.
fn acquire_ack_lock(run_dir: &Path, consumer_id: &str) -> Result<Option<AckLockGuard>> {
    paths::validate_path_component("event consumer id", consumer_id)?;
    let dir = run_dir.join(EVENT_ACK_DIR);
    paths::ensure_dir(&dir)?;
    let path = dir.join(format!("{consumer_id}.lock"));
    let start = Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                let _ = f.sync_all();
                return Ok(Some(AckLockGuard { path }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|m| m.elapsed().ok())
                    .map(|age| age > ACK_STALE_LOCK)
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if start.elapsed() > ACK_LOCK_TIMEOUT {
                    return Ok(None);
                }
                std::thread::sleep(ACK_LOCK_RETRY);
            }
            Err(e) => return Err(e).with_context(|| format!("acquire ack lock {consumer_id:?}")),
        }
    }
}

/// Why an ack commit was refused, mapped to HTTP status by the handler.
#[derive(Debug)]
pub enum AckError {
    /// high-water below the stored value (409).
    Backwards,
    /// high-water beyond the current ledger last_seq (409).
    BeyondLedger,
    /// another ack for this consumer is mid-commit (409 — retry).
    Contended,
    /// the event ledger is unreadable (500).
    CorruptLedger(anyhow::Error),
    /// the stored ack is corrupt/mismatched (500).
    CorruptAck(anyhow::Error),
    /// a filesystem/lock failure (500).
    Io(anyhow::Error),
}

/// Atomically commit a consumer's ack update: under the per-consumer lock, read the
/// ledger last_seq + the stored ack, enforce the MONOTONIC high-water against the
/// CURRENT persisted value, and write. The lock makes read→plan→write a single
/// critical section, so concurrent acks can never regress the high-water (and never
/// stomp the shared `.json.tmp`). `last_seq` is read inside the lock so the bound is
/// consistent. `now` is caller-supplied RFC3339.
pub fn commit_ack(
    run_dir: &Path,
    run_id: &str,
    req: &RunEventAckRequest,
    now: &str,
) -> Result<RunEventAck, AckError> {
    let Some(_lock) = acquire_ack_lock(run_dir, &req.consumer_id).map_err(AckError::Io)? else {
        return Err(AckError::Contended);
    };
    let last_seq = crate::scheduler::events::read_events(run_dir)
        .map_err(AckError::CorruptLedger)?
        .last()
        .map(|e| e.seq)
        .unwrap_or(0);
    let stored = read_ack(run_dir, &req.consumer_id).map_err(AckError::CorruptAck)?;
    let ack = plan_ack(
        run_id,
        &req.consumer_id,
        stored.as_ref(),
        req,
        last_seq,
        now,
    )
    .map_err(|e| match e {
        AckUpdateError::Backwards => AckError::Backwards,
        AckUpdateError::BeyondLedger => AckError::BeyondLedger,
    })?;
    write_ack_atomic(run_dir, &ack).map_err(AckError::Io)?;
    Ok(ack)
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

    fn req(high_water: u64) -> RunEventAckRequest {
        RunEventAckRequest {
            consumer_id: "webui-main".into(),
            high_water_seq: high_water,
            delivery: Some(DeliveryMode::Balanced),
            acked_events: Some(3),
            acked_gaps: Some(1),
            shed_events: Some(4),
        }
    }

    #[test]
    fn plan_ack_monotonic_and_bounded() {
        let now = "2026-06-05T00:00:00Z";
        // first ack within the ledger
        let a = plan_ack("r-1", "webui-main", None, &req(5), 10, now).unwrap();
        assert_eq!((a.high_water_seq, a.last_seen_seq), (5, 10));
        assert_eq!(a.delivery, DeliveryMode::Balanced);

        // equal/forward ok
        assert!(plan_ack("r-1", "webui-main", Some(&a), &req(5), 10, now).is_ok());
        assert!(plan_ack("r-1", "webui-main", Some(&a), &req(8), 10, now).is_ok());

        // backwards → refused
        assert_eq!(
            plan_ack("r-1", "webui-main", Some(&a), &req(4), 10, now).unwrap_err(),
            AckUpdateError::Backwards
        );
        // beyond ledger → refused
        assert_eq!(
            plan_ack("r-1", "webui-main", None, &req(11), 10, now).unwrap_err(),
            AckUpdateError::BeyondLedger
        );

        // delivery falls back to stored when the request omits it
        let mut r = req(6);
        r.delivery = None;
        assert_eq!(
            plan_ack("r-1", "webui-main", Some(&a), &r, 10, now)
                .unwrap()
                .delivery,
            DeliveryMode::Balanced
        );
    }

    #[test]
    #[serial]
    fn ack_storage_roundtrips_missing_is_none_corrupt_is_err() {
        let dir = workspace();
        let run_dir = dir.path().join(".maestro/runs/r-1");
        std::fs::create_dir_all(&run_dir).unwrap();

        // missing → None
        assert!(read_ack(&run_dir, "webui-main").unwrap().is_none());

        // write + read back
        let ack = plan_ack(
            "r-1",
            "webui-main",
            None,
            &req(5),
            10,
            "2026-06-05T00:00:00Z",
        )
        .unwrap();
        write_ack_atomic(&run_dir, &ack).unwrap();
        assert_eq!(read_ack(&run_dir, "webui-main").unwrap().unwrap(), ack);

        // a serialized ack carries no path / payload
        let json = std::fs::read_to_string(ack_path(&run_dir, "webui-main").unwrap()).unwrap();
        assert!(!json.contains('/') && !json.contains("payload"));

        // corrupt → Err (never silently None)
        std::fs::write(ack_path(&run_dir, "webui-main").unwrap(), "{not json").unwrap();
        assert!(read_ack(&run_dir, "webui-main").is_err());

        // unsafe consumer id rejected before FS
        assert!(ack_path(&run_dir, "../escape").is_err());
        assert!(read_ack(&run_dir, "a/b").is_err());
        clear();
    }

    #[test]
    #[serial]
    fn read_ack_rejects_identity_mismatch() {
        // N2: a valid-JSON ack whose recorded consumer_id / run_id does not match its
        // location is corrupt, not a usable stored ack.
        let dir = workspace();
        let run = dir.path().join(".maestro/runs/r-1");
        std::fs::create_dir_all(run.join(EVENT_ACK_DIR)).unwrap();
        let file = run.join(EVENT_ACK_DIR).join("webui-main.json");

        // wrong consumer_id in the file body
        let mut a = plan_ack(
            "r-1",
            "webui-main",
            None,
            &req(3),
            10,
            "2026-06-05T00:00:00Z",
        )
        .unwrap();
        a.consumer_id = "someone-else".into();
        std::fs::write(&file, serde_json::to_string(&a).unwrap()).unwrap();
        assert!(
            read_ack(&run, "webui-main").is_err(),
            "consumer_id mismatch must be corrupt"
        );

        // wrong run_id in the file body
        let a = plan_ack(
            "other-run",
            "webui-main",
            None,
            &req(3),
            10,
            "2026-06-05T00:00:00Z",
        )
        .unwrap();
        std::fs::write(&file, serde_json::to_string(&a).unwrap()).unwrap();
        assert!(
            read_ack(&run, "webui-main").is_err(),
            "run_id mismatch must be corrupt"
        );
        clear();
    }

    #[test]
    #[serial]
    fn concurrent_commits_never_regress_high_water() {
        // N1: with the per-consumer lock, two interleaved commits (15 and 9) can never
        // leave the persisted high-water below the max that committed.
        use crate::scheduler::{append_event, RunEventKind};
        let dir = workspace();
        let run = dir.path().join(".maestro/runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        for i in 1..=20u64 {
            append_event(
                &run,
                "r-1",
                RunEventKind::TaskStarted,
                Some("t1"),
                None,
                serde_json::json!({ "i": i }),
            )
            .unwrap();
        }
        let now = "2026-06-05T00:00:00Z".to_string();
        let (ra, rb, na, nb) = (run.clone(), run.clone(), now.clone(), now.clone());
        let t1 = std::thread::spawn(move || {
            let _ = commit_ack(&ra, "r-1", &req(15), &na);
        });
        let t2 = std::thread::spawn(move || {
            let _ = commit_ack(&rb, "r-1", &req(9), &nb);
        });
        t1.join().unwrap();
        t2.join().unwrap();
        let stored = read_ack(&run, "webui-main").unwrap().unwrap();
        assert!(
            stored.high_water_seq >= 15,
            "concurrent acks must not regress below the max committed (got {})",
            stored.high_water_seq
        );
        clear();
    }
}
