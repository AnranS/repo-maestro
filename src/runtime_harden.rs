//! F-136b1 — cross-platform agent-runtime hardening: env scrub + output cap.
//!
//! The agent is an **untrusted code executor** (the provider CLI: codex /
//! cursor). We can only enforce at the boundary we own — the `Command` we build
//! and the bytes we persist. This module provides two such controls, both
//! **off by default** (an honest run under default config is byte-for-byte
//! unchanged) and both **hard when on**:
//!
//! 1. [`allowlist_env`] / [`apply_env_scrub`] — rebuild the child environment
//!    from an explicit allowlist instead of inheriting maestro's whole env, so
//!    unrelated env-borne secrets are not handed to the agent. A scrubbed var is
//!    genuinely absent from the child (OS-hard).
//! 2. [`CappedLog`] — bound a single task log's size, keeping head + a clear
//!    truncation marker + tail (where the final result lives). Never silent.
//!
//! HONESTY: env scrub blocks **env-borne** secrets only. `HOME` is kept (the
//! provider CLIs authenticate via their config dir under it), so the child can
//! still read files under the home directory — this is NOT filesystem
//! isolation. True FS confinement (and wall-clock / rlimit) are separate later
//! slices. The opaque provider `--sandbox` is the provider's own concern; we
//! never claim to enforce inside it.

use std::collections::VecDeque;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Resolved per-run hardening, threaded onto the agent task. `Default` is the
/// fully-off state (no behavior change) so test constructors and non-agent
/// paths need no config.
#[derive(Debug, Clone, Default)]
pub struct Hardening {
    pub scrub_env: bool,
    pub extra_allow_env: Vec<String>,
    pub max_task_log_bytes: usize,
}

impl From<&crate::config::projects::RuntimeHardening> for Hardening {
    fn from(c: &crate::config::projects::RuntimeHardening) -> Self {
        Self {
            scrub_env: c.scrub_env,
            extra_allow_env: c.extra_allow_env.clone(),
            max_task_log_bytes: c.max_task_log_bytes,
        }
    }
}

/// Base allowlist — the variables any program needs to run and to locate its
/// own config/auth. Deliberately explicit (no `MAESTRO_*` wildcard, pin 2): the
/// `MAESTRO_*` vars are maestro's own (binary discovery, session id) and the
/// agent child does not need them.
const BASE_ALLOW: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TERM", "TMPDIR", "TZ",
];

/// Prefixes kept wholesale — only the locale family, which programs read piece
/// by piece (`LC_ALL`, `LC_CTYPE`, …) and which carry no secrets.
const ALLOW_PREFIX: &[&str] = &["LC_"];

/// The provider CLI's own auth environment, declared per adapter (pin 2: an
/// explicit name, never a `*_KEY` wildcard). Empty for a provider that
/// authenticates purely via its config dir under `HOME` (kept by the base
/// allowlist). These are the conventional auth vars each CLI reads; anything a
/// user's setup needs beyond them goes through `extra_allow_env`.
pub fn provider_auth_env(provider: &str) -> &'static [&'static str] {
    match provider {
        "codex" => &["OPENAI_API_KEY", "OPENAI_BASE_URL"],
        "claude" => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
        ],
        "cursor" => &["CURSOR_API_KEY"],
        _ => &[],
    }
}

/// Pure: pick the env pairs to keep for an agent child of `provider`, given the
/// parent env and the configured `extra_allow` names. The caller does
/// `cmd.env_clear(); cmd.envs(kept)`.
pub fn allowlist_env<I>(parent: I, provider: &str, extra_allow: &[String]) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let auth = provider_auth_env(provider);
    parent
        .into_iter()
        .filter(|(k, _)| {
            BASE_ALLOW.contains(&k.as_str())
                || auth.contains(&k.as_str())
                || ALLOW_PREFIX.iter().any(|p| k.starts_with(p))
                || extra_allow.iter().any(|e| e == k)
        })
        .collect()
}

/// Apply the env allowlist to a not-yet-spawned command: clear the inherited
/// environment and set only the allowlisted vars. Reads the live process env.
pub fn apply_env_scrub(cmd: &mut tokio::process::Command, provider: &str, extra_allow: &[String]) {
    let kept = allowlist_env(std::env::vars(), provider, extra_allow);
    cmd.env_clear();
    cmd.envs(kept);
}

/// An append log that enforces a byte cap. `cap == 0` is an unbounded
/// passthrough (today's behavior, used when hardening is off). When a cap is
/// set, bytes up to the head budget go straight to the file; the overflow feeds
/// a bounded tail ring; [`finalize`](CappedLog::finalize) appends a truncation
/// marker + the retained tail. So the file is head + marker + tail — the middle
/// (usually verbose tool spam) is dropped, the final result (written last) is
/// preserved. Never silent.
pub struct CappedLog {
    file: tokio::fs::File,
    cap: usize,
    head_budget: usize,
    head_used: usize,
    tail: VecDeque<u8>,
    tail_cap: usize,
    truncated: bool,
    finalized: bool,
}

impl CappedLog {
    /// Open (create+append) the log at `path` with byte `cap` (0 = unbounded).
    pub async fn open(path: &Path, cap: usize) -> std::io::Result<Self> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        // Reserve roughly a quarter (min 1 KiB) of the budget for the tail, so
        // the final result/transcript — emitted last — survives truncation.
        // B1: explicit branches — `clamp(1024, cap)` would panic when
        // `cap < 1024` (min > max). A tiny cap keeps only a tail (head budget 0).
        let tail_cap = if cap == 0 {
            0
        } else if cap <= 1024 {
            cap
        } else {
            (cap / 4).max(1024).min(cap)
        };
        let head_budget = cap.saturating_sub(tail_cap);
        Ok(Self {
            file,
            cap,
            head_budget,
            head_used: 0,
            tail: VecDeque::new(),
            tail_cap,
            truncated: false,
            finalized: false,
        })
    }

    /// Append `bytes`, enforcing the cap. With `cap == 0` this is a direct
    /// passthrough write (byte-for-byte the un-hardened behavior).
    pub async fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.cap == 0 {
            return self.file.write_all(bytes).await;
        }
        let mut rest = bytes;
        if self.head_used < self.head_budget {
            let room = self.head_budget - self.head_used;
            let take = room.min(rest.len());
            self.file.write_all(&rest[..take]).await?;
            self.head_used += take;
            rest = &rest[take..];
        }
        if !rest.is_empty() {
            self.truncated = true;
            self.tail.extend(rest.iter().copied());
            while self.tail.len() > self.tail_cap {
                self.tail.pop_front();
            }
        }
        Ok(())
    }

    /// Append the truncation marker + retained tail (if the cap was exceeded),
    /// then flush so the whole log is durable on the OS. Call once after the
    /// stream ends. The marker/tail write is idempotent (the `finalized` guard);
    /// `truncated` stays set so callers can still query whether the cap bit.
    ///
    /// The flush is UNCONDITIONAL — `tokio::fs::File::write_all` only accepts
    /// bytes into tokio's internal buffer, it does not guarantee they reach the
    /// OS file, so without a flush an immediate reader (or a not-yet-dropped
    /// handle) can race and see a partial/empty log.
    pub async fn finalize(&mut self) -> std::io::Result<()> {
        if self.truncated && !self.finalized && self.cap != 0 {
            self.finalized = true;
            let marker = format!(
                "\n[maestro] \u{2026}task log truncated at cap {} bytes (head kept, middle dropped, tail follows)\u{2026}\n",
                self.cap
            );
            self.file.write_all(marker.as_bytes()).await?;
            let tail: Vec<u8> = self.tail.drain(..).collect();
            self.file.write_all(&tail).await?;
        }
        self.file.flush().await
    }

    /// Whether the cap has been exceeded (any bytes diverted to the tail ring).
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// Tee a child's stderr into the task log, bounded. Reads the RAW stderr in
/// fixed-size chunks all the way to the child's real EOF (so the pipe is always
/// drained — no EPIPE / SIGPIPE / blocked child — even after we stop persisting,
/// B3) and re-assembles it into LINES so the on-disk format is byte-for-byte the
/// historical `[stderr] <line>\n` with per-line redaction (B4: `cap == 0` and any
/// under-cap honest output are unchanged from before hardening).
///
/// Memory is bounded: a newline-less line is flushed once it reaches `MAX_LINE`,
/// so a giant line never enters memory whole. The byte budget (`cap > 0`) is a
/// HARD cap: once a line would cross it, the fitting head + an explicit marker
/// are written and nothing more is persisted (never silent), while the rest is
/// still read+discarded to EOF. Flushes for durability.
///
/// NOTE: this is a PER-STREAM budget. The stdout digest (a `CappedLog`) and this
/// stderr pump each get `max_task_log_bytes`, so the task log is bounded by
/// ~2×cap + markers, not a single shared budget. (A shared budget would need a
/// mutex across the two concurrent writers; per-stream is the simpler bound.)
pub async fn pump_stderr_capped<R>(stderr: R, log_path: &Path, cap: usize)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    const MAX_LINE: usize = 65536; // bound memory for a newline-less giant line
    let mut log = match tokio::fs::OpenOptions::new()
        .append(true)
        .open(log_path)
        .await
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut reader = stderr; // RAW — drained to the child's real EOF, never `.take`
    let mut buf = [0u8; 16384];
    let mut line: Vec<u8> = Vec::new();
    let mut written = 0usize;
    let mut capped_out = false;
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break, // child closed stderr
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            if b == b'\n' {
                write_stderr_line(&mut log, &line, cap, &mut written, &mut capped_out).await;
                line.clear();
            } else {
                line.push(b);
                if line.len() >= MAX_LINE {
                    write_stderr_line(&mut log, &line, cap, &mut written, &mut capped_out).await;
                    line.clear();
                }
            }
        }
    }
    if !line.is_empty() {
        write_stderr_line(&mut log, &line, cap, &mut written, &mut capped_out).await;
    }
    let _ = log.flush().await;
}

/// Persist one assembled stderr line as `[stderr] <redacted>\n`, honoring the
/// byte budget. Once `capped_out` it discards (the caller keeps draining the
/// pipe). On the first line that would cross `cap`, writes the fitting head + a
/// marker and trips `capped_out`.
async fn write_stderr_line(
    log: &mut tokio::fs::File,
    line: &[u8],
    cap: usize,
    written: &mut usize,
    capped_out: &mut bool,
) {
    if *capped_out {
        return;
    }
    let red = crate::schema::redaction::redact_secret_blob(&String::from_utf8_lossy(line));
    let entry = red.len() + 10; // "[stderr] " (9) + line + "\n" (1)
    if cap != 0 && *written + entry > cap {
        let room = cap.saturating_sub(*written);
        if room > 10 {
            let keep = (room - 10).min(red.len());
            let _ = log.write_all(b"[stderr] ").await;
            let _ = log.write_all(&red.as_bytes()[..keep]).await;
            let _ = log.write_all(b"\n").await;
        }
        let _ = log
            .write_all(b"[maestro] stderr truncated (cap exceeded)\n")
            .await;
        *capped_out = true;
        return;
    }
    let _ = log.write_all(b"[stderr] ").await;
    let _ = log.write_all(red.as_bytes()).await;
    let _ = log.write_all(b"\n").await;
    *written += entry;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(kv: &[(&str, &str)]) -> Vec<(String, String)> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn allowlist_keeps_base_and_provider_key_strips_unrelated_secrets() {
        let parent = pairs(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/home/u"),
            ("LANG", "en_US.UTF-8"),
            ("LC_ALL", "C"),
            ("TMPDIR", "/tmp"),
            ("OPENAI_API_KEY", "sk-codex"),
            ("AWS_SECRET_ACCESS_KEY", "leak-me"),
            ("GH_TOKEN", "ghp_leak"),
            ("SOME_OTHER_API_KEY", "leak-too"),
            ("MAESTRO_CODEX", "/path/to/codex"),
        ]);
        let kept: std::collections::BTreeMap<_, _> =
            allowlist_env(parent, "codex", &[]).into_iter().collect();
        // base + locale + provider auth kept
        assert!(kept.contains_key("PATH"));
        assert!(kept.contains_key("HOME"));
        assert!(kept.contains_key("LANG"));
        assert!(kept.contains_key("LC_ALL"));
        assert!(kept.contains_key("TMPDIR"));
        assert_eq!(
            kept.get("OPENAI_API_KEY").map(String::as_str),
            Some("sk-codex")
        );
        // unrelated secrets stripped — NOT generalized to *_KEY (pin 2)
        assert!(!kept.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!kept.contains_key("GH_TOKEN"));
        assert!(!kept.contains_key("SOME_OTHER_API_KEY"));
        // no MAESTRO_* wildcard handed to the child (pin 2)
        assert!(!kept.contains_key("MAESTRO_CODEX"));
    }

    #[test]
    fn extra_allow_env_is_an_explicit_escape_hatch() {
        let parent = pairs(&[("CUSTOM_REGISTRY_TOKEN", "needed"), ("UNRELATED", "no")]);
        let kept: std::collections::BTreeMap<_, _> =
            allowlist_env(parent, "cursor", &["CUSTOM_REGISTRY_TOKEN".to_string()])
                .into_iter()
                .collect();
        assert!(kept.contains_key("CUSTOM_REGISTRY_TOKEN"));
        assert!(!kept.contains_key("UNRELATED"));
    }

    #[test]
    fn unknown_provider_keeps_only_base_no_auth_guess() {
        let parent = pairs(&[
            ("PATH", "/b"),
            ("OPENAI_API_KEY", "x"),
            ("ANTHROPIC_API_KEY", "y"),
        ]);
        let kept: std::collections::BTreeMap<_, _> =
            allowlist_env(parent, "mystery", &[]).into_iter().collect();
        assert!(kept.contains_key("PATH"));
        // no provider-auth allowlist for an unknown provider → its keys stripped
        assert!(!kept.contains_key("OPENAI_API_KEY"));
        assert!(!kept.contains_key("ANTHROPIC_API_KEY"));
    }

    #[tokio::test]
    async fn capped_log_zero_is_byte_for_byte_passthrough() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut log = CappedLog::open(tmp.path(), 0).await.unwrap();
        log.write_all(b"hello ").await.unwrap();
        log.write_all(b"world\n").await.unwrap();
        log.finalize().await.unwrap();
        let body = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(body, "hello world\n");
        assert!(!log.truncated());
    }

    #[tokio::test]
    async fn capped_log_keeps_head_marker_and_tail() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // cap 100 -> head_budget 75, tail_cap 25 (clamped to >=1024 actually) —
        // use a bigger cap so the split is meaningful.
        let cap = 8192usize;
        let mut log = CappedLog::open(tmp.path(), cap).await.unwrap();
        let head = "H".repeat(7000);
        let mid = "M".repeat(5000);
        let tail = "TAILRESULT".to_string();
        log.write_all(head.as_bytes()).await.unwrap();
        log.write_all(mid.as_bytes()).await.unwrap();
        log.write_all(tail.as_bytes()).await.unwrap();
        log.finalize().await.unwrap();
        let body = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(log.truncated());
        assert!(body.starts_with("HHHH"), "head preserved");
        assert!(
            body.contains("task log truncated at cap"),
            "explicit marker"
        );
        assert!(
            body.ends_with("TAILRESULT"),
            "final result (tail) preserved"
        );
        // middle dropped: total is bounded near the cap, not the full input
        assert!(
            body.len() < cap + 512,
            "bounded near cap, got {}",
            body.len()
        );
        assert!(!body.contains(&"M".repeat(5000)), "middle dropped");
    }

    /// B1: a tiny cap (< 1 KiB) must not panic in `open` (the old `clamp(1024,
    /// cap)` did) and must still bound + mark.
    #[tokio::test]
    async fn capped_log_tiny_caps_do_not_panic() {
        for cap in [1usize, 100, 1023, 1024] {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let mut log = CappedLog::open(tmp.path(), cap).await.unwrap();
            log.write_all(&vec![b'Z'; 50_000]).await.unwrap();
            log.finalize().await.unwrap();
            let body = std::fs::read(tmp.path()).unwrap();
            assert!(log.truncated(), "cap {cap} truncates a 50KB write");
            assert!(
                String::from_utf8_lossy(&body).contains("task log truncated at cap"),
                "cap {cap} writes a marker"
            );
        }
    }

    /// B3: a single newline-less giant stderr line must be hard-bounded on disk
    /// AND memory (chunked, never read whole) with a marker — and the WHOLE
    /// 5 MB is still consumed to EOF (the pump drains, it doesn't `.take`-stop).
    #[tokio::test]
    async fn stderr_pump_hard_caps_a_giant_single_line() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let cap = 4096usize;
        let giant = vec![b'X'; 5_000_000]; // 5 MB, no newline
        pump_stderr_capped(&giant[..], tmp.path(), cap).await;
        let body = std::fs::read(tmp.path()).unwrap();
        // bounded near the cap (NOT 5 MB) and explicitly marked
        assert!(
            body.len() <= cap + 80,
            "stderr file bounded near cap, got {}",
            body.len()
        );
        assert!(
            String::from_utf8_lossy(&body).contains("stderr truncated (cap exceeded)"),
            "explicit stderr marker"
        );
    }

    /// B4: default (`cap=0`) and under-cap output must be byte-for-byte the
    /// historical per-line `[stderr] <line>\n` format — no regression.
    #[tokio::test]
    async fn stderr_pump_zero_cap_and_under_cap_keep_exact_legacy_format() {
        // cap=0 → full passthrough, exact legacy per-line format.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        pump_stderr_capped(&b"line one\nline two\n"[..], tmp.path(), 0).await;
        let body = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(body, "[stderr] line one\n[stderr] line two\n");
        // under a cap → full, exact legacy format, no marker.
        let tmp2 = tempfile::NamedTempFile::new().unwrap();
        pump_stderr_capped(&b"short\n"[..], tmp2.path(), 8192).await;
        let body2 = std::fs::read_to_string(tmp2.path()).unwrap();
        assert_eq!(body2, "[stderr] short\n");
    }

    #[tokio::test]
    async fn capped_log_under_budget_is_unchanged() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut log = CappedLog::open(tmp.path(), 8192).await.unwrap();
        log.write_all(b"small output").await.unwrap();
        log.finalize().await.unwrap();
        let body = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(body, "small output");
        assert!(!log.truncated());
    }

    /// Integration: a real child sees the secret with the default (no scrub),
    /// and genuinely does NOT see it once `apply_env_scrub` rebuilds the env —
    /// while an allowlisted var (here via `extra_allow`) is still visible. Uses
    /// uniquely-named vars so the process-global env mutation can't clobber a
    /// parallel test. Unix-only (spawns `sh`).
    #[cfg(unix)]
    #[tokio::test]
    async fn env_scrub_hides_unrelated_secret_from_a_real_child() {
        let secret = "MAESTRO_T_SECRET_Q7W";
        let kept = "MAESTRO_T_KEEP_Q7W";
        // SAFETY: test-only, unique names; removed at the end.
        unsafe {
            std::env::set_var(secret, "leak-me");
            std::env::set_var(kept, "keep-me");
        }
        let script = format!("printf '%s|%s' \"${secret}\" \"${kept}\"");

        // OFF (no scrub): default inherit → child sees the secret.
        let mut off = tokio::process::Command::new("sh");
        off.arg("-c").arg(&script);
        let off_out = off.output().await.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&off_out.stdout),
            "leak-me|keep-me",
            "without scrub the child inherits the full env"
        );

        // ON: rebuild env from allowlist (+ the kept var via extra_allow).
        let mut on = tokio::process::Command::new("sh");
        on.arg("-c").arg(&script);
        apply_env_scrub(&mut on, "cursor", &[kept.to_string()]);
        let on_out = on.output().await.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&on_out.stdout),
            "|keep-me",
            "scrub strips the unrelated secret but keeps the allowlisted var"
        );

        // SAFETY: test-only cleanup.
        unsafe {
            std::env::remove_var(secret);
            std::env::remove_var(kept);
        }
    }
}
